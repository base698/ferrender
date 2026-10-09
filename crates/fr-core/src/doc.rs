//! The document: parameters and an ordered list of features. Bodies are
//! never stored; they are rebuilt from the features.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::{DAffine3, DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::csg::{self, Bool};
use crate::exact::{self, Lumps, Place};
use crate::expr::{self, Kind, Quantity, Value};
use crate::mesh::{self, Mesh};
use crate::tag::{EdgeTag, Level, Tag};
use crate::profile::{self, Profile};
use crate::sketch::{Geom, Id, Plane, Sketch};
use crate::solver;
use crate::threads;
pub use crate::components::{Component,Placement};
pub use crate::planes::{ConstructionPlane, OriginPlane, PlaneKind, PlaneRef, PointRef};
use crate::units::{Unit, fmt_len, trim_num};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub expr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    #[default]
    New,
    Join,
    Cut,
    Intersect,
}

impl Op {
    pub const ALL: [Op; 4] = [Op::New, Op::Join, Op::Cut, Op::Intersect];

    pub fn name(self) -> &'static str {
        match self {
            Op::New => "new",
            Op::Join => "join",
            Op::Cut => "cut",
            Op::Intersect => "intersect",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Op::New => "New Body",
            Op::Join => "Join",
            Op::Cut => "Cut",
            Op::Intersect => "Intersect",
        }
    }

    pub fn parse(s: &str) -> Option<Op> {
        Op::ALL.into_iter().find(|o| o.name() == s || (s == "new_body" && *o == Op::New))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// The sketch's own x axis.
    X,
    Y,
    /// A line in the sketch.
    Line(Id),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extrude {
    pub sketch: Id,
    /// Each profile is named by the entities on its outer boundary.
    pub profiles: Vec<Vec<Id>>,
    pub distance: Value,
    #[serde(default)]
    pub symmetric: bool,
    #[serde(default)]
    pub op: Op,
    /// Degrees the walls lean outward as they leave the sketch plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taper: Option<Value>,
    /// Go past every body in the direction of `distance` instead of stopping at it.
    #[serde(default)]
    pub through_all: bool,
}

/// An additional direction in a rectangular pattern, in the component's frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearDirection {
    pub axis: usize,
    /// Includes the source row or column.
    pub count: u32,
    pub spacing: Value,
}

/// Where copies go. Axes are component-local: 0 = X, 1 = Y, 2 = Z.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternKind {
    /// `count` copies in all, spread around an axis through the origin over `angle` degrees.
    Circular { axis: usize, count: u32, angle: Value },
    /// Counts include the source. An optional second direction forms a grid.
    Linear {
        axis: usize, count: u32, spacing: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        second: Option<LinearDirection>,
    },
    /// One copy, reflected through the origin plane whose normal is `axis`.
    Mirror { axis: usize },
}

/// Repeats an earlier extrusion, revolution, primitive, import or standalone text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub source: Id,
    pub kind: PatternKind,
}

impl Pattern {
    /// Validate bounds before allocating copies or accepting a native file.
    pub fn validate(&self) -> Result<(), String> {
        let axis = |axis: usize| if axis < 3 { Ok(()) } else { Err("the axis must be x, y or z".to_owned()) };
        let expression = |value: &Value| {
            if value.expr.len() > 4096 { Err("an expression is too long (maximum 4096 bytes)".to_owned()) } else { Ok(()) }
        };
        let linear = |count: u32, spacing: &Value, require_nonzero: bool| {
            expression(spacing)?;
            if !(2..=1000).contains(&count) {
                return Err("each linear pattern direction needs between 2 and 1000 instances, including the source".to_owned());
            }
            let reach = spacing.v * f64::from(count - 1);
            if !reach.is_finite() || !(reach as f32).is_finite() {
                return Err("linear pattern spacing must be finite and within the supported range".to_owned());
            }
            if require_nonzero && spacing.v == 0. { return Err("both rectangular pattern spacings must be nonzero".to_owned()); }
            Ok(())
        };
        match &self.kind {
            PatternKind::Circular { axis: a, count, angle } => {
                axis(*a)?;
                expression(angle)?;
                if !(2..=360).contains(count) { return Err("a pattern needs between 2 and 360 copies".into()); }
                if !angle.v.is_finite() { return Err("the pattern angle must be finite".into()); }
            }
            PatternKind::Linear { axis: a, count, spacing, second } => {
                axis(*a)?;
                // Keep legacy single-direction patterns (including coincident
                // zero-spacing copies) readable and behaviorally unchanged.
                linear(*count, spacing, second.is_some())?;
                if let Some(second) = second {
                    axis(second.axis)?;
                    if second.axis == *a { return Err("the second pattern direction must use a different axis".into()); }
                    linear(second.count, &second.spacing, true)?;
                    if count.checked_mul(second.count).is_none_or(|total| total > 1000) {
                        return Err("a rectangular pattern can have at most 1000 instances total, including the source".into());
                    }
                }
            }
            PatternKind::Mirror { axis: a } => axis(*a)?,
        }
        Ok(())
    }

    /// Where each copy goes.
    pub fn placements(&self) -> Result<Vec<Place>, String> {
        self.validate()?;
        let unit = |axis: usize| [DVec3::X, DVec3::Y, DVec3::Z].get(axis).copied().ok_or("the axis must be x, y or z".to_owned());
        match &self.kind {
            PatternKind::Circular { axis, count, angle } => {
                // A full turn spaces the copies evenly; a part turn puts one at each end.
                let step = if angle.v.abs() >= 360.0 - 1e-9 { angle.v / *count as f64 } else { angle.v / (*count - 1) as f64 };
                let u = unit(*axis)?;
                Ok((1..*count).map(|k| Place::Turn { origin: DVec3::ZERO, axis: u, angle: (step * k as f64).to_radians() }).collect())
            }
            PatternKind::Linear { axis, count, spacing, second } => {
                let u = unit(*axis)?;
                let (rows, row_step) = match second {
                    Some(second) => (second.count, unit(second.axis)? * second.spacing.v),
                    None => (1, DVec3::ZERO),
                };
                let mut copies = Vec::with_capacity((*count * rows - 1) as usize);
                for row in 0..rows {
                    for column in 0..*count {
                        if row == 0 && column == 0 { continue; }
                        copies.push(Place::Shift(u * spacing.v * f64::from(column) + row_step * f64::from(row)));
                    }
                }
                Ok(copies)
            }
            PatternKind::Mirror { axis } => {
                let u = unit(*axis)?;
                Ok(vec![Place::Mirror { origin: DVec3::ZERO, normal: u }])
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Revolve {
    pub sketch: Id,
    pub profiles: Vec<Vec<Id>>,
    pub axis: Axis,
    pub angle: Value,
    #[serde(default)]
    pub op: Op,
}

/// Moves a body: scale about the origin, then rotate about X, Y and Z, then translate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub body: Id,
    pub translate: [Value; 3],
    pub rotate: [Value; 3],
    pub scale: Value,
}

/// Rounds or bevels edges of a body. Edges are named by a point on them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Blend {
    pub body: Id,
    pub edges: Vec<DVec3>,
    /// Fillet radius, or the distance a chamfer takes off each face.
    pub size: Value,
    #[serde(default)]
    pub chamfer: bool,
    /// The body's bounds when the edges were picked, so they can be found again if it changes size.
    #[serde(default)]
    pub frame: Option<[DVec3; 2]>,
    /// What each edge was made of, one per entry of `edges`; empty until the first build learns them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Option<EdgeTag>>,
}

/// Hollows a body, leaving the named faces open. Faces are named by a point on them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shell {
    pub body: Id,
    pub faces: Vec<DVec3>,
    pub thickness: Value,
    #[serde(default)]
    pub frame: Option<[DVec3; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Option<Tag>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoleShape {
    #[default]
    Simple,
    Counterbore,
    Countersink,
}

impl HoleShape {
    pub const ALL: [HoleShape; 3] = [HoleShape::Simple, HoleShape::Counterbore, HoleShape::Countersink];

    pub fn name(self) -> &'static str {
        match self {
            HoleShape::Simple => "simple",
            HoleShape::Counterbore => "counterbore",
            HoleShape::Countersink => "countersink",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HoleShape::Simple => "Simple",
            HoleShape::Counterbore => "Counterbore",
            HoleShape::Countersink => "Countersink",
        }
    }
}

/// What the hole is for, which with a thread size decides how wide it is drilled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoleFit {
    /// The diameter is given outright.
    #[default]
    Plain,
    /// A screw passes through: close, normal or loose around it.
    Close,
    Normal,
    Loose,
    /// The screw threads into it.
    Tapped,
}

impl HoleFit {
    pub const ALL: [HoleFit; 5] = [HoleFit::Plain, HoleFit::Close, HoleFit::Normal, HoleFit::Loose, HoleFit::Tapped];

    pub fn name(self) -> &'static str {
        match self {
            HoleFit::Plain => "plain",
            HoleFit::Close => "close",
            HoleFit::Normal => "normal",
            HoleFit::Loose => "loose",
            HoleFit::Tapped => "tapped",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HoleFit::Plain => "Plain (by diameter)",
            HoleFit::Close => "Clearance, close",
            HoleFit::Normal => "Clearance, normal",
            HoleFit::Loose => "Clearance, loose",
            HoleFit::Tapped => "Tapped",
        }
    }
}

/// Drilled holes, sized by hand or from the thread catalog. Each enters the
/// body at a point of `at` and runs along `dir`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hole {
    pub body: Id,
    pub at: Vec<DVec3>,
    pub dir: DVec3,
    #[serde(default)]
    pub shape: HoleShape,
    #[serde(default)]
    pub fit: HoleFit,
    /// A name from the thread catalog; needed by every fit but plain.
    #[serde(default)]
    pub thread: String,
    /// For plain holes.
    #[serde(default)]
    pub diameter: Option<Value>,
    /// `None` goes all the way through.
    #[serde(default)]
    pub depth: Option<Value>,
    /// The included angle of a pointed bottom; `None` is flat.
    #[serde(default)]
    pub tip_angle: Option<Value>,
    /// The counterbore or countersink; left out, they come from the thread.
    #[serde(default)]
    pub head_diameter: Option<Value>,
    #[serde(default)]
    pub head_depth: Option<Value>,
    #[serde(default)]
    pub head_angle: Option<Value>,
    /// Cut the thread itself into a tapped hole, rather than leaving it at the tap drill size.
    #[serde(default)]
    pub modeled: bool,
    #[serde(default)]
    pub left: bool,
    /// Added to every diameter, to allow for a printer that makes holes small.
    #[serde(default)]
    pub extra: Option<Value>,
}

/// A hole's sizes once the catalog and the defaults have been applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoleSizes {
    pub diameter: f64,
    pub head: Option<exact::DrillHead>,
    /// The major diameter and pitch of a thread to cut; `diameter` is then across its roots.
    pub thread: Option<(f64, f64)>,
}

impl Hole {
    pub fn sizes(&self) -> Result<HoleSizes, String> {
        let spec = if self.thread.trim().is_empty() { None } else { Some(threads::find(&self.thread)?) };
        let extra = self.extra.as_ref().map_or(0.0, |v| v.v);
        let need = |what: &str| format!("{what} needs a thread size, such as M3");
        let diameter = extra
            + match self.fit {
                HoleFit::Plain => self.diameter.as_ref().map(|v| v.v).ok_or("a plain hole needs a diameter")?,
                HoleFit::Close => spec.ok_or_else(|| need("a clearance hole"))?.clearance[0],
                HoleFit::Normal => spec.ok_or_else(|| need("a clearance hole"))?.clearance[1],
                HoleFit::Loose => spec.ok_or_else(|| need("a clearance hole"))?.clearance[2],
                HoleFit::Tapped => spec.ok_or_else(|| need("a tapped hole"))?.tap_drill,
            };
        let given = |v: &Option<Value>, from_spec: Option<f64>, what: &str| v.as_ref().map(|v| v.v).or(from_spec).ok_or(format!("the {what} is needed when there is no thread size to take it from"));
        let head = match self.shape {
            HoleShape::Simple => None,
            HoleShape::Counterbore => Some(exact::DrillHead::Counterbore {
                diameter: extra + given(&self.head_diameter, spec.map(|t| t.counterbore), "counterbore diameter")?,
                depth: given(&self.head_depth, spec.map(|t| t.counterbore_depth), "counterbore depth")?,
            }),
            HoleShape::Countersink => Some(exact::DrillHead::Countersink {
                diameter: extra + given(&self.head_diameter, spec.map(|t| t.countersink().0), "countersink diameter")?,
                angle: given(&self.head_angle, spec.map(|t| t.countersink().1), "countersink angle")?,
            }),
        };
        let thread = match (self.fit, self.modeled, spec) {
            (HoleFit::Tapped, true, Some(t)) => Some((t.major + extra, t.pitch)),
            _ => None,
        };
        Ok(HoleSizes { diameter, head, thread })
    }
}

/// A screw thread cut onto a rod or into a hole. The cylinder is named by a point on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub body: Id,
    pub face: DVec3,
    #[serde(default)]
    pub frame: Option<[DVec3; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Tag>,
    /// A name from the thread catalog.
    pub thread: String,
    /// How far from the start of the cylinder the thread begins; with `length`, `None` is the whole face.
    #[serde(default)]
    pub offset: Option<Value>,
    #[serde(default)]
    pub length: Option<Value>,
    #[serde(default)]
    pub left: bool,
    /// Room for the thread to turn: a hole's thread is made this much wider across and a
    /// rod's this much thinner. Printed threads at their exact sizes do not go together.
    #[serde(default)]
    pub extra: Option<Value>,
}

/// Editable lettering, either a standalone solid or raised/recessed on an exact flat face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub text: String,
    pub plane: Plane,
    pub height: Value,
    pub depth: Value,
    pub spacing: Value,
    pub angle: Value,
    pub x: Value,
    pub y: Value,
    pub align: crate::text::Align,
    pub op: Op,
    pub body: Option<Id>,
    pub face: Option<DVec3>,
    pub frame: Option<[DVec3; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Tag>,
}

impl Text {
    /// Resolve an attached face before applying local offsets and rotation. Like
    /// other face features, this uses the original and bounds-mapped anchor; reject
    /// candidates off a flat face or facing a different direction.
    pub fn placement(&self, body: Option<&Body>) -> Result<Plane, String> {
        self.placement_tagged(body).map(|p| p.0)
    }

    /// [`placement`](Self::placement), plus the tag of the face the text landed on and how it was found.
    pub fn placement_tagged(&self, body: Option<&Body>) -> Result<(Plane, Option<Tag>, Level), String> {
        let mut plane = self.plane;
        let mut learned = None;
        let mut level = Level::Position;
        if let Some(body) = body {
            if !body.is_exact() { return Err("text on a surface needs an exact body with a flat face".into()); }
            let anchor = self.face.ok_or("select a flat face for the text")?;
            let mut found = None;
            let [original, mapped] = exact::candidates(&body.solids, &[anchor], self.frame)[0];
            let mut candidates = vec![mapped, original];
            // A tagged face is tried first: the anchor dropped onto wherever that face is now.
            let mut by_tag = 0;
            if let Some(tag) = &self.tag {
                let faces = exact::faces_tagged(&body.solids, &body.tags);
                let exact_hit = faces.iter().find(|f| f.tag.as_ref() == Some(tag));
                let family = exact_hit.or_else(|| faces.iter().filter(|f| f.tag.as_ref().is_some_and(|t| t.family() == tag.family())).min_by(|a, b| a.at.distance(mapped).total_cmp(&b.at.distance(mapped))));
                if let Some(f) = family {
                    level = if exact_hit.is_some() { Level::Tag } else { Level::Origin };
                    let drop = |p: DVec3| p - f.normal * (p - f.at).dot(f.normal);
                    candidates.insert(0, drop(mapped));
                    candidates.insert(1, drop(original));
                    by_tag = 2;
                }
            }
            // An earlier emboss can extend the bounds beyond this face. When
            // the base grows, proportional mapping need not land on the face;
            // also try the change in the outward extent along its normal.
            if let (Some([lo, hi]), Some((now_lo, now_hi))) = (self.frame, body.mesh.bbox()) {
                let n = plane.normal();
                let extent = |lo: DVec3, hi: DVec3| n.dot(DVec3::new(if n.x >= 0.0 { hi.x } else { lo.x }, if n.y >= 0.0 { hi.y } else { lo.y }, if n.z >= 0.0 { hi.z } else { lo.z }));
                candidates.push(anchor + n * (extent(now_lo, now_hi) - extent(lo, hi)));
            }
            for (k, point) in candidates.into_iter().enumerate() {
                let Some(face) = crate::face::Face::near(body, point) else { continue };
                let Some(surface) = face.plane else { continue };
                let normal = surface.normal();
                if normal.dot(plane.normal()) < 1.0 - 1e-6 || (point - surface.origin).dot(normal).abs() > 1e-5 { continue; }
                let local = surface.to_local(point);
                if face.loops.iter().filter(|ring| profile::inside(ring, local)).count() % 2 == 0 { continue; }
                // Keep the user-chosen baseline and orientation on the resolved face.
                plane.origin += point - anchor;
                plane.origin -= normal * (plane.origin - surface.origin).dot(normal);
                found = Some(plane);
                if k >= by_tag { level = Level::Position; }
                learned = exact::face_tag_at(&body.solids, &body.tags, point);
                break;
            }
            plane = found.ok_or("the text's flat face is no longer there; select the face again")?;
            let _ = &mut learned;
        }
        plane.origin += plane.x * self.x.v + plane.y * self.y.v;
        let (sin, cos) = self.angle.v.to_radians().sin_cos();
        (plane.x, plane.y) = (plane.x * cos + plane.y * sin, plane.y * cos - plane.x * sin);
        Ok((plane, learned, level))
    }

    fn outlines(&self) -> Result<Vec<Profile>, String> {
        crate::validation::text(self)?;
        if self.depth.v < 0.001 || self.depth.v > 10000.0 { return Err("text depth must be between 0.001 and 10000 mm".into()); }
        crate::text::profiles(&self.text, self.height.v, self.spacing.v, self.align)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Combine {
    pub target: Id,
    pub tools: Vec<Id>,
    pub op: Op,
    #[serde(default)]
    pub keep_tools: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureKind {
    Sketch(Sketch),
    Plane(ConstructionPlane),
    Component(Component),
    Primitive(crate::primitives::Primitive),
    Remove(crate::body_ops::Remove),
    Split(crate::body_ops::Split),
    Extrude(Extrude),
    Revolve(Revolve),
    /// A mesh brought in from a file, already in millimetres.
    Import(Mesh),
    Transform(Transform),
    Combine(Combine),
    Pattern(Pattern),
    Blend(Blend),
    Shell(Shell),
    Hole(Hole),
    Thread(Thread),
    Text(Text),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub suppressed: bool,
    #[serde(default, skip_serializing_if = "is_root")]
    pub owner: Id,
    pub kind: FeatureKind,
}

fn is_root(id: &Id) -> bool { *id == 0 }

impl Feature {
    pub fn type_name(&self) -> &'static str {
        match &self.kind {
            FeatureKind::Sketch(_) => "sketch",
            FeatureKind::Plane(_) => "plane",
            FeatureKind::Component(_) => "component",
            FeatureKind::Primitive(p) => p.shape.name(),
            FeatureKind::Remove(_) => "remove",
            FeatureKind::Split(_) => "split",
            FeatureKind::Extrude(_) => "extrude",
            FeatureKind::Revolve(_) => "revolve",
            FeatureKind::Import(_) => "import",
            FeatureKind::Transform(_) => "transform",
            FeatureKind::Combine(_) => "combine",
            FeatureKind::Pattern(_) => "pattern",
            FeatureKind::Blend(b) if b.chamfer => "chamfer",
            FeatureKind::Blend(_) => "fillet",
            FeatureKind::Shell(_) => "shell",
            FeatureKind::Hole(_) => "hole",
            FeatureKind::Thread(_) => "thread",
            FeatureKind::Text(_) => "text",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default, skip_serializing_if = "is_root")]
    pub active_component: Id,
    #[serde(default)]
    pub units: Unit,
    #[serde(default)]
    pub params: Vec<Param>,
    pub features: Vec<Feature>,
    #[serde(default)]
    pub hidden_bodies: Vec<Id>,
    pub next_id: Id,
    /// The timeline's roll-back marker: only this many features are built, and
    /// new ones go in at this point. `None` is the end of the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub component: Id,
    pub placement: DAffine3,
    pub local_bounds: Option<[DVec3;2]>,
    /// The feature that created it.
    pub id: Id,
    pub name: String,
    /// Triangles for display, picking and STL.
    pub mesh: Mesh,
    /// The exact shape, for bodies made from sketches. Empty for imported
    /// meshes and for anything that has been combined with one.
    pub solids: Lumps,
    /// What each face of `solids` was made of, in the kernel's face order (see `tag`).
    pub tags: exact::Tags,
    /// The exact shape's edges as polylines, for picking and drawing.
    pub edges: Vec<Vec<DVec3>>,
    /// Modeled threads. Each is a closed shell of its own that overlaps the
    /// body, and its triangles follow the body's own at the end of `mesh`.
    pub threads: Vec<Mesh>,
    /// How many of `mesh`'s triangles are the body itself.
    pub plain: usize,
}

impl Body {
    pub fn to_local(&self,p:DVec3)->DVec3 {self.placement.inverse().transform_point3(p)}
    pub fn plane_to_local(&self,plane:Plane)->Plane {plane.transformed(self.placement.inverse())}
    pub fn local_frame(&self)->Option<[DVec3;2]> {self.local_bounds.or_else(||self.mesh.bbox().map(|(lo,hi)|[lo,hi]))}
    pub fn local_copy(&self)->Result<Self,String> {
        let mut body=self.clone();
        let steps=crate::components::steps(self.placement.inverse());
        if !steps.is_empty() {body.place(&steps)?;}
        body.placement=DAffine3::IDENTITY;
        body.local_bounds=None;
        Ok(body)
    }

    pub fn is_exact(&self) -> bool {
        !self.solids.is_empty()
    }

    fn set_exact(&mut self, solids: Lumps, tags: exact::Tags) -> Result<(), String> {
        (self.mesh, self.edges) = exact::tessellate(&solids)?;
        debug_assert_eq!(solids.len(), tags.len(), "one tag list per lump");
        self.solids = solids;
        self.tags = tags;
        self.dress();
        Ok(())
    }

    fn set_mesh(&mut self, mesh: Mesh) {
        self.mesh = mesh;
        self.solids.clear();
        self.tags.clear();
        self.edges.clear();
        self.dress();
    }

    /// Puts the threads back on a mesh that has just been rebuilt without them.
    fn dress(&mut self) {
        self.plain = self.mesh.len();
        let faces = !self.mesh.face_ids.is_empty();
        for (k, t) in self.threads.iter().enumerate() {
            self.mesh.append(t);
            if faces {
                // Clear of anything the kernel numbers its faces with.
                self.mesh.face_ids.extend(t.face_ids.iter().map(|part| u64::MAX - (k as u64 * 8 + part)));
            }
        }
    }

    /// The body without its threads, as booleans need it.
    pub fn bare(&self) -> Mesh {
        if self.threads.is_empty() { let mut m = self.mesh.clone(); m.face_ids.clear(); return m; }
        let mut m = self.mesh.head(self.plain);
        m.face_ids.clear();
        m
    }

    fn add_thread(&mut self, thread: Mesh) {
        let mesh = self.mesh.head(self.plain);
        self.mesh = mesh;
        self.threads.push(thread);
        self.dress();
    }

    /// Takes an exact shape out of the body, which stays exact if it was. The tool's new faces are `feature`'s.
    fn cut(&mut self, tool: &Lumps, feature: Id) -> Result<(), String> {
        if self.is_exact() {
            let (made, tags) = exact::boolean_tagged((&self.solids, &self.tags), (tool, &exact::fresh_tags(tool, feature)), Bool::Subtract, feature)?;
            return self.set_exact(made, tags);
        }
        let tool = exact::tessellate(tool)?.0;
        let mut tool = tool; tool.face_ids.clear();
        let made = csg::boolean(&self.bare(), &tool, Bool::Subtract)?;
        self.set_mesh(made);
        Ok(())
    }

    /// Moves the body through a series of placements.
    fn place(&mut self, steps: &[Place]) -> Result<(), String> {
        // The kernel has no inside-out solids, so a negative scale falls back to the mesh.
        for t in &mut self.threads {
            for p in steps {
                t.map(|v| p.point(v));
                if p.flips() {
                    t.flip();
                }
            }
        }
        if self.is_exact() && !steps.iter().any(|p| matches!(p, Place::Scale { factor, .. } if *factor < 0.0)) {
            // Moves keep the kernel's face order, so the tags stay as they are.
            let moved = steps.iter().fold(std::mem::take(&mut self.solids), |s, p| exact::place(s, p));
            let tags = std::mem::take(&mut self.tags);
            return self.set_exact(moved, tags);
        }
        let mut mesh = self.bare();
        for p in steps {
            mesh.map(|v| p.point(v));
            if p.flips() {
                mesh.flip();
            }
        }
        self.set_mesh(mesh);
        Ok(())
    }
}

/// What a feature adds or removes, before it meets the bodies.
enum Shape {
    Exact(Lumps, exact::Tags),
    Mesh(Mesh),
}

impl Shape {
    fn bbox(&self) -> Option<(DVec3, DVec3)> {
        match self {
            Shape::Exact(l, _) => exact::bounds(l),
            Shape::Mesh(m) => m.bbox(),
        }
    }

    fn mesh(&self) -> Result<Mesh, String> {
        match self {
            Shape::Exact(l, _) => exact::tessellate(l).map(|t| { let mut m = t.0; m.face_ids.clear(); m }),
            Shape::Mesh(m) => Ok(m.clone()),
        }
    }

    /// Copy `n` of a pattern: the same shape moved, its faces tagged as copies.
    fn copied(&self, p: &Place, n: u32) -> Shape {
        match self.placed(p) {
            Shape::Exact(l, t) if n > 0 => Shape::Exact(l, t.into_iter().map(|lump| lump.into_iter().map(|tag| tag.map(|t| t.copy(n))).collect()).collect()),
            other => other,
        }
    }

    fn placed(&self, p: &Place) -> Shape {
        match self {
            Shape::Exact(l, t) => Shape::Exact(exact::place(l.clone(), p), t.clone()),
            Shape::Mesh(m) => {
                let mut m = m.clone();
                m.map(|v| p.point(v));
                if p.flips() {
                    m.flip();
                }
                Shape::Mesh(m)
            }
        }
    }

    fn body(self, id: Id, name: String, component: Id) -> Result<Body, String> {
        let mut b = Body { id, name, component, placement: DAffine3::IDENTITY, local_bounds: None, mesh: Mesh::default(), solids: Vec::new(), tags: Vec::new(), edges: Vec::new(), threads: Vec::new(), plain: 0 };
        match self {
            Shape::Exact(l, t) => b.set_exact(l, t)?,
            Shape::Mesh(m) => b.set_mesh(m),
        }
        Ok(b)
    }
}

/// What the features produce.
#[derive(Clone, Debug, Default)]
pub struct Built {
    pub components: BTreeMap<Id,crate::components::BuiltComponent>,
    pub placements_applied: bool,
    pub planes: BTreeMap<Id, crate::planes::ResolvedPlane>,
    pub bodies: Vec<Body>,
    /// Features that failed, with the reason.
    pub errors: BTreeMap<Id, String>,
    /// How each feature that picks faces or edges found them again (the weakest of its picks).
    pub resolutions: BTreeMap<Id, Level>,
}

impl Built {
    pub fn body(&self, id: Id) -> Option<&Body> {
        self.bodies.iter().find(|b| b.id == id)
    }

    /// A body's bounds, as fillets, chamfers and shells record them when their edges or faces are picked.
    pub fn frame(&self, id: Id) -> Option<[DVec3; 2]> {
        self.body(id).and_then(Body::local_frame)
    }
}

fn overlap(a: Option<(DVec3, DVec3)>, b: Option<(DVec3, DVec3)>) -> bool {
    match (a, b) {
        (Some((alo, ahi)), Some((blo, bhi))) => (alo - DVec3::splat(1e-6)).cmple(bhi).all() && (blo - DVec3::splat(1e-6)).cmple(ahi).all(),
        _ => false,
    }
}

impl Document {
    pub fn new(units: Unit) -> Document {
        Document { units, next_id: 1, ..Default::default() }
    }

    pub fn feature(&self, id: Id) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    pub fn feature_mut(&mut self, id: Id) -> Option<&mut Feature> {
        self.features.iter_mut().find(|f| f.id == id)
    }

    pub fn sketch(&self, id: Id) -> Option<&Sketch> {
        match &self.feature(id)?.kind {
            FeatureKind::Sketch(s) => Some(s),
            _ => None,
        }
    }

    pub fn sketch_mut(&mut self, id: Id) -> Option<&mut Sketch> {
        match &mut self.feature_mut(id)?.kind {
            FeatureKind::Sketch(s) => Some(s),
            _ => None,
        }
    }

    pub fn sketches(&self) -> impl Iterator<Item = (&Feature, &Sketch)> {
        self.features.iter().filter_map(|f| match &f.kind {
            FeatureKind::Sketch(s) => Some((f, s)),
            _ => None,
        })
    }

    /// Appends a feature, naming it after its type (`Sketch2`, `Extrude1`).
    pub fn add_feature(&mut self, kind: FeatureKind) -> Id {
        // Reserve every copy slot, including copies added by later pattern edits.
        while let Some(end)=self.features.iter().filter(|f|matches!(f.kind,FeatureKind::Pattern(_)|FeatureKind::Split(_)))
            .map(|f|f.id.saturating_mul(1000)).find(|start|self.next_id>*start && self.next_id<start.saturating_add(1000)) {
            self.next_id=end.saturating_add(1000);
        }
        let id = self.next_id;
        self.next_id += 1;
        let target=match &kind {
            FeatureKind::Transform(t)=>Some(t.body), FeatureKind::Combine(c)=>Some(c.target),
            FeatureKind::Blend(b)=>Some(b.body), FeatureKind::Shell(s)=>Some(s.body),
            FeatureKind::Hole(h)=>Some(h.body), FeatureKind::Thread(t)=>Some(t.body), FeatureKind::Text(t)=>t.body,
            FeatureKind::Split(s)=>Some(s.body), FeatureKind::Remove(r)=>r.bodies.first().copied(),
            _=>None,
        };
        let owner=match &kind {
            FeatureKind::Pattern(p)=>self.feature(p.source).map(|f|f.owner),
            _=>target.and_then(|id|self.body_owner(id)),
        }.unwrap_or(self.active_component);
        let mut f = Feature { id, name: String::new(), suppressed: false, owner, kind };
        let n = self.features.iter().filter(|o| o.type_name() == f.type_name()).count() + 1;
        let t = f.type_name();
        f.name = format!("{}{}{n}", t[..1].to_uppercase(), &t[1..]);
        // With the timeline rolled back, new features go in at the marker.
        match self.rollback.filter(|at| *at < self.features.len()) {
            Some(at) => {
                self.features.insert(at, f);
                self.rollback = Some(at + 1);
            }
            None => self.features.push(f),
        }
        id
    }

    /// How many features the timeline currently builds.
    pub fn active(&self) -> usize {
        self.rollback.map_or(self.features.len(), |at| at.min(self.features.len()))
    }

    /// Moves the roll-back marker so that `count` features are built; the end clears it.
    pub fn roll_to(&mut self, count: usize) {
        self.rollback = (count < self.features.len()).then_some(count);
        if !self.component_available(self.active_component) {self.active_component=0;}
    }

    fn parameter_at(&self, name: &str, depth: usize, cache: &RefCell<BTreeMap<String, Result<Quantity, String>>>) -> Option<Result<Quantity, String>> {
        let p = self.params.iter().find(|p| p.name == name)?;
        if let Some(value) = cache.borrow().get(name) {
            return Some(value.clone());
        }
        if depth >= 24 {
            return Some(Err(format!("parameter '{name}' is cyclic or nested too deeply")));
        }
        let value = expr::eval(&p.expr, self.units, &|name| self.parameter_at(name, depth + 1, cache));
        cache.borrow_mut().insert(name.to_owned(), value.clone());
        Some(value)
    }

    pub fn quantity(&self, expr: &str) -> Result<Quantity, String> {
        let cache = RefCell::new(BTreeMap::new());
        expr::eval(expr, self.units, &|name| self.parameter_at(name, 0, &cache))
    }

    fn pinned_quantity(&self, expr: &str) -> Result<(Quantity, String), String> {
        let cache = RefCell::new(BTreeMap::new());
        expr::eval_pinned(expr, self.units, &|name| self.parameter_at(name, 0, &cache))
    }

    /// Evaluates an expression to millimetres, degrees or a plain number.
    pub fn eval(&self, expr: &str, kind: Kind) -> Result<f64, String> {
        expr::to_kind(self.quantity(expr)?, kind, self.units)
    }

    /// Turns typed text into a stored value, pinning bare numbers to the
    /// current units.
    pub fn value(&self, text: &str, kind: Kind) -> Result<Value, String> {
        let (q, text) = self.pinned_quantity(text)?;
        let v = expr::to_kind(q, kind, self.units)?;
        let pinned = expr::pin_unit(&text, q, kind, self.units);
        if pinned != text {
            // The outer unit annotation also counts toward input/nesting limits.
            self.quantity(&pinned)?;
        }
        Ok(Value { expr: pinned, v })
    }

    /// Like [`Document::value`], and also accepts `name = expression`, which
    /// defines (or redefines) a parameter and uses it.
    pub fn enter(&mut self, text: &str, kind: Kind) -> Result<Value, String> {
        let Some((name, rhs)) = text.split_once('=') else { return self.value(text, kind) };
        let name = name.trim().trim_start_matches('$');
        let v = self.value(rhs, kind)?;
        self.set_param(name, &v.expr)?;
        Ok(Value { expr: format!("${name}"), v: v.v })
    }

    pub fn set_param(&mut self, name: &str, expr: &str) -> Result<(), String> {
        if !expr::valid_name(name) {
            return Err(format!("'{name}' cannot be used as a parameter name"));
        }
        let old = self.params.clone();
        match self.params.iter_mut().find(|p| p.name == name) {
            Some(p) => p.expr = expr.trim().to_owned(),
            None => self.params.push(Param { name: name.to_owned(), expr: expr.trim().to_owned() }),
        }
        match self.quantity(&format!("${name}")).and_then(|_| self.pinned_quantity(expr)) {
            Ok((_, pinned)) => self.params.iter_mut().find(|p| p.name == name).unwrap().expr = pinned,
            Err(e) => {
                self.params = old;
                return Err(e);
            }
        }
        Ok(())
    }

    /// A parameter's value for display, such as `10 mm` or `45 deg`.
    pub fn show_param(&self, name: &str) -> String {
        match self.quantity(&format!("${name}")) {
            Ok(q) => match q.dim {
                expr::Dim::Length => format!("{} {}", fmt_len(q.v, self.units), self.units.name()),
                expr::Dim::Angle => format!("{} deg", trim_num(q.v, 4)),
                expr::Dim::None => trim_num(q.v, 6),
            },
            Err(e) => e,
        }
    }

    /// A stored value for display in the document's units.
    pub fn show(&self, v: &Value, kind: Kind) -> String {
        let num = match kind {
            Kind::Length => fmt_len(v.v, self.units),
            Kind::Angle => format!("{}\u{b0}", trim_num(v.v, 3)),
            Kind::Scalar => trim_num(v.v, 6),
        };
        if v.is_formula() { format!("fx: {num}") } else { num }
    }

    /// The profiles of a sketch that `refs` name.
    fn pick<'a>(all: &'a [Profile], refs: &[Vec<Id>]) -> Result<Vec<&'a Profile>, String> {
        if refs.is_empty() {
            return Err("no profile is selected".into());
        }
        refs.iter().map(|r| all.iter().find(|p| &p.edges == r).ok_or_else(|| "a profile it used is no longer closed".to_owned())).collect()
    }

    fn tool(&self, f: &Feature, bodies: &[Body], context: &Built) -> Result<Option<(Shape, Op)>, String> {
        let sk = |id: Id| {
            if let Some(e)=context.errors.get(&id) { return Err(format!("its sketch could not be placed: {e}")); }
            if self.feature(id).is_some_and(|f|!context.components.contains_key(&f.owner)) {return Err("its sketch's component is suppressed or unavailable".into());}
            if !self.features.iter().take(self.active()).take_while(|p|p.id!=f.id).any(|p|p.id==id && !p.suppressed) { return Err("its sketch was deleted, suppressed, or comes later in the timeline".into()); }
            self.sketch(id).ok_or("its sketch was deleted".to_owned())
        };
        match &f.kind {
            FeatureKind::Primitive(p) => { let l = p.solids()?; let t = exact::fresh_tags(&l, f.id); Ok(Some((Shape::Exact(l, t), p.op))) }
            FeatureKind::Extrude(e) => {
                let s = sk(e.sketch)?;
                let source=self.feature(e.sketch).unwrap().owner;
                let plane=s.plane.transformed(context.component_placement(f.owner).inverse()*context.component_placement(source));
                let all = profile::profiles(s);
                let (mut z0, mut z1) = if e.symmetric { (-e.distance.v / 2.0, e.distance.v / 2.0) } else { (0.0, e.distance.v) };
                if e.through_all {
                    // Far enough to clear every body on the side the distance points to (both, if symmetric).
                    let n = plane.normal();
                    let reach = bodies.iter().filter(|b|b.component==f.owner).filter_map(|b| b.mesh.bbox()).flat_map(|(lo, hi)| (0..8).map(move |i| DVec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z }))).map(|c| (c - plane.origin).dot(n));
                    let (lo, hi) = reach.fold((0.0f64, 0.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
                    (z0, z1) = match (e.symmetric, e.distance.v >= 0.0) {
                        (true, _) => (lo - 1.0, hi + 1.0),
                        (false, true) => (0.0, hi + 1.0),
                        (false, false) => (lo - 1.0, 0.0),
                    };
                }
                let picked = Self::pick(&all, &e.profiles)?;
                // The kernel has no tapered sweep here, so a taper is built as a mesh.
                let shape = match e.taper.as_ref().filter(|t| t.v.abs() > 1e-9) {
                    Some(t) => Shape::Mesh(mesh::extrude_tapered(&picked, &plane, z0, z1, t.v)?),
                    None => {
                        let l = exact::extrude(&picked, &plane, z0, z1)?;
                        let t = exact::tag_extrude(&l, &picked, &plane, z0, z1, f.id);
                        Shape::Exact(l, t)
                    }
                };
                Ok(Some((shape, e.op)))
            }
            FeatureKind::Revolve(r) => {
                let s = sk(r.sketch)?;
                let plane=s.plane.transformed(context.component_placement(f.owner).inverse()*context.component_placement(self.feature(r.sketch).unwrap().owner));
                let all = profile::profiles(s);
                let (a, b) = match r.axis {
                    Axis::X => (DVec2::ZERO, DVec2::X),
                    Axis::Y => (DVec2::ZERO, DVec2::Y),
                    Axis::Line(l) => s.line(l).ok_or("its axis line was deleted")?,
                };
                let picked = Self::pick(&all, &r.profiles)?;
                let l = exact::revolve(&picked, &plane, a, b, r.angle.v)?;
                let t = exact::tag_revolve(&l, &picked, &plane, a, b, r.angle.v, f.id);
                Ok(Some((Shape::Exact(l, t), r.op)))
            }
            FeatureKind::Text(t) if t.op == Op::New => {
                let profiles = t.outlines()?;
                let plane = t.placement(None)?;
                let l = exact::extrude(&profiles.iter().collect::<Vec<_>>(), &plane, 0.0, t.depth.v)?;
                let tags = exact::fresh_tags(&l, f.id);
                Ok(Some((Shape::Exact(l, tags), Op::New)))
            }
            FeatureKind::Import(m) => Ok(Some((Shape::Mesh(m.clone()), Op::New))),
            _ => Ok(None),
        }
    }

    /// Re-evaluates every expression, re-solves the sketches and regenerates the bodies.
    pub fn rebuild(&mut self) -> Built {
        self.rebuild_with(None)
    }

    /// [`rebuild`](Self::rebuild), taking the bodies and planes from a verified geometry
    /// cache when one is given: expressions, sketches and components are still evaluated,
    /// and the cached results stand in for every body-making step.
    pub fn rebuild_with(&mut self, cache: Option<crate::cache::Restored>) -> Built {
        let mut built = Built::default();
        // Legacy documents have no implicit-unit metadata. Make their existing
        // expressions explicit in the units in which the document was opened.
        let parameters = Document { units: self.units, params: self.params.clone(), ..Document::default() };
        for param in &mut self.params {
            if let Ok((_, pinned)) = parameters.pinned_quantity(&param.expr) {
                param.expr = pinned;
            }
        }
        let probe = self.clone();
        for f in self.features.iter_mut().take(probe.active()).filter(|f|!f.suppressed && probe.component_available(f.owner)) {
            let mut err = None;
            let mut set = |v: &mut Value, kind: Kind| match probe.value(&v.expr, kind) {
                Ok(x) => *v = x,
                Err(e) => err = Some(e),
            };
            match &mut f.kind {
                FeatureKind::Primitive(p) => p.evaluate(&mut set),
                FeatureKind::Sketch(s) => {
                    for c in s.constraints.values_mut() {
                        if let (Some(v), Some(kind)) = (&mut c.value, c.kind.value_kind()) {
                            set(v, kind);
                        }
                    }
                    if err.is_none() && !solver::solve(s, &[]).ok {
                        err = Some("the sketch's constraints cannot all be satisfied".into());
                    }
                }
                FeatureKind::Extrude(e) => {
                    set(&mut e.distance, Kind::Length);
                    if let Some(t) = &mut e.taper {
                        set(t, Kind::Angle);
                    }
                }
                FeatureKind::Pattern(p) => match &mut p.kind {
                    PatternKind::Circular { angle, .. } => set(angle, Kind::Angle),
                    PatternKind::Linear { spacing, second, .. } => {
                        set(spacing, Kind::Length);
                        if let Some(second) = second { set(&mut second.spacing, Kind::Length); }
                    },
                    PatternKind::Mirror { .. } => {}
                },
                FeatureKind::Revolve(r) => set(&mut r.angle, Kind::Angle),
                FeatureKind::Transform(t) => {
                    t.translate.iter_mut().for_each(|v| set(v, Kind::Length));
                    t.rotate.iter_mut().for_each(|v| set(v, Kind::Angle));
                    set(&mut t.scale, Kind::Scalar);
                }
                FeatureKind::Blend(b) => set(&mut b.size, Kind::Length),
                FeatureKind::Shell(sh) => set(&mut sh.thickness, Kind::Length),
                FeatureKind::Hole(h) => {
                    for v in [&mut h.diameter, &mut h.depth, &mut h.head_diameter, &mut h.head_depth, &mut h.extra].into_iter().flatten() {
                        set(v, Kind::Length);
                    }
                    for v in [&mut h.tip_angle, &mut h.head_angle].into_iter().flatten() {
                        set(v, Kind::Angle);
                    }
                }
                FeatureKind::Thread(t) => {
                    for v in [&mut t.offset, &mut t.length, &mut t.extra].into_iter().flatten() {
                        set(v, Kind::Length);
                    }
                }
                FeatureKind::Text(t) => {
                    for v in [&mut t.height, &mut t.depth, &mut t.spacing, &mut t.x, &mut t.y] { set(v, Kind::Length); }
                    set(&mut t.angle, Kind::Angle);
                }
                FeatureKind::Plane(p) => { if let PlaneKind::Offset {distance,..}=&mut p.kind { set(distance,Kind::Length); } }
                FeatureKind::Component(c) => {
                    c.placement.translate.iter_mut().for_each(|v|set(v,Kind::Length));
                    c.placement.rotate.iter_mut().for_each(|v|set(v,Kind::Angle));
                }
                FeatureKind::Import(_) | FeatureKind::Combine(_) | FeatureKind::Remove(_) | FeatureKind::Split(_) => {}
            }
            if let Some(e) = err {
                built.errors.insert(f.id, e);
            }
        }

        built.components.insert(0,crate::components::BuiltComponent {placement:DAffine3::IDENTITY,visible:true});
        let mut counts:BTreeMap<Id,usize>=BTreeMap::new();
        for index in 0..self.active() {
            let f = self.features[index].clone();
            if f.suppressed || !built.components.contains_key(&f.owner) { continue; }
            if let FeatureKind::Sketch(sketch)=&f.kind && let Some(id)=sketch.on {
                match built.planes.get(&id) {
                    Some(p) => self.sketch_mut(f.id).unwrap().plane=p.plane.transformed(built.component_placement(f.owner).inverse()*built.component_placement(p.component)),
                    None => { built.errors.insert(f.id,format!("plane {id} is missing, rolled back, suppressed, or could not be built")); }
                }
            }
            if built.errors.contains_key(&f.id) {
                continue;
            }
            if let FeatureKind::Component(c)=&f.kind {
                let parent=&built.components[&f.owner];
                let placement=parent.placement*c.placement.affine();
                if !placement.is_finite() || !placement.translation.as_vec3().is_finite() {
                    built.errors.insert(f.id,"the component placement is out of range".into()); continue;
                }
                built.components.insert(f.id,crate::components::BuiltComponent {placement,visible:parent.visible && c.visible});
                continue;
            }
            if matches!(f.kind, FeatureKind::Sketch(_)) { continue; }
            let mut f = f;
            if let Some(cache) = &cache {
                // The cache stands in for every plane and body step; errors it recorded are reported again.
                if let Some(p) = cache.planes.get(&f.id) { built.planes.insert(f.id, p.clone()); }
                if let Some(e) = cache.errors.get(&f.id) { built.errors.insert(f.id, e.clone()); }
                continue;
            }
            if matches!(f.kind, FeatureKind::Plane(_)) {
                match self.resolve_plane(&mut f,&built) {
                    Ok((p, level)) => { built.planes.insert(f.id,p); if let Some(level) = level { built.resolutions.insert(f.id, level); } }
                    Err(e) => { built.errors.insert(f.id,e); }
                }
                if f != self.features[index] { self.features[index] = f; }
                continue;
            }
            // A feature can touch several bodies or drill several holes. Publish
            // its result only after all of those operations have succeeded.
            let mut bodies = built.bodies.clone();
            let mut next_count = counts.get(&f.owner).copied().unwrap_or(0);
            match self.apply(&mut f, &mut bodies, &mut next_count, &built) {
                Ok(level) => {
                    built.bodies = bodies;
                    counts.insert(f.owner,next_count);
                    if let Some(level) = level { built.resolutions.insert(f.id, level); }
                    // References learn the tags of what they found, so later rebuilds can find it by name.
                    if f != self.features[index] { self.features[index] = f; }
                }
                Err(e) => {
                    built.errors.insert(f.id, e);
                }
            }
        }
        if let Some(cache) = cache {
            built.bodies = cache.bodies;
            built.resolutions = cache.resolutions;
            built.placements_applied = true;
            if !built.components.contains_key(&self.active_component) {self.active_component=0;}
            return built;
        }
        let components=built.components.clone();
        built.bodies.retain_mut(|body| {
            let placement=components[&body.component].placement;
            body.local_bounds=body.mesh.bbox().map(|(lo,hi)|[lo,hi]);
            let steps=crate::components::steps(placement);
            if !steps.is_empty() && let Err(e)=body.place(&steps) {built.errors.insert(body.id,e);return false;}
            body.placement=placement;
            true
        });
        for p in built.planes.values_mut() {
            let placement=components[&p.component].placement;
            p.plane=p.plane.transformed(placement);
            p.corners=p.corners.map(|p|placement.transform_point3(p));
        }
        built.placements_applied=true;
        if !built.components.contains_key(&self.active_component) {self.active_component=0;}
        built
    }

    /// Applies a feature to the bodies. `f` is the feature's own copy: references that
    /// resolved by position write the tags they found into it. Returns how its picks resolved.
    fn apply(&self, f: &mut Feature, bodies: &mut Vec<Body>, count: &mut usize, context: &Built) -> Result<Option<Level>, String> {
        let find = |bodies: &[Body], id: Id| bodies.iter().position(|b| b.id == id).ok_or("a body it used no longer exists".to_owned());
        const MESH_ONLY: &str = "this body is a mesh (imported, tapered, or combined with one); this operation needs an exact body made from a sketch, primitive or text";
        let id = f.id;
        match &mut f.kind {
            FeatureKind::Remove(remove) => {
                remove.validate()?;
                for id in &remove.bodies {find(bodies,*id)?;}
                bodies.retain(|body|!remove.bodies.contains(&body.id));
                Ok(None)
            }
            FeatureKind::Split(split) => {
                split.validate()?;
                let index=find(bodies,split.body)?;
                let component=bodies[index].component;
                if component!=f.owner {return Err("the split must belong to its target body's component".into());}
                let (plane,_,level)=self.plane_reference_mut(&mut split.plane,context,component)?;
                let pieces=crate::body_ops::pieces(&bodies[index],plane)?;
                let tags=exact::carry(&[(&bodies[index].solids,&bodies[index].tags)],&pieces,id);
                let mut pieces=pieces.into_iter().zip(tags);
                let (first,first_tags)=pieces.next().ok_or("the split produced no pieces")?;
                bodies[index].set_exact(vec![first],vec![first_tags])?;
                for (offset,(solid,tags)) in pieces.enumerate() {
                    *count+=1;
                    bodies.push(Shape::Exact(vec![solid],vec![tags]).body(id*1000+offset as Id+1,format!("Body{count}"),component)?);
                }
                Ok(level)
            }
            FeatureKind::Text(t) if t.op != Op::New => {
                let profiles = t.outlines()?;
                let i = find(bodies, t.body.ok_or("select a body and flat face for the text")?)?;
                let (plane, learned, level) = t.placement_tagged(Some(&bodies[i]))?;
                if t.tag.is_none() { t.tag = learned; }
                let refs: Vec<_> = profiles.iter().collect();
                // All lettering must sit on material. This also catches overhangs,
                // holes through letters, and disconnected punctuation over an edge.
                let overlap = (t.depth.v * 0.01).min(0.01);
                let footprint = exact::extrude(&refs, &plane, -overlap, 0.0)?;
                let supported = exact::boolean(&footprint, &bodies[i].solids, Bool::Intersect)?;
                let expected: f64 = footprint.iter().map(|s| s.volume()).sum();
                let actual: f64 = supported.iter().map(|s| s.volume()).sum();
                if expected <= 1e-10 || actual < expected * (1.0 - 1e-5) {
                    return Err("the text extends beyond the flat face or over a hole; move it, reduce its height, or choose another face".into());
                }
                let (z0, z1, operation) = match t.op {
                    Op::Join => (-overlap, t.depth.v, Bool::Union),
                    Op::Cut => (-t.depth.v, overlap, Bool::Subtract),
                    _ => return Err("text supports New Body, Raise, or Engrave".into()),
                };
                let tool = exact::extrude(&refs, &plane, z0, z1)?;
                let (made, tags) = exact::boolean_tagged((&bodies[i].solids, &bodies[i].tags), (&tool, &exact::fresh_tags(&tool, id)), operation, id)?;
                bodies[i].set_exact(made, tags)?;
                bodies.retain(|b| !b.mesh.is_empty());
                Ok(Some(level))
            }
            FeatureKind::Transform(t) => {
                let i = find(bodies, t.body)?;
                if t.scale.v.abs() < 1e-9 {
                    return Err("the scale is zero".into());
                }
                let turn = |axis: DVec3, v: &Value| Place::Turn { origin: DVec3::ZERO, axis, angle: v.v.to_radians() };
                bodies[i].place(&[
                    Place::Scale { centre: DVec3::ZERO, factor: t.scale.v },
                    turn(DVec3::X, &t.rotate[0]),
                    turn(DVec3::Y, &t.rotate[1]),
                    turn(DVec3::Z, &t.rotate[2]),
                    Place::Shift(DVec3::new(t.translate[0].v, t.translate[1].v, t.translate[2].v)),
                ])?;
                Ok(None)
            }
            FeatureKind::Combine(c) => {
                let op = match c.op {
                    Op::Cut => Bool::Subtract,
                    Op::Intersect => Bool::Intersect,
                    _ => Bool::Union,
                };
                let ti = find(bodies, c.target)?;
                let tools: Vec<usize> = c.tools.iter().map(|t| if *t == c.target { Err("a body cannot be combined with itself".to_owned()) } else { find(bodies, *t) }).collect::<Result<_, _>>()?;
                let target_frame=context.component_placement(bodies[ti].component);
                let placed_tools=tools.iter().map(|i| {
                    let mut tool=bodies[*i].clone();
                    let transform=target_frame.inverse()*context.component_placement(tool.component);
                    let steps=crate::components::steps(transform);
                    if !steps.is_empty() {tool.place(&steps)?;}
                    Ok(tool)
                }).collect::<Result<Vec<_>,String>>()?;
                if bodies[ti].is_exact() && placed_tools.iter().all(Body::is_exact) {
                    let (mut result, mut tags) = (bodies[ti].solids.clone(), bodies[ti].tags.clone());
                    for t in &placed_tools {
                        (result, tags) = exact::boolean_tagged((&result, &tags), (&t.solids, &t.tags), op, id)?;
                    }
                    bodies[ti].set_exact(result, tags)?;
                } else {
                    let mut result = bodies[ti].bare();
                    for t in &placed_tools {
                        result = csg::boolean(&result, &t.bare(), op)?;
                    }
                    bodies[ti].set_mesh(result);
                }
                // What is joined on brings its threads with it.
                if c.op == Op::Join {
                    let carried: Vec<Mesh> = placed_tools.iter().flat_map(|t| t.threads.clone()).collect();
                    for t in carried {
                        bodies[ti].add_thread(t);
                    }
                }
                if !c.keep_tools {
                    bodies.retain(|b| !c.tools.contains(&b.id));
                }
                bodies.retain(|b| !b.mesh.is_empty());
                Ok(None)
            }
            FeatureKind::Blend(b) => {
                let i = find(bodies, b.body)?;
                if !bodies[i].is_exact() {
                    return Err(MESH_ONLY.into());
                }
                let picks: Vec<exact::EdgePick> = exact::candidates(&bodies[i].solids, &b.edges, b.frame).into_iter().enumerate().map(|(k, points)| exact::EdgePick { points, tag: b.tags.get(k).cloned().flatten() }).collect();
                let made = exact::blend(&bodies[i].solids, &bodies[i].tags, &picks, b.size.v, b.chamfer, id)?;
                if b.tags.len() != b.edges.len() { b.tags = made.picked; }
                bodies[i].set_exact(made.lumps, made.tags)?;
                Ok(Some(made.level))
            }
            FeatureKind::Shell(sh) => {
                let i = find(bodies, sh.body)?;
                if !bodies[i].is_exact() {
                    return Err(MESH_ONLY.into());
                }
                let picks: Vec<exact::FacePick> = exact::candidates(&bodies[i].solids, &sh.faces, sh.frame).into_iter().enumerate().map(|(k, points)| exact::FacePick { points, tag: sh.tags.get(k).cloned().flatten() }).collect();
                let made = exact::shell(&bodies[i].solids, &bodies[i].tags, &picks, sh.thickness.v, id)?;
                if sh.tags.len() != sh.faces.len() { sh.tags = made.picked; }
                bodies[i].set_exact(made.lumps, made.tags)?;
                Ok(Some(made.level))
            }
            FeatureKind::Hole(h) => {
                let i = find(bodies, h.body)?;
                let sizes = h.sizes()?;
                let dir = h.dir.try_normalize().ok_or("the hole has no direction")?;
                if h.at.is_empty() {
                    return Err("the hole needs at least one position".into());
                }
                let (lo, hi) = bodies[i].mesh.bbox().ok_or("the body is empty")?;
                // Far enough to come out of the other side, wherever that is.
                let through = |at: DVec3| (0..8).map(|k| (DVec3::new(if k & 1 == 0 { lo.x } else { hi.x }, if k & 2 == 0 { lo.y } else { hi.y }, if k & 4 == 0 { lo.z } else { hi.z }) - at).dot(dir)).fold(0.0, f64::max) + 1.0;
                let depth = |at: DVec3| h.depth.as_ref().map_or_else(|| through(at), |d| d.v);
                let before = bodies[i].bare().volume();
                // Where the drill comes out, for a thread that runs the whole way: the last surface on its line.
                let bare = bodies[i].bare();
                let exit = |at: DVec3| bare.ray(at + dir * through(at), -dir).map_or(through(at) - 1.0, |hit| through(at) - hit.0);
                // A modeled thread sits in a hole drilled to its full diameter.
                let drilled = sizes.thread.map_or(sizes.diameter, |(major, _)| major + threads::BED);
                let sunk = match sizes.head {
                    Some(exact::DrillHead::Counterbore { depth, .. }) => depth,
                    Some(exact::DrillHead::Countersink { diameter, angle }) => (diameter - drilled).max(0.0) / 2.0 / (angle.to_radians() / 2.0).tan(),
                    None => 0.0,
                };
                let mut sleeves = Vec::new();
                for at in &h.at {
                    if let Some((major, pitch)) = sizes.thread {
                        let length = h.depth.as_ref().map_or_else(|| exit(*at), |d| d.v) - sunk;
                        if length <= 1e-6 {
                            return Err("the counterbore or countersink leaves no depth for the thread".into());
                        }
                        let mut sleeve = threads::sleeve(major, sizes.diameter, major + 2.0 * threads::BED, pitch, length, h.left)?;
                        let turn = glam::DQuat::from_rotation_arc(DVec3::Z, dir);
                        sleeve.map(|v| *at + turn * (v + DVec3::Z * sunk));
                        sleeves.push(sleeve);
                    }
                    let tool = exact::drill(*at, dir, &exact::Drill { diameter: drilled, depth: depth(*at), tip_angle: h.tip_angle.as_ref().map(|v| v.v), head: sizes.head })?;
                    bodies[i].cut(&tool, id)?;
                }
                if before - bodies[i].bare().volume() < 1e-9 {
                    return Err("the hole does not touch the body; check its position and direction".into());
                }
                for sleeve in sleeves {
                    bodies[i].add_thread(sleeve);
                }
                Ok(None)
            }
            FeatureKind::Thread(t) => {
                let i = find(bodies, t.body)?;
                if !bodies[i].is_exact() {
                    return Err(MESH_ONLY.into());
                }
                let spec = threads::find(&t.thread)?;
                let pick = exact::FacePick { points: exact::candidates(&bodies[i].solids, &[t.face], t.frame)[0], tag: t.tag.clone() };
                let (mut barrel, level, learned) = exact::barrel_tagged(&bodies[i].solids, &bodies[i].tags, &pick)?;
                if t.tag.is_none() { t.tag = learned; }
                // Offsets are measured from the cylinder's outer end: a rod's tip, a hole's mouth.
                if let Some((lo, hi)) = bodies[i].mesh.bbox() {
                    let (middle, far) = ((lo + hi) / 2.0, barrel.start + barrel.axis * barrel.length);
                    if far.distance(middle) > barrel.start.distance(middle) + 1e-9 {
                        barrel = exact::Barrel { start: far, axis: -barrel.axis, ..barrel };
                    }
                }
                let across = barrel.radius * 2.0;
                let room = t.extra.as_ref().map_or(0.0, |v| v.v);
                if room < 0.0 || room > spec.pitch {
                    return Err(format!("the allowance should be between 0 and the thread's pitch, {} mm", fmt_len(spec.pitch, Unit::Mm)));
                }
                // A hole is remade to suit the thread and a thick rod turned down to it, but a thin rod cannot be built up.
                if !barrel.internal && across < spec.major * 0.9 {
                    return Err(format!("{} goes on a rod of {} mm or more; this one is {} mm", spec.name, fmt_len(spec.major, Unit::Mm), fmt_len(across, Unit::Mm)));
                }
                let (from, length) = match (&t.offset, &t.length) {
                    (None, None) => (0.0, barrel.length),
                    (offset, length) => {
                        let from = offset.as_ref().map_or(0.0, |v| v.v);
                        (from, length.as_ref().map_or(barrel.length - from, |v| v.v))
                    }
                };
                if from < -1e-9 || length <= 1e-9 || from + length > barrel.length + 1e-6 {
                    return Err("the thread's offset and length do not fit on the cylinder".into());
                }
                let (start, end) = (barrel.start + barrel.axis * from, barrel.start + barrel.axis * (from + length));
                let turn = glam::DQuat::from_rotation_arc(DVec3::Z, barrel.axis);
                let mut thread = if barrel.internal {
                    // A hole wider than the thread (a clearance hole, say) is filled in first. Then it is
                    // opened out to the thread's full diameter and the thread set into it.
                    let major = spec.major + room;
                    let wide = across > major + threads::BED + 1e-9;
                    if wide {
                        let plug = exact::cylinder(start, end, barrel.radius)?;
                        let (filled, tags) = exact::boolean_tagged((&bodies[i].solids, &bodies[i].tags), (&plug, &exact::fresh_tags(&plug, id)), Bool::Union, id)?;
                        bodies[i].set_exact(filled, tags)?;
                    }
                    if wide || across < major + threads::BED {
                        bodies[i].cut(&exact::cylinder(start, end, (major + threads::BED) / 2.0)?, id)?;
                    }
                    // The crests stand where the hole's wall was if that is near the tap drill size, and at that size otherwise.
                    let crests = if across >= spec.minor() * 0.9 && across <= spec.major - 0.2 * spec.pitch { across.max(spec.tap_drill + room) } else { spec.tap_drill + room };
                    threads::sleeve(major, crests, major + 2.0 * threads::BED, spec.pitch, length, t.left)?
                } else {
                    // The rod is turned down to just under the thread's roots, and the thread set over it.
                    // At a free end the core stops just short, so the thread's own end is the one that shows.
                    let bare = bodies[i].bare();
                    // Looking back along the axis from well outside, a free end is the first thing in the way.
                    let far = bare.bbox().map_or(1.0, |(lo, hi)| lo.distance(hi)) + 1.0;
                    let free = |p: DVec3, out: DVec3| bare.ray(p + out * far, -out).is_some_and(|hit| (hit.0 - far).abs() < 1e-6);
                    let ends = [free(start, -barrel.axis), free(end, barrel.axis)];
                    // A chamfer made before threading lies beyond the picked cylindrical face.
                    // Leaving it untouched leaves a full-diameter collar that cannot pass the
                    // nut's crests. Turn that free-end chamfer down to a pilot as well.
                    let chamfer = |at: DVec3, out: DVec3, at_face_end: bool| {
                        if !at_face_end { return 0.0; }
                        exact::end_chamfer(&bodies[i].solids, at, out, barrel.radius)
                            .filter(|d| free(at + out * *d, out)).unwrap_or(0.0)
                    };
                    let caps = [chamfer(start, -barrel.axis, from.abs() < 1e-6), chamfer(end, barrel.axis, (from + length - barrel.length).abs() < 1e-6)];
                    let (stock_start, stock_end) = (start - barrel.axis * caps[0], end + barrel.axis * caps[1]);
                    let inset = |is_free: bool| if is_free { threads::BED.min(length / 4.0) } else { 0.0 };
                    let core = exact::cylinder(stock_start + barrel.axis * inset(ends[0]), stock_end - barrel.axis * inset(ends[1]), (spec.minor() - room) / 2.0 - threads::BED)?;
                    let stock = exact::cylinder(stock_start, stock_end, barrel.radius.max(spec.major / 2.0) + 0.01)?;
                    bodies[i].cut(&exact::boolean(&stock, &core, Bool::Subtract)?, id)?;
                    threads::rod_with_lead(spec.major - room, spec.pitch, length, t.left, [ends[0] || caps[0] > 0.0, ends[1] || caps[1] > 0.0])?
                };
                thread.map(|v| start + turn * v);
                bodies[i].add_thread(thread);
                Ok(Some(level))
            }
            FeatureKind::Pattern(p) => {
                let source = self.features.iter().take_while(|source| source.id != f.id).find(|source|source.id == p.source && !source.suppressed).ok_or("the feature it repeats is missing or suppressed")?;
                if context.errors.contains_key(&source.id) || !context.components.contains_key(&source.owner) {
                    return Err("the feature it repeats could not be built".into());
                }
                let (tool, op) = self.tool(source, bodies, context)?.ok_or("only extrudes, revolves, primitives, imports and standalone text can be patterned")?;
                let mut landed = 0;
                for (k, place) in p.placements()?.iter().enumerate() {
                    let copy = tool.copied(place, k as u32);
                    // A cut that lands clear of every body has nothing to do; the others still apply.
                    if matches!(op, Op::Cut | Op::Intersect) && !bodies.iter().any(|b| b.component==f.owner && overlap(b.mesh.bbox(), copy.bbox())) {
                        continue;
                    }
                    landed += 1;
                    // Bodies are named by the feature that made them; copies get ids of their own beside it.
                    Self::merge(copy, op, id * 1000 + k as Id + 1, f.owner, bodies, count)?;
                }
                if landed == 0 {
                    return Err("none of the copies reach a body; try another axis, or a negative spacing or angle".into());
                }
                Ok(None)
            }
            _ => match self.tool(f, bodies, context)? {
                Some((tool, op)) => Self::merge(tool, op, id, f.owner, bodies, count).map(|_| None),
                None => Ok(None),
            },
        }
    }

    /// Adds a feature's shape to the bodies it touches, as its operation says.
    fn merge(tool: Shape, op: Op, id: Id, component: Id, bodies: &mut Vec<Body>, count: &mut usize) -> Result<(), String> {
        let reach = tool.bbox();
        let hits: Vec<usize> = (0..bodies.len()).filter(|i| bodies[*i].component==component && overlap(bodies[*i].mesh.bbox(), reach)).collect();
        // Exact against exact stays exact; a mesh on either side makes the result a mesh.
        let exact_tool = |bodies: &[Body]| match &tool {
            Shape::Exact(l, t) if hits.iter().all(|i| bodies[*i].is_exact()) => Some((l.clone(), t.clone())),
            _ => None,
        };
        match op {
            Op::Join if !hits.is_empty() => {
                match exact_tool(bodies) {
                    Some((mut all, mut tags)) => {
                        for i in &hits {
                            (all, tags) = exact::boolean_tagged((&bodies[*i].solids, &bodies[*i].tags), (&all, &tags), Bool::Union, id)?;
                        }
                        bodies[hits[0]].set_exact(all, tags)?;
                    }
                    None => {
                        let mut m = tool.mesh()?;
                        for i in &hits {
                            m = csg::boolean(&bodies[*i].bare(), &m, Bool::Union)?;
                        }
                        bodies[hits[0]].set_mesh(m);
                    }
                }
                for i in hits[1..].iter().rev() {
                    for t in bodies.remove(*i).threads {
                        bodies[hits[0]].add_thread(t);
                    }
                }
            }
            Op::New | Op::Join => {
                *count += 1;
                bodies.push(tool.body(id, format!("Body{count}"), component)?);
            }
            Op::Cut | Op::Intersect => {
                if hits.is_empty() {
                    return Err(format!("there is no body here to {}", op.name()));
                }
                let how = if op == Op::Cut { Bool::Subtract } else { Bool::Intersect };
                match exact_tool(bodies) {
                    Some((solids, tool_tags)) => {
                        for i in hits {
                            let (made, tags) = exact::boolean_tagged((&bodies[i].solids, &bodies[i].tags), (&solids, &tool_tags), how, id)?;
                            bodies[i].set_exact(made, tags)?;
                        }
                    }
                    None => {
                        let m = tool.mesh()?;
                        for i in hits {
                            let made = csg::boolean(&bodies[i].bare(), &m, how)?;
                            bodies[i].set_mesh(made);
                        }
                    }
                }
                bodies.retain(|b| !b.mesh.is_empty());
            }
        }
        Ok(())
    }

    /// The middle of what feature `id` adds or removes, for showing where its copies will go.
    pub fn tool_center(&self, id: Id, built: &Built) -> Option<DVec3> {
        let bodies=built.bodies.iter().map(Body::local_copy).collect::<Result<Vec<_>,_>>().ok()?;
        let feature=self.feature(id)?;
        let (shape, _) = self.tool(feature, &bodies, built).ok()??;
        shape.bbox().map(|(lo, hi)| built.component_placement(feature.owner).transform_point3((lo + hi) / 2.0))
    }

    /// The axis line of a revolve in sketch coordinates, for drawing.
    pub fn axis_line(s: &Sketch, axis: Axis) -> Option<(DVec2, DVec2)> {
        match axis {
            Axis::X => Some((DVec2::ZERO, DVec2::X)),
            Axis::Y => Some((DVec2::ZERO, DVec2::Y)),
            Axis::Line(l) => match s.entities.get(&l)?.geom {
                Geom::Line { .. } => s.line(l),
                _ => None,
            },
        }
    }
}

/// A document being worked on: its built bodies, undo history and file.
pub struct Session {
    pub doc: Document,
    pub built: Built,
    undo: Vec<Document>,
    redo: Vec<Document>,
    checkpoint: Option<Checkpoint>,
    pub path: Option<PathBuf>,
    /// Changed since it was last saved.
    pub dirty: bool,
    /// Goes up every time the bodies are rebuilt.
    pub rev: u64,
    /// Goes up whenever the document may have changed.
    pub edits: u64,
    /// The file at `path` is (or was last written as) a ZIP container rather than plain JSON.
    pub container: bool,
    /// The bodies came from the file's geometry cache rather than a rebuild (until the next edit).
    pub from_cache: bool,
    /// The design was written by a newer Ferrender: only its cached geometry is shown, and nothing can be edited.
    pub read_only: bool,
    /// How long the last full rebuild took, which decides whether a save writes a cache.
    pub rebuild_ms: u32,
    /// Whether saves write the geometry cache.
    pub cache_policy: CachePolicy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CachePolicy {
    /// Cache when the file is a container anyway or the design takes a while to rebuild.
    #[default]
    Auto,
    Always,
    Never,
}

/// History metadata belonging to the most recent in-progress edit. Keeping
/// this separate lets failed commands and cancelled drags restore redo too.
struct Checkpoint {
    redo: Vec<Document>,
    dirty: bool,
    evicted: Option<Document>,
}

impl Default for Session {
    fn default() -> Self {
        Session::new(Document::new(Unit::Mm))
    }
}

impl Session {
    pub fn new(mut doc: Document) -> Session {
        let t = std::time::Instant::now();
        let built = doc.rebuild();
        Session { doc, built, undo: Vec::new(), redo: Vec::new(), checkpoint: None, path: None, dirty: false, rev: 1, edits: 0, container: false, from_cache: false, read_only: false, rebuild_ms: t.elapsed().as_millis() as u32, cache_policy: CachePolicy::Auto }
    }

    /// A session from a file's design and, when it matches and verifies, its geometry cache.
    pub fn with_cache(mut doc: Document, cache: Option<&crate::cache::Cache>) -> Session {
        let restored = cache.filter(|c| c.matches(&doc)).and_then(|c| c.restore().ok());
        let Some(restored) = restored else { return Session::new(doc) };
        let built = doc.rebuild_with(Some(restored));
        Session { doc, built, undo: Vec::new(), redo: Vec::new(), checkpoint: None, path: None, dirty: false, rev: 1, edits: 0, container: false, from_cache: true, read_only: false, rebuild_ms: 0, cache_policy: CachePolicy::Auto }
    }

    /// A read-only view of a newer file's cached geometry.
    fn from_cache_only(cache: &crate::cache::Cache) -> Result<Session, String> {
        let restored = cache.restore().map_err(|e| format!("this file was written by a newer version of Ferrender, and its saved geometry could not be shown: {e}"))?;
        let mut built = Built { placements_applied: true, ..Default::default() };
        built.components.insert(0, crate::components::BuiltComponent { placement: DAffine3::IDENTITY, visible: true });
        built.bodies = restored.bodies;
        built.planes = restored.planes;
        let doc = Document::new(Unit::Mm);
        Ok(Session { doc, built, undo: Vec::new(), redo: Vec::new(), checkpoint: None, path: None, dirty: false, rev: 1, edits: 0, container: true, from_cache: true, read_only: true, rebuild_ms: 0, cache_policy: CachePolicy::Never })
    }

    /// Records the current state as an undo step; call before changing the document.
    pub fn snapshot(&mut self) {
        self.undo.push(self.doc.clone());
        let evicted = (self.undo.len() > 200).then(|| self.undo.remove(0));
        self.checkpoint = Some(Checkpoint { redo: std::mem::take(&mut self.redo), dirty: self.dirty, evicted });
        self.dirty = true;
        self.edits += 1;
    }

    /// Drops the last undo step and returns to it, abandoning a change in progress.
    pub fn abort(&mut self) {
        let Some(checkpoint) = self.checkpoint.take() else { return };
        if let Some(d) = self.undo.pop() {
            self.doc = d;
            self.redo = checkpoint.redo;
            self.dirty = checkpoint.dirty;
            if let Some(evicted) = checkpoint.evicted {
                self.undo.insert(0, evicted);
            }
            // Revisions stay monotonic: a cancelled drag may already have been
            // drawn, and recovery/rendering must observe the restored document.
            self.rebuild();
        }
    }

    /// The document as it was at the last snapshot.
    pub fn before(&self) -> Option<&Document> {
        self.undo.last()
    }

    pub fn rebuild(&mut self) {
        let t = std::time::Instant::now();
        self.built = self.doc.rebuild();
        self.rebuild_ms = t.elapsed().as_millis() as u32;
        self.from_cache = false;
        self.rev += 1;
        self.edits += 1;
    }

    /// Applies a change as one undo step; an error leaves the document as it was.
    pub fn edit<T>(&mut self, f: impl FnOnce(&mut Document) -> Result<T, String>) -> Result<T, String> {
        if self.read_only {
            return Err("this design was written by a newer version of Ferrender and is shown read-only; update Ferrender to edit it".into());
        }
        self.snapshot();
        match f(&mut self.doc) {
            Ok(v) => {
                self.rebuild();
                Ok(v)
            }
            Err(e) => {
                self.abort();
                Err(e)
            }
        }
    }

    /// Like [`Session::edit`], but also undoes the change if the feature `id` fails to build.
    pub fn edit_feature<T>(&mut self, f: impl FnOnce(&mut Document) -> Result<(Id, T), String>) -> Result<T, String> {
        let (id, v) = self.edit(f)?;
        if let Some(e) = self.built.errors.get(&id).cloned() {
            self.abort();
            return Err(e);
        }
        Ok(v)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        self.checkpoint = None;
        let Some(d) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.doc, d));
        self.dirty = true;
        self.rebuild();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.checkpoint = None;
        let Some(d) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.doc, d));
        self.dirty = true;
        self.rebuild();
        true
    }

    /// Saves as plain JSON, or as a container with a rendered thumbnail when the
    /// design carries images or meshes. See [`crate::io::save_with`] for the backup rule.
    pub fn save(&mut self, path: &Path) -> Result<crate::io::Saved, String> {
        self.save_as(path, None)
    }

    /// Like [`Session::save`], naming the application in the container manifest.
    pub fn save_as(&mut self, path: &Path, app: Option<String>) -> Result<crate::io::Saved, String> {
        if self.read_only {
            return Err("this design was written by a newer version of Ferrender and is shown read-only; it cannot be saved from here".into());
        }
        let mut extras = crate::io::Extras { thumbnail_png: None, app, cache: None };
        let wants_cache = match self.cache_policy {
            CachePolicy::Always => true,
            CachePolicy::Never => false,
            CachePolicy::Auto => crate::io::needs_container(&self.doc) || self.rebuild_ms >= crate::cache::WORTH_CACHING_MS,
        };
        if wants_cache && !self.built.bodies.is_empty() && !self.built.errors.keys().any(|id| self.doc.feature(*id).is_some_and(|f| !f.suppressed)) {
            extras.cache = crate::cache::Cache::capture(&self.doc, &self.built)?;
        }
        if (crate::io::needs_container(&self.doc) || extras.cache.is_some()) && self.built.bodies.iter().map(|b| b.mesh.len()).sum::<usize>() <= crate::io::THUMBNAIL_MAX_TRIANGLES {
            let size = crate::io::THUMBNAIL_SIZE;
            extras.thumbnail_png = Some(crate::render::snapshot(self, None, size, size).png());
        }
        let saved = crate::io::save_with(&self.doc, path, &extras)?;
        self.checkpoint = None;
        self.path = Some(path.to_owned());
        self.container = saved.container;
        self.dirty = false;
        Ok(saved)
    }

    /// Opens a file, from its geometry cache when that matches the design, and as a
    /// read-only view of the cache when the design is from a newer Ferrender.
    pub fn open(path: &Path) -> Result<Session, String> {
        let opened = crate::io::open(path)?;
        let mut s = match opened.doc {
            Some(doc) => Session::with_cache(doc, opened.cache.as_ref()),
            None => Session::from_cache_only(opened.cache.as_ref().ok_or("this file was written by a newer version of Ferrender")?)?,
        };
        s.path = Some(path.to_owned());
        s.container = opened.container;
        Ok(s)
    }

    /// Bodies that are not hidden.
    pub fn visible_bodies(&self) -> impl Iterator<Item = &Body> {
        self.built.bodies.iter().filter(|b| !self.doc.hidden_bodies.contains(&b.id) && self.built.component_visible(b.component))
    }
}
