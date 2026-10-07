use std::path::{Path, PathBuf};

use egui::{Color32, Context, Key, Modifiers, Pos2, Rect, ViewportCommand};
use fr_core::doc::{Blend, Combine, Extrude, Hole, HoleFit, HoleShape, Pattern, PatternKind, Revolve, Shell, Thread, Transform};
pub use fr_core::face::Face;
use fr_core::render::Camera;
use fr_core::sketch::Clip;
use fr_core::{Axis, Built, CKind, Document, FeatureKind, Geom, Id, Kind, ORIGIN, Op, Plane, Session, Sketch, Unit, api, io, solver};
use glam::{DVec2, DVec3};
use serde_json::json;

use crate::ai::Assistant;
use crate::bridge::Bridge;
use crate::config::Config;
use crate::recovery::{Found, Recovery};
use crate::{panels, view};

pub const BG_VIEW: Color32 = Color32::from_rgb(250, 250, 251);
pub const BG_PANEL: Color32 = Color32::from_rgb(243, 244, 246);
pub const BG_BAR: Color32 = Color32::from_rgb(236, 237, 240);
pub const ACCENT: Color32 = Color32::from_rgb(0, 104, 214);

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
    Point,
    Dimension,
    Polygon,
    /// Click a stretch of an entity to cut it back to its crossings.
    Trim,
    /// Click a face of a body to copy its outline into the sketch.
    Project,
}

impl Tool {
    /// Clicks needed to place one of these.
    pub fn clicks(self) -> usize {
        match self {
            Tool::Line | Tool::Rect | Tool::Circle | Tool::Polygon => 2,
            Tool::Arc => 3,
            Tool::Point => 1,
            Tool::Select | Tool::Dimension | Tool::Trim | Tool::Project => 0,
        }
    }

    pub fn hint(self, placed: usize) -> &'static str {
        match (self, placed) {
            (Tool::Select, _) => "Click to select, drag to move. Shift-click adds to the selection.",
            (Tool::Line, 0) => "Click to start a line.",
            (Tool::Line, _) => "Click the next point. Esc or Enter ends the line.",
            (Tool::Rect, 0) => "Click the first corner.",
            (Tool::Rect, _) => "Click the opposite corner.",
            (Tool::Circle, 0) => "Click the centre.",
            (Tool::Circle, _) => "Click to set the radius.",
            (Tool::Arc, 0) => "Click the arc's centre.",
            (Tool::Arc, 1) => "Click where the arc starts.",
            (Tool::Arc, _) => "Click where the arc ends.",
            (Tool::Point, _) => "Click to place a point.",
            (Tool::Polygon, 0) => "Click the polygon's centre. Set the number of sides in the Sketch Palette.",
            (Tool::Polygon, _) => "Click to place a corner.",
            (Tool::Trim, _) => "Click the part of a line, arc or circle to remove. It is cut back to where other geometry crosses it.",
            (Tool::Project, _) => "Click a face of a body to copy its outline into the sketch.",
            (Tool::Dimension, _) => "Click a line, circle or arc, or two points or lines, then type the size.",
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

/// The Extrude and Revolve dialogs.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureDlg {
    pub revolve: bool,
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

#[derive(Clone, Debug, PartialEq)]
pub struct PatternDlg {
    pub source: Option<Id>,
    /// 0 circular, 1 linear, 2 mirror.
    pub kind: usize,
    pub axis: usize,
    pub count: u32,
    /// Total angle or spacing.
    pub text: String,
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
pub enum Dialog {
    None,
    /// Waiting for a plane or a flat face to sketch on.
    PickPlane,
    Feature(FeatureDlg),
    Transform(TransformDlg),
    Combine(CombineDlg),
    Pattern(PatternDlg),
    Blend(BlendDlg),
    Shell(ShellDlg),
    Hole(HoleDlg),
    Thread(ThreadDlg),
    Measure(MeasureDlg),
    Export(Unit),
    Import(PathBuf, Unit),
}

impl PatternDlg {
    /// The pattern the dialog describes.
    pub fn pattern(&self, d: &Document) -> Result<PatternKind, String> {
        let text = self.text.split_once('=').map_or(self.text.as_str(), |t| t.0.trim());
        let text = if self.text.contains('=') { format!("${}", text.trim_start_matches('$')) } else { text.to_owned() };
        Ok(match self.kind {
            0 => PatternKind::Circular { axis: self.axis, count: self.count, angle: d.value(&text, Kind::Angle)? },
            1 => PatternKind::Linear { axis: self.axis, count: self.count, spacing: d.value(&text, Kind::Length)? },
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
    /// Where the plane sits along that axis, in millimetres.
    pub offset: f64,
    pub flip: bool,
}

impl Dialog {
    /// Adds or updates the feature the dialog describes; returns its id.
    pub fn apply(&self, d: &mut Document) -> Result<Id, String> {
        match self {
            Dialog::Feature(f) => {
                if let (Some(face), false, None) = (&f.face, f.revolve, f.editing) {
                    let (sk, profiles) = face.sketch()?;
                    let distance = d.enter(&f.text, Kind::Length)?;
                    // Pushing a face inward removes material, as pulling it out adds.
                    let op = if distance.v < 0.0 && f.op == Op::Join { Op::Cut } else { f.op };
                    let sketch = d.add_feature(FeatureKind::Sketch(sk));
                    d.feature_mut(sketch).unwrap().name = format!("Face{sketch}");
                    let taper = f.taper(d)?;
                    return Ok(d.add_feature(FeatureKind::Extrude(Extrude { sketch, profiles, distance, symmetric: false, op, taper, through_all: f.through_all })));
                }
                let sketch = f.sketch.filter(|_| !f.profiles.is_empty()).ok_or(if f.revolve { "Click a closed sketch profile in the viewport." } else { "Click a closed sketch profile or a flat face in the viewport." })?;
                let kind = if f.revolve {
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
                        Ok(d.add_feature(kind))
                    }
                }
            }
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
                let kind = p.pattern(d)?;
                Ok(d.add_feature(FeatureKind::Pattern(Pattern { source, kind })))
            }
            Dialog::Blend(b) => {
                let body = b.body.filter(|_| !b.edges.is_empty()).ok_or("Click the edges to blend.")?;
                let size = d.enter(&b.text, Kind::Length)?;
                Ok(d.add_feature(FeatureKind::Blend(Blend { body, edges: b.edges.clone(), size, chamfer: b.chamfer, frame: b.frame })))
            }
            Dialog::Shell(sh) => {
                let body = sh.body.filter(|_| !sh.faces.is_empty()).ok_or("Click the faces to leave open.")?;
                let thickness = d.enter(&sh.text, Kind::Length)?;
                Ok(d.add_feature(FeatureKind::Shell(Shell { body, faces: sh.faces.iter().map(|f| f.0).collect(), thickness, frame: sh.frame })))
            }
            Dialog::Hole(h) => {
                let hole = h.hole(d)?;
                hole.sizes()?;
                Ok(d.add_feature(FeatureKind::Hole(hole)))
            }
            Dialog::Thread(t) => {
                let (body, face) = t.body.zip(t.face).ok_or("Click the rod or the hole to thread.")?;
                let (offset, length) = if t.full { (None, None) } else { (Some(d.enter(&t.offset, Kind::Length)?), Some(d.enter(&t.length, Kind::Length)?)) };
                let extra = if t.extra.trim().is_empty() { None } else { Some(d.enter(&t.extra, Kind::Length)?) };
                Ok(d.add_feature(FeatureKind::Thread(Thread { body, face, frame: t.frame, thread: t.thread.clone(), offset, length, left: t.left, extra })))
            }
            _ => Err("Nothing to apply.".into()),
        }
    }

    pub fn has_preview(&self) -> bool {
        matches!(self, Dialog::Feature(_) | Dialog::Transform(_) | Dialog::Combine(_) | Dialog::Pattern(_) | Dialog::Blend(_) | Dialog::Shell(_) | Dialog::Hole(_) | Dialog::Thread(_))
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
    /// Sides for the polygon tool.
    pub sides: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
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
    Transform,
    Combine,
    Parameters,
    Assistant,
    View(&'static str),
    Fit,
    Tool(Tool),
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
    Measure,
    ExportStep,
    Cancel,
}

pub struct App {
    pub session: Session,
    /// Scale is in points per millimetre.
    pub cam: Camera,
    pub mode: Mode,
    pub tool: Tool,
    /// Selected points, entities and constraints of the active sketch.
    pub sel: Vec<Id>,
    pub sel_body: Option<Id>,
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
    /// Where the timeline's chips were drawn last frame, for dragging the roll-back marker.
    pub chips: Vec<Rect>,
    pub dialog: Dialog,
    /// The dialog's result, built on a copy of the document: (dialog it was built for, bodies, error).
    pub preview: Option<(Dialog, Built, Option<String>)>,
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
    pub report: solver::Report,
    pub toast: Option<(String, f64)>,
    pub now: f64,
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
    pub fit_pending: bool,
    pub bridge: Option<Bridge>,
    /// This app's recovery copy; none while testing unless a test sets one.
    pub recovery: Option<Recovery>,
    /// Unsaved designs left by an app that did not close, offered for recovery.
    pub recover: Vec<Found>,
    pub ai: Assistant,
    pub config: Config,
    pub ctx: Context,
    title: String,
}

const CLIP_MARK: &str = "ferrender_clip";
/// The room a modeled thread is given unless told otherwise, in millimetres across: enough for
/// a printed thread to turn on most printers.
pub const PRINT_ALLOWANCE: f64 = 0.2;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>) -> App {
        setup_style(&cc.egui_ctx);
        let gpu = match &cc.wgpu_render_state {
            Some(rs) => {
                rs.renderer.write().callback_resources.insert(crate::gpu::Gpu::new(&rs.device, rs.target_format));
                true
            }
            None => false,
        };
        let (config, config_error) = if cfg!(test) { (Config::default(), None) } else { Config::load() };
        let mut app = App {
            session: Session::default(),
            cam: Camera::iso(),
            mode: Mode::Model,
            tool: Tool::Select,
            sel: Vec::new(),
            sel_body: None,
            sel_face: None,
            sel_feature: None,
            clicks: Vec::new(),
            dim_refs: Vec::new(),
            drag: Drag::None,
            drag_value: 0.0,
            pattern_at: None,
            chips: Vec::new(),
            dialog: Dialog::None,
            preview: None,
            value_edit: None,
            typed: None,
            clipboard: None,
            pastes: 0,
            opts: Opts { construction: false, grid: true, snap_grid: false, constraints: true, dimensions: true, sides: 6 },
            vp: Rect::NOTHING,
            labels: Vec::new(),
            report: solver::Report::default(),
            toast: None,
            now: 0.0,
            show_params: false,
            show_section: false,
            section: Section { on: false, axis: 1, offset: 0.0, flip: false },
            plane_offset: String::new(),
            param_new: Default::default(),
            param_edit: None,
            rename: None,
            gpu,
            scene: view::Scene::default(),
            fit_pending: false,
            bridge: if cfg!(test) || !config.bridge.enabled { None } else { Bridge::start(config.bridge.port, cc.egui_ctx.clone()) },
            recovery: if cfg!(test) { None } else { Config::path().parent().map(|d| Recovery::start(d.join("recovery"))) },
            recover: Vec::new(),
            ai: Assistant::default(),
            config,
            ctx: cc.egui_ctx.clone(),
            title: String::new(),
        };
        if let Some(e) = config_error {
            app.toast(format!("Couldn't read the settings file, using defaults. {e}"));
        }
        if let Some(f) = file {
            app.open_path(&f);
        }
        app.recover = app.recovery.as_ref().map_or(Vec::new(), Recovery::found);
        app
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
        self.preview = None;
    }

    /// Changes the active sketch as one undo step, rejecting changes its constraints cannot hold.
    pub fn sketch_edit(&mut self, f: impl FnOnce(&mut Sketch, &mut Document) -> Result<(), String>) -> bool {
        let Mode::Sketch(sid) = self.mode else { return false };
        let r = self.session.edit(|d| {
            let mut sk = d.sketch(sid).cloned().ok_or("The sketch no longer exists.")?;
            f(&mut sk, d)?;
            let report = solver::solve(&mut sk, &[]);
            *d.sketch_mut(sid).unwrap() = sk;
            if report.ok { Ok(()) } else { Err("That conflicts with the sketch's other constraints.".to_owned()) }
        });
        if let Err(e) = &r {
            self.toast(e.clone());
        }
        self.refresh();
        r.is_ok()
    }

    pub fn edit_sketch(&mut self, id: Id) {
        let Some(sk) = self.session.doc.sketch_mut(id) else { return };
        sk.visible = true;
        let (plane, bounds) = (sk.plane, sk.bbox());
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
        self.leave_sketch();
        (self.cam.yaw, self.cam.pitch) = (Camera::iso().yaw, Camera::iso().pitch);
        self.session.rebuild();
        self.refresh();
    }

    pub fn create_sketch(&mut self, plane: Plane) {
        let plane = match self.doc().eval(&self.plane_offset, Kind::Length) {
            Ok(d) if !self.plane_offset.trim().is_empty() => plane.offset(d),
            _ => plane,
        };
        self.plane_offset.clear();
        match self.session.edit(|d| Ok(d.add_feature(FeatureKind::Sketch(Sketch::new(plane))))) {
            Ok(id) => self.edit_sketch(id),
            Err(e) => self.toast(e),
        }
    }

    /// Abandons whatever the current tool was in the middle of.
    pub fn cancel_tool(&mut self) {
        self.clicks.clear();
        self.dim_refs.clear();
        self.value_edit = None;
        self.typed = None;
        self.drag = Drag::None;
    }

    /// Turns the placed clicks into geometry once the tool has enough of them.
    pub fn commit_clicks(&mut self) {
        let (tool, c, construction, sides) = (self.tool, self.clicks.clone(), self.opts.construction, self.opts.sides.clamp(3, 64));
        if c.len() < tool.clicks() || tool.clicks() == 0 {
            return;
        }
        let place = |sk: &mut Sketch, s: &Snap| match s.point {
            Some(p) => p,
            None => {
                let id = sk.add_point(s.p);
                if let Some(e) = s.on {
                    let _ = sk.add_constraint(CKind::Coincident, &[id, e], None);
                }
                id
            }
        };
        let mut last = None;
        // Sizes typed into the boxes become dimensions on what is drawn.
        let typed = self.typed.take().map_or(Vec::new(), |t| t.fields);
        let size = |d: &mut Document, i: usize| typed.get(i).filter(|t| Self::typed_size(d, t).is_some()).and_then(|t| d.enter(t, Kind::Length).ok());
        let done = self.sketch_edit(|sk, d| {
            match tool {
                Tool::Line => {
                    if c[0].p.distance(c[1].p) < 1e-6 || (c[0].point.is_some() && c[0].point == c[1].point) {
                        return Err("A line needs two different points.".into());
                    }
                    let (a, b) = (place(sk, &c[0]), place(sk, &c[1]));
                    let l = sk.add(Geom::Line { a, b }, construction);
                    if c[1].point.is_none() && (c[1].h || c[1].v) {
                        let _ = sk.add_constraint(if c[1].h { CKind::Horizontal } else { CKind::Vertical }, &[l], None);
                    }
                    if let Some(v) = size(d, 0) {
                        sk.add_constraint(CKind::Distance, &[l], Some(v))?;
                    }
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
                Tool::Select | Tool::Dimension | Tool::Trim | Tool::Project => {}
            }
            Ok(())
        });
        self.clicks.clear();
        // A line carries on from its end until it lands on an existing point.
        if let (true, Tool::Line, Some(b), Some(end)) = (done, tool, last, c.get(1).filter(|s| s.point.is_none())) {
            self.clicks.push(Snap { p: end.p, point: Some(b), on: None, h: false, v: false });
        } else if !done && tool == Tool::Line {
            self.clicks.push(c[0]);
        }
    }

    /// The size a typed box holds, in millimetres, if what is in it is a usable length.
    pub fn typed_size(d: &Document, text: &str) -> Option<f64> {
        let rhs = text.split_once('=').map_or(text, |p| p.1);
        d.value(rhs, Kind::Length).ok().map(|v| v.v).filter(|v| *v > 1e-9)
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

    /// Confirms the dimension box. Returns false, keeping it open, if the value is not usable.
    pub fn commit_value(&mut self) -> bool {
        let Some(edit) = self.value_edit.clone() else { return true };
        let Mode::Sketch(sid) = self.mode else { return true };
        let r = self.session.edit(|d| {
            let mut sk = d.sketch(sid).cloned().ok_or("The sketch no longer exists.")?;
            let positive = |v: fr_core::Value| if v.v > 0.0 { Ok(v) } else { Err("Dimensions must be greater than zero.".to_owned()) };
            match &edit.target {
                EditTarget::Existing(cid) => {
                    let kind = sk.constraints.get(cid).and_then(|c| c.kind.value_kind()).ok_or("That dimension no longer exists.")?;
                    sk.constraints.get_mut(cid).unwrap().value = Some(positive(d.enter(&edit.text, kind)?)?);
                }
                EditTarget::New(kind, refs) => {
                    let v = positive(d.enter(&edit.text, kind.value_kind().unwrap())?)?;
                    sk.add_constraint(*kind, refs, Some(v))?;
                }
                EditTarget::Fillet(p) | EditTarget::Chamfer(p) => {
                    sk.round_corner(*p, positive(d.enter(&edit.text, Kind::Length)?)?, matches!(edit.target, EditTarget::Chamfer(_)))?;
                }
                EditTarget::Offset(ids) => {
                    sk.offset(ids, d.enter(&edit.text, Kind::Length)?)?;
                }
            }
            let report = solver::solve(&mut sk, &[]);
            *d.sketch_mut(sid).unwrap() = sk;
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
        let Some((_, sk)) = self.sketch() else {
            self.toast("Open a sketch to paste into.");
            return;
        };
        // Under the pointer if it is over the viewport, otherwise next to the original.
        let under = self.ctx.input(|i| i.pointer.hover_pos()).filter(|p| self.vp.contains(*p)).and_then(|p| view::sketch_pos(self, sk, p));
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
        let _ = self.session.edit(|d| {
            d.features.retain(|f| f.id != id);
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
            FeatureKind::Sketch(_) => self.edit_sketch(id),
            FeatureKind::Extrude(e) => {
                self.finish_sketch();
                self.dialog = Dialog::Feature(FeatureDlg { revolve: false, editing: Some(id), sketch: Some(e.sketch), profiles: e.profiles.clone(), text: shown(&e.distance), symmetric: e.symmetric, op: e.op, axis: Axis::Y, pick_axis: false, face: None, taper: e.taper.as_ref().map_or(String::new(), shown), through_all: e.through_all, pick_to: false });
            }
            FeatureKind::Revolve(r) => {
                self.finish_sketch();
                self.dialog = Dialog::Feature(FeatureDlg { revolve: true, editing: Some(id), sketch: Some(r.sketch), profiles: r.profiles.clone(), text: shown(&r.angle), symmetric: false, op: r.op, axis: r.axis, pick_axis: false, face: None, taper: String::new(), through_all: false, pick_to: false });
            }
            _ => self.toast("This feature has no settings to edit; delete it and add it again to change it."),
        }
    }

    fn open_feature_dialog(&mut self, revolve: bool) {
        // Coming straight from a sketch with one closed shape, use it.
        let from = self.sketch().map(|(id, _)| id);
        let face = self.sel_face.clone().filter(|f| !revolve && from.is_none() && f.plane.is_some());
        self.finish_sketch();
        let doc = self.doc();
        let candidates: Vec<Id> = match from {
            Some(id) => vec![id],
            None => doc.sketches().filter(|(_, s)| s.visible).map(|(f, _)| f.id).collect(),
        };
        let mut picked = (None, Vec::new());
        if let [only] = candidates.as_slice() {
            let all = fr_core::profile::profiles(doc.sketch(*only).unwrap());
            let outer: Vec<Vec<Id>> = all.iter().filter(|p| p.depth % 2 == 0).map(|p| p.edges.clone()).collect();
            if all.len() == 1 || from.is_some() && !outer.is_empty() {
                picked = (Some(*only), outer);
            }
        }
        if face.is_some() {
            picked = (None, Vec::new());
        }
        let op = if self.session.built.bodies.is_empty() { Op::New } else { Op::Join };
        let unit = doc.units;
        let text = if revolve { "360 deg".to_owned() } else { format!("{} {}", if unit == Unit::In { "0.5" } else if unit == Unit::Cm { "1" } else { "10" }, unit.name()) };
        self.dialog = Dialog::Feature(FeatureDlg { revolve, editing: None, sketch: picked.0, profiles: picked.1, text, symmetric: false, op, axis: Axis::Y, pick_axis: false, face, taper: String::new(), through_all: false, pick_to: false });
    }

    /// Confirms the open dialog.
    pub fn apply_dialog(&mut self) {
        let dlg = self.dialog.clone();
        match self.session.edit_feature(|d| dlg.apply(d).map(|id| (id, id))) {
            Ok(id) => {
                self.dialog = Dialog::None;
                self.sel_feature = Some(id);
                self.refresh();
            }
            Err(e) => self.toast(e),
        }
    }

    /// Keeps the dialog's preview current.
    pub fn update_preview(&mut self) {
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
                let built = doc.rebuild();
                let e = built.errors.get(&id).cloned();
                (if e.is_some() { self.session.built.clone() } else { built }, e)
            }
            Err(e) => (self.session.built.clone(), Some(e)),
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
        let mut bounds = fr_core::render::scene_bounds(&self.session);
        if let Some((_, sk)) = self.sketch()
            && let Some((lo, hi)) = sk.bbox()
        {
            let (a, b) = (sk.plane.to_world(lo), sk.plane.to_world(hi));
            bounds = Some(bounds.map_or((a.min(b), a.max(b)), |(l, h)| (l.min(a).min(b), h.max(a).max(b))));
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
        if let Some(r) = &mut self.recovery {
            r.new_document();
        }
        self.leave_sketch();
        self.typed = None;
        self.param_edit = None;
        self.rename = None;
        self.pattern_at = None;
        self.scene.invalidate();
        self.section.on = false;
        self.dialog = Dialog::None;
        self.sel_body = None;
        self.sel_feature = None;
        self.cam = Camera::iso();
        self.fit_pending = true;
        self.refresh();
    }

    pub fn open_path(&mut self, path: &Path) {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("stl")) {
            self.dialog = Dialog::Import(path.to_owned(), Unit::Mm);
            return;
        }
        match Session::open(path) {
            Ok(s) => self.replace_session(s),
            Err(e) => self.toast(e),
        }
    }

    fn save(&mut self, ask: bool) {
        self.finish_value();
        let path = match (&self.session.path, ask) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new().add_filter("Ferrender design", &["ferr"]).set_file_name(format!("{}.ferr", self.doc_name())).save_file(),
        };
        if let Some(p) = path {
            match self.session.save(&p) {
                Ok(()) => self.toast(format!("Saved {}", p.display())),
                Err(e) => self.toast(e),
            }
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

    pub fn import_stl(&mut self, path: &Path, unit: Unit) {
        let r = io::read_stl(path, unit).and_then(|mesh| {
            let tris = mesh.tris.len();
            let name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
            self.session.edit(|d| {
                let id = d.add_feature(FeatureKind::Import(mesh));
                if let Some(n) = name {
                    d.feature_mut(id).unwrap().name = n;
                }
                Ok((id, tris))
            })
        });
        match r {
            Ok((id, tris)) => {
                self.sel_body = Some(id);
                self.fit_pending = true;
                self.toast(format!("Imported {tris} triangles."));
            }
            Err(e) => self.toast(e),
        }
        self.refresh();
    }

    pub fn export_stl(&mut self, unit: Unit) {
        if self.session.visible_bodies().next().is_none() {
            self.toast("There are no visible bodies to export.");
            return;
        }
        let Some(path) = rfd::FileDialog::new().add_filter("STL mesh", &["stl"]).set_file_name(format!("{}.stl", self.doc_name())).save_file() else { return };
        match io::write_stl(self.session.visible_bodies(), unit, &path) {
            Ok(n) => self.toast(format!("Exported {n} triangles in {} to {}", unit.name(), path.display())),
            Err(e) => self.toast(e),
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
        self.refresh();
        r
    }

    pub fn run(&mut self, ctx: &Context, a: Action) {
        match a {
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
                if let Some(p) = rfd::FileDialog::new().add_filter("STL mesh", &["stl"]).pick_file() {
                    self.finish_sketch();
                    self.dialog = Dialog::Import(p, Unit::Mm);
                }
            }
            Action::Export => {
                self.finish_sketch();
                self.dialog = Dialog::Export(Unit::Mm);
            }
            Action::Undo | Action::Redo => {
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
                if let Some((_, sk)) = self.sketch() {
                    self.sel = sk.entities.keys().copied().chain(sk.points.keys().copied().filter(|p| *p != ORIGIN)).collect();
                    self.tool = Tool::Select;
                }
            }
            Action::NewSketch => {
                // With a flat face selected, sketch straight on it.
                let on = self.sel_face.as_ref().filter(|_| self.sketch().is_none()).and_then(|f| f.plane);
                self.finish_sketch();
                match on {
                    Some(plane) => self.create_sketch(plane),
                    None => self.dialog = Dialog::PickPlane,
                }
            }
            Action::FinishSketch => self.finish_sketch(),
            Action::Extrude => self.open_feature_dialog(false),
            Action::Revolve => self.open_feature_dialog(true),
            Action::Transform => {
                self.finish_sketch();
                match self.target_body().or(self.session.built.bodies.first().map(|b| b.id).filter(|_| self.session.built.bodies.len() == 1)) {
                    Some(body) => {
                        let z = || "0".to_owned();
                        self.dialog = Dialog::Transform(TransformDlg { body, translate: [z(), z(), z()], rotate: [z(), z(), z()], scale: "1".into() });
                    }
                    None => self.toast("Click a body to select it first."),
                }
            }
            Action::Combine => {
                self.finish_sketch();
                if self.session.built.bodies.len() < 2 {
                    self.toast("Combine needs at least two bodies.");
                } else {
                    self.dialog = Dialog::Combine(CombineDlg { target: self.target_body(), tools: Vec::new(), op: Op::Join, keep_tools: false });
                }
            }
            Action::Parameters => self.show_params = !self.show_params,
            Action::Assistant => self.ai.open = !self.ai.open,
            Action::View(name) => {
                if let Some((yaw, pitch)) = Camera::named(name) {
                    (self.cam.yaw, self.cam.pitch) = (yaw, pitch);
                }
            }
            Action::Fit => self.fit(),
            Action::Tool(t) => {
                if self.sketch().is_some() {
                    self.cancel_tool();
                    self.tool = t;
                    if t != Tool::Select {
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
                    let at = self.sketch().map(|(_, sk)| view::to_screen(self, sk.plane.to_world(sk.pos(p))));
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
            Action::Pattern => {
                self.finish_sketch();
                let ok = |k: &FeatureKind| matches!(k, FeatureKind::Extrude(_) | FeatureKind::Revolve(_) | FeatureKind::Import(_));
                let chosen = self.sel_feature.filter(|f| self.doc().feature(*f).is_some_and(|f| ok(&f.kind)));
                let source = chosen.or(self.doc().features.iter().rfind(|f| ok(&f.kind)).map(|f| f.id));
                match source {
                    Some(_) => self.dialog = Dialog::Pattern(PatternDlg { source, kind: 0, axis: 2, count: 4, text: "360 deg".into() }),
                    None => self.toast("There is no extrude, revolve or imported mesh to repeat yet."),
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
                let frame = body.and_then(|b| self.session.built.frame(b));
                self.dialog = Dialog::Blend(BlendDlg { chamfer, body, edges, text, frame });
            }
            Action::Shell => {
                let face = self.sel_face.clone().filter(|f| self.session.built.body(f.body).is_some_and(|b| b.is_exact()));
                self.finish_sketch();
                let unit = self.doc().units;
                let text = format!("{} {}", if unit == Unit::In { "0.08" } else if unit == Unit::Cm { "0.2" } else { "2" }, unit.name());
                let body = face.as_ref().map(|f| f.body);
                let faces = face.into_iter().map(|f| (view::face_point(&f), f)).collect();
                self.dialog = Dialog::Shell(ShellDlg { body, faces, text, frame: body.and_then(|b| self.session.built.frame(b)) });
            }
            Action::Hole => {
                // Start on the selected face, where it was clicked.
                let face = self.sel_face.clone().filter(|f| f.plane.is_some());
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
                let bytes = fr_core::exact::step(exact.iter().flat_map(|b| &b.solids));
                let Some(path) = rfd::FileDialog::new().add_filter("STEP", &["step", "stp"]).set_file_name(format!("{}.step", self.doc_name())).save_file() else { return };
                match bytes.and_then(|b| std::fs::write(&path, b).map_err(|e| format!("Could not write {}: {e}", path.display()))) {
                    Ok(()) if skipped > 0 => self.toast(format!("Exported to {}. {skipped} mesh bod{} left out.", path.display(), if skipped == 1 { "y was" } else { "ies were" })),
                    Ok(()) => self.toast(format!("Exported to {}", path.display())),
                    Err(e) => self.toast(e),
                }
            }
            Action::Cancel => {
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
                    self.sel_face = None;
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &Context) {
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
        let enter = ctx.input(|i| i.modifiers.is_none() && i.key_pressed(Key::Enter));
        if typing {
            // Enter in one of a dialog's own boxes confirms the dialog.
            if enter && self.dialog.has_preview() && self.value_edit.is_none() && !self.show_params && !self.ai.open && self.rename.is_none() {
                self.update_preview();
                self.apply_dialog();
            }
            return;
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
            if self.dialog.has_preview() {
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
            for (k, a) in [(Key::L, Action::Tool(Tool::Line)), (Key::R, Action::Tool(Tool::Rect)), (Key::C, Action::Tool(Tool::Circle)), (Key::A, Action::Tool(Tool::Arc)), (Key::P, Action::Tool(Tool::Point)), (Key::D, Action::Tool(Tool::Dimension)), (Key::S, Action::Tool(Tool::Select)), (Key::X, Action::Construction), (Key::T, Action::Tool(Tool::Trim)), (Key::O, Action::Offset)] {
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

fn setup_style(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    // Light whatever the system says: the viewport and sketch colours are drawn for it.
    ctx.set_visuals_of(egui::Theme::Dark, egui::Visuals::light());
    ctx.set_visuals_of(egui::Theme::Light, egui::Visuals::light());
    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(|s| {
        s.visuals.panel_fill = BG_PANEL;
        s.visuals.window_fill = Color32::from_rgb(250, 250, 251);
        s.visuals.selection.bg_fill = Color32::from_rgb(190, 220, 252);
        s.visuals.selection.stroke.color = Color32::from_rgb(20, 60, 120);
        s.spacing.item_spacing = egui::vec2(6.0, 5.0);
    });
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
        self.now = ctx.input(|i| i.time);

        // Commands from the MCP server and the assistant run here, on the UI thread.
        if let Some(bridge) = self.bridge.take() {
            bridge.serve(|cmd| self.execute(cmd));
            self.bridge = Some(bridge);
        }
        let mut ai = std::mem::take(&mut self.ai);
        ai.poll(self);
        self.ai = ai;

        self.shortcuts(&ctx);
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            if let Some(p) = Some(f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()) {
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("stl")) || self.confirm_discard() {
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
        self.update_preview();
        if let Some(r) = &mut self.recovery
            && let Some(wait) = r.tick(&self.session, self.now)
        {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }

        let bar = egui::Frame::new().fill(BG_BAR).inner_margin(egui::Margin::symmetric(10, 4));
        egui::Panel::top("menu").frame(bar).show(ui, |ui| panels::menu_bar(self, ui));
        let bar = egui::Frame::new().fill(BG_PANEL).inner_margin(egui::Margin::symmetric(10, 6));
        egui::Panel::top("toolbar").frame(bar).show(ui, |ui| panels::toolbar(self, ui));
        let bar = egui::Frame::new().fill(BG_BAR).inner_margin(egui::Margin::symmetric(10, 3));
        egui::Panel::bottom("status").frame(bar).show(ui, |ui| {
            panels::status(self, ui);
            if let Some(error) = self.recovery.as_ref().and_then(Recovery::error) {
                ui.colored_label(Color32::from_rgb(196, 40, 40), "Recovery unavailable — save your work. Retrying…").on_hover_text(error);
            }
        });
        let bar = egui::Frame::new().fill(BG_PANEL).inner_margin(egui::Margin::symmetric(10, 6));
        egui::Panel::bottom("timeline").frame(bar).show(ui, |ui| panels::timeline(self, ui));
        let side = egui::Frame::new().fill(BG_PANEL).inner_margin(egui::Margin::symmetric(8, 8));
        egui::Panel::left("browser").frame(side).exact_size(230.0).resizable(false).show(ui, |ui| panels::browser(self, ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG_VIEW)).show(ui, |ui| view::viewport(self, ui));
        panels::windows(self, &ctx);

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
