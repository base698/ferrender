//! The document: parameters and an ordered list of features. Bodies are
//! never stored; they are rebuilt from the features.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::{DAffine3, DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::csg::Bool;
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

/// How a swept profile is turned as it travels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepOrient {
    /// The profile turns with the path, staying as square to it as it was where they meet.
    #[default]
    Follow,
    /// The profile keeps the orientation it was drawn in.
    Fixed,
}

impl SweepOrient {
    pub fn name(self) -> &'static str {
        match self { SweepOrient::Follow => "follow", SweepOrient::Fixed => "fixed" }
    }

    pub fn parse(s: &str) -> Option<SweepOrient> {
        match s { "follow" => Some(SweepOrient::Follow), "fixed" => Some(SweepOrient::Fixed), _ => None }
    }
}

/// Carries profiles of one sketch along a path drawn in another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sweep {
    pub sketch: Id,
    pub profiles: Vec<Vec<Id>>,
    /// The sketch holding the path.
    pub path_sketch: Id,
    /// The entities of that sketch that make up the path, joined end to end.
    /// Empty means every entity of the sketch that is not construction geometry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<Id>,
    /// The parts of the path to sweep, each a start and an end as fractions of the
    /// path's length from 0 to 1, for example `[[0.1, 0.3], [0.6, 0.7]]`.
    /// Empty means the whole path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<[f64; 2]>,
    #[serde(default)]
    pub orient: SweepOrient,
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
            let mut resolved_tag=None;
            if let Some(tag) = &self.tag {
                let (f,found_level)=exact::resolve_face(&body.solids,&body.tags,&exact::FacePick {points:[original,mapped],tag:Some(tag.clone())})?;
                level=found_level;
                let drop=|p:DVec3|p-f.normal*(p-f.at).dot(f.normal);
                candidates=vec![drop(mapped),drop(original),f.at];
                by_tag=3; resolved_tag=f.tag;
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
                let found_tag = exact::face_tag_at(&body.solids, &body.tags, point);
                if let (Some(wanted), Some(found)) = (&resolved_tag, &found_tag) {
                    if wanted != found { continue; }
                }
                // Keep the user-chosen baseline and orientation on the resolved face.
                plane.origin += point - anchor;
                plane.origin -= normal * (plane.origin - surface.origin).dot(normal);
                found = Some(plane);
                if k >= by_tag { level = Level::Position; }
                learned = found_tag;
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

/// An operation on a mesh body (see `meshops`). Applied to an exact body, it turns the body into a mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshOp {
    pub body: Id,
    pub op: MeshOpKind,
    /// Limits smoothing to a region, or names the region to extrude.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<crate::meshops::RegionSpec>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshOpKind {
    Repair { fill_holes: u32 },
    Decimate { target: u32, #[serde(default)] method: crate::meshops::DecimateMethod, #[serde(default = "yes")] preserve_boundary: bool },
    Smooth { iterations: u32, strength: f64 },
    Subdivide { levels: u32, #[serde(default)] scheme: crate::meshops::Scheme },
    Cut { plane: PlaneRef, #[serde(default)] keep: crate::meshops::Keep, #[serde(default)] cap: bool },
    Mirror { plane: PlaneRef, #[serde(default)] weld: bool },
    Offset { distance: Value, #[serde(default, skip_serializing_if = "Option::is_none")] direction: Option<DVec3> },
    ExtrudeRegion { distance: Value, #[serde(default, skip_serializing_if = "Option::is_none")] direction: Option<DVec3> },
    /// One brush stroke (see `meshops::sculpt`).
    Sculpt { brush: crate::meshops::Brush, at: DVec3, radius: Value, strength: Value },
}

fn yes() -> bool { true }

impl MeshOpKind {
    pub fn name(&self) -> &'static str {
        match self {
            MeshOpKind::Repair { .. } => "mesh_repair",
            MeshOpKind::Decimate { .. } => "mesh_decimate",
            MeshOpKind::Smooth { .. } => "mesh_smooth",
            MeshOpKind::Subdivide { .. } => "mesh_subdivide",
            MeshOpKind::Cut { .. } => "mesh_cut",
            MeshOpKind::Mirror { .. } => "mesh_mirror",
            MeshOpKind::Offset { .. } => "mesh_offset",
            MeshOpKind::ExtrudeRegion { .. } => "mesh_extrude_region",
            MeshOpKind::Sculpt { .. } => "mesh_sculpt",
        }
    }
}

/// A height field from an image: a relief or lithophane, built as a mesh body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relief {
    /// The pixels, embedded like a sketch's reference image. Its placement fields are unused.
    pub image: crate::reference::ReferenceImage,
    /// Where the relief's lower-left corner sits and which way it faces; Z of the plane is up.
    pub plane: Plane,
    pub width: Value,
    /// How high the brightest pixel stands above the plane.
    pub depth: Value,
    /// Slab under the relief; zero leaves an open surface.
    pub base: Value,
    pub resolution: u32,
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub blur: u32,
    #[serde(default = "one")]
    pub gamma: f64,
    pub op: Op,
}

fn one() -> f64 { 1.0 }

impl Relief {
    pub fn params(&self) -> crate::meshops::ReliefParams {
        crate::meshops::ReliefParams { width: self.width.v, depth: self.depth.v, base: self.base.v, resolution: self.resolution, invert: self.invert, blur: self.blur, gamma: self.gamma }
    }

    /// The relief as a mesh in the document's frame.
    pub fn mesh(&self) -> Result<Mesh, String> {
        if self.resolution < 2 || self.resolution > 1200 { return Err("the relief resolution must be between 2 and 1200 cells".into()); }
        if !self.gamma.is_finite() || self.gamma <= 0.0 || self.gamma > 10.0 { return Err("the relief gamma must be between 0 and 10".into()); }
        let pixels = self.image.pixels()?;
        let mut m = crate::meshops::from_image(&pixels, &self.params())?;
        let plane = self.plane;
        let n = plane.normal();
        m.map(|p| plane.to_world(DVec2::new(p.x, p.y)) + n * p.z);
        m.snap();
        Ok(m)
    }
}

/// A script run's place in the timeline: it builds nothing itself, but owns the features
/// the run made (their `made_by`), carries the script and its inputs so it can run again,
/// and suppressing or deleting it does the same to everything it made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptRun {
    pub script_name: String,
    pub source: String,
    /// CRC-32 of `source`, so a changed script file can be noticed.
    pub source_hash: u32,
    pub inputs: serde_json::Value,
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
    Sweep(Sweep),
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
    MeshOp(MeshOp),
    Relief(Relief),
    ScriptRun(ScriptRun),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub suppressed: bool,
    #[serde(default, skip_serializing_if = "is_root")]
    pub owner: Id,
    /// The script run that made this feature, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<Id>,
    /// Stable identity of an output within its script run; never inferred from order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_key: Option<String>,
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
            FeatureKind::Sweep(_) => "sweep",
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
            FeatureKind::MeshOp(m) => m.op.name(),
            FeatureKind::Relief(_) => "relief",
            FeatureKind::ScriptRun(_) => "script",
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
    /// Temporary allocation hints while rerunning a script. Never persisted.
    #[serde(skip)]
    reuse_feature_ids: Vec<(Id, String, std::mem::Discriminant<FeatureKind>)>,
    #[serde(skip)]
    script_command: Option<(String, u32)>,
    #[serde(skip)]
    script_keys: std::collections::BTreeSet<String>,
    #[serde(skip)]
    script_identity_error: Option<String>,
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
    fn cut_tagged(&mut self, tool: &Lumps, tool_tags: &exact::Tags, feature: Id) -> Result<(), String> {
        if self.is_exact() {
            let (made, tags) = exact::boolean_tagged((&self.solids, &self.tags), (tool, tool_tags), Bool::Subtract, feature)?;
            return self.set_exact(made, tags);
        }
        let tool = exact::tessellate(tool)?.0;
        let mut tool = tool; tool.face_ids.clear();
        let made = crate::meshops::boolean(&self.bare(), &tool, Bool::Subtract)?;
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

/// The body list while one feature is applied. Changes are journaled: a body is
/// copied only when the feature first changes or removes it, so a feature that
/// fails part-way can be undone without copying every body beforehand.
pub(crate) struct Bodies {
    list: Vec<Body>,
    /// The ids before the feature, in order.
    order: Vec<Id>,
    /// Bodies as they were before the feature, taken at their first change or removal.
    saved: BTreeMap<Id, Body>,
}

impl Bodies {
    fn new(list: Vec<Body>) -> Self {
        let order = list.iter().map(|b| b.id).collect();
        Bodies { list, order, saved: BTreeMap::new() }
    }

    /// Keeps the feature's changes.
    fn commit(self) -> Vec<Body> {
        self.list
    }

    /// The list as it was before the feature: saved copies where one was taken,
    /// the untouched bodies as they are, and nothing the feature added.
    fn rollback(self) -> Vec<Body> {
        let Bodies { list, order, mut saved } = self;
        let mut current: BTreeMap<Id, Body> = list.into_iter().map(|b| (b.id, b)).collect();
        order.iter().filter_map(|id| saved.remove(id).or_else(|| current.remove(id))).collect()
    }

    fn original(&self, id: Id) -> bool {
        self.order.contains(&id) && !self.saved.contains_key(&id)
    }

    fn push(&mut self, body: Body) {
        self.list.push(body);
    }

    fn remove(&mut self, i: usize) -> Body {
        let id = self.list[i].id;
        if self.original(id) {
            self.saved.insert(id, self.list[i].clone());
        }
        self.list.remove(i)
    }

    fn retain(&mut self, mut keep: impl FnMut(&Body) -> bool) {
        let mut i = 0;
        while i < self.list.len() {
            if keep(&self.list[i]) {
                i += 1;
            } else {
                let body = self.list.remove(i);
                if self.original(body.id) {
                    self.saved.insert(body.id, body);
                }
            }
        }
    }
}

impl std::ops::Deref for Bodies {
    type Target = [Body];
    fn deref(&self) -> &[Body] {
        &self.list
    }
}

impl std::ops::Index<usize> for Bodies {
    type Output = Body;
    fn index(&self, i: usize) -> &Body {
        &self.list[i]
    }
}

impl std::ops::IndexMut<usize> for Bodies {
    fn index_mut(&mut self, i: usize) -> &mut Body {
        let id = self.list[i].id;
        if self.original(id) {
            self.saved.insert(id, self.list[i].clone());
        }
        &mut self.list[i]
    }
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

    /// Reuse only an output's recorded identity and kind, never its timeline position.
    /// Legacy outputs without an identity get fresh IDs, so old references fail
    /// explicitly instead of attaching to a different same-kind output.
    pub fn reuse_feature_ids(&mut self, previous: &[Feature]) {
        self.reuse_feature_ids = previous.iter().filter_map(|f| f.script_key.as_ref().map(|key|
            (f.id, key.clone(), std::mem::discriminant(&f.kind)))).collect();
    }

    pub fn begin_script_run(&mut self) {
        self.script_command = None;
        self.script_keys.clear();
        self.script_identity_error = None;
    }

    pub fn begin_script_command(&mut self, key: String) {
        self.script_command = Some((key, 0));
    }

    pub fn end_script_command(&mut self) -> Result<(), String> {
        self.script_command = None;
        self.script_identity_error.take().map_or(Ok(()), Err)
    }

    pub fn finish_feature_id_reuse(&mut self) {
        self.reuse_feature_ids.clear();
        self.begin_script_run();
    }

    /// Appends a feature, naming it after its type (`Sketch2`, `Extrude1`).
    pub fn add_feature(&mut self, kind: FeatureKind) -> Id {
        // Reserve every copy slot, including copies added by later pattern edits.
        while let Some(end)=self.features.iter().filter(|f|matches!(f.kind,FeatureKind::Pattern(_)|FeatureKind::Split(_)))
            .map(|f|f.id.saturating_mul(1000)).find(|start|self.next_id>*start && self.next_id<start.saturating_add(1000)) {
            self.next_id=end.saturating_add(1000);
        }
        let script_key = self.script_command.as_mut().map(|(key, n)| {
            let value = format!("{key}/{n}");
            *n += 1;
            value
        });
        if let Some(key) = &script_key && !self.script_keys.insert(key.clone()) {
            self.script_identity_error = Some("a modeling command produced the same output identity more than once; give each repeated output a unique output_key (for example, the source sketch point ID)".into());
        }
        let reused = script_key.as_ref().and_then(|key| {
            let matches: Vec<usize> = self.reuse_feature_ids.iter().enumerate().filter_map(|(at, (id, previous_key, previous_kind))|
                (key == previous_key && *previous_kind == std::mem::discriminant(&kind) && self.feature(*id).is_none()).then_some(at)).collect();
            if matches.len() > 1 {
                self.script_identity_error = Some("the previous script run contains ambiguous output identities; its dependent references cannot be reassigned safely".into());
                None
            } else { matches.first().copied() }
        });
        let id = if let Some(at) = reused { self.reuse_feature_ids.remove(at).0 } else { let id = self.next_id; self.next_id += 1; id };
        let target=match &kind {
            FeatureKind::Transform(t)=>Some(t.body), FeatureKind::Combine(c)=>Some(c.target),
            FeatureKind::Blend(b)=>Some(b.body), FeatureKind::Shell(s)=>Some(s.body),
            FeatureKind::Hole(h)=>Some(h.body), FeatureKind::Thread(t)=>Some(t.body), FeatureKind::Text(t)=>t.body,
            FeatureKind::Split(s)=>Some(s.body), FeatureKind::Remove(r)=>r.bodies.first().copied(),
            FeatureKind::MeshOp(m)=>Some(m.body),
            _=>None,
        };
        let owner=match &kind {
            FeatureKind::Pattern(p)=>self.feature(p.source).map(|f|f.owner),
            _=>target.and_then(|id|self.body_owner(id)),
        }.unwrap_or(self.active_component);
        let mut f = Feature { id, name: String::new(), suppressed: false, owner, made_by: None, script_key, kind };
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

    /// Whether a feature is suppressed directly or through the script run that made it.
    /// Inherited suppression is evaluated, never written into the child's own flag.
    pub fn is_suppressed(&self, id: Id) -> bool {
        let mut next = Some(id);
        for _ in 0..=self.features.len() {
            let Some(id) = next else { return false };
            let Some(f) = self.feature(id) else { return true };
            if f.suppressed { return true; }
            next = f.made_by;
        }
        true // A malformed ownership cycle must not make a feature available.
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
            if !self.features.iter().take(self.active()).take_while(|p|p.id!=f.id).any(|p|p.id==id && !self.is_suppressed(p.id)) { return Err("its sketch was deleted, suppressed, or comes later in the timeline".into()); }
            self.sketch(id).ok_or("its sketch was deleted".to_owned())
        };
        match &f.kind {
            FeatureKind::Primitive(p) => { let l = p.solids()?; let t = exact::primitive_tags(&l, p, f.id); Ok(Some((Shape::Exact(l, t), p.op))) }
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
            FeatureKind::Sweep(w) => {
                let s = sk(w.sketch)?;
                let along = sk(w.path_sketch).map_err(|e| e.replace("its sketch", "its path sketch"))?;
                // Both sketches in the frame of the component the sweep belongs to.
                let placed = |id: Id, plane: Plane| plane.transformed(context.component_placement(f.owner).inverse() * context.component_placement(self.feature(id).unwrap().owner));
                let (plane, path_plane) = (placed(w.sketch, s.plane), placed(w.path_sketch, along.plane));
                let all = profile::profiles(s);
                let picked = Self::pick(&all, &w.profiles)?;
                let path = profile::chain(along, &w.path)?;
                let follow = w.orient == SweepOrient::Follow;
                let l = exact::sweep(&picked, &plane, &path, &path_plane, follow, &w.spans)?;
                let t = exact::tag_swept(&l, &picked, &plane, &path, &path_plane, follow, &w.spans, f.id);
                Ok(Some((Shape::Exact(l, t), w.op)))
            }
            FeatureKind::Text(t) if t.op == Op::New => {
                let profiles = t.outlines()?;
                let plane = t.placement(None)?;
                let l = exact::extrude(&profiles.iter().collect::<Vec<_>>(), &plane, 0.0, t.depth.v)?;
                let tags = exact::tag_text(&l, &profiles, &plane, 0.0, t.depth.v, &t.text, f.id);
                Ok(Some((Shape::Exact(l, tags), Op::New)))
            }
            FeatureKind::Import(m) => Ok(Some((Shape::Mesh(m.clone()), Op::New))),
            FeatureKind::Relief(r) => Ok(Some((Shape::Mesh(r.mesh()?), r.op))),
            _ => Ok(None),
        }
    }

    /// Re-evaluates every expression, re-solves the sketches and regenerates the bodies.
    pub fn rebuild(&mut self) -> Built {
        self.rebuild_with(None)
    }

    /// Recomputes which components are shown, after a change that touched no geometry.
    /// A component is shown when it and every ancestor are; the root is always shown.
    pub fn refresh_visibility(&self, built: &mut Built) {
        if let Some(root) = built.components.get_mut(&0) { root.visible = true; }
        for f in self.features.iter().take(self.active()) {
            let FeatureKind::Component(c) = &f.kind else { continue };
            if !built.components.contains_key(&f.id) { continue; }
            let shown = built.components.get(&f.owner).is_some_and(|parent| parent.visible) && c.visible;
            built.components.get_mut(&f.id).unwrap().visible = shown;
        }
    }

    /// Whether two documents build the same geometry: they differ at most in what is
    /// shown, which component is active, and feature names.
    pub fn same_geometry(&self, other: &Document) -> bool {
        fn display_free(doc: &Document) -> Document {
            let mut d = doc.clone();
            d.active_component = 0;
            d.hidden_bodies.clear();
            for f in &mut d.features {
                f.name.clear();
                match &mut f.kind {
                    FeatureKind::Component(c) => c.visible = true,
                    FeatureKind::Sketch(s) => s.visible = true,
                    FeatureKind::Plane(p) => { p.visible = true; p.visibility_pinned = false; }
                    _ => {}
                }
            }
            d
        }
        display_free(self) == display_free(other)
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
        for f in self.features.iter_mut().take(probe.active()).filter(|f| !probe.is_suppressed(f.id) && probe.component_available(f.owner)) {
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
                FeatureKind::MeshOp(m) => match &mut m.op {
                    MeshOpKind::Offset { distance, .. } | MeshOpKind::ExtrudeRegion { distance, .. } => set(distance, Kind::Length),
                    MeshOpKind::Sculpt { brush, radius, strength, .. } => {
                        set(radius, Kind::Length);
                        set(strength, if matches!(brush, crate::meshops::Brush::Smooth | crate::meshops::Brush::Flatten) { Kind::Scalar } else { Kind::Length });
                    }
                    _ => {}
                },
                FeatureKind::Relief(r) => { for v in [&mut r.width, &mut r.depth, &mut r.base] { set(v, Kind::Length); } }
                FeatureKind::Import(_) | FeatureKind::Combine(_) | FeatureKind::Remove(_) | FeatureKind::Split(_) | FeatureKind::ScriptRun(_) | FeatureKind::Sweep(_) => {}
            }
            if let Some(e) = err {
                built.errors.insert(f.id, e);
            }
        }

        built.components.insert(0,crate::components::BuiltComponent {placement:DAffine3::IDENTITY,visible:true});
        let mut counts:BTreeMap<Id,usize>=BTreeMap::new();
        for index in 0..self.active() {
            let f = self.features[index].clone();
            if self.is_suppressed(f.id) || !built.components.contains_key(&f.owner) { continue; }
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
            if matches!(f.kind, FeatureKind::Sketch(_) | FeatureKind::ScriptRun(_)) { continue; }
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
            // its result only after all of those operations have succeeded: the
            // journal copies a body only when the feature first changes it, so a
            // long history does not copy every body for every feature.
            let mut bodies = Bodies::new(std::mem::take(&mut built.bodies));
            let mut next_count = counts.get(&f.owner).copied().unwrap_or(0);
            match self.apply(&mut f, &mut bodies, &mut next_count, &built) {
                Ok(level) => {
                    built.bodies = bodies.commit();
                    counts.insert(f.owner,next_count);
                    if let Some(level) = level { built.resolutions.insert(f.id, level); }
                    // References learn the tags of what they found, so later rebuilds can find it by name.
                    if f != self.features[index] { self.features[index] = f; }
                }
                Err(e) => {
                    built.bodies = bodies.rollback();
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
    fn apply(&self, f: &mut Feature, bodies: &mut Bodies, count: &mut usize, context: &Built) -> Result<Option<Level>, String> {
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
                let (plane,_,level)=self.plane_reference_mut_in(&mut split.plane,context,bodies,component)?;
                let pieces=crate::body_ops::pieces(&bodies[index],plane)?;
                let tags=exact::split_tags(&bodies[index].solids,&bodies[index].tags,&pieces,plane,id);
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
                if t.tag.as_ref().is_none_or(Tag::legacy) { t.tag = learned; }
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
                let tool_tags=exact::tag_text(&tool,&profiles,&plane,z0,z1,&t.text,id);
                let (made, tags) = exact::boolean_tagged((&bodies[i].solids, &bodies[i].tags), (&tool, &tool_tags), operation, id)?;
                bodies[i].set_exact(made, tags)?;
                bodies.retain(|b| !b.mesh.is_empty());
                Ok(Some(level))
            }
            FeatureKind::MeshOp(op) => {
                use crate::meshops as mo;
                let i = find(bodies, op.body)?;
                if !bodies[i].threads.is_empty() {
                    return Err("mesh operations cannot keep modeled threads; put the operation before the thread".into());
                }
                let component = bodies[i].component;
                let mut src = bodies[i].bare();
                if !src.is_welded() { src.weld_exact(); }
                let mask = op.region.as_ref().map(|r| mo::region(&src, r));
                let made = match &op.op {
                    MeshOpKind::Repair { fill_holes } => mo::repair(&src, *fill_holes as usize)?.0,
                    MeshOpKind::Decimate { target, method, preserve_boundary } => mo::decimate_by(&src, *target as usize, *method, *preserve_boundary)?,
                    MeshOpKind::Smooth { iterations, strength } => mo::smooth(&src, *iterations, *strength, mask.as_deref())?,
                    MeshOpKind::Subdivide { levels, scheme } => mo::subdivide(&src, *levels, *scheme)?,
                    MeshOpKind::Mirror { plane, weld } => { let (plane, _) = self.plane_reference_in(plane, context, bodies, component)?; mo::mirror(&src, plane, *weld)? }
                    MeshOpKind::Offset { distance, direction } => mo::offset(&src, distance.v, *direction)?,
                    MeshOpKind::ExtrudeRegion { distance, direction } => mo::extrude_region(&src, mask.as_deref().ok_or("extruding a region needs a \"region\"")?, distance.v, *direction)?,
                    MeshOpKind::Sculpt { brush, at, radius, strength } => mo::sculpt(&src, *brush, *at, radius.v, strength.v)?,
                    MeshOpKind::Cut { plane, keep, cap } => {
                        let (plane, _) = self.plane_reference_in(plane, context, bodies, component)?;
                        let mut pieces = mo::cut(&src, plane, *keep, *cap)?.into_iter();
                        let first = pieces.next().ok_or("the cut left nothing")?;
                        for (k, piece) in pieces.enumerate() {
                            *count += 1;
                            bodies.push(Shape::Mesh(piece).body(id * 1000 + k as Id + 1, format!("Body{count}"), component)?);
                        }
                        first
                    }
                };
                bodies[i].set_mesh(made);
                Ok(None)
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
                        result = crate::meshops::boolean(&result, &t.bare(), op)?;
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
                // Learning a reference also moves its point onto the edge it found, so that a
                // point from a click or a rounded report later tells the edge from another with the same tag.
                if b.tags.len() != b.edges.len() || b.tags.iter().any(|t|t.as_ref().is_none_or(crate::tag::EdgeTag::legacy)) { b.tags = made.picked; b.edges = made.points; }
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
                if sh.tags.len() != sh.faces.len() || sh.tags.iter().any(|t|t.as_ref().is_none_or(Tag::legacy)) { sh.tags = made.picked; sh.faces = made.points; }
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
                    let drill=exact::Drill { diameter: drilled, depth: depth(*at), tip_angle: h.tip_angle.as_ref().map(|v| v.v), head: sizes.head };
                    let tool=exact::drill(*at,dir,&drill)?;
                    let tags=exact::drill_tags(&tool,id,*at,dir,&drill);
                    bodies[i].cut_tagged(&tool,&tags,id)?;
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
                if t.tag.as_ref().is_none_or(Tag::legacy) { t.tag = learned; }
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
                        let plug_tags=exact::cylinder_tags(&plug,id,start,end,"thread:plug");
                        let (filled, tags) = exact::boolean_tagged((&bodies[i].solids, &bodies[i].tags), (&plug, &plug_tags), Bool::Union, id)?;
                        bodies[i].set_exact(filled, tags)?;
                    }
                    if wide || across < major + threads::BED {
                        let clearance=exact::cylinder(start,end,(major+threads::BED)/2.)?;
                        let tags=exact::cylinder_tags(&clearance,id,start,end,"thread:clearance");
                        bodies[i].cut_tagged(&clearance,&tags,id)?;
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
                    let stock_tags=exact::cylinder_tags(&stock,id,stock_start,stock_end,"thread:stock");
                    let core_tags=exact::cylinder_tags(&core,id,stock_start,stock_end,"thread:core");
                    let (tool,tags)=exact::boolean_tagged((&stock,&stock_tags),(&core,&core_tags),Bool::Subtract,id)?;
                    bodies[i].cut_tagged(&tool,&tags,id)?;
                    threads::rod_with_lead(spec.major - room, spec.pitch, length, t.left, [ends[0] || caps[0] > 0.0, ends[1] || caps[1] > 0.0])?
                };
                thread.map(|v| start + turn * v);
                bodies[i].add_thread(thread);
                Ok(Some(level))
            }
            FeatureKind::Pattern(p) => {
                let source = self.features.iter().take_while(|source| source.id != f.id).find(|source|source.id == p.source && !self.is_suppressed(source.id)).ok_or("the feature it repeats is missing or suppressed")?;
                if context.errors.contains_key(&source.id) || !context.components.contains_key(&source.owner) {
                    return Err("the feature it repeats could not be built".into());
                }
                let (tool, op) = self.tool(source, bodies, context)?.ok_or("only extrudes, revolves, sweeps, primitives, imports and standalone text can be patterned")?;
                let mut landed = 0;
                for (k, place) in p.placements()?.iter().enumerate() {
                    // Include the pattern feature in copy identity; separate patterns of the
                    // same source must not give their faces the same tags.
                    let copy = tool.copied(place, id * 1000 + k as u32 + 1);
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
    fn merge(tool: Shape, op: Op, id: Id, component: Id, bodies: &mut Bodies, count: &mut usize) -> Result<(), String> {
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
                            m = crate::meshops::boolean(&bodies[*i].bare(), &m, Bool::Union)?;
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
                            let made = crate::meshops::boolean(&bodies[i].bare(), &m, how)?;
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
        // The newer timeline cannot be interpreted, but cached bodies already
        // carry world-space geometry. Keep every referenced owner available so
        // non-root bodies are visible in the read-only preview and exports.
        for component in restored.bodies.iter().map(|b| b.component).chain(restored.planes.values().map(|p| p.component)) {
            built.components.entry(component).or_insert(crate::components::BuiltComponent { placement: DAffine3::IDENTITY, visible: true });
        }
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
        if self.restore_checkpoint() {
            // Revisions stay monotonic: a cancelled drag may already have been
            // drawn, and recovery/rendering must observe the restored document.
            self.rebuild();
        }
    }

    /// Puts the document back as it was at the last snapshot, without rebuilding.
    /// False when there is no change in progress to abandon.
    fn restore_checkpoint(&mut self) -> bool {
        let Some(checkpoint) = self.checkpoint.take() else { return false };
        let Some(d) = self.undo.pop() else { return false };
        self.doc = d;
        self.redo = checkpoint.redo;
        self.dirty = checkpoint.dirty;
        if let Some(evicted) = checkpoint.evicted {
            self.undo.insert(0, evicted);
        }
        true
    }

    /// The document as it was at the last snapshot.
    pub fn before(&self) -> Option<&Document> {
        self.undo.last()
    }

    /// A copy for a worker thread: the document and its built state, no history.
    pub fn fork(&self) -> Session {
        Session { doc: self.doc.clone(), built: self.built.clone(), undo: Vec::new(), redo: Vec::new(), checkpoint: None, path: self.path.clone(), dirty: self.dirty, rev: self.rev, edits: self.edits, container: self.container, from_cache: false, read_only: self.read_only, rebuild_ms: self.rebuild_ms, cache_policy: self.cache_policy }
    }

    /// How many undo steps there are, so a run of many edits can later be folded into one.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Drops the undo steps above `depth`, so everything since then undoes as one step.
    pub fn collapse_undo(&mut self, depth: usize) {
        self.undo.truncate(depth);
        self.redo.clear();
        self.checkpoint = None;
    }

    pub fn rebuild(&mut self) {
        // A future-version session has only a placeholder Document. Rebuilding
        // it would silently replace the only available cached geometry with nothing.
        if self.read_only { return; }
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

    /// Applies a change that cannot alter geometry (what is shown, the active component,
    /// a name) as one undo step without rebuilding. The bodies stay as they are, component
    /// visibility is refreshed from the document, and the revision goes up so views redraw.
    pub fn edit_without_rebuild<T>(&mut self, f: impl FnOnce(&mut Document) -> Result<T, String>) -> Result<T, String> {
        if self.read_only {
            return Err("this design was written by a newer version of Ferrender and is shown read-only; update Ferrender to edit it".into());
        }
        self.snapshot();
        match f(&mut self.doc) {
            Ok(v) => {
                self.refresh_display();
                Ok(v)
            }
            Err(e) => {
                if self.restore_checkpoint() {
                    self.refresh_display();
                }
                Err(e)
            }
        }
    }

    /// After a change to what is shown: the same bodies, under the document's current visibility.
    fn refresh_display(&mut self) {
        self.doc.refresh_visibility(&mut self.built);
        self.rev += 1;
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
        let same_geometry = d.same_geometry(&self.doc);
        self.redo.push(std::mem::replace(&mut self.doc, d));
        self.dirty = true;
        self.restore_built(same_geometry);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.checkpoint = None;
        let Some(d) = self.redo.pop() else { return false };
        let same_geometry = d.same_geometry(&self.doc);
        self.undo.push(std::mem::replace(&mut self.doc, d));
        self.dirty = true;
        self.restore_built(same_geometry);
        true
    }

    /// Brings the built state to the document after an undo or redo: a step that
    /// changed only what is shown keeps its bodies; anything else rebuilds.
    fn restore_built(&mut self, same_geometry: bool) {
        if same_geometry && !self.read_only {
            self.edits += 1;
            self.refresh_display();
        } else {
            self.rebuild();
        }
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
        let mut extras = crate::io::Extras { thumbnail_png: None, app, cache: None, cache_skipped: None };
        let wants_cache = match self.cache_policy {
            CachePolicy::Always => true,
            CachePolicy::Never => false,
            CachePolicy::Auto => crate::io::needs_container(&self.doc) || self.rebuild_ms >= crate::cache::WORTH_CACHING_MS,
        };
        if wants_cache {
            if self.built.bodies.is_empty() {
                extras.cache_skipped = Some("there are no bodies to cache".into());
            } else if self.built.errors.keys().any(|id| self.doc.feature(*id).is_some_and(|f| !f.suppressed)) {
                extras.cache_skipped = Some("the design has feature errors".into());
            } else {
                match crate::cache::Cache::capture(&self.doc, &self.built) {
                    Ok(cache) => extras.cache = Some(cache),
                    Err(crate::cache::NoCache::Skipped(reason)) => extras.cache_skipped = Some(reason),
                    Err(crate::cache::NoCache::Failed(e)) => return Err(e),
                }
            }
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

    /// How displayed geometry was obtained. A future timeline is never verified
    /// by this build, even when its saved preview has a local authentication MAC.
    pub fn geometry_trust(&self) -> &'static str {
        if self.read_only { "unverified_preview" }
        else if self.from_cache { "local_authenticated_cache" }
        else { "rebuilt" }
    }

    /// Bodies that are not hidden.
    pub fn visible_bodies(&self) -> impl Iterator<Item = &Body> {
        self.built.bodies.iter().filter(|b| !self.doc.hidden_bodies.contains(&b.id) && self.built.component_visible(b.component))
    }
}
