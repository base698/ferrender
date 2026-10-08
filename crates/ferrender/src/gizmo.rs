//! Preview-only position and rotation handles shared by Move and primitives.
use egui::{Align2, Color32, CursorIcon, FontId, Painter, PointerButton, Pos2, Rect, Shape, Stroke, Ui, Vec2};
use fr_core::Kind;
use glam::{DAffine3, DQuat, DVec3, EulerRot};
use crate::app::{App, Dialog};
use crate::view::{ray, to_screen};

const AXES: [DVec3; 3] = [DVec3::X, DVec3::Y, DVec3::Z];
const NAMES: [&str; 3] = ["X", "Y", "Z"];
const COLORS: [Color32; 3] = [Color32::from_rgb(230, 76, 86), Color32::from_rgb(49, 173, 112), Color32::from_rgb(67, 140, 239)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle { Move(usize), Turn(usize) }

#[derive(Clone)]
struct Frame {
    parent: DAffine3,
    pivot: DVec3,
    position: DVec3,
    rotation: DQuat,
    /// A body Transform rotates around the owner origin. Compensation makes a
    /// ring gesture keep its visible center still without changing that schema.
    body_center: Option<DVec3>,
}
impl Frame {
    fn world_pivot(&self) -> DVec3 { self.parent.transform_point3(self.pivot) }
    fn axis(&self, axis: usize) -> DVec3 { self.parent.transform_vector3(AXES[axis]).normalize() }
}

fn values(app: &App, fields: &[String; 3], kind: Kind) -> Option<DVec3> {
    let read = |text: &str| {
        let text = text.split_once('=').map_or(text,|(_,right)|right);
        app.doc().eval(if text.trim().is_empty() { "0" } else { text }, kind).ok()
    };
    Some(DVec3::new(read(&fields[0])?, read(&fields[1])?, read(&fields[2])?))
}
fn rotation(degrees: DVec3) -> DQuat {
    let r = DVec3::from_array(degrees.to_array().map(|v| (v % 360.0).to_radians()));
    DQuat::from_rotation_z(r.z) * DQuat::from_rotation_y(r.y) * DQuat::from_rotation_x(r.x)
}
fn frame(app: &App) -> Option<Frame> {
    match &app.dialog {
        Dialog::Transform(d) => {
            let body = app.session.built.body(d.body)?;
            let bounds = body.local_frame()?;
            let position = values(app, &d.translate, Kind::Length)?;
            let rotation = rotation(values(app, &d.rotate, Kind::Angle)?);
            let scale = app.doc().eval(if d.scale.trim().is_empty() { "1" } else { &d.scale }, Kind::Scalar).ok()?;
            let center = (bounds[0] + bounds[1]) * (0.5 * scale);
            Some(Frame { parent: body.placement, pivot: rotation * center + position, position, rotation, body_center: Some(center) })
        }
        Dialog::MoveComponent(d) => {
            let owner = app.doc().feature(d.component)?.owner;
            let position = values(app, &d.translate, Kind::Length)?;
            Some(Frame { parent: app.session.built.component_placement(owner), pivot: position, position,
                rotation: rotation(values(app, &d.rotate, Kind::Angle)?), body_center: None })
        }
        Dialog::Primitive(d) => {
            if d.placing || d.pick_surface { return None; }
            let owner = d.editing.and_then(|id| app.doc().feature(id).map(|f| f.owner)).unwrap_or(app.doc().active_component);
            let position = values(app, &d.position, Kind::Length)?;
            Some(Frame { parent: app.session.built.component_placement(owner), pivot: position, position,
                rotation: rotation(values(app, &d.rotate, Kind::Angle)?), body_center: None })
        }
        _ => None,
    }
}

struct Arrow { from: Pos2, to: Pos2, per_mm: Vec2, depth: bool }
struct Ring { points: Vec<Pos2>, bar: Option<(Pos2, Pos2)>, label: Option<(Pos2, Pos2)> }
struct Layout { center: Pos2, arrows: [Arrow; 3], rings: [Ring; 3] }
impl Layout {
    fn new(app: &App, frame: &Frame) -> Self {
        let pivot = frame.world_pivot();
        let center = to_screen(app, pivot);
        let arrows: [Arrow;3] = std::array::from_fn(|axis| {
            let projected = to_screen(app, pivot + frame.axis(axis)) - center;
            let depth = projected.length() < app.cam.scale as f32 * 0.15;
            // An axis pointing at the eye has no screen projection. Its clearly
            // labelled depth handle still moves only along that same axis.
            let per_mm = if depth { Vec2::new(-0.65, -0.76).normalized() * app.cam.scale as f32 } else { projected };
            Arrow { from: center + per_mm.normalized() * 14.0, to: center + per_mm.normalized() * 100.0, per_mm, depth }
        });
        let mut rings: [Ring;3] = std::array::from_fn(|axis| {
            let normal = frame.axis(axis);
            if normal.dot(app.cam.basis().0).abs() < 0.18 {
                // An edge-on ring collapses into a line. A labelled rotation
                // bar gives the missing gesture a stable, unambiguous target.
                let y = 100.0 + axis as f32 * 24.0;
                Ring { points: Vec::new(), bar: Some((center + Vec2::new(-38.0, y), center + Vec2::new(38.0, y))), label: None }
            } else {
                let radius = (56.0 + axis as f64 * 7.0) / app.cam.scale;
                let a = frame.axis((axis + 1) % 3) * radius;
                let b = frame.axis((axis + 2) % 3) * radius;
                let points = (0..=72).map(|i| {
                    let angle = i as f64 * std::f64::consts::TAU / 72.0;
                    to_screen(app, pivot + a * angle.cos() + b * angle.sin())
                }).collect();
                Ring { points, bar: None, label: None }
            }
        });
        let mut labels = vec![center];
        labels.extend(arrows.iter().map(|a|a.to+(a.to-a.from).normalized()*18.0));
        for ring in &mut rings {
            if let Some((a,b))=ring.bar { labels.push(a+(b-a)*0.5); continue; }
            // Keep labels clear of one another and the translation arrow tips.
            ring.label=ring.points.iter().copied().map(|point| {
                let label=point+(point-center).normalized()*18.0;
                let room=labels.iter().map(|other|label.distance(*other)).fold(f32::INFINITY,f32::min);
                (room,point,label)
            }).max_by(|a,b|a.0.total_cmp(&b.0)).map(|(_,point,label)|(point,label));
            if let Some((_,label))=ring.label {labels.push(label);}
        }
        Self { center, arrows, rings }
    }
    fn hit(&self, pos: Pos2) -> Option<Handle> {
        // Arrow tips and their outward stems win intersections with rings.
        for (axis, arrow) in self.arrows.iter().enumerate() {
            if pos.distance(arrow.to) <= 13.0 || distance(pos, arrow.from, arrow.to) <= 6.0 { return Some(Handle::Move(axis)); }
        }
        self.rings.iter().enumerate().filter_map(|(axis, ring)| {
            if ring.label.is_some_and(|(_,label)|Rect::from_center_size(label,Vec2::new(30.0,20.0)).contains(pos)) {return Some((0.0,Handle::Turn(axis)));}
            let d = if let Some((a,b)) = ring.bar { distance(pos,a,b) }
                else { ring.points.windows(2).map(|p| distance(pos,p[0],p[1])).fold(f32::INFINITY, f32::min) };
            (d <= 7.0).then_some((d, Handle::Turn(axis)))
        }).min_by(|a,b| a.0.total_cmp(&b.0)).map(|(_, handle)| handle)
    }
}
fn distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let v = b-a;
    (p - (a + v * ((p-a).dot(v) / v.length_sq().max(1e-12)).clamp(0.0,1.0))).length()
}
fn angle_at(app: &App, frame: &Frame, axis: usize, pos: Pos2) -> Option<f64> {
    let (origin, direction) = ray(app, pos);
    let normal = frame.axis(axis);
    let denominator = direction.dot(normal);
    if denominator.abs() < 0.15 { return None; }
    let pivot = frame.world_pivot();
    let at = origin + direction * ((pivot-origin).dot(normal) / denominator) - pivot;
    if at.length_squared() < 1e-12 { return None; }
    Some(at.dot(frame.axis((axis+2)%3)).atan2(at.dot(frame.axis((axis+1)%3))))
}

#[derive(Clone)]
struct Gesture {
    revision: u64,
    original: Dialog,
    expected: Dialog,
    frame: Frame,
    handle: Handle,
    start: Pos2,
    started: bool,
    previous: Pos2,
    per_mm: Vec2,
    previous_angle: Option<f64>,
    amount: f64,
    bar: bool,
}

#[derive(Default)]
pub struct State { gesture: Option<Gesture>, blocked: bool }
impl State {
    pub fn clear(&mut self) { self.blocked |= self.is_active(); self.gesture = None; }
    pub fn is_active(&self) -> bool { self.gesture.is_some() }
}

fn format_vector(value: DVec3, unit: &str) -> [String; 3] { value.to_array().map(|v| format!("{:.8} {unit}", if v.abs() < 1e-10 { 0.0 } else { v })) }
fn apply_gesture(app: &mut App, drag: &mut Gesture, position: Pos2, shift: bool) {
    // A click selects the handle without replacing parameter expressions. Keep
    // the first sample until motion is deliberate, then include that motion.
    if !drag.started {
        if position.distance(drag.start) <= 3.0 { return; }
        drag.started = true;
    }
    let mut dialog = drag.original.clone();
    let mut translate = drag.frame.position;
    let mut turn = None;
    let motion;
    match drag.handle {
        Handle::Move(axis) => {
            drag.amount += (position-drag.previous).dot(drag.per_mm) as f64 / drag.per_mm.length_sq().max(1e-12) as f64 * if shift { 0.1 } else { 1.0 };
            translate[axis] += drag.amount;
            motion = drag.amount;
        }
        Handle::Turn(axis) => {
            if drag.bar { drag.amount += (position.x-drag.previous.x) as f64 * std::f64::consts::PI / 180.0; }
            else if let Some(angle) = angle_at(app,&drag.frame,axis,position) {
                if let Some(previous) = drag.previous_angle {
                    let delta = (angle-previous+std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)-std::f64::consts::PI;
                    drag.amount += delta;
                }
                drag.previous_angle = Some(angle);
            }
            let angle = if shift { (drag.amount.to_degrees()/15.0).round()*15.0_f64.to_radians() } else { drag.amount };
            motion = angle;
            let rotation = DQuat::from_axis_angle(AXES[axis], angle) * drag.frame.rotation;
            let (z,y,x) = rotation.to_euler(EulerRot::ZYX);
            turn = Some(format_vector(DVec3::new(x.to_degrees(),y.to_degrees(),z.to_degrees()), "deg"));
            if let Some(center) = drag.frame.body_center { translate = drag.frame.pivot - rotation * center; }
        }
    }
    drag.previous = position;
    if motion.abs() <= 1e-10 {
        // Returning to zero (including Shift snapping to zero) restores the
        // exact expressions, even if Euler decomposition has another form.
        drag.expected = drag.original.clone();
        app.dialog = drag.original.clone();
        return;
    }
    if !translate.is_finite() || translate.abs().max_element() > 1e9 { return; }
    let fields = format_vector(translate,"mm");
    let write_position = |target: &mut [String;3]| {
        for axis in 0..3 {
            if (translate[axis]-drag.frame.position[axis]).abs() > 1e-10 { target[axis] = fields[axis].clone(); }
        }
    };
    let write_rotation = |target: &mut [String;3]| {
        if let Some(turn) = &turn {
            for axis in 0..3 {
                let old=app.doc().eval(&target[axis],Kind::Angle).ok();
                let new=app.doc().eval(&turn[axis],Kind::Angle).unwrap_or(0.0);
                if old.is_none_or(|old| ((old-new+180.0).rem_euclid(360.0)-180.0).abs()>1e-8) { target[axis]=turn[axis].clone(); }
            }
        }
    };
    match &mut dialog {
        Dialog::Transform(d) => { write_position(&mut d.translate); write_rotation(&mut d.rotate); }
        Dialog::MoveComponent(d) => { write_position(&mut d.translate); write_rotation(&mut d.rotate); }
        Dialog::Primitive(d) => { write_position(&mut d.position); write_rotation(&mut d.rotate); }
        _ => return,
    }
    drag.expected = dialog.clone();
    app.dialog = dialog;
}

/// Call before navigation and model picking. True consumes this pointer event;
/// dragging only edits dialog inputs, so normal OK/Cancel owns the transaction.
pub fn interact(app: &mut App, ui: &Ui, resp: &egui::Response) -> bool {
    let mut state = std::mem::take(&mut app.gizmo);
    let (pressed, down, released, shift, escape, pos) = ui.input(|i| (i.pointer.button_pressed(PointerButton::Primary), i.pointer.button_down(PointerButton::Primary), i.pointer.button_released(PointerButton::Primary), i.modifiers.shift, i.key_pressed(egui::Key::Escape), i.pointer.interact_pos()));
    if state.gesture.as_ref().is_some_and(|d| d.revision != app.session.rev || d.expected != app.dialog) {
        state.gesture = None; state.blocked = down;
    }
    let mut consumed = false;
    if state.blocked {
        consumed = true;
        if !down { state.blocked = false; }
    } else if let Some(mut drag) = state.gesture.take() {
        consumed = true;
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        if escape {
            app.dialog = drag.original;
            ui.ctx().request_repaint();
            state.blocked = down;
        } else {
            if let Some(position) = pos {
                let before = app.dialog.clone();
                apply_gesture(app,&mut drag,position,shift);
                if app.dialog != before { ui.ctx().request_repaint(); }
            }
            if down && !released { state.gesture = Some(drag); }
        }
    } else if let (Some(frame), Some(pos)) = (frame(app), pos) {
        let layout = Layout::new(app,&frame);
        if resp.contains_pointer() && let Some(handle) = layout.hit(pos) {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
            if pressed {
                let axis = match handle { Handle::Move(i) | Handle::Turn(i) => i };
                state.gesture = Some(Gesture { revision: app.session.rev, original: app.dialog.clone(), expected: app.dialog.clone(), frame: frame.clone(), handle,
                    start: pos, started: false, previous: pos, per_mm: layout.arrows[axis].per_mm, previous_angle: angle_at(app,&frame,axis,pos), amount: 0.0, bar: layout.rings[axis].bar.is_some() });
                consumed = true;
            }
        }
    }
    app.gizmo = state;
    consumed
}

pub fn draw(app: &App, painter: &Painter, hover: Option<Pos2>) {
    let Some(frame) = frame(app) else { return; };
    let layout = Layout::new(app,&frame);
    let active = app.gizmo.gesture.as_ref().map(|d| d.handle);
    let hot = active.or_else(|| hover.and_then(|p| layout.hit(p)));
    let palette = crate::theme::Palette::from_ctx(painter.ctx());
    let label = |pos: Pos2, text: &str, color: Color32| {
        let galley = painter.layout_no_wrap(text.into(),FontId::proportional(12.0),color);
        let rect = Rect::from_center_size(pos,galley.size()+Vec2::new(8.0,4.0));
        painter.rect_filled(rect,4.0,palette.panel.gamma_multiply(0.94));
        painter.galley(rect.min+Vec2::new(4.0,2.0),galley,color);
    };
    for (axis,ring) in layout.rings.iter().enumerate() {
        let color = COLORS[axis];
        let stroke = Stroke::new(if hot == Some(Handle::Turn(axis)) { 3.5 } else { 2.0 },color.gamma_multiply(0.85));
        if let Some((a,b)) = ring.bar {
            painter.line_segment([a,b],stroke);
            painter.circle_filled(a,3.0,color); painter.circle_filled(b,3.0,color);
            label(a+(b-a)*0.5,&format!("↻ {}",NAMES[axis]),color);
        } else {
            painter.add(Shape::line(ring.points.clone(),stroke));
            if let Some((point,at))=ring.label {
                painter.line_segment([point,at],Stroke::new(1.0,color));
                label(at,&format!("↻ {}",NAMES[axis]),color);
            }
        }
    }
    for (axis,arrow) in layout.arrows.iter().enumerate() {
        let color = COLORS[axis];
        let direction = (arrow.to-arrow.from).normalized();
        painter.line_segment([arrow.from,arrow.to-direction*10.0],Stroke::new(if hot == Some(Handle::Move(axis)) { 4.5 } else { 3.0 },color));
        painter.add(Shape::convex_polygon(vec![arrow.to+direction*3.0,arrow.to-direction*12.0+direction.rot90()*6.0,arrow.to-direction*12.0-direction.rot90()*6.0],color,Stroke::NONE));
        label(arrow.to+direction*18.0,&format!("{}{}",NAMES[axis],if arrow.depth { " depth" } else { "" }),color);
    }
    painter.circle(layout.center,4.0,palette.panel,Stroke::new(1.5,palette.ink));
    if let Some(drag) = &app.gizmo.gesture {
        let text = match drag.handle { Handle::Move(i) => format!("{}  {:+.2} mm",NAMES[i],drag.amount), Handle::Turn(i) => format!("{}  {:+.1}°",NAMES[i],drag.amount.to_degrees()) };
        painter.text(layout.center+Vec2::new(0.0,-145.0),Align2::CENTER_BOTTOM,text,FontId::proportional(13.0),palette.ink);
    }
}

#[cfg(test)]
pub(crate) fn test_handle(app: &App, axis: usize, turn: bool) -> Option<Pos2> {
    let layout = Layout::new(app,&frame(app)?);
    if !turn { return Some(layout.arrows[axis].to); }
    let ring = &layout.rings[axis];
    if let Some((a,b)) = ring.bar { return Some(a+(b-a)*0.75); }
    ring.points.iter().copied().find(|point| layout.hit(*point) == Some(Handle::Turn(axis)))
}
