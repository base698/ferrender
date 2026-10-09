use std::path::{Path, PathBuf};

use egui::{Color32, Context, Key, Modifiers, Pos2, Rect, ViewportCommand};
use fr_core::doc::{Blend, Combine, Extrude, Hole, HoleFit, HoleShape, Pattern, PatternKind, LinearDirection, Revolve, Shell, Sweep, SweepOrient, Text, Thread, Transform};
pub use fr_core::face::Face;
use fr_core::render::Camera;
use fr_core::sketch::Clip;
use fr_core::{Axis, Built, CKind, Document, FeatureKind, Geom, Id, Kind, ORIGIN, Op, Plane, Session, Sketch, Unit, api, io, solver};
use glam::{DVec2, DVec3};
use serde_json::json;

use crate::ai::Assistant;
use crate::bridge::Bridge;
use crate::config::{Appearance, Config};
use crate::recovery::{Found, Recovery};
use crate::{panels, theme, view};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Model,
    /// Editing the sketch feature with this id.
    Sketch(Id),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Line,
    Rect,
    Circle,
    Arc,
    Arc3,
    TangentArc,
    Spline,
    Point,
    Dimension,
    Polygon,
    /// Click a stretch of an entity to cut it back to its crossings.
    Trim,
    /// Click a face of a body to copy its outline into the sketch.
    Project,
    /// Like Select, but a drag from empty space moves the selection instead of box-selecting.
    Move,
}

impl Tool {
    /// Clicks needed to place one of these.
    pub fn clicks(self) -> usize {
        match self {
            Tool::Line | Tool::Rect | Tool::Circle | Tool::Polygon | Tool::TangentArc => 2,
            Tool::Arc | Tool::Arc3 => 3,
            Tool::Spline => 4,
            Tool::Point => 1,
            Tool::Select | Tool::Move | Tool::Dimension | Tool::Trim | Tool::Project => 0,
        }
    }

    pub fn hint(self, placed: usize) -> &'static str {
        match (self, placed) {
            (Tool::Select, _) => "Click to select, drag to move. Shift-click adds to the selection.",
            (Tool::Move, _) => "Drag a point or entity to move it, or select geometry and drag anywhere to move it all. Escape returns to Select.",
            (Tool::Line, 0) => "Click to start a line.",
            (Tool::Line, _) => "Click the next point. Hold Shift to lock the angle; Tab types an exact angle. Esc ends the line.",
            (Tool::Rect, 0) => "Click the first corner.",
            (Tool::Rect, _) => "Click the opposite corner.",
            (Tool::Circle, 0) => "Click the centre.",
            (Tool::Circle, _) => "Click to set the radius.",
            (Tool::Arc, 0) => "Click the arc's centre.",
            (Tool::Arc, 1) => "Click where the arc starts.",
            (Tool::Arc, _) => "Click where the arc ends.",
            (Tool::Arc3, 0) => "Click the arc's start point.",
            (Tool::Arc3, 1) => "Click the arc’s other endpoint.",
            (Tool::Arc3, _) => "Move to choose the bulge, then click. Type a diameter for an exact size.",
            (Tool::TangentArc, 0) => "Click a line or arc endpoint. At a junction, select the source edge first.",
            (Tool::TangentArc, _) => "Click the new endpoint. Hold Shift to keep the sweep angle fixed; the join stays tangent.",
            (Tool::Spline, 0) => "Click the spline's start point.",
            (Tool::Spline, 1) => "Click the first interior fit point.",
            (Tool::Spline, 2) => "Click the second interior fit point.",
            (Tool::Spline, _) => "Click the spline's end point. Drag its four fit points later to refine the curve.",
            (Tool::Point, _) => "Click to place a point.",
            (Tool::Polygon, 0) => "Click the polygon's centre. Set the number of sides in the Sketch Palette.",
            (Tool::Polygon, _) => "Click to place a corner.",
            (Tool::Trim, _) => "Click the part of a line, arc or circle to remove. It is cut back to where other geometry crosses it.",
            (Tool::Project, _) => "Click a face of a body to copy its outline into the sketch.",
            (Tool::Dimension, _) => "Click a line, circle or arc to set its size. For an angle or spacing, Shift-select two items first, then choose Dimension.",
        }
    }
}

/// Where a click in a sketch lands after snapping.
#[derive(Clone, Copy, Debug)]
pub struct Snap {
    pub p: DVec2,
    /// An existing point to reuse.
    pub point: Option<Id>,
    /// An entity the new point should stay on.
    pub on: Option<Id>,
    /// The segment from the previous click is horizontal or vertical.
    pub h: bool,
    pub v: bool,
    /// The point landed on the sketch's X axis (y = 0) or Y axis (x = 0), and stays there.
    pub axis: [bool; 2],
    /// The point is the midpoint of the entity in `on`, and stays there.
    pub mid: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Drag {
    None,
    Point(Id),
    /// Moving whole entities: their points and where each was grabbed relative to the pointer.
    Move(Vec<(Id, DVec2)>),
    Radius(Id),
    Box(Pos2),
    /// Sliding the Move dialog's body across the screen.
    Body,
    /// Pulling the Extrude dialog's arrow.
    Arrow,
}

/// The path half of the Sweep dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SweepDlg {
    /// The sketch the path is drawn in; set by clicking one of its lines or curves.
    pub path_sketch: Option<Id>,
    /// The entities to follow; empty means the whole sketch.
    pub path: Vec<Id>,
    /// The parts of the path to sweep, as fractions of its length; empty means all of it.
    pub spans: Vec<[f64; 2]>,
    pub orient: SweepOrient,
}

/// The Extrude, Revolve and Sweep dialogs.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureDlg {
    pub revolve: bool,
    /// Set when this is the Sweep dialog.
    pub sweep: Option<SweepDlg>,
    /// The feature being edited, or none when creating one.
    pub editing: Option<Id>,
    pub sketch: Option<Id>,
    pub profiles: Vec<Vec<Id>>,
    /// Distance or angle.
    pub text: String,
    pub symmetric: bool,
    pub op: Op,
    pub axis: Axis,
    /// The next click on a sketch line sets the axis.
    pub pick_axis: bool,
    /// A flat face of a body to extrude instead of sketch profiles.
    pub face: Option<Face>,
    /// Component-local frame of the selected face.
    pub face_owner: Id,
    /// Wall lean in degrees; empty for straight walls.
    pub taper: String,
    pub through_all: bool,
    /// The next click on a face sets the distance to reach it.
    pub pick_to: bool,
}

/// The Fillet and Chamfer dialogs for the edges of a body.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendDlg {
    pub chamfer: bool,
    pub body: Option<Id>,
    /// A point on each chosen edge.
    pub edges: Vec<DVec3>,
    pub text: String,
    /// The body's bounds when its edges were picked.
    pub frame: Option<[DVec3; 2]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShellDlg {
    pub body: Option<Id>,
    /// The faces to leave open, each with the point that names it.
    pub faces: Vec<(DVec3, Face)>,
    pub text: String,
    pub frame: Option<[DVec3; 2]>,
}

/// The Hole dialog: drilled, counterbored and countersunk holes, sized by hand or from a thread.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleDlg {
    pub body: Option<Id>,
    /// Where each hole enters the body.
    pub at: Vec<DVec3>,
    /// The way the drill goes: into the face that was clicked.
    pub dir: DVec3,
    pub shape: HoleShape,
    pub fit: HoleFit,
    /// A name from the thread catalog.
    pub thread: String,
    pub diameter: String,
    pub through: bool,
    pub depth: String,
    /// A 118 degree drill point at the bottom of a blind hole.
    pub pointed: bool,
    pub modeled: bool,
    pub left: bool,
    /// Counterbore and countersink sizes typed in, rather than taken from the thread.
    pub custom_head: bool,
    pub head_diameter: String,
    pub head_depth: String,
    pub head_angle: String,
    /// Added to every diameter; empty for none.
    pub extra: String,
}

impl HoleDlg {
    /// The hole the dialog describes. Boxes that say `name = value` define the parameter first.
    pub fn hole(&self, d: &mut Document) -> Result<Hole, String> {
        let body = self.body.filter(|_| !self.at.is_empty()).ok_or("Click a flat face where the hole goes.")?;
        let plain = self.fit == HoleFit::Plain;
        let mut some = |on: bool, text: &str, kind: Kind| if on && !text.trim().is_empty() { d.enter(text, kind).map(Some) } else { Ok(None) };
        let own_head = self.shape != HoleShape::Simple && (plain || self.custom_head);
        Ok(Hole {
            body,
            at: self.at.clone(),
            dir: self.dir,
            shape: self.shape,
            fit: self.fit,
            thread: if plain { String::new() } else { self.thread.clone() },
            diameter: some(plain, &self.diameter, Kind::Length)?,
            depth: some(!self.through, &self.depth, Kind::Length)?,
            tip_angle: some(!self.through && self.pointed, "118 deg", Kind::Angle)?,
            head_diameter: some(own_head, &self.head_diameter, Kind::Length)?,
            head_depth: some(own_head && self.shape == HoleShape::Counterbore, &self.head_depth, Kind::Length)?,
            head_angle: some(own_head && self.shape == HoleShape::Countersink, &self.head_angle, Kind::Angle)?,
            modeled: self.fit == HoleFit::Tapped && self.modeled,
            left: self.left,
            extra: some(true, &self.extra, Kind::Length)?,
        })
    }
}

/// The Thread dialog: a modeled thread on a rod or in a hole.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadDlg {
    pub body: Option<Id>,
    /// The point clicked on the cylinder.
    pub face: Option<DVec3>,
    /// The cylinder's diameter, and whether it is a hole.
    pub found: Option<(f64, bool)>,
    pub thread: String,
    pub full: bool,
    pub offset: String,
    pub length: String,
    pub left: bool,
    /// Room for the thread to turn; empty for none.
    pub extra: String,
    pub frame: Option<[DVec3; 2]>,
}

/// Editable text placement and extrusion, with lengths kept as parameter expressions.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDlg {
    pub editing: Option<Id>,
    pub owner: Id,
    pub text: String,
    pub plane: Plane,
    pub height: String,
    pub depth: String,
    pub spacing: String,
    pub angle: String,
    pub x: String,
    pub y: String,
    pub align: fr_core::text::Align,
    pub op: Op,
    pub body: Option<Id>,
    pub face: Option<DVec3>,
    pub frame: Option<[DVec3; 2]>,
}

impl TextDlg {
    pub fn feature(&self, d: &mut Document) -> Result<Text, String> {
        if self.op != Op::New && self.body.zip(self.face).is_none() {
            return Err("Click a flat face to raise or engrave text.".into());
        }
        Ok(Text {
            tag: None,
            text: self.text.clone(), plane: self.plane,
            height: d.enter(&self.height, Kind::Length)?, depth: d.enter(&self.depth, Kind::Length)?,
            spacing: d.enter(&self.spacing, Kind::Length)?, angle: d.enter(&self.angle, Kind::Angle)?,
            x: d.enter(&self.x, Kind::Length)?, y: d.enter(&self.y, Kind::Length)?,
            align: self.align, op: self.op,
            body: if self.op == Op::New { None } else { self.body },
            face: if self.op == Op::New { None } else { self.face },
            frame: if self.op == Op::New { None } else { self.frame },
        })
    }
}

/// File errors stay visible until dismissed, including after a blocking native file picker.
#[derive(Clone, Debug, PartialEq)]
pub struct FileError {
    pub title: String,
    pub path: PathBuf,
    pub message: String,
    pub guidance: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatternDlg {
    pub editing: Option<Id>,
    pub source: Option<Id>,
    /// 0 circular, 1 linear, 2 mirror.
    pub kind: usize,
    pub axis: usize,
    pub count: u32,
    /// Total angle or spacing.
    pub text: String,
    pub second: bool,
    pub axis2: usize,
    pub count2: u32,
    pub text2: String,
}

/// The Mesh menu's operations, in dialog order.
pub const MESH_OPS: [&str; 7] = ["Repair Mesh", "Decimate", "Smooth", "Subdivide", "Cut Mesh", "Mirror Mesh", "Offset / Thicken"];

#[derive(Clone, Debug, PartialEq)]
pub struct MeshDlg {
    pub body: Option<Id>,
    /// Index into [`MESH_OPS`].
    pub kind: usize,
    /// Fill-hole size, target triangles, iterations or levels.
    pub count: u32,
    /// Smoothing strength.
    pub amount: f64,
    /// Method, scheme or kept side.
    pub choice: usize,
    /// Preserve boundary, cap, weld, or straight down.
    pub flag: bool,
    /// Origin plane index for cut and mirror.
    pub plane: usize,
    /// Plane offset or thickness.
    pub text: String,
}

impl MeshDlg {
    pub fn new(kind: usize, body: Option<Id>, unit: Unit) -> Self {
        let count = match kind { 0 => 12, 1 => 50_000, 2 => 10, _ => 1 };
        let text = match kind { 6 => format!("{} {}", if unit == Unit::In { "0.1" } else if unit == Unit::Cm { "0.2" } else { "2" }, unit.name()), _ => format!("0 {}", unit.name()) };
        MeshDlg { body, kind, count, amount: 0.5, choice: 0, flag: true, plane: 0, text }
    }
}

/// The sculpt brush: every click on a body adds one stroke as a feature.
#[derive(Clone, Debug, PartialEq)]
pub struct SculptDlg {
    pub brush: usize,
    pub radius: String,
    pub strength: String,
    pub strokes: usize,
}

pub const BRUSHES: [&str; 5] = ["Pull", "Push", "Inflate", "Smooth", "Flatten"];

#[derive(Clone, Debug, PartialEq)]
pub struct ReliefDlg {
    pub path: PathBuf,
    pub width: String,
    pub depth: String,
    pub base: String,
    pub resolution: u32,
    pub blur: u32,
    pub invert: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransformDlg {
    pub body: Id,
    pub translate: [String; 3],
    pub rotate: [String; 3],
    pub scale: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CombineDlg {
    pub target: Option<Id>,
    pub tools: Vec<Id>,
    pub op: Op,
    pub keep_tools: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PointCoordsDlg {
    pub point: Option<Id>,
    pub x: String,
    pub y: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    None,
    Plane(crate::construction::PlaneDlg),
    Primitive(crate::primitives::PrimitiveDlg),
    Remove(crate::body_ops_ui::RemoveDlg),
    Split(crate::body_ops_ui::SplitDlg),
    MoveComponent(crate::components_ui::MoveDlg),
    DeleteComponent(Id),
    /// Waiting for a plane or a flat face to sketch on.
    PickPlane,
    PointCoordinates(PointCoordsDlg),
    Feature(FeatureDlg),
    Transform(TransformDlg),
    Combine(CombineDlg),
    Pattern(PatternDlg),
    Blend(BlendDlg),
    Shell(ShellDlg),
    Hole(HoleDlg),
    Thread(ThreadDlg),
    Text(TextDlg),
    Measure(MeasureDlg),
    Export(Unit),
    Import(PathBuf, Unit),
    Mesh(MeshDlg),
    Relief(ReliefDlg),
    Sculpt(SculptDlg),
    Script(crate::scripts_ui::ScriptDlg),
}

impl PatternDlg {
    pub fn new(source: Option<Id>) -> Self {
        Self { editing: None, source, kind: 0, axis: 2, count: 4, text: "360 deg".into(),
            second: false, axis2: 1, count2: 2, text2: "10 mm".into() }
    }

    fn from_feature(id: Id, pattern: &Pattern) -> Self {
        let mut dialog = Self { editing: Some(id), ..Self::new(Some(pattern.source)) };
        match &pattern.kind {
            PatternKind::Circular { axis, count, angle } => {
                dialog.axis = *axis; dialog.count = *count; dialog.text = angle.expr.clone();
            }
            PatternKind::Linear { axis, count, spacing, second } => {
                dialog.kind = 1; dialog.axis = *axis; dialog.count = *count; dialog.text = spacing.expr.clone();
                dialog.axis2 = if *axis == 0 { 1 } else { 0 };
                if let Some(direction) = second {
                    dialog.second = true; dialog.axis2 = direction.axis; dialog.count2 = direction.count; dialog.text2 = direction.spacing.expr.clone();
                }
            }
            PatternKind::Mirror { axis } => { dialog.kind = 2; dialog.axis = *axis; }
        }
        dialog
    }

    /// The pattern the dialog describes, resolving any named-value entries.
    pub fn pattern(&self, d: &Document) -> Result<PatternKind, String> {
        let value = |text: &str, kind| {
            let text = text.split_once('=').map_or_else(|| text.to_owned(), |(name, _)| format!("${}", name.trim().trim_start_matches('$')));
            d.value(&text, kind)
        };
        Ok(match self.kind {
            0 => PatternKind::Circular { axis: self.axis, count: self.count, angle: value(&self.text, Kind::Angle)? },
            1 => PatternKind::Linear { axis: self.axis, count: self.count, spacing: value(&self.text, Kind::Length)?,
                second: if self.second { Some(LinearDirection { axis: self.axis2, count: self.count2, spacing: value(&self.text2, Kind::Length)? }) } else { None } },
            _ => PatternKind::Mirror { axis: self.axis },
        })
    }
}

impl FeatureDlg {
    fn taper(&self, d: &Document) -> Result<Option<fr_core::Value>, String> {
        if self.taper.trim().is_empty() { Ok(None) } else { d.value(&self.taper, Kind::Angle).map(Some) }
    }
}

/// A cut-away view: everything on one side of a plane is hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    pub on: bool,
    /// The world axis the cutting plane faces: 0 = X, 1 = Y, 2 = Z.
    pub axis: usize,
    /// A construction plane to cut along instead of a world axis, when it is built.
    pub plane: Option<Id>,
    /// Where the plane sits along that axis or plane normal, in millimetres.
    pub offset: f64,
    pub flip: bool,
}

impl Dialog {
    /// Adds or updates the feature the dialog describes; returns its id.
    pub fn apply(&self, d: &mut Document) -> Result<Id, String> {
        match self {
            Dialog::Plane(p) => p.apply(d),
            Dialog::Remove(p) => p.apply(d),
            Dialog::Split(p) => p.apply(d),
            Dialog::MoveComponent(c) => c.apply(d),
            Dialog::Feature(f) => {
                if let (Some(face), false, None) = (&f.face, f.revolve, f.editing) {
                    let (sk, profiles) = face.sketch()?;
                    let distance = d.enter(&f.text, Kind::Length)?;
                    // Pushing a face inward removes material, as pulling it out adds.
                    let op = if distance.v < 0.0 && f.op == Op::Join { Op::Cut } else { f.op };
                    let sketch = d.add_feature_to(f.face_owner, FeatureKind::Sketch(sk))?;
                    d.feature_mut(sketch).unwrap().name = format!("Face{sketch}");
                    let taper = f.taper(d)?;
                    return d.add_feature_to(f.face_owner, FeatureKind::Extrude(Extrude { sketch, profiles, distance, symmetric: false, op, taper, through_all: f.through_all }));
                }
                let sketch = f.sketch.filter(|_| !f.profiles.is_empty()).ok_or(if f.revolve || f.sweep.is_some() { "Click a closed sketch profile in the viewport." } else { "Click a closed sketch profile or a flat face in the viewport." })?;
                let kind = if let Some(w) = &f.sweep {
                    let path_sketch = w.path_sketch.ok_or("Click a line or curve of the path, drawn in another sketch.")?;
                    if path_sketch == sketch { return Err("The profile and the path must be in different sketches.".into()); }
                    FeatureKind::Sweep(Sweep { sketch, profiles: f.profiles.clone(), path_sketch, path: w.path.clone(), spans: w.spans.clone(), orient: w.orient, op: f.op })
                } else if f.revolve {
                    FeatureKind::Revolve(Revolve { sketch, profiles: f.profiles.clone(), axis: f.axis, angle: d.enter(&f.text, Kind::Angle)?, op: f.op })
                } else {
                    FeatureKind::Extrude(Extrude { sketch, profiles: f.profiles.clone(), distance: d.enter(&f.text, Kind::Length)?, symmetric: f.symmetric, op: f.op, taper: f.taper(d)?, through_all: f.through_all })
                };
                match f.editing {
                    Some(id) => {
                        d.feature_mut(id).ok_or("That feature no longer exists.")?.kind = kind;
                        Ok(id)
                    }
                    None => {
                        if let Some(s) = d.sketch_mut(sketch) {
                            s.visible = false;
                        }
                        if let Some(s) = f.sweep.as_ref().and_then(|w| w.path_sketch).and_then(|id| d.sketch_mut(id)) {
                            s.visible = false;
                        }
                        Ok(d.add_feature(kind))
                    }
                }
            }
            Dialog::Primitive(p) => p.apply(d),
            Dialog::Transform(t) => {
                let v = |d: &mut Document, s: &String, kind| d.enter(if s.trim().is_empty() { "0" } else { s }, kind);
                let translate = [v(d, &t.translate[0], Kind::Length)?, v(d, &t.translate[1], Kind::Length)?, v(d, &t.translate[2], Kind::Length)?];
                let rotate = [v(d, &t.rotate[0], Kind::Angle)?, v(d, &t.rotate[1], Kind::Angle)?, v(d, &t.rotate[2], Kind::Angle)?];
                let scale = d.enter(if t.scale.trim().is_empty() { "1" } else { &t.scale }, Kind::Scalar)?;
                Ok(d.add_feature(FeatureKind::Transform(Transform { body: t.body, translate, rotate, scale })))
            }
            Dialog::Combine(c) => {
                let target = c.target.ok_or("Click the body to keep.")?;
                if c.tools.is_empty() {
                    return Err("Click the bodies to combine with it.".into());
                }
                Ok(d.add_feature(FeatureKind::Combine(Combine { target, tools: c.tools.clone(), op: c.op, keep_tools: c.keep_tools })))
            }
            Dialog::Pattern(p) => {
                let source = p.source.ok_or("Choose the feature to repeat.")?;
                if p.kind < 2 {
                    // Lets `name = value` in the box define a parameter first.
                    d.enter(&p.text, if p.kind == 0 { Kind::Angle } else { Kind::Length })?;
                }
                if p.kind == 1 && p.second { d.enter(&p.text2, Kind::Length)?; }
                let pattern = Pattern { source, kind: p.pattern(d)? };
                pattern.validate()?;
                if let Some(id) = p.editing {
                    let owner = d.feature(source).ok_or("The source feature no longer exists.")?.owner;
                    let index = d.features.iter().position(|f| f.id == id).ok_or("The pattern no longer exists.")?;
                    if !d.features.iter().take(index).any(|f| f.id == source && !d.is_suppressed(f.id)) { return Err("Choose a source before this pattern in the timeline.".into()); }
                    let feature = d.feature_mut(id).unwrap();
                    if feature.owner != owner { return Err("Choose a source in the pattern's component.".into()); }
                    feature.kind = FeatureKind::Pattern(pattern);
                    Ok(id)
                } else { Ok(d.add_feature(FeatureKind::Pattern(pattern))) }
            }
            Dialog::Blend(b) => {
                let body = b.body.filter(|_| !b.edges.is_empty()).ok_or("Click the edges to blend.")?;
                let size = d.enter(&b.text, Kind::Length)?;
                Ok(d.add_feature(FeatureKind::Blend(Blend { body, edges: b.edges.clone(), size, chamfer: b.chamfer, frame: b.frame, tags: Vec::new() })))
            }
            Dialog::Shell(sh) => {
                let body = sh.body.filter(|_| !sh.faces.is_empty()).ok_or("Click the faces to leave open.")?;
                let thickness = d.enter(&sh.text, Kind::Length)?;
                Ok(d.add_feature(FeatureKind::Shell(Shell { body, faces: sh.faces.iter().map(|f| f.0).collect(), thickness, frame: sh.frame, tags: Vec::new() })))
            }
            Dialog::Hole(h) => {
                let hole = h.hole(d)?;
                hole.sizes()?;
                Ok(d.add_feature(FeatureKind::Hole(hole)))
            }
            Dialog::Text(t) => {
                let kind = FeatureKind::Text(t.feature(d)?);
                match t.editing {
                    Some(id) => {
                        let feature = d.feature_mut(id).ok_or("That text feature no longer exists.")?;
                        if feature.owner != t.owner { return Err("An existing text feature cannot move to another component. Create new text in the target component instead.".into()); }
                        feature.kind = kind;
                        Ok(id)
                    }
                    None => d.add_feature_to(t.owner, kind),
                }
            }
            Dialog::Mesh(m) => {
                use fr_core::doc::{MeshOp, MeshOpKind};
                use fr_core::meshops::{DecimateMethod, Keep, Scheme};
                use fr_core::planes::{OriginPlane, PlaneRef};
                let body = m.body.ok_or("Click a body first.")?;
                let offset = d.enter(&m.text, Kind::Length)?;
                let plane = || -> Result<PlaneRef, String> {
                    let base = [OriginPlane::XY, OriginPlane::XZ, OriginPlane::YZ][m.plane];
                    Ok(if offset.v.abs() < 1e-12 { PlaneRef::Origin(base) } else { PlaneRef::Free(base.plane().offset(offset.v)) })
                };
                let op = match m.kind {
                    0 => MeshOpKind::Repair { fill_holes: m.count },
                    1 => MeshOpKind::Decimate { target: m.count.max(4), method: if m.choice == 1 { DecimateMethod::Cluster } else { DecimateMethod::Quadric }, preserve_boundary: m.flag },
                    2 => MeshOpKind::Smooth { iterations: m.count.max(1), strength: m.amount },
                    3 => MeshOpKind::Subdivide { levels: m.count.clamp(1, 6), scheme: if m.choice == 1 { Scheme::Midpoint } else { Scheme::Loop } },
                    4 => MeshOpKind::Cut { plane: plane()?, keep: [Keep::Negative, Keep::Positive, Keep::Both][m.choice.min(2)], cap: m.flag },
                    5 => MeshOpKind::Mirror { plane: plane()?, weld: m.flag },
                    _ => MeshOpKind::Offset { distance: offset, direction: m.flag.then_some(glam::DVec3::NEG_Z) },
                };
                Ok(d.add_feature(FeatureKind::MeshOp(MeshOp { body, op, region: None })))
            }
            Dialog::Relief(r) => {
                let image = fr_core::reference::ReferenceImage::from_file(&r.path, 100.0)?;
                let relief = fr_core::doc::Relief { image, plane: Plane::XY, width: d.enter(&r.width, Kind::Length)?, depth: d.enter(&r.depth, Kind::Length)?, base: d.enter(&r.base, Kind::Length)?, resolution: r.resolution.clamp(2, 1200), invert: r.invert, blur: r.blur.min(64), gamma: 1.0, op: Op::New };
                let id = d.add_feature(FeatureKind::Relief(relief));
                if let Some(n) = r.path.file_stem().map(|n| n.to_string_lossy().into_owned()) { d.feature_mut(id).unwrap().name = n; }
                Ok(id)
            }
            Dialog::Thread(t) => {
                let (body, face) = t.body.zip(t.face).ok_or("Click the rod or the hole to thread.")?;
                let (offset, length) = if t.full { (None, None) } else { (Some(d.enter(&t.offset, Kind::Length)?), Some(d.enter(&t.length, Kind::Length)?)) };
                let extra = if t.extra.trim().is_empty() { None } else { Some(d.enter(&t.extra, Kind::Length)?) };
                Ok(d.add_feature(FeatureKind::Thread(Thread { body, face, frame: t.frame, tag: None, thread: t.thread.clone(), offset, length, left: t.left, extra })))
            }
            _ => Err("Nothing to apply.".into()),
        }
    }

    pub fn has_preview(&self) -> bool {
        matches!(self, Dialog::Remove(_) | Dialog::Split(_) | Dialog::Primitive(_) | Dialog::Plane(_) | Dialog::MoveComponent(_) | Dialog::Feature(_) | Dialog::Transform(_) | Dialog::Combine(_) | Dialog::Pattern(_) | Dialog::Blend(_) | Dialog::Shell(_) | Dialog::Hole(_) | Dialog::Thread(_) | Dialog::Text(_) | Dialog::Mesh(_) | Dialog::Relief(_))
    }
}

/// Sizes typed while a shape is being drawn: a rectangle's width and height, a
/// circle's diameter, a line's length. A box with a size in it holds that size
/// whatever the pointer does, and becomes a dimension when the shape is placed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Typed {
    pub fields: Vec<String>,
    /// The box that has the keyboard; Tab moves to the next.
    pub active: usize,
    /// Where the pointer last was, for when it is over the boxes themselves.
    pub last: Option<(DVec2, Option<Id>, Option<Id>)>,
    /// Soft angular inference, retained through small pointer movements.
    pub guide: Option<(f64, &'static str)>,
    /// Explicit Shift lock in degrees: line direction, or signed tangent-arc sweep.
    pub locked: Option<f64>,
    pub shift_down: bool,
}

/// One thing picked with the Inspect tool.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub item: fr_core::measure::Item,
    /// What it is, with its own size: "Edge, 20 mm long".
    pub label: String,
    /// A face's outlines, for drawing.
    pub outline: Vec<Vec<DVec3>>,
}

/// The Inspect tool: the distance between two picked points, edges or faces.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MeasureDlg {
    pub picks: Vec<Picked>,
    pub result: Option<fr_core::measure::Measure>,
}

/// What the dimension box will create or change when confirmed.
#[derive(Clone, Debug, PartialEq)]
pub enum EditTarget {
    Existing(Id),
    New(CKind, Vec<Id>),
    /// Round or chamfer the corner at this point.
    Fillet(Id),
    Chamfer(Id),
    Offset(Vec<Id>),
}

/// The small box for typing a dimension, floating in the viewport.
#[derive(Clone, Debug)]
pub struct ValueEdit {
    pub target: EditTarget,
    pub text: String,
    pub pos: Pos2,
    pub focus: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// New geometry is construction geometry.
    pub construction: bool,
    pub grid: bool,
    pub snap_grid: bool,
    pub constraints: bool,
    pub dimensions: bool,
    pub gaps: bool,
    /// Sides for the polygon tool.
    pub sides: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    CommandSearch,
    /// A Mesh menu operation, by index into [`MESH_OPS`].
    Mesh(usize),
    Relief,
    Sculpt,
    ScriptLog,
    ExportTimelineScript,
    Primitive(usize),
    RemoveBody,
    SplitBody,
    JoinBodies,
    Plane,
    NewComponent,
    ActivateRoot,
    New,
    Open,
    Save,
    SaveAs,
    Recover,
    Import,
    Export,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
    NewSketch,
    FinishSketch,
    Extrude,
    Revolve,
    Sweep,
    Transform,
    Combine,
    Parameters,
    Assistant,
    About,
    View(&'static str),
    Fit,
    Tool(Tool),
    PointCoordinates,
    ReferenceImage,
    Constrain(CKind),
    Construction,
    Fillet,
    Chamfer,
    Offset,
    MirrorSketch,
    Pattern,
    Section,
    /// Fillet (false) or chamfer (true) the edges of a body.
    Blend(bool),
    Shell,
    Hole,
    Thread,
    Text,
    Measure,
    ExportStep,
    Cancel,
}

pub struct App {
    pub command_search: crate::command_search::CommandSearch,
    pub session: Session,
    /// Scale is in points per millimetre.
    pub cam: Camera,
    pub mode: Mode,
    pub tool: Tool,
    /// Selected points, entities and constraints of the active sketch.
    pub sel: Vec<Id>,
    pub sel_body: Option<Id>,
    pub sel_component: Option<Id>,
    last_active_component: Id,
    /// The face last clicked in the viewport.
    pub sel_face: Option<Face>,
    pub sel_feature: Option<Id>,
    /// Clicks placed so far with a drawing tool.
    pub clicks: Vec<Snap>,
    pub dim_refs: Vec<Id>,
    pub drag: Drag,
    /// The unrounded value an arrow drag has reached, in millimetres.
    pub drag_value: f64,
    /// Where the Pattern dialog's source feature sits, and what that was worked out for.
    pub pattern_at: Option<((u64, Id), DVec3)>,
    pub pattern_drag: crate::model_drag::PatternDrag,
    pub gizmo: crate::gizmo::State,
    pub sketch_capture: crate::sketch_capture::Capture,
    pub body_ops_source: Option<(u64, Id, Built)>,
    /// Where the timeline's chips were drawn last frame, for dragging the roll-back marker.
    pub chips: Vec<Rect>,
    pub timeline: crate::timeline::Timeline,
    render_queue: Option<eframe::egui_wgpu::wgpu::Queue>,
    pub dialog: Dialog,
    /// The dialog's result, built on a copy of the document: (dialog it was built for, bodies, error).
    pub preview: Option<(Dialog, Built, Option<String>)>,
    /// Geometry immediately before an edited Text feature, keyed by document revision and feature.
    text_base: Option<(u64, Id, Built)>,
    pub(crate) construction_source: Option<(u64, Id, Built)>,
    /// Local exact source geometry is shared across unchanged Text preview frames.
    text_local: Option<((u64, Option<Id>, Id), fr_core::Body)>,
    pub value_edit: Option<ValueEdit>,
    pub typed: Option<Typed>,
    pub clipboard: Option<Clip>,
    /// Pastes since the last copy, to stagger them.
    pub pastes: u32,
    pub opts: Opts,
    /// The viewport's rectangle on screen.
    pub vp: Rect,
    /// Clickable dimension labels and constraint badges from the last frame.
    pub labels: Vec<(Rect, Id)>,
    pub gap_cache: Option<(u64, Id, Vec<Id>)>,
    pub open_requests: crate::open_requests::OpenRequests,
    pub reference_editor: crate::reference::Editor,
    pub reference_texture: crate::reference::TextureCache,
    pub reference_drag: crate::reference_drag::State,
    pub report: solver::Report,
    pub toast: Option<(String, f64)>,
    pub file_error: Option<FileError>,
    pub now: f64,
    pub show_about: bool,
    pub show_params: bool,
    pub show_section: bool,
    pub section: Section,
    /// Distance from the chosen plane or face for a new sketch.
    pub plane_offset: String,
    pub param_new: (String, String),
    pub param_edit: Option<(String, String)>,
    pub rename: Option<(Id, String)>,
    /// The GPU viewport is available; otherwise bodies are drawn in software.
    pub gpu: bool,
    pub scene: view::Scene,
    pub scripts: crate::scripts_ui::Scripts,
    pub fit_pending: bool,
    pub bridge: Option<Bridge>,
    /// This app's recovery copy; none while testing unless a test sets one.
    pub recovery: Option<Recovery>,
    /// Unsaved designs left by an app that did not close, offered for recovery.
    pub recover: Vec<Found>,
    pub ai: Assistant,
    pub config: Config,
    pub recent: crate::recent::RecentFiles,
    native_theme: crate::native_theme::NativeTheme,
    pub ctx: Context,
    title: String,
}

const CLIP_MARK: &str = "ferrender_clip";
/// The room a modeled thread is given unless told otherwise, in millimetres across: enough for
/// a printed thread to turn on most printers.
pub const PRINT_ALLOWANCE: f64 = 0.2;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>) -> App {
        let (config, config_error) = if cfg!(test) { (Config::default(), None) } else { Config::load() };
        let (recent, recent_error) = if cfg!(test) { (crate::recent::RecentFiles::default(), None) } else { crate::recent::RecentFiles::load(Config::path().with_file_name("recent.json")) };
        setup_style(&cc.egui_ctx, config.appearance);
        let native_theme = crate::native_theme::NativeTheme::new(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, config.appearance, native_theme.current());
        let gpu = match &cc.wgpu_render_state {
            Some(rs) => {
                rs.renderer.write().callback_resources.insert(crate::gpu::Gpu::new(&rs.device, rs.target_format));
                true
            }
            None => false,
        };
        let mut app = App {
            command_search: Default::default(),
            session: Session::default(),
            cam: Camera::iso(),
            mode: Mode::Model,
            tool: Tool::Select,
            sel: Vec::new(),
            sel_body: None,
            sel_component: None,
            last_active_component: 0,
            sel_face: None,
            sel_feature: None,
            clicks: Vec::new(),
            dim_refs: Vec::new(),
            drag: Drag::None,
            drag_value: 0.0,
            pattern_at: None,
            pattern_drag: Default::default(),
            gizmo: Default::default(),
            sketch_capture: Default::default(),
            body_ops_source: None,
            chips: Vec::new(),
            timeline: crate::timeline::Timeline::default(),
            render_queue: cc.wgpu_render_state.as_ref().map(|rs| rs.queue.clone()),
            dialog: Dialog::None,
            preview: None,
            text_base: None,
            construction_source: None,
            text_local: None,
            value_edit: None,
            typed: None,
            clipboard: None,
            pastes: 0,
            opts: Opts { construction: false, grid: true, snap_grid: false, constraints: true, dimensions: true, gaps: true, sides: 6 },
            vp: Rect::NOTHING,
            labels: Vec::new(),
            gap_cache: None,
            open_requests: Default::default(),
            reference_editor: Default::default(),
            reference_texture: Default::default(),
            reference_drag: Default::default(),
            report: solver::Report::default(),
            toast: None,
            file_error: None,
            now: 0.0,
            show_about: false,
            show_params: false,
            show_section: false,
            section: Section { on: false, axis: 1, plane: None, offset: 0.0, flip: false },
            plane_offset: String::new(),
            param_new: Default::default(),
            param_edit: None,
            rename: None,
            gpu,
            scene: view::Scene::default(),
            scripts: Default::default(),
            fit_pending: false,
            bridge: if cfg!(test) || !config.bridge.enabled { None } else { Bridge::start(config.bridge.port, cc.egui_ctx.clone()) },
            recovery: if cfg!(test) { None } else { Config::path().parent().map(|d| Recovery::start(d.join("recovery"))) },
            recover: Vec::new(),
            ai: Assistant::default(),
            config,
            recent,
            native_theme,
            ctx: cc.egui_ctx.clone(),
            title: String::new(),
        };
        if let Some(e) = config_error {
            app.toast(format!("Couldn't read the settings file, using defaults. {e}"));
        }
        if let Some(e) = recent_error { app.toast(e); }
        if let Some(f) = file {
            app.open_path(&f);
        }
        app.recover = app.recovery.as_ref().map_or(Vec::new(), Recovery::found);
        app
    }

    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.config.appearance = appearance;
        theme::apply(&self.ctx, appearance, self.native_theme.current());
        if !cfg!(test) && let Err(error) = self.config.save() {
            self.toast(format!("Appearance changed for this window, but couldn't be saved. {error}"));
        }
    }

    /// Opens an unsaved design left by an app that did not close.
    pub fn recover(&mut self, found: &Found) {
        if !self.confirm_discard() {
            return;
        }
        match found.load() {
            Ok(doc) => {
                let mut s = Session::new(doc);
                (s.path, s.dirty) = (found.path.clone(), true);
                self.replace_session(s);
                // The old copy goes only once this app holds its own.
                match self.recovery.as_mut().map(|r| r.write_now(&self.session)) {
                    Some(Err(e)) => self.toast(e),
                    _ => {
                        found.delete();
                        self.recover.retain(|f| f != found);
                        self.toast(format!("Recovered {}. Save it to keep it.", found.name()));
                    }
                }
            }
            Err(e) => self.toast(e),
        }
    }

    /// The body that Move and Combine act on: the selected one, or the one whose face is selected.
    pub fn target_body(&self) -> Option<Id> {
        self.sel_body.or(self.sel_face.as_ref().map(|f| f.body))
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), self.now + 4.0));
    }

    fn file_error(&mut self, title: &str, path: &Path, message: String, guidance: &'static str) {
        self.file_error = Some(FileError { title: title.into(), path: path.to_owned(), message, guidance });
        // A native picker may block longer than a toast's entire lifetime. This is
        // independent of frame time and requests a fresh frame when the picker returns.
        self.ctx.request_repaint();
    }

    pub fn doc(&self) -> &Document {
        &self.session.doc
    }

    /// The sketch being edited.
    pub fn sketch(&self) -> Option<(Id, &Sketch)> {
        match self.mode {
            Mode::Sketch(id) => self.session.doc.sketch(id).map(|s| (id, s)),
            Mode::Model => None,
        }
    }

    /// The bodies to show: the open dialog's preview if it has one.
    pub fn shown(&self) -> &Built {
        match &self.preview {
            Some((_, built, _)) => built,
            None => &self.session.built,
        }
    }

    /// Brings the interface back in line after the document changed under it.
    pub fn refresh(&mut self) {
        self.sketch_capture.clear();
        self.gizmo.clear();
        self.pattern_drag.clear();
        self.body_ops_source = None;
        if self.last_active_component != 0 && self.doc().active_component == 0 && !self.session.built.components.contains_key(&self.last_active_component) {
            self.toast("The active component is unavailable at this history position. Root is now active.");
        }
        self.last_active_component = self.doc().active_component;
        if self.sel_component.is_some_and(|id| !self.session.built.components.contains_key(&id)) { self.sel_component = None; }
        if let Mode::Sketch(id) = self.mode {
            match self.session.doc.sketch(id) {
                Some(sk) => {
                    self.report = solver::solve(&mut sk.clone(), &[]);
                    let sk = self.session.doc.sketch(id).unwrap();
                    let alive = |i: &Id| sk.points.contains_key(i) || sk.entities.contains_key(i) || sk.constraints.contains_key(i);
                    self.sel.retain(alive);
                    self.dim_refs.retain(alive);
                    self.clicks.retain(|c| c.point.is_none_or(|p| sk.points.contains_key(&p)));
                }
                None => self.leave_sketch(),
            }
        }
        if self.sel_body.is_some_and(|b| self.session.built.body(b).is_none()) {
            self.sel_body = None;
        }
        // Triangle indices do not survive a rebuild.
        self.sel_face = None;
        if self.sel_feature.is_some_and(|f| self.session.doc.feature(f).is_none()) {
            self.sel_feature = None;
        }
        let primitive_edit_closed = matches!(&self.preview, Some((Dialog::Primitive(p), _, _)) if p.editing.is_some())
            && !matches!(&self.dialog, Dialog::Primitive(p) if p.editing.is_some());
        if primitive_edit_closed || (matches!(&self.preview, Some((Dialog::Text(t), _, _)) if t.editing.is_some()) && !matches!(&self.dialog, Dialog::Text(t) if t.editing.is_some())) {
            self.fit_pending = true;
        }
        self.preview = None;
        self.text_base = None;
        self.text_local = None;
        self.construction_source = None;
    }

    /// Changes the active sketch as one undo step, rejecting changes its constraints cannot hold.
    pub fn sketch_edit(&mut self, f: impl FnOnce(&mut Sketch, &mut Document) -> Result<(), String>) -> bool {
        let Mode::Sketch(sid) = self.mode else { return false };
        let r = self.session.edit(|d| {
            let mut sk = d.sketch(sid).cloned().ok_or("The sketch no longer exists.")?;
            f(&mut sk, d)?;
            sk.validate()?;
            let report = solver::solve(&mut sk, &[]);
            sk.validate()?;
            *d.sketch_mut(sid).unwrap() = sk;
            fr_core::validation::document(d)?;
            if report.ok { Ok(()) } else { Err("That conflicts with the sketch's other constraints.".to_owned()) }
        });
        if let Err(e) = &r {
            self.toast(e.clone());
        }
        self.refresh();
        r.is_ok()
    }

    pub fn edit_sketch(&mut self, id: Id) {
        if !self.doc().features.iter().take(self.doc().active()).any(|f| f.id == id && !self.doc().is_suppressed(f.id) && self.session.built.components.contains_key(&f.owner)) {
            self.toast("This sketch or its component is suppressed or rolled back. Restore it in the timeline before editing.");
            return;
        }
        self.reference_editor.cancel();
        self.reference_texture.clear();
        let Some(sk) = self.session.doc.sketch_mut(id) else { return };
        sk.visible = true;
        let (bounds, local_plane) = (sk.bbox(), sk.plane);
        let owner = self.doc().feature(id).map_or(0, |f| f.owner);
        let plane = self.session.built.sketch_plane(&self.session.doc, id).unwrap_or_else(|| local_plane.transformed(self.session.built.component_placement(owner)));
        self.mode = Mode::Sketch(id);
        self.dialog = Dialog::None;
        self.cancel_tool();
        self.tool = Tool::Select;
        (self.cam.yaw, self.cam.pitch) = Camera::facing(&plane);
        match bounds {
            Some((lo, hi)) if self.vp.is_positive() => {
                self.cam.fit(plane.to_world(lo), plane.to_world(hi), self.vp.width() as f64, self.vp.height() as f64);
                self.cam.scale = self.cam.scale.min(40.0);
            }
            _ => self.cam.target = plane.origin + (self.cam.target - plane.origin).reject_from(plane.normal()),
        }
        self.sel_feature = Some(id);
        self.refresh();
    }

    fn leave_sketch(&mut self) {
        self.sketch_capture.clear();
        self.reference_editor.cancel();
        self.reference_texture.clear();
        self.reference_drag.clear();
        self.mode = Mode::Model;
        self.tool = Tool::Select;
        self.clicks.clear();
        self.dim_refs.clear();
        self.sel.clear();
        self.value_edit = None;
        self.drag = Drag::None;
    }

    pub fn finish_sketch(&mut self) {
        if self.sketch().is_none() {
            return;
        }
        if let Some(plane) = self.sketch().and_then(|(_, sk)| sk.on)
            && let Some(feature) = self.session.doc.feature_mut(plane)
            && let FeatureKind::Plane(p) = &mut feature.kind
            && !p.visibility_pinned && p.visible { p.visible = false; self.session.dirty = true; }
        self.leave_sketch();
        self.dialog = Dialog::None;
        (self.cam.yaw, self.cam.pitch) = (Camera::iso().yaw, Camera::iso().pitch);
        self.session.rebuild();
        self.refresh();
    }

    pub fn create_sketch(&mut self, plane: Plane) {
        let plane = if self.plane_offset.trim().is_empty() { plane } else {
            match self.doc().eval(&self.plane_offset, Kind::Length) {
                Ok(distance) => plane.offset(distance),
                Err(error) => { self.toast(error); return; }
            }
        };
        self.plane_offset.clear();
        match self.session.edit(|d| Ok(d.add_feature(FeatureKind::Sketch(Sketch::new(plane))))) {
            Ok(id) => self.edit_sketch(id),
            Err(e) => self.toast(e),
        }
    }

    /// Abandons whatever the current tool was in the middle of.
    pub fn cancel_tool(&mut self) {
        self.sketch_capture.clear();
        self.gizmo.clear();
        self.pattern_drag.clear();
        self.reference_drag.clear();
        self.clicks.clear();
        self.dim_refs.clear();
        self.value_edit = None;
        self.typed = None;
        self.drag = Drag::None;
    }

    /// Turns the placed clicks into geometry once the tool has enough of them.
    pub fn commit_clicks(&mut self) {
        let (tool, mut c, construction, sides) = (self.tool, self.clicks.clone(), self.opts.construction, self.opts.sides.clamp(3, 64));
        // A spline that lands on an existing point before its fourth click is finished
        // there: the missing fit points go evenly along the last stretch, so the curve
        // still passes through every clicked point and ends where it was closed.
        if tool == Tool::Spline && c.len() >= 2 && c.len() < 4 && c.last().is_some_and(|s| s.point.is_some()) {
            let (from, to) = (c[c.len() - 2].p, c[c.len() - 1].p);
            let missing = 4 - c.len();
            let fill: Vec<Snap> = (1..=missing).map(|i| Snap { p: from.lerp(to, i as f64 / (missing + 1) as f64), point: None, on: None, h: false, v: false, axis: [false, false], mid: false }).collect();
            let at = c.len() - 1;
            c.splice(at..at, fill);
        }
        if c.len() < tool.clicks() || tool.clicks() == 0 {
            return;
        }
        let place = |sk: &mut Sketch, s: &Snap| match s.point {
            Some(p) => p,
            None => {
                let id = sk.add_point(s.p);
                if let Some(e) = s.on {
                    // A midpoint snap is held at the midpoint; otherwise the point stays on the entity.
                    if !(s.mid && sk.add_constraint(CKind::Midpoint, &[id, e], None).is_ok()) {
                        let _ = sk.add_constraint(CKind::Coincident, &[id, e], None);
                    }
                }
                // A point snapped onto an axis is held there, level with or above the fixed origin.
                if s.axis[0] { let _ = sk.add_constraint(CKind::Horizontal, &[id, 0], None); }
                if s.axis[1] { let _ = sk.add_constraint(CKind::Vertical, &[id, 0], None); }
                id
            }
        };
        let selected = self.sel.clone();
        let mut last = None;
        // Sizes typed into the boxes become dimensions on what is drawn.
        let drawing = self.typed.take().unwrap_or_default();
        let locked = drawing.locked;
        let typed = drawing.fields;
        let size = |d: &mut Document, i: usize| typed.get(i).filter(|t| Self::typed_size(d, t).is_some()).and_then(|t| d.enter(t, Kind::Length).ok());
        let angle = |d: &mut Document, i: usize| -> Result<Option<fr_core::Value>, String> {
            if let Some(text) = typed.get(i).filter(|t| !t.trim().is_empty()) {
                return d.enter(text, Kind::Angle).map(Some);
            }
            locked.map(|v| d.enter(&format!("{} deg", if tool == Tool::TangentArc { v.abs() } else { v }), Kind::Angle)).transpose()
        };
        let done = self.sketch_edit(|sk, d| {
            match tool {
                Tool::Line => {
                    if c[0].p.distance(c[1].p) < 1e-6 || (c[0].point.is_some() && c[0].point == c[1].point) {
                        return Err("A line needs two different points.".into());
                    }
                    let (a, b) = (place(sk, &c[0]), place(sk, &c[1]));
                    let l = sk.add(Geom::Line { a, b }, construction);
                    let direction = angle(d, 1)?;
                    if direction.is_none() && c[1].point.is_none() && (c[1].h || c[1].v) {
                        let _ = sk.add_constraint(if c[1].h { CKind::Horizontal } else { CKind::Vertical }, &[l], None);
                    }
                    if let Some(v) = size(d, 0) {
                        sk.add_constraint(CKind::Distance, &[l], Some(v))?;
                    }
                    if let Some(v) = direction { sk.add_constraint(CKind::Angle, &[l], Some(v))?; }
                    last = Some(b);
                }
                Tool::Rect => {
                    if (c[0].p.x - c[1].p.x).abs() < 1e-6 || (c[0].p.y - c[1].p.y).abs() < 1e-6 {
                        return Err("A rectangle needs width and height.".into());
                    }
                    let (a, b) = (place(sk, &c[0]), place(sk, &c[1]));
                    let sides = sk.add_rect(a, b, construction);
                    for i in 0..2 {
                        if let Some(v) = size(d, i) {
                            sk.add_constraint(CKind::Distance, &[sides[i]], Some(v))?;
                        }
                    }
                }
                Tool::Circle => {
                    let r = c[0].p.distance(c[1].p);
                    if r < 1e-6 {
                        return Err("A circle needs a radius.".into());
                    }
                    let centre = place(sk, &c[0]);
                    let circle = sk.add(Geom::Circle { c: centre, r }, construction);
                    if let Some(p) = c[1].point {
                        let _ = sk.add_constraint(CKind::Coincident, &[p, circle], None);
                    }
                    if let Some(v) = size(d, 0) {
                        sk.add_constraint(CKind::Diameter, &[circle], Some(v))?;
                    }
                }
                Tool::Arc => {
                    let r = c[0].p.distance(c[1].p);
                    if r < 1e-6 || c[0].p.distance(c[2].p) < 1e-6 {
                        return Err("An arc needs a radius.".into());
                    }
                    let centre = place(sk, &c[0]);
                    let mut s = place(sk, &c[1]);
                    let mut e = match c[2].point {
                        Some(p) => p,
                        None => sk.add_point(c[0].p + (c[2].p - c[0].p).normalize() * r),
                    };
                    // Arcs run counter-clockwise; draw the short way round.
                    if (c[1].p - c[0].p).perp_dot(c[2].p - c[0].p) < 0.0 {
                        std::mem::swap(&mut s, &mut e);
                    }
                    sk.add(Geom::Arc { c: centre, s, e }, construction);
                }
                Tool::Arc3 => {
                    let ids = [place(sk, &c[0]), place(sk, &c[1]), place(sk, &c[2])];
                    // UI order is endpoints first, bulge last; core/API order remains start-through-end.
                    let diameter = size(d, 0);
                    if diameter.as_ref().is_some_and(|v| v.v + 1e-9 < c[0].p.distance(c[1].p)) {
                        return Err("The diameter cannot be smaller than the distance between the endpoints.".into());
                    }
                    let arc = sk.add_arc3(ids[0], ids[2], ids[1], construction)?;
                    if let Some(v) = diameter { sk.add_constraint(CKind::Diameter, &[arc], Some(v))?; }
                }
                Tool::TangentArc => {
                    let start = c[0].point.ok_or("Start at an existing line or arc endpoint.")?;
                    let source = Self::tangent_source(sk, start, &selected)?;
                    let end = place(sk, &c[1]);
                    let arc = sk.add_tangent_arc(source, start, end, construction)?;
                    if let Some(v) = angle(d, 0)? { sk.add_constraint(CKind::Angle, &[arc], Some(v))?; }
                }
                Tool::Spline => {
                    let ids = [place(sk, &c[0]), place(sk, &c[1]), place(sk, &c[2]), place(sk, &c[3])];
                    sk.add_spline(ids, construction)?;
                    last = Some(ids[3]);
                }
                Tool::Point => {
                    place(sk, &c[0]);
                }
                Tool::Polygon => {
                    if c[0].p.distance(c[1].p) < 1e-6 {
                        return Err("A polygon needs a size.".into());
                    }
                    let centre = place(sk, &c[0]);
                    sk.add_polygon(centre, c[1].p, sides, construction);
                }
                Tool::Select | Tool::Move | Tool::Dimension | Tool::Trim | Tool::Project => {}
            }
            Ok(())
        });
        self.clicks.clear();
        // A line or spline carries on from its end until it lands on an existing point
        // (or Escape ends the run). A continued spline is a new four-point spline that
        // shares the end point; it is not tangent to the last one.
        let continues = matches!(tool, Tool::Line | Tool::Spline);
        if let (true, true, Some(b), Some(end)) = (done, continues, last, c.get(tool.clicks() - 1).filter(|s| s.point.is_none())) {
            self.clicks.push(Snap { p: end.p, point: Some(b), on: None, h: false, v: false, axis: [false, false], mid: false });
        } else if !done && tool == Tool::Line {
            self.clicks.push(c[0]);
        }
    }

    pub fn tangent_source(sk: &Sketch, start: Id, selected: &[Id]) -> Result<Id, String> {
        let candidates: Vec<Id> = sk.entities.keys().copied().filter(|id| sk.endpoint_tangent(*id, start).is_some()).collect();
        let preferred: Vec<Id> = candidates.iter().copied().filter(|id| selected.contains(id)).collect();
        match if preferred.len() == 1 { preferred.as_slice() } else { candidates.as_slice() } {
            [id] => Ok(*id),
            [] => Err("Start at an existing line or arc endpoint.".into()),
            _ => Err("Several edges meet here. Select the source line or arc, then choose Tangent Arc.".into()),
        }
    }

    pub fn open_point_coordinates(&mut self, point: Option<Id>) {
        self.reference_editor.cancel();
        let Some((_, sk)) = self.sketch() else { self.toast("Start or edit a sketch first."); return };
        let point = point.filter(|id| sk.points.contains_key(id));
        let value = |kind: CKind, coordinate: usize| {
            point.and_then(|p| sk.constraints.values().find(|c| c.kind == kind && c.refs == [p]).and_then(|c| c.value.as_ref()).map(|v| v.expr.clone()))
                .unwrap_or_else(|| format!("{} mm", point.map_or(0.0, |p| sk.pos(p)[coordinate])))
        };
        let d = PointCoordsDlg { point, x: value(CKind::PositionX, 0), y: value(CKind::PositionY, 1), error: None };
        self.cancel_tool();
        self.tool = Tool::Select;
        self.dialog = Dialog::PointCoordinates(d);
    }

    pub fn apply_point_coordinates(&mut self, dialog: &PointCoordsDlg) -> bool {
        let mut point = None;
        let ok = self.sketch_edit(|sk, doc| {
            let x = doc.enter(&dialog.x, Kind::Length)?;
            let y = doc.enter(&dialog.y, Kind::Length)?;
            let at = DVec2::new(x.v, y.v);
            let p = dialog.point.unwrap_or_else(|| {
                if at.length() < 1e-7 && (x.is_formula() || y.is_formula()) { sk.add_point(at) }
                else { sk.point_at(at, 1e-7) }
            });
            if p != ORIGIN || dialog.point.is_some() { sk.set_point_coordinates(p, x, y)?; }
            point = Some(p);
            Ok(())
        });
        if ok {
            self.sel = point.into_iter().collect();
        }
        ok
    }

    /// The size a typed box holds, in millimetres, if what is in it is a usable length.
    pub fn typed_size(d: &Document, text: &str) -> Option<f64> {
        let rhs = text.split_once('=').map_or(text, |p| p.1);
        d.value(rhs, Kind::Length).ok().map(|v| v.v).filter(|v| *v > 1e-9)
    }

    /// Angles accept signs and zero for line directions; tangent sweeps use (0°, 360°).
    pub fn typed_angle(d: &Document, text: &str, sweep: bool) -> Option<f64> {
        let rhs = text.split_once('=').map_or(text, |p| p.1);
        d.value(rhs, Kind::Angle).ok().map(|v| v.v).filter(|v| v.is_finite() && (!sweep || (*v > 0.0 && *v < 360.0)))
    }

    /// Applies a constraint to the selection.
    pub fn constrain(&mut self, kind: CKind) {
        let Some((_, sk)) = self.sketch() else { return };
        let refs: Vec<Id> = self.sel.iter().copied().filter(|i| sk.ref_kind(*i).is_some()).collect();
        // Several lines can be made horizontal or vertical at once.
        let each = matches!(kind, CKind::Horizontal | CKind::Vertical | CKind::Fix) && refs.len() > 1 && refs.iter().all(|r| sk.line(*r).is_some());
        if let Err(e) = sk.normalize(kind, &refs)
            && !each
        {
            self.toast(format!("Select {} first, then click the constraint. ({e})", kind.needs()));
            return;
        }
        let ok = self.sketch_edit(|sk, _| {
            if each {
                for r in &refs {
                    sk.add_constraint(kind, &[*r], None)?;
                }
            } else {
                sk.add_constraint(kind, &refs, None)?;
            }
            Ok(())
        });
        if ok {
            self.sel.clear();
        }
    }

    /// The corner the selection names: a point, or two lines that share one.
    fn selected_corner(&self) -> Option<Id> {
        let (_, sk) = self.sketch()?;
        match self.sel.as_slice() {
            [p] if sk.points.contains_key(p) => Some(*p),
            [a, b] => sk.ent_points(*a).into_iter().find(|p| sk.line(*a).is_some() && sk.line(*b).is_some() && sk.ent_points(*b).contains(p)),
            _ => None,
        }
    }

    /// Opens the value box for a tool that needs one size.
    fn ask(&mut self, target: EditTarget, at: Option<Pos2>) {
        let unit = self.doc().units;
        let text = format!("{} {}", if unit == Unit::In { "0.1" } else if unit == Unit::Cm { "0.2" } else { "2" }, unit.name());
        let pos = at.or(self.ctx.input(|i| i.pointer.hover_pos()).filter(|p| self.vp.contains(*p))).unwrap_or(self.vp.center());
        self.value_edit = Some(ValueEdit { target, text, pos, focus: true, error: None });
    }

    /// Opens the dimension box for an existing dimension.
    pub fn edit_dimension(&mut self, cid: Id, pos: Pos2) {
        let Some((_, sk)) = self.sketch() else { return };
        let Some(v) = sk.constraints.get(&cid).and_then(|c| c.value.as_ref()) else { return };
        self.value_edit = Some(ValueEdit { target: EditTarget::Existing(cid), text: v.expr.clone(), pos, focus: true, error: None });
    }

    /// Opens the dimension box for a new dimension on `refs`.
    pub fn new_dimension(&mut self, kind: CKind, refs: Vec<Id>, pos: Pos2) {
        let Some((_, sk)) = self.sketch() else { return };
        let Ok(refs) = sk.normalize(kind, &refs) else { return };
        if let Some((cid, _)) = sk.constraints.iter().find(|(_, c)| c.kind == kind && c.refs == refs) {
            return self.edit_dimension(*cid, pos);
        }
        let now = sk.measure(kind, &refs);
        let text = match kind {
            CKind::Angle => format!("{} deg", fr_core::units::trim_num(now, 2)),
            _ => format!("{} {}", fr_core::units::fmt_len(now, self.doc().units), self.doc().units.name()),
        };
        self.value_edit = Some(ValueEdit { target: EditTarget::New(kind, refs), text, pos, focus: true, error: None });
    }

    /// Dimension the selected sketch geometry without requiring another viewport click.
    pub fn dimension_selection(&mut self, refs: Vec<Id>, pos: Pos2) {
        let Some((_, sk)) = self.sketch() else { return };
        if let [cid] = refs.as_slice() && sk.constraints.get(cid).is_some_and(|c| c.value.is_some()) {
            self.edit_dimension(*cid, pos);
            return;
        }
        use fr_core::sketch::Ref;
        let kinds: Vec<_> = refs.iter().filter_map(|r| sk.ref_kind(*r)).collect();
        if kinds.len() != refs.len() {
            self.toast("Select sketch geometry or one dimension label to edit.");
            return;
        }
        let kind = match kinds.as_slice() {
            [Ref::Line] => Some(CKind::Distance),
            [Ref::Curve] => {
                let existing = sk.constraints.values().find(|c| c.refs == refs && matches!(c.kind, CKind::Radius | CKind::Diameter)).map(|c| c.kind);
                existing.or(Some(if matches!(sk.entities[&refs[0]].geom, Geom::Circle { .. }) { CKind::Diameter } else { CKind::Radius }))
            }
            [Ref::Line, Ref::Line] => {
                let (a, b) = (sk.line(refs[0]).unwrap(), sk.line(refs[1]).unwrap());
                let parallel = (a.1 - a.0).normalize_or_zero().perp_dot((b.1 - b.0).normalize_or_zero()).abs() < 0.02;
                Some(if parallel { CKind::Distance } else { CKind::Angle })
            }
            [Ref::Point, Ref::Point] | [Ref::Point, Ref::Line] | [Ref::Line, Ref::Point] => Some(CKind::Distance),
            [Ref::Point] => { self.dim_refs = refs; return; }
            _ => None,
        };
        if let Some(kind) = kind {
            self.dim_refs = refs.clone();
            self.new_dimension(kind, refs, pos);
        } else if !refs.is_empty() {
            self.toast("Select a line, circle or arc, or two points or lines to dimension.");
        }
    }

    /// Confirms the dimension box. Returns false, keeping it open, if the value is not usable.
    pub fn commit_value(&mut self) -> bool {
        let Some(edit) = self.value_edit.clone() else { return true };
        let Mode::Sketch(sid) = self.mode else { return true };
        let r = self.session.edit(|d| {
            let mut sk = d.sketch(sid).cloned().ok_or("The sketch no longer exists.")?;
            let positive = |v: fr_core::Value| if v.v > 0.0 { Ok(v) } else { Err("Dimensions must be greater than zero.".to_owned()) };
            match &edit.target {
                EditTarget::Existing(cid) => {
                    let constraint = sk.constraints.get(cid).ok_or("That dimension no longer exists.")?;
                    let kind = constraint.kind;
                    let signed = matches!(kind, CKind::PositionX | CKind::PositionY)
                        || (kind == CKind::Angle && constraint.refs.len() == 1 && sk.line(constraint.refs[0]).is_some());
                    let value = d.enter(&edit.text, kind.value_kind().ok_or("That constraint is not a dimension.")?)?;
                    sk.constraints.get_mut(cid).unwrap().value = Some(if signed { value } else { positive(value)? });
                }
                EditTarget::New(kind, refs) => {
                    let v = d.enter(&edit.text, kind.value_kind().unwrap())?;
                    let signed = matches!(kind, CKind::PositionX | CKind::PositionY)
                        || (*kind == CKind::Angle && refs.len() == 1 && sk.line(refs[0]).is_some());
                    let v = if signed { v } else { positive(v)? };
                    sk.add_constraint(*kind, refs, Some(v))?;
                }
                EditTarget::Fillet(p) | EditTarget::Chamfer(p) => {
                    sk.round_corner(*p, positive(d.enter(&edit.text, Kind::Length)?)?, matches!(edit.target, EditTarget::Chamfer(_)))?;
                }
                EditTarget::Offset(ids) => {
                    sk.offset(ids, d.enter(&edit.text, Kind::Length)?)?;
                }
            }
            sk.validate()?;
            let report = solver::solve(&mut sk, &[]);
            sk.validate()?;
            *d.sketch_mut(sid).unwrap() = sk;
            fr_core::validation::document(d)?;
            if report.ok { Ok(()) } else { Err("That size conflicts with the sketch's other constraints.".to_owned()) }
        });
        match r {
            Ok(()) => {
                self.value_edit = None;
                self.dim_refs.clear();
                if !matches!(edit.target, EditTarget::Existing(_) | EditTarget::New(..)) {
                    self.sel.clear();
                }
                self.refresh();
                true
            }
            Err(e) => {
                if let Some(v) = &mut self.value_edit {
                    v.error = Some(e);
                    v.focus = true;
                }
                false
            }
        }
    }

    fn copy(&mut self, ctx: &Context) -> bool {
        let Some((_, sk)) = self.sketch() else { return false };
        let clip = sk.copy(&self.sel);
        if clip.points.is_empty() {
            return false;
        }
        let mut v = serde_json::to_value(&clip).unwrap();
        v[CLIP_MARK] = json!(1);
        ctx.copy_text(v.to_string());
        self.clipboard = Some(clip);
        self.pastes = 0;
        true
    }

    /// Pastes sketch geometry from the system clipboard text, or failing that the last copy.
    fn paste(&mut self, text: Option<&str>) {
        if text.is_some_and(|t| t.len() > 4 * 1024 * 1024) {
            self.toast("Clipboard text is too large. Sketch clipboard data is limited to 4 MiB.");
            return;
        }
        let from_text = text.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()).filter(|v| v[CLIP_MARK] == 1).map(serde_json::from_value::<Clip>);
        let clip = match from_text {
            Some(Ok(clip)) => Some(clip),
            Some(Err(e)) => {
                self.toast(format!("Could not paste the sketch: {e}"));
                return;
            }
            None => self.clipboard.clone(),
        };
        let Some(clip) = clip else { return };
        if let Err(e) = clip.validate() {
            self.toast(format!("Could not paste the sketch: {e}"));
            return;
        }
        let Some(sk) = self.sketch().and_then(|(id, _)| self.world_sketch(id)) else {
            self.toast("Open a sketch to paste into.");
            return;
        };
        // Under the pointer if it is over the viewport, otherwise next to the original.
        let under = self.ctx.input(|i| i.pointer.hover_pos()).filter(|p| self.vp.contains(*p)).and_then(|p| view::sketch_pos(self, &sk, p));
        self.pastes += 1;
        let offset = match under {
            Some(p) => p - clip.center(),
            None => DVec2::splat(12.0 / self.cam.scale * self.pastes as f64) * DVec2::new(1.0, -1.0),
        };
        let mut new = Vec::new();
        if self.sketch_edit(|sk, _| {
            new = sk.paste(&clip, offset);
            Ok(())
        }) {
            self.sel = new;
            self.tool = Tool::Select;
            self.cancel_tool();
        }
    }

    fn delete(&mut self) {
        match self.mode {
            Mode::Sketch(_) if !self.sel.is_empty() => {
                let ids = std::mem::take(&mut self.sel);
                self.sketch_edit(|sk, _| {
                    sk.remove(&ids);
                    Ok(())
                });
            }
            Mode::Model => {
                if let Some(id) = self.sel_feature {
                    self.delete_feature(id);
                }
            }
            _ => {}
        }
    }

    pub fn delete_feature(&mut self, id: Id) {
        if self.doc().feature(id).is_some_and(|f| matches!(f.kind, FeatureKind::Component(_))) {
            self.dialog = Dialog::DeleteComponent(id);
            return;
        }
        let _ = self.session.edit(|d| {
            d.delete_feature(id)?;
            Ok(())
        });
        self.refresh();
        if let Some(e) = self.session.built.errors.values().next().cloned() {
            self.toast(format!("Deleted, but a later feature now fails: {e} Undo brings it back."));
        }
    }

    /// Opens the dialog for an existing feature, or the sketch editor for a sketch.
    pub fn edit_feature(&mut self, id: Id) {
        let Some(kind) = self.session.doc.feature(id).map(|f| f.kind.clone()) else { return };
        let shown = |v: &fr_core::Value| v.expr.clone();
        match &kind {
            FeatureKind::Plane(p) => { self.finish_sketch(); self.dialog = Dialog::Plane(crate::construction::PlaneDlg::from_plane(id, p)); }
            FeatureKind::Remove(p) => { self.finish_sketch(); self.dialog = Dialog::Remove(crate::body_ops_ui::RemoveDlg { editing: Some(id), bodies: p.bodies.clone() }); }
            FeatureKind::Split(p) => { self.finish_sketch(); self.dialog = Dialog::Split(crate::body_ops_ui::SplitDlg { editing: Some(id), body: Some(p.body), plane: Some(p.plane.clone()), picking_body: false }); }
            FeatureKind::Primitive(p) => { self.finish_sketch(); self.dialog = Dialog::Primitive(crate::primitives::PrimitiveDlg::from_feature(id, p)); self.fit_pending = true; }
            FeatureKind::Component(_) => self.move_component_dialog(id),
            FeatureKind::Pattern(pattern) => { self.finish_sketch(); self.dialog = Dialog::Pattern(PatternDlg::from_feature(id, pattern)); }
            FeatureKind::Sketch(_) => self.edit_sketch(id),
            FeatureKind::Extrude(e) => {
                self.finish_sketch();
                self.dialog = Dialog::Feature(FeatureDlg { revolve: false, sweep: None, editing: Some(id), sketch: Some(e.sketch), profiles: e.profiles.clone(), text: shown(&e.distance), symmetric: e.symmetric, op: e.op, axis: Axis::Y, pick_axis: false, face: None, face_owner: 0, taper: e.taper.as_ref().map_or(String::new(), shown), through_all: e.through_all, pick_to: false });
            }
            FeatureKind::Sweep(w) => {
                self.finish_sketch();
                self.dialog = Dialog::Feature(FeatureDlg { revolve: false, sweep: Some(SweepDlg { path_sketch: Some(w.path_sketch), path: w.path.clone(), spans: w.spans.clone(), orient: w.orient }), editing: Some(id), sketch: Some(w.sketch), profiles: w.profiles.clone(), text: String::new(), symmetric: false, op: w.op, axis: Axis::Y, pick_axis: false, face: None, face_owner: 0, taper: String::new(), through_all: false, pick_to: false });
            }
            FeatureKind::Revolve(r) => {
                self.finish_sketch();
                self.dialog = Dialog::Feature(FeatureDlg { revolve: true, sweep: None, editing: Some(id), sketch: Some(r.sketch), profiles: r.profiles.clone(), text: shown(&r.angle), symmetric: false, op: r.op, axis: r.axis, pick_axis: false, face: None, face_owner: 0, taper: String::new(), through_all: false, pick_to: false });
            }
            FeatureKind::Text(t) => {
                self.finish_sketch();
                self.dialog = Dialog::Text(TextDlg {
                    editing: Some(id), owner: self.doc().feature(id).map_or(0, |f| f.owner), text: t.text.clone(), plane: t.plane,
                    height: shown(&t.height), depth: shown(&t.depth), spacing: shown(&t.spacing),
                    angle: shown(&t.angle), x: shown(&t.x), y: shown(&t.y), align: t.align,
                    op: t.op, body: t.body, face: t.face, frame: t.frame,
                });
                self.fit_pending = true;
            }
            _ => self.toast("This feature has no settings to edit; delete it and add it again to change it."),
        }
    }

    /// Opens Sweep with what can be told apart already: the newest visible sketch with a closed
    /// region is the profile, and the newest other visible sketch that is one run is the path.
    fn open_sweep_dialog(&mut self) {
        self.finish_sketch();
        let doc = self.doc();
        let visible: Vec<Id> = doc.sketches().filter(|(f, s)| s.visible && !doc.is_suppressed(f.id)).map(|(f, _)| f.id).collect();
        let closed = |id: &Id| fr_core::profile::profiles(doc.sketch(*id).unwrap());
        let sketch = visible.iter().rev().copied().find(|id| !closed(id).is_empty());
        let mut profiles = Vec::new();
        if let Some(id) = sketch && let [only] = closed(&id).as_slice() { profiles.push(only.edges.clone()); }
        let path_sketch = visible.iter().rev().copied().find(|id| Some(*id) != sketch && fr_core::profile::chain(doc.sketch(*id).unwrap(), &[]).is_ok());
        let op = if self.session.built.bodies.is_empty() { Op::New } else { Op::Join };
        self.dialog = Dialog::Feature(FeatureDlg { revolve: false, sweep: Some(SweepDlg { path_sketch, path: Vec::new(), spans: Vec::new(), orient: SweepOrient::Follow }), editing: None, sketch, profiles, text: String::new(), symmetric: false, op, axis: Axis::Y, pick_axis: false, face: None, face_owner: 0, taper: String::new(), through_all: false, pick_to: false });
    }

    fn open_feature_dialog(&mut self, revolve: bool) {
        // Only one closed region is unambiguous. Projected outlines and circles
        // remain usable geometry, but must never all be extruded implicitly.
        let from = self.sketch().map(|(id, _)| id);
        let face_owner = self.sel_face.as_ref().and_then(|f| self.session.built.body(f.body)).map_or(self.doc().active_component, |b| b.component);
        let face = self.sel_face.clone().filter(|f| !revolve && from.is_none() && f.plane.is_some()).map(|mut f| {
            if let Some(body) = self.session.built.body(f.body) { f.prepare_exact(body); }
            self.local_face(f)
        });
        self.finish_sketch();
        let doc = self.doc();
        let candidates: Vec<Id> = match from {
            Some(id) => vec![id],
            None => doc.sketches().filter(|(_, s)| s.visible).map(|(f, _)| f.id).collect(),
        };
        let mut picked = (None, Vec::new());
        if let [only] = candidates.as_slice() {
            let all = fr_core::profile::profiles(doc.sketch(*only).unwrap());
            picked.0 = Some(*only); // Keep an edited, hidden sketch pickable.
            if let [profile] = all.as_slice() { picked.1.push(profile.edges.clone()); }
        }
        if face.is_some() {
            picked = (None, Vec::new());
        }
        let op = if self.session.built.bodies.is_empty() { Op::New } else { Op::Join };
        let unit = doc.units;
        let text = if revolve { "360 deg".to_owned() } else { format!("{} {}", if unit == Unit::In { "0.5" } else if unit == Unit::Cm { "1" } else { "10" }, unit.name()) };
        self.dialog = Dialog::Feature(FeatureDlg { revolve, sweep: None, editing: None, sketch: picked.0, profiles: picked.1, text, symmetric: false, op, axis: Axis::Y, pick_axis: false, face, face_owner, taper: String::new(), through_all: false, pick_to: false });
    }

    fn prepare_text_source(&mut self) {
        let Dialog::Text(t) = &self.dialog else { self.text_base = None; return };
        let Some(id) = t.editing else { self.text_base = None; return };
        if self.text_base.as_ref().is_some_and(|(rev, feature, _)| *rev == self.session.rev && *feature == id) { return; }
        let Some(index) = self.doc().features.iter().position(|f| f.id == id) else { self.text_base = None; return };
        let mut before = self.doc().clone();
        before.roll_to(index);
        self.text_base = Some((self.session.rev, id, before.rebuild()));
    }

    /// Face picking while editing uses the same geometry that the feature will see
    /// during rebuild, before this text and any later moves, holes or lettering.
    pub fn text_source(&self) -> &Built {
        match (&self.dialog, &self.text_base) {
            (Dialog::Text(t), Some((rev, id, built))) if t.editing == Some(*id) && *rev == self.session.rev => built,
            _ => &self.session.built,
        }
    }

    pub fn text_baseline(&self) -> Option<DVec3> {
        let Dialog::Text(t) = &self.dialog else { return None };
        let feature = t.feature(&mut self.doc().clone()).ok()?;
        if feature.op != Op::New {
            let body = self.text_source().body(feature.body?)?;
            let key = (self.session.rev, t.editing, body.id);
            let local = self.text_local.as_ref().filter(|(cached, _)| *cached == key).map(|(_, body)| body)?;
            feature.placement(Some(local)).ok().map(|p| body.placement.transform_point3(p.origin))
        } else {
            let owner = t.owner;
            feature.placement(None).ok().map(|p| self.session.built.component_placement(owner).transform_point3(p.origin))
        }
    }

    /// Uses the actual clicked point as the baseline origin, keeping offsets editable.
    pub fn text_on_face(&mut self, face: Face) -> Result<(), String> {
        self.prepare_text_source();
        let face = self.local_face(face);
        let mut plane = face.plane.ok_or("Text / Emboss supports flat faces only; curved wrapping is not available.")?;
        let body = self.text_source().body(face.body).ok_or("Choose a face that exists before this text feature in the timeline.")?;
        if !body.is_exact() {
            return Err("Text / Emboss needs a flat face of a solid; imported mesh faces are not supported.".into());
        }
        let (frame, owner) = (self.text_source().frame(face.body), body.component);
        let Dialog::Text(t) = &mut self.dialog else { return Err("Open Text / Emboss first.".into()) };
        if t.editing.is_some() && t.owner != owner { return Err("An existing text feature cannot move to another component. Create new text in the target component instead.".into()); }
        t.owner = owner;
        let n = plane.normal();
        plane.origin = face.at - n * (face.at - plane.origin).dot(n);
        (t.plane, t.body, t.face, t.frame) = (plane, Some(face.body), Some(plane.origin), frame);
        if t.op == Op::New { t.op = Op::Join; }
        self.preview = None;
        Ok(())
    }

    fn open_text_dialog(&mut self) {
        let face = self.sel_face.clone();
        self.finish_sketch();
        self.dialog = Dialog::Text(TextDlg {
            editing: None, owner: self.doc().active_component, text: "Text".into(), plane: Plane::XY,
            height: "5 mm".into(), depth: "1 mm".into(), spacing: "0 mm".into(),
            angle: "0 deg".into(), x: "0 mm".into(), y: "0 mm".into(),
            align: fr_core::text::Align::Left, op: Op::New, body: None, face: None, frame: None,
        });
        if let Some(face) = face && let Err(e) = self.text_on_face(face) { self.toast(e); }
    }

    /// Confirms the open dialog.
    pub fn apply_dialog(&mut self) {
        if let Dialog::Script(d) = self.dialog.clone() {
            self.start_script(d);
            return;
        }
        let dlg = self.dialog.clone();
        match self.session.edit_feature(|d| { let id = dlg.apply(d)?; fr_core::validation::document(d)?; Ok((id, id)) }) {
            Ok(id) => {
                self.dialog = Dialog::None;
                self.sel_feature = Some(id);
                self.refresh();
                // A hollowed body looks the same from outside, so say what happened.
                if let Dialog::Shell(sh) = &dlg {
                    let faces = sh.faces.len();
                    self.toast(format!("Hollowed to a {} wall, open at {faces} face{}. Look inside with Section Analysis.", sh.text.trim(), if faces == 1 { "" } else { "s" }));
                }
            }
            Err(e) => self.toast(e),
        }
    }

    /// Keeps the dialog's preview current.
    pub fn update_preview(&mut self) {
        let primitive_edit_closed = matches!(&self.preview, Some((Dialog::Primitive(p), _, _)) if p.editing.is_some())
            && !matches!(&self.dialog, Dialog::Primitive(p) if p.editing.is_some());
        if primitive_edit_closed || (matches!(&self.preview, Some((Dialog::Text(t), _, _)) if t.editing.is_some()) && !matches!(&self.dialog, Dialog::Text(t) if t.editing.is_some())) {
            self.fit_pending = true;
        }
        self.prepare_text_source();
        self.prepare_plane_source();
        crate::body_ops_ui::prepare(self);
        let text_key = match &self.dialog { Dialog::Text(t) if t.op != Op::New => t.body.map(|id| (self.session.rev, t.editing, id)), _ => None };
        if self.text_local.as_ref().map(|(key, _)| *key) != text_key {
            self.text_local = text_key.and_then(|key| self.text_source().body(key.2).and_then(|body| body.local_copy().ok()).map(|body| (key, body)));
        }
        if !self.dialog.has_preview() {
            self.preview = None;
            return;
        }
        if self.preview.as_ref().is_some_and(|p| p.0 == self.dialog) {
            return;
        }
        let mut doc = self.session.doc.clone();
        let (built, error) = match self.dialog.apply(&mut doc) {
            Ok(id) => {
                // Editing sees the feature in its original place in the timeline.
                // Later transforms must not make a clicked face move a second time.
                if (matches!(&self.dialog, Dialog::Text(t) if t.editing.is_some()) || matches!(&self.dialog, Dialog::Plane(p) if p.editing.is_some()) || matches!(&self.dialog, Dialog::Primitive(p) if p.editing.is_some()) || crate::body_ops_ui::editing(&self.dialog).is_some())
                    && let Some(index) = doc.features.iter().position(|f| f.id == id)
                { doc.roll_to(index + 1); }
                let built = if matches!(&self.dialog, Dialog::Plane(_)) {
                    // A plane never changes the bodies before it, and the preview shows the
                    // timeline only up to the plane, so resolve the plane against what is
                    // already built instead of rebuilding every body on each drag step.
                    let mut built = self.modeling_source().clone();
                    let resolved = match doc.feature(id).cloned() {
                        Some(mut feature) => doc.resolve_plane(&mut feature, &built),
                        None => Err("the plane no longer exists".to_owned()),
                    };
                    match resolved {
                        Ok((mut plane, _)) => {
                            let placement = built.component_placement(plane.component);
                            plane.plane = plane.plane.transformed(placement);
                            plane.corners = plane.corners.map(|p| placement.transform_point3(p));
                            built.planes.insert(id, plane);
                            built.errors.remove(&id);
                        }
                        Err(e) => { built.errors.insert(id, e); }
                    }
                    built
                } else {
                    doc.rebuild()
                };
                let e = built.errors.get(&id).cloned();
                (if e.is_some() { self.modeling_source().clone() } else { built }, e)
            }
            Err(e) => (self.modeling_source().clone(), Some(e)),
        };
        self.preview = Some((self.dialog.clone(), built, error));
    }

    /// Moves the timeline's roll-back marker so that `count` features are built.
    pub fn roll_to(&mut self, count: usize) {
        if count == self.doc().active() {
            return;
        }
        self.finish_sketch();
        self.dialog = Dialog::None;
        let _ = self.session.edit(|d| {
            d.roll_to(count);
            Ok(())
        });
        self.refresh();
    }

    pub fn fit(&mut self) {
        let mut bounds = if self.dialog.has_preview() {
            self.shown().bodies.iter().filter(|b| self.body_visible(b)).filter_map(|b| b.mesh.bbox()).reduce(|(lo, hi), (l, h)| (lo.min(l), hi.max(h)))
        } else { fr_core::render::scene_bounds(&self.session) };
        if let Some(sk) = self.sketch().and_then(|(id, _)| self.world_sketch(id))
            && let Some((lo, hi)) = sk.bbox()
        {
            let (a, b) = (sk.plane.to_world(lo), sk.plane.to_world(hi));
            bounds = Some(bounds.map_or((a.min(b), a.max(b)), |(l, h)| (l.min(a).min(b), h.max(a).max(b))));
        }
        if let Some(sk) = self.sketch().and_then(|(id, _)| self.world_sketch(id))
            && let Some(image) = self.reference_editor.calibration_image().or(sk.reference.as_ref()).filter(|i| i.visible)
        {
            for point in image.corners() {
                let p = sk.plane.to_world(point);
                bounds = Some(bounds.map_or((p,p), |(lo,hi)| (lo.min(p),hi.max(p))));
            }
        }
        for (id, plane) in &self.shown().planes {
            let editing = matches!(&self.dialog, Dialog::Plane(d) if d.editing == Some(*id));
            let visible = editing || self.doc().feature(*id).is_none_or(|f| matches!(&f.kind, FeatureKind::Plane(p) if p.visible));
            if visible && self.shown().component_visible(plane.component) {
                for point in plane.corners { bounds = Some(bounds.map_or((point, point), |(lo, hi)| (lo.min(point), hi.max(point)))); }
            }
        }
        match bounds {
            Some((lo, hi)) if self.vp.is_positive() => self.cam.fit(lo, hi, self.vp.width() as f64, self.vp.height() as f64),
            _ => (self.cam.target, self.cam.scale) = (DVec3::ZERO, 4.0),
        }
    }

    fn confirm_discard(&self) -> bool {
        if !self.session.dirty || cfg!(test) {
            return true;
        }
        rfd::MessageDialog::new()
            .set_title("Unsaved changes")
            .set_description("This design has changes that are not saved. Discard them?")
            .set_buttons(rfd::MessageButtons::OkCancelCustom("Discard".into(), "Cancel".into()))
            .show()
            == rfd::MessageDialogResult::Custom("Discard".into())
    }

    fn replace_session(&mut self, s: Session) {
        self.session = s;
        self.reset_document_ui();
    }

    /// Session revision counters restart when opening a document, so cached UI state must too.
    fn reset_document_ui(&mut self) {
        self.command_search.open = false;
        self.ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("command-search-query")));
        if let Some(r) = &mut self.recovery {
            r.new_document();
        }
        self.leave_sketch();
        self.typed = None;
        self.param_edit = None;
        self.rename = None;
        self.pattern_at = None;
        self.scene.invalidate();
        self.gap_cache = None;
        self.timeline = crate::timeline::Timeline::default();
        self.file_error = None;
        self.section.on = false;
        self.dialog = Dialog::None;
        self.sel_body = None;
        self.sel_component = None;
        self.sel_feature = None;
        self.cam = Camera::iso();
        self.fit_pending = true;
        self.refresh();
    }

    /// Desktop requests use the same unsaved-work guard as File > Open. A pending
    /// error stays visible until dismissed; further requests wait their turn.
    pub fn process_open_request(&mut self, confirm: impl FnOnce(&Self) -> bool) {
        if self.file_error.is_some() { return; }
        let Some(request) = self.open_requests.pop() else { return; };
        match request {
            Ok(path) if is_mesh_file(&path) || confirm(self) => self.open_path(&path),
            Ok(_) => {},
            Err(error) => self.toast(error),
        }
    }

    pub fn open_path(&mut self, path: &Path) {
        if is_mesh_file(path) {
            self.dialog = Dialog::Import(path.to_owned(), Unit::Mm);
            return;
        }
        match Session::open(path) {
            Ok(s) => {
                self.replace_session(s);
                if self.session.read_only {
                    self.toast("Unverified preview: this design needs a newer Ferrender. Its saved geometry is shown read-only and has not been rebuilt.");
                } else if self.session.from_cache {
                    self.toast(format!("Opened from the saved geometry: {} bodies without a rebuild.", self.session.built.bodies.len()));
                }
                self.remember_document();
            },
            Err(e) => self.file_error("Could not open design", path, e, "Your current design has been kept."),
        }
    }

    fn save(&mut self, ask: bool) {
        self.finish_value();
        let path = match (&self.session.path, ask) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new().add_filter("Ferrender design", &["ferr"]).set_file_name(format!("{}.ferr", self.doc_name())).save_file(),
        };
        if let Some(p) = path { self.save_path(&p); }
    }

    pub(crate) fn save_path(&mut self, path: &Path) {
        match self.session.save_as(path, Some(crate::build_info::summary().lines().next().unwrap_or("Ferrender").to_owned())) {
            Ok(saved) => {
                self.file_error = None;
                let mut message = match saved.backup {
                    Some(backup) => format!("Saved {} in the 0.4 format. The previous file was kept as {}", path.display(), backup.display()),
                    None => format!("Saved {}", path.display()),
                };
                if let Some(reason) = &saved.cache_skipped {
                    message.push_str(&format!(". No geometry cache was written ({reason}), so the design will rebuild when opened"));
                }
                self.toast(message);
                self.remember_document();
            }
            Err(e) => self.file_error("Could not save design", path, e, "Your changes are still in memory. Save again to keep them."),
        }
    }

    fn remember_document(&mut self) {
        if let Some(path) = &self.session.path && let Err(error) = self.recent.remember(path) {
            self.toast(error);
        }
    }

    fn finish_value(&mut self) {
        if self.value_edit.is_some() && !self.commit_value() {
            self.value_edit = None;
        }
    }

    pub fn doc_name(&self) -> String {
        self.session.path.as_ref().and_then(|p| p.file_stem()).map_or("Untitled".to_owned(), |n| n.to_string_lossy().into_owned())
    }

    /// One sculpt stroke at a world point on a body, as the open Sculpt dialog says.
    pub fn sculpt_at(&mut self, body: Id, at: glam::DVec3) {
        let Dialog::Sculpt(d) = self.dialog.clone() else { return };
        use fr_core::doc::{MeshOp, MeshOpKind};
        use fr_core::meshops::Brush;
        let brush = [Brush::Pull, Brush::Push, Brush::Inflate, Brush::Smooth, Brush::Flatten][d.brush.min(4)];
        let local = self.session.built.body(body).map_or(at, |b| b.to_local(at));
        let r = self.session.edit_feature(|doc| {
            let radius = doc.enter(&d.radius, Kind::Length)?;
            let strength = doc.enter(&d.strength, if matches!(brush, Brush::Smooth | Brush::Flatten) { Kind::Scalar } else { Kind::Length })?;
            let id = doc.add_feature(FeatureKind::MeshOp(MeshOp { body, op: MeshOpKind::Sculpt { brush, at: local, radius, strength }, region: None }));
            Ok((id, id))
        });
        match r {
            Ok(_) => {
                if let Dialog::Sculpt(d) = &mut self.dialog { d.strokes += 1; }
                self.sel_body = Some(body);
            }
            Err(e) => self.toast(e),
        }
        self.refresh();
    }

    pub fn import_stl(&mut self, path: &Path, unit: Unit) {
        let r = io::import_mesh(path, unit).and_then(|(mesh, report)| {
            let name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
            self.session.edit(|d| {
                let id = d.add_feature(FeatureKind::Import(mesh));
                if let Some(n) = name {
                    d.feature_mut(id).unwrap().name = n;
                }
                Ok((id, report))
            })
        });
        match r {
            Ok((id, report)) => {
                self.file_error = None;
                self.sel_body = Some(id);
                self.fit_pending = true;
                self.toast(format!("Imported {}.", report.summary()));
            }
            Err(e) => self.file_error("Could not import the mesh", path, e, "Your current design has been kept."),
        }
        self.refresh();
    }

    pub fn export_stl(&mut self, unit: Unit) {
        if self.session.visible_bodies().next().is_none() {
            self.toast("There are no visible bodies to export.");
            return;
        }
        let Some(path) = rfd::FileDialog::new().add_filter("STL mesh", &["stl"]).set_file_name(format!("{}.stl", self.doc_name())).save_file() else { return };
        self.export_stl_path(&path, unit);
    }

    pub fn export_stl_path(&mut self, path: &Path, unit: Unit) {
        match io::write_stl_with_provenance(self.session.visible_bodies(), unit, path, self.session.read_only) {
            Ok(n) => {
                self.file_error = None;
                self.toast(format!("Exported {n} triangles in {} to {}", unit.name(), path.display()));
            }
            Err(e) => self.file_error("Could not export STL", path, e, "Your design is still open. The export did not complete."),
        }
    }

    /// Runs a command from the MCP bridge or the assistant.
    pub fn execute(&mut self, cmd: &serde_json::Value) -> Result<serde_json::Value, String> {
        api::validate_request(cmd)?;
        self.execute_command(cmd)
    }

    fn execute_command(&mut self, cmd: &serde_json::Value) -> Result<serde_json::Value, String> {
        if cmd["op"] == "batch" {
            let commands = cmd["commands"].as_array().ok_or("batch needs \"commands\"")?;
            let mut results = Vec::new();
            for (i, command) in commands.iter().enumerate() {
                results.push(self.execute_command(command).map_err(|e| format!("command {i} ({}) failed: {e}. The {i} before it were applied.", command["op"].as_str().unwrap_or("?")))?);
            }
            return Ok(json!(results));
        }
        let replacing = matches!(cmd["op"].as_str(), Some("new" | "open"));
        if replacing && self.session.dirty && cmd["discard_unsaved"].as_bool() != Some(true) {
            return Err("The open design has unsaved changes. Save it first, or explicitly set \"discard_unsaved\": true to discard them.".into());
        }
        let mut cam = self.cam;
        if self.vp.is_positive() {
            // Screenshots keep what the viewport shows, whatever size is asked for.
            cam.scale *= cmd["width"].as_f64().unwrap_or(900.0) / self.vp.width() as f64;
        }
        let r = api::execute(&mut self.session, cmd, Some(cam));
        if replacing && r.is_ok() {
            self.reset_document_ui();
        }
        if r.is_ok() && matches!(cmd["op"].as_str(), Some("open" | "save")) {
            self.remember_document();
        }
        self.refresh();
        r
    }

    pub fn run(&mut self, ctx: &Context, a: Action) {
        if self.session.read_only && !matches!(a,
            Action::New | Action::Open | Action::Recover | Action::Export | Action::ExportStep
            | Action::Copy | Action::SelectAll | Action::About | Action::Assistant
            | Action::View(_) | Action::Fit | Action::Section | Action::Measure
            | Action::CommandSearch | Action::ScriptLog | Action::Cancel | Action::Tool(Tool::Select)) {
            self.toast("This design is read-only because it was written by a newer Ferrender. Update Ferrender to edit it.");
            return;
        }
        if matches!(a, Action::Open | Action::Import | Action::Save | Action::SaveAs | Action::About | Action::Parameters | Action::Recover | Action::View(_) | Action::Fit) {
            self.reference_drag.clear();
        }
        match a {
            Action::CommandSearch => crate::command_search::open(self, ctx),
            Action::Plane => self.open_plane_dialog(),
            Action::NewComponent => self.new_component(self.doc().active_component),
            Action::ActivateRoot => self.activate_component(0),
            Action::New => {
                if self.confirm_discard() {
                    let units = self.doc().units;
                    self.replace_session(Session::new(Document::new(units)));
                }
            }
            Action::Open => {
                if self.confirm_discard()
                    && let Some(p) = rfd::FileDialog::new().add_filter("Ferrender design", &["ferr"]).pick_file()
                {
                    self.open_path(&p);
                }
            }
            Action::Save => self.save(false),
            Action::SaveAs => self.save(true),
            Action::Recover => {
                self.recover = self.recovery.as_ref().map_or(Vec::new(), Recovery::found);
                if self.recover.is_empty() {
                    self.toast("There is no unsaved work to recover.");
                }
            }
            Action::Import => {
                if let Some(p) = rfd::FileDialog::new().add_filter("Mesh (STL, OBJ, 3MF)", &["stl", "obj", "3mf"]).pick_file() {
                    self.finish_sketch();
                    self.dialog = Dialog::Import(p, Unit::Mm);
                }
            }
            Action::Export => {
                self.finish_sketch();
                self.dialog = Dialog::Export(Unit::Mm);
            }
            Action::Undo | Action::Redo => {
                self.reference_editor.cancel();
                self.cancel_tool();
                self.dialog = Dialog::None;
                if a == Action::Undo { self.session.undo() } else { self.session.redo() };
                self.refresh();
            }
            Action::Copy => {
                self.copy(ctx);
            }
            Action::Cut => {
                if self.copy(ctx) {
                    self.delete();
                }
            }
            Action::Paste => self.paste(None),
            Action::Delete => self.delete(),
            Action::SelectAll => {
                self.reference_drag.clear();
                if let Some((_, sk)) = self.sketch() {
                    self.sel = sk.entities.keys().copied().chain(sk.points.keys().copied().filter(|p| *p != ORIGIN)).collect();
                    self.tool = Tool::Select;
                }
            }
            Action::NewSketch => {
                if let Some(id) = self.sel_feature.filter(|id| self.session.built.planes.contains_key(id)) { self.finish_sketch(); self.create_sketch_on(id); return; }
                // With a flat face selected, sketch straight on it.
                let on = self.sel_face.as_ref().filter(|_| self.sketch().is_none()).and_then(|f| f.plane);
                self.finish_sketch();
                match on {
                    Some(plane) => self.create_sketch_world(plane),
                    None => self.dialog = Dialog::PickPlane,
                }
            }
            Action::FinishSketch => self.finish_sketch(),
            Action::Extrude => self.open_feature_dialog(false),
            Action::Revolve => self.open_feature_dialog(true),
            Action::Sweep => self.open_sweep_dialog(),
            Action::Text => self.open_text_dialog(),
            Action::Transform => {
                if let Some(component) = self.sel_component { self.move_component_dialog(component); return; }
                // In a sketch, Move moves sketch geometry: drag the selection, or a point or entity.
                if self.sketch().is_some() {
                    self.run(ctx, Action::Tool(Tool::Move));
                    if self.sel.is_empty() { self.toast("Drag a point or entity to move it, or select geometry and drag anywhere."); }
                    return;
                }
                self.finish_sketch();
                match self.target_body().or(self.session.built.bodies.first().map(|b| b.id).filter(|_| self.session.built.bodies.len() == 1)) {
                    Some(body) => {
                        let z = || "0".to_owned();
                        self.dialog = Dialog::Transform(TransformDlg { body, translate: [z(), z(), z()], rotate: [z(), z(), z()], scale: "1".into() });
                    }
                    None => self.toast("Click a body to select it first."),
                }
            }
            Action::Mesh(kind) => {
                self.finish_sketch();
                let body = self.target_body().or(self.session.built.bodies.first().map(|b| b.id).filter(|_| self.session.built.bodies.len() == 1));
                if body.is_none() { self.toast("Click a body to select it first."); }
                self.dialog = Dialog::Mesh(MeshDlg::new(kind, body, self.doc().units));
            }
            Action::Sculpt => {
                self.finish_sketch();
                let u = self.doc().units;
                let mm = |v: f64| format!("{} {}", fr_core::units::trim_num(v / u.mm(), 3), u.name());
                self.dialog = Dialog::Sculpt(SculptDlg { brush: 0, radius: mm(8.0), strength: mm(1.0), strokes: 0 });
            }
            Action::ScriptLog | Action::ExportTimelineScript => { crate::scripts_ui::action(self, &a); }
            Action::Relief => {
                self.finish_sketch();
                if let Some(path) = rfd::FileDialog::new().add_filter("PNG or JPEG", &["png", "jpg", "jpeg"]).pick_file() {
                    let u = self.doc().units;
                    let mm = |v: f64| format!("{} {}", fr_core::units::trim_num(v / u.mm(), 3), u.name());
                    self.dialog = Dialog::Relief(ReliefDlg { path, width: mm(100.0), depth: mm(4.0), base: mm(2.0), resolution: 300, blur: 1, invert: false });
                }
            }
            Action::RemoveBody => {
                self.finish_sketch();
                let bodies = self.target_body().into_iter().collect();
                self.dialog = Dialog::Remove(crate::body_ops_ui::RemoveDlg { editing: None, bodies });
            }
            Action::SplitBody => {
                self.finish_sketch();
                let body = self.target_body();
                self.dialog = Dialog::Split(crate::body_ops_ui::SplitDlg { editing: None, body, plane: None, picking_body: body.is_none() });
            }
            Action::Combine | Action::JoinBodies => {
                self.finish_sketch();
                if self.session.built.bodies.len() < 2 {
                    self.toast("Combine needs at least two bodies.");
                } else {
                    self.dialog = Dialog::Combine(CombineDlg { target: self.target_body(), tools: Vec::new(), op: Op::Join, keep_tools: false });
                }
            }
            Action::Parameters => self.show_params = !self.show_params,
            Action::Assistant => self.ai.open = !self.ai.open,
            Action::About => self.show_about = true,
            Action::View(name) => {
                if let Some((yaw, pitch)) = Camera::named(name) {
                    (self.cam.yaw, self.cam.pitch) = (yaw, pitch);
                }
            }
            Action::Fit => self.fit(),
            Action::PointCoordinates => {
                let point = self.sketch().and_then(|(_, sk)| (self.sel.len() == 1).then(|| self.sel[0]).filter(|p| sk.points.contains_key(p)));
                self.open_point_coordinates(point);
            }
            Action::ReferenceImage => {
                if let Some((sid, sk)) = self.sketch() {
                    let editor = crate::reference::Editor::open(sid, sk.reference.as_ref());
                    self.cancel_tool();
                    self.tool = Tool::Select;
                    self.dialog = Dialog::None;
                    self.reference_editor = editor;
                } else { self.toast("Start or edit a sketch first."); }
            }
            Action::Tool(t) => {
                if self.sketch().is_some() {
                    self.reference_editor.cancel();
                    self.cancel_tool();
                    self.dialog = Dialog::None;
                    self.tool = t;
                    if t == Tool::Dimension {
                        let pos = self.ctx.input(|i| i.pointer.hover_pos()).filter(|p| self.vp.contains(*p)).unwrap_or(self.vp.center());
                        self.dimension_selection(self.sel.clone(), pos);
                    } else if !matches!(t, Tool::Select | Tool::Move | Tool::TangentArc) {
                        self.sel.clear();
                    }
                }
            }
            Action::Constrain(kind) => self.constrain(kind),
            Action::Construction => {
                let Some((_, sk)) = self.sketch() else { return };
                let ents: Vec<Id> = self.sel.iter().copied().filter(|i| sk.entities.contains_key(i)).collect();
                if ents.is_empty() {
                    self.opts.construction = !self.opts.construction;
                } else {
                    self.sketch_edit(|sk, _| {
                        for e in &ents {
                            let e = sk.entities.get_mut(e).unwrap();
                            e.construction = !e.construction;
                        }
                        Ok(())
                    });
                }
            }
            Action::Fillet | Action::Chamfer => match self.selected_corner() {
                Some(p) => {
                    let at = self.sketch().and_then(|(id, _)| self.world_sketch(id)).map(|sk| view::to_screen(self, sk.plane.to_world(sk.pos(p))));
                    self.ask(if a == Action::Fillet { EditTarget::Fillet(p) } else { EditTarget::Chamfer(p) }, at);
                }
                None if self.sketch().is_some() => self.toast("Select a corner point, or the two lines that meet at it, first."),
                None => {}
            },
            Action::Offset => {
                let Some((_, sk)) = self.sketch() else { return };
                let ents: Vec<Id> = self.sel.iter().copied().filter(|i| sk.entities.contains_key(i)).collect();
                if ents.is_empty() {
                    self.toast("Select the lines or circles to offset first.");
                } else {
                    self.ask(EditTarget::Offset(ents), None);
                }
            }
            Action::MirrorSketch => {
                let Some((_, sk)) = self.sketch() else { return };
                // The mirror line is the line selected last.
                let axis = self.sel.last().copied().filter(|l| sk.line(*l).is_some());
                let rest: Vec<Id> = self.sel.iter().copied().filter(|i| Some(*i) != axis).collect();
                match axis {
                    Some(axis) if !rest.is_empty() => {
                        let mut new = Vec::new();
                        if self.sketch_edit(|sk, _| {
                            new = sk.mirror(&rest, axis)?;
                            Ok(())
                        }) {
                            self.sel = new;
                        }
                    }
                    _ => self.toast("Select what to mirror, then Shift-click the mirror line last."),
                }
            }
            Action::Primitive(kind) => {
                self.finish_sketch();
                self.dialog = Dialog::Primitive(crate::primitives::PrimitiveDlg::new(kind));
                self.fit_pending = true;
            }
            Action::Pattern => {
                self.finish_sketch();
                let ok = |k: &FeatureKind| matches!(k, FeatureKind::Extrude(_) | FeatureKind::Revolve(_) | FeatureKind::Sweep(_) | FeatureKind::Import(_) | FeatureKind::Primitive(_)) || matches!(k, FeatureKind::Text(t) if t.op == Op::New);
                let available = |f: &&fr_core::Feature| !self.doc().is_suppressed(f.id) && !self.session.built.errors.contains_key(&f.id) && self.session.built.components.contains_key(&f.owner) && ok(&f.kind);
                let sources: Vec<_> = self.doc().features.iter().take(self.doc().active()).filter(available).collect();
                let chosen = self.sel_feature.or(self.sel_body).filter(|id| sources.iter().any(|f| f.id == *id));
                let source = chosen.or_else(|| sources.iter().rev().find(|f| f.owner == self.doc().active_component).map(|f| f.id));
                match source {
                    Some(_) => self.dialog = Dialog::Pattern(PatternDlg::new(source)),
                    None => self.toast("There is no extrusion, revolve, sweep, primitive, standalone text or imported mesh to repeat yet."),
                }
            }
            Action::Section => self.show_section = !self.show_section,
            Action::Measure => {
                self.finish_sketch();
                self.dialog = Dialog::Measure(MeasureDlg::default());
            }
            Action::Blend(chamfer) => {
                // Start from the edges around the selected face, if there is one.
                let face = self.sel_face.clone();
                self.finish_sketch();
                let unit = self.doc().units;
                let text = format!("{} {}", if unit == Unit::In { "0.05" } else if unit == Unit::Cm { "0.1" } else { "1" }, unit.name());
                let (body, edges) = match face.as_ref().and_then(|f| self.session.built.body(f.body).map(|b| (f, b))) {
                    Some((f, b)) if b.is_exact() => (Some(b.id), b.edges.iter().map(|e| view::midpoint(e)).filter(|m| f.outline.iter().any(|ring| view::path_dist3(ring, *m) < 1e-3)).collect()),
                    _ => (None, Vec::new()),
                };
                let edges = edges.into_iter().map(|point| body.and_then(|id| self.session.built.body(id)).map_or(point, |b| b.to_local(point))).collect();
                let frame = body.and_then(|b| self.session.built.frame(b));
                self.dialog = Dialog::Blend(BlendDlg { chamfer, body, edges, text, frame });
            }
            Action::Shell => {
                let face = self.sel_face.clone().filter(|f| self.session.built.body(f.body).is_some_and(|b| b.is_exact()));
                self.finish_sketch();
                let unit = self.doc().units;
                let text = format!("{} {}", if unit == Unit::In { "0.08" } else if unit == Unit::Cm { "0.2" } else { "2" }, unit.name());
                let body = face.as_ref().map(|f| f.body);
                let faces = face.into_iter().map(|f| (self.session.built.body(f.body).map_or(f.at, |b| b.to_local(f.at)), f)).collect();
                self.dialog = Dialog::Shell(ShellDlg { body, faces, text, frame: body.and_then(|b| self.session.built.frame(b)) });
            }
            Action::Hole => {
                // Start on the selected face, where it was clicked.
                let face = self.sel_face.clone().filter(|f| f.plane.is_some()).map(|f| self.local_face(f));
                self.finish_sketch();
                let unit = self.doc().units;
                let len = |mm: f64, inch: &str| if unit == Unit::In { format!("{inch} in") } else { format!("{} {}", mm / unit.mm(), unit.name()) };
                self.dialog = Dialog::Hole(HoleDlg {
                    body: face.as_ref().map(|f| f.body),
                    at: face.iter().map(|f| f.at).collect(),
                    dir: face.as_ref().and_then(|f| f.plane).map_or(-DVec3::Z, |p| -p.normal()),
                    shape: HoleShape::Simple,
                    fit: HoleFit::Normal,
                    thread: if unit == Unit::In { "#6-32" } else { "M3x0.5" }.into(),
                    diameter: len(3.0, "0.125"),
                    through: true,
                    depth: len(5.0, "0.25"),
                    pointed: false,
                    modeled: false,
                    left: false,
                    custom_head: false,
                    head_diameter: len(6.0, "0.25"),
                    head_depth: len(3.0, "0.125"),
                    head_angle: if unit == Unit::In { "82 deg" } else { "90 deg" }.into(),
                    extra: String::new(),
                });
            }
            Action::Thread => {
                self.finish_sketch();
                let unit = self.doc().units;
                let len = |mm: f64, inch: &str| if unit == Unit::In { format!("{inch} in") } else { format!("{} {}", mm / unit.mm(), unit.name()) };
                self.dialog = Dialog::Thread(ThreadDlg { body: None, face: None, found: None, thread: String::new(), full: true, offset: len(0.0, "0"), length: len(5.0, "0.25"), left: false, extra: len(PRINT_ALLOWANCE, "0.008"), frame: None });
            }
            Action::ExportStep => {
                let exact: Vec<&fr_core::Body> = self.session.visible_bodies().filter(|b| b.is_exact()).collect();
                let skipped = self.session.visible_bodies().count() - exact.len();
                if exact.is_empty() {
                    self.toast("There are no exact bodies to write. STEP cannot hold meshes such as imported STL.");
                    return;
                }
                let bytes = io::step_with_provenance(exact.iter().flat_map(|b| &b.solids), self.session.read_only);
                let Some(path) = rfd::FileDialog::new().add_filter("STEP", &["step", "stp"]).set_file_name(format!("{}.step", self.doc_name())).save_file() else { return };
                match bytes.and_then(|b| std::fs::write(&path, b).map_err(|e| format!("Could not write {}: {e}", path.display()))) {
                    Ok(()) if skipped > 0 => self.toast(format!("Exported to {}. {skipped} mesh bod{} left out.", path.display(), if skipped == 1 { "y was" } else { "ies were" })),
                    Ok(()) => self.toast(format!("Exported to {}", path.display())),
                    Err(e) => self.file_error("Could not export STEP", &path, e, "Your design is still open. The export did not complete."),
                }
            }
            Action::Cancel => {
                if self.reference_editor.sketch_id().is_some() { self.reference_editor.cancel(); return; }
                if self.reference_drag.is_dragging() { self.reference_drag.cancel_drag(); return; }
                if let Mode::Sketch(sid) = self.mode && self.reference_drag.selected(sid) { self.reference_drag.clear(); return; }
                if self.timeline.preview.take().is_some() {
                    return;
                }
                if self.value_edit.is_some() {
                    self.value_edit = None;
                } else if self.dialog != Dialog::None {
                    self.dialog = Dialog::None;
                } else if !self.clicks.is_empty() || !self.dim_refs.is_empty() {
                    self.cancel_tool();
                } else if self.tool != Tool::Select {
                    self.tool = Tool::Select;
                } else {
                    self.sel.clear();
                    self.sel_body = None;
                    self.sel_component = None;
                    self.sel_face = None;
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &Context) {
        if self.command_search.block_input { return; }
        if self.file_error.is_some() {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape) || i.consume_key(Modifiers::NONE, Key::Enter)) {
                self.file_error = None;
            }
            return;
        }
        if self.show_about {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape) || i.consume_key(Modifiers::NONE, Key::Enter)) { self.show_about = false; }
            return;
        }
        // The clipboard keys arrive as events, not key presses.
        let typing = ctx.egui_wants_keyboard_input();
        for e in ctx.input(|i| i.events.clone()) {
            match e {
                egui::Event::Copy if !typing => self.run(ctx, Action::Copy),
                egui::Event::Cut if !typing => self.run(ctx, Action::Cut),
                egui::Event::Paste(text) if !typing => self.paste(Some(&text)),
                _ => {}
            }
        }
        let cmd = Modifiers::COMMAND;
        let always = [(cmd | Modifiers::SHIFT, Key::Z, Action::Redo), (cmd, Key::Z, Action::Undo), (cmd, Key::Y, Action::Redo), (cmd | Modifiers::SHIFT, Key::S, Action::SaveAs), (cmd, Key::S, Action::Save), (cmd, Key::O, Action::Open), (cmd, Key::N, Action::New), (cmd, Key::E, Action::Export), (cmd, Key::I, Action::Import)];
        for (m, k, a) in always {
            if !(typing && k == Key::Z) && ctx.input_mut(|i| i.consume_key(m, k)) {
                self.run(ctx, a);
            }
        }
        if self.reference_editor.sketch_id().is_some() || matches!(self.dialog, Dialog::PointCoordinates(_)) {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) { self.run(ctx, Action::Cancel); }
            else if !typing && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F)) { self.fit(); }
            return;
        }
        let enter = ctx.input(|i| i.modifiers.is_none() && i.key_pressed(Key::Enter));
        if typing {
            // Enter in one of a dialog's own boxes confirms the dialog.
            if enter && (self.dialog.has_preview() || matches!(self.dialog, Dialog::Script(_))) && self.value_edit.is_none() && !self.show_params && !self.ai.open && self.rename.is_none() {
                self.update_preview();
                self.apply_dialog();
            }
            return;
        }
        if crate::command_search::can_open(self) && ctx.input(|i| i.modifiers.is_none() && !i.pointer.any_down() && !i.pointer.any_pressed()) {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::S)) {
                crate::command_search::consume_shortcut_text(ctx, 's');
                self.run(ctx, Action::CommandSearch);
                return;
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::M)) {
                crate::command_search::consume_shortcut_text(ctx, 'm');
                self.run(ctx, Action::Transform);
                return;
            }
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::A)) {
            self.run(ctx, Action::SelectAll);
        }
        let key = |k: Key| ctx.input(|i| i.modifiers.is_none() && i.key_pressed(k));
        if key(Key::Escape) {
            self.run(ctx, Action::Cancel);
        }
        if key(Key::Delete) || key(Key::Backspace) {
            self.run(ctx, Action::Delete);
        }
        if key(Key::Enter) {
            if self.dialog.has_preview() || matches!(self.dialog, Dialog::Script(_)) {
                self.apply_dialog();
            } else {
                self.cancel_tool();
            }
        }
        if key(Key::F) {
            self.run(ctx, Action::Fit);
        }
        if key(Key::I) && self.sketch().is_none() {
            self.run(ctx, Action::Measure);
        }
        if self.sketch().is_some() {
            for (k, a) in [(Key::L, Action::Tool(Tool::Line)), (Key::R, Action::Tool(Tool::Rect)), (Key::C, Action::Tool(Tool::Circle)), (Key::A, Action::Tool(Tool::Arc)), (Key::P, Action::Tool(Tool::Point)), (Key::D, Action::Tool(Tool::Dimension)), (Key::V, Action::Tool(Tool::Select)), (Key::X, Action::Construction), (Key::T, Action::Tool(Tool::Trim)), (Key::O, Action::Offset)] {
                if key(k) {
                    self.run(ctx, a);
                }
            }
        }
        if key(Key::E) {
            self.run(ctx, Action::Extrude);
        }
    }
}

fn setup_style(ctx: &Context, appearance: Appearance) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    theme::setup(ctx, appearance);
}

impl eframe::App for App {
    fn on_exit(&mut self) {
        // Closing was agreed to, so nothing here is lost work.
        if let Some(r) = &mut self.recovery {
            r.close();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.command_search.begin_frame(&ctx);
        self.now = ctx.input(|i| i.time);
        theme::apply(&ctx, self.config.appearance, self.native_theme.current());
        ui.set_style(ctx.global_style());
        // Native appearance can change without a window event while it has an override.
        if !cfg!(test) && self.config.appearance == Appearance::System {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }

        // Commands from the MCP server and the assistant run here, on the UI thread.
        if let Some(bridge) = self.bridge.take() {
            bridge.serve(|cmd| self.execute(cmd));
            self.bridge = Some(bridge);
        }
        let mut ai = std::mem::take(&mut self.ai);
        ai.poll(self);
        self.ai = ai;

        self.shortcuts(&ctx);
        crate::command_search::show(self, &ctx);
        if self.command_search.block_input { ui.disable(); }
        self.process_open_request(Self::confirm_discard);
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            if let Some(p) = Some(f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()) {
                if is_mesh_file(&p) || self.confirm_discard() {
                    self.open_path(&p);
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.confirm_discard() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
        let title = format!("{}{} \u{2013} Ferrender", self.doc_name(), if self.session.dirty { "*" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.timeline.poll(&ctx, self.session.rev, self.render_queue.as_ref());
        self.update_preview();
        if let Some(r) = &mut self.recovery
            && let Some(wait) = r.tick(&self.session, self.now)
        {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }

        let colors = theme::Palette::from_ctx(&ctx);
        let bar = egui::Frame::new().fill(colors.bar).inner_margin(egui::Margin::symmetric(10, 4));
        egui::Panel::top("menu").frame(bar).show(ui, |ui| panels::menu_bar(self, ui));
        let bar = egui::Frame::new().fill(colors.panel).inner_margin(egui::Margin::symmetric(10, 6));
        egui::Panel::top("toolbar").frame(bar).show(ui, |ui| panels::toolbar(self, ui));
        let bar = egui::Frame::new().fill(colors.bar).inner_margin(egui::Margin::symmetric(10, 3));
        egui::Panel::bottom("status").frame(bar).show(ui, |ui| {
            panels::status(self, ui);
            if self.session.read_only {
                ui.colored_label(colors.error, "Unverified preview — newer design; read-only.")
                    .on_hover_text("This Ferrender cannot rebuild the newer timeline. Saved geometry is unverified; exports retain that warning.");
            }
            if let Some(error) = self.recovery.as_ref().and_then(Recovery::error) {
                ui.colored_label(colors.error, "Recovery unavailable — save your work. Retrying…").on_hover_text(error);
            }
        });
        let bar = egui::Frame::new().fill(colors.panel).inner_margin(egui::Margin::symmetric(10, 6));
        egui::Panel::bottom("timeline").frame(bar).show(ui, |ui| panels::timeline(self, ui));
        let side = egui::Frame::new().fill(colors.panel).inner_margin(egui::Margin::symmetric(8, 8));
        egui::Panel::left("browser").frame(side).exact_size(230.0).resizable(false).show(ui, |ui| panels::browser(self, ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(colors.background)).show(ui, |ui| view::viewport(self, ui));
        panels::windows(self, &ctx);
        crate::scripts_ui::windows(self, &ctx);

        if let Some((msg, until)) = &self.toast {
            if self.now > *until {
                self.toast = None;
            } else {
                let pos = self.vp.center_bottom() - egui::vec2(0.0, 28.0);
                egui::Area::new("toast".into()).order(egui::Order::Tooltip).fixed_pos(pos).pivot(egui::Align2::CENTER_BOTTOM).show(&ctx, |ui| {
                    egui::Frame::new().fill(Color32::from_rgb(40, 44, 52)).corner_radius(6).inner_margin(egui::Margin::symmetric(12, 7)).show(ui, |ui| {
                        ui.set_max_width(520.0);
                        ui.label(egui::RichText::new(msg).color(Color32::WHITE));
                    });
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
    }
}

/// A file the mesh importer reads rather than a design.
pub fn is_mesh_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| ["stl", "obj", "3mf"].iter().any(|m| e.eq_ignore_ascii_case(m)))
}
