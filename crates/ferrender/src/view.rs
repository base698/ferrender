//! The viewport: navigation, drawing of bodies and sketches, and the
//! sketch and modelling tools that work by clicking in it.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Align2, Color32, FontId, Painter, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextureHandle, Ui, Vec2, pos2, vec2};
use fr_core::profile::{Profile, profiles};
use fr_core::render::{self, Camera};
use fr_core::sketch::{Constraint, Ref};
use fr_core::{Axis, FeatureKind, Body, CKind, Document, Geom, Id, Kind, ORIGIN, Plane, Sketch, solver};
use glam::{DVec2, DVec3};

use crate::app::{App, Dialog, Drag, EditTarget, Face, Mode, Picked, Snap, Tool, Typed};
use fr_core::measure::Item;
use crate::{gpu, theme};

/// Recomputed from the active theme each frame, including when the OS changes it.
/// Neutral body materials stay the same; both renderers composite them over this view.
struct ViewColors {
    ink: Color32,
    sketch: Color32,
    sketch_dim: Color32,
    construction: Color32,
    selected: Color32,
    dim_ink: Color32,
    accent: Color32,
    paper: Color32,
    fixed: Color32,
    gap: Color32,
    error: Color32,
}

impl ViewColors {
    fn new(ctx: &egui::Context) -> Self {
        let palette = theme::Palette::from_ctx(ctx);
        let choose = |light, dark| theme::choose(ctx, light, dark);
        Self {
            ink: palette.ink,
            sketch: palette.accent,
            sketch_dim: choose(Color32::from_rgb(96, 146, 214), Color32::from_rgb(115, 160, 218)),
            construction: choose(Color32::from_rgb(204, 128, 36), Color32::from_rgb(238, 175, 83)),
            selected: choose(Color32::from_rgb(0, 168, 255), Color32::from_rgb(74, 205, 255)),
            dim_ink: choose(Color32::from_rgb(70, 74, 84), Color32::from_rgb(203, 211, 224)),
            accent: palette.accent,
            paper: palette.panel,
            fixed: choose(Color32::from_rgb(150, 60, 200), Color32::from_rgb(205, 148, 250)),
            gap: choose(Color32::from_rgb(225, 119, 15), Color32::from_rgb(255, 176, 72)),
            error: palette.error,
        }
    }
}

/// The bodies as last sent to the renderer.
#[derive(Default)]
pub struct Scene {
    key: Option<(u64, Option<Dialog>, Vec<Id>, Option<(Id, usize, usize)>)>,
    rev: u64,
    verts: Arc<Vec<f32>>,
    /// The large mesh bodies, uploaded indexed and never re-sent for a selection change.
    big_key: Option<(u64, Option<Dialog>, Vec<Id>)>,
    big: Arc<Vec<gpu::BigMesh>>,
    /// When the camera last changed, so large meshes draw coarse while the view moves.
    moved: Option<std::time::Instant>,
    last_cam: Option<Camera>,
    bounds: Option<(DVec3, DVec3)>,
    /// Software-rendered fallback, with what it was rendered for.
    /// Its background is transparent, so appearance changes do not invalidate it.
    cpu: Option<((u64, Camera, [usize; 2], Option<Id>), TextureHandle)>,
}

impl Scene {
    /// How many bodies are drawn through the indexed large-mesh path.
    #[cfg(test)]
    pub fn big_count(&self) -> usize { self.big.len() }

    /// A new document can reuse session revisions; keep GPU revisions monotonic.
    pub fn invalidate(&mut self) {
        self.key = None;
        self.big_key = None;
        self.big = Arc::default();
        self.verts = Arc::default();
        self.bounds = None;
        self.last_cam = None;
        self.moved = None;
        self.cpu = None;
    }

    #[cfg(test)]
    pub(crate) fn big_data(&self) -> Arc<Vec<gpu::BigMesh>> { self.big.clone() }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    None,
    Point(Id),
    Entity(Id),
    /// A dimension label or constraint badge.
    Label(Id),
}

pub fn to_screen(app: &App, p: DVec3) -> Pos2 {
    let s = app.cam.project(p).0;
    app.vp.center() + vec2(s.x as f32, s.y as f32)
}

pub(crate) fn ray(app: &App, pos: Pos2) -> (DVec3, DVec3) {
    let d = pos - app.vp.center();
    app.cam.ray(DVec2::new(d.x as f64, d.y as f64))
}

/// The point of a sketch's plane under a screen position.
pub fn sketch_pos(app: &App, sk: &Sketch, pos: Pos2) -> Option<DVec2> {
    let (o, d) = ray(app, pos);
    sk.plane.ray_hit(o, d)
}

fn on_screen(app: &App, sk: &Sketch, p: DVec2) -> Pos2 {
    to_screen(app, sk.plane.to_world(p))
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let d = b - a;
    let l = d.length_sq();
    let t = if l < 1e-9 { 0.0 } else { ((p - a).dot(d) / l).clamp(0.0, 1.0) };
    (a + d * t).distance(p)
}

fn path(app: &App, sk: &Sketch, id: Id) -> Vec<Pos2> {
    sk.polyline(id).into_iter().map(|p| on_screen(app, sk, p)).collect()
}

fn path_dist(pts: &[Pos2], p: Pos2) -> f32 {
    pts.windows(2).map(|w| seg_dist(p, w[0], w[1])).fold(f32::MAX, f32::min)
}

pub fn hit(app: &App, sk: &Sketch, pos: Pos2) -> Hit {
    if let Some((_, id)) = app.labels.iter().rev().find(|(r, _)| r.contains(pos)) {
        return Hit::Label(*id);
    }
    let near = |a: &(Id, f32), b: &(Id, f32)| a.1.total_cmp(&b.1);
    if let Some((id, d)) = sk.points.iter().map(|(id, p)| (*id, on_screen(app, sk, *p).distance(pos))).min_by(near)
        && d <= 8.0
    {
        return Hit::Point(id);
    }
    match sk.entities.keys().map(|id| (*id, path_dist(&path(app, sk, *id), pos))).min_by(near) {
        Some((id, d)) if d <= 6.0 => Hit::Entity(id),
        _ => Hit::None,
    }
}

/// Grid spacing in millimetres: a power of ten of the document unit that stays readable.
fn grid_step(app: &App) -> f64 {
    let mut step = app.doc().units.mm() * 1e-3;
    while step * app.cam.scale < 9.0 {
        step *= 10.0;
    }
    step
}

/// Where a drawing-tool click at `pos` would land. `from` is the previous
/// point of a line, for horizontal and vertical inference.
pub fn snap(app: &App, sk: &Sketch, pos: Pos2, from: Option<DVec2>) -> Option<Snap> {
    let raw = sketch_pos(app, sk, pos)?;
    if let Some((id,p)) = crate::sketch_capture::nearest_point(sk,pos,|p|on_screen(app,sk,p)) {
        return Some(Snap { p,point:Some(id),on:None,h:false,v:false,axis:[false,false], mid:false });
    }
    if let Some(c) = sk.entities.keys().filter_map(|id| crate::sketch_capture::project_entity(sk,*id,pos,|p|on_screen(app,sk,p)))
        .filter(|c|c.distance<=6.0).min_by(|a,b|a.distance.total_cmp(&b.distance)) {
        return Some(Snap { p:c.point,point:None,on:Some(c.entity),h:false,v:false,axis:[false,false], mid:c.mid });
    }
    Some(free_snap(app,raw,from,false))
}

fn free_snap(app: &App, raw: DVec2, from: Option<DVec2>, bypass: bool) -> Snap {
    let mut s = Snap { p: raw, point: None, on: None, h: false, v: false, axis: [false, false], mid: false };
    if bypass { return s; }
    if app.opts.snap_grid {
        let step = grid_step(app);
        s.p = (raw / step).round() * step;
    }
    if let Some(f) = from {
        let d = (s.p - f).abs() * app.cam.scale;
        if d.y < 6.0 && d.x > 8.0 {
            s.p.y = f.y;
            s.h = true;
        } else if d.x < 6.0 && d.y > 8.0 {
            s.p.x = f.x;
            s.v = true;
        }
    }
    // The sketch axes snap like drawn lines: a point within a few pixels of one lands
    // on it, so a revolve profile closes on its axis instead of just past it.
    let near = |v: f64| v.abs() * app.cam.scale < 6.0;
    if !s.h && near(s.p.y) {
        s.p.y = 0.0;
        s.axis[0] = true;
    }
    if !s.v && near(s.p.x) {
        s.p.x = 0.0;
        s.axis[1] = true;
    }
    s
}

fn captured_snap(app: &mut App, ui: &Ui, sk: &Sketch, pos: Pos2, from: Option<DVec2>) -> Option<Snap> {
    let raw = sketch_pos(app,sk,pos)?;
    let Mode::Sketch(sid) = app.mode else { return snap(app,sk,pos,from); };
    let (time,bypass) = ui.input(|i|(i.time,i.modifiers.alt || i.key_down(egui::Key::Space) || !i.focused));
    let endpoint = crate::sketch_capture::nearest_point(sk,pos,|p|on_screen(app,sk,p));
    let candidates: Vec<_> = sk.entities.keys().filter_map(|id| crate::sketch_capture::project_entity(sk,*id,pos,|p|on_screen(app,sk,p))).collect();
    let context = crate::sketch_capture::Context { sketch:sid,tool:app.tool,revision:app.session.rev,camera:app.cam,plane:sk.plane };
    app.sketch_capture.update(context,time,endpoint,&candidates,bypass).or_else(||Some(free_snap(app,raw,from,bypass)))
}

/// Adds a screen-space drag to the Move dialog's distances.
fn slide_body(app: &mut App, delta: DVec2) {
    let Dialog::Transform(mut t) = app.dialog.clone() else { return };
    let (_, right, up) = app.cam.basis();
    let by = (right * delta.x - up * delta.y) / app.cam.scale;
    let by = app.session.built.body(t.body).map_or(by, |body| body.placement.inverse().transform_vector3(by));
    let unit = app.doc().units;
    for (i, text) in t.translate.iter_mut().enumerate() {
        let now = app.doc().eval(text, Kind::Length).unwrap_or(0.0);
        *text = format!("{} {}", fr_core::units::fmt_len(now + by[i], unit), unit.name());
    }
    app.dialog = Dialog::Transform(t);
}

/// The Extrude dialog's arrow: where it starts, which way it points, and how it lies on screen.
struct Arrow {
    /// Current distance in millimetres.
    dist: f64,
    base: Pos2,
    tip: Pos2,
    /// Screen direction of a positive distance.
    dir: Vec2,
    /// Screen pixels per millimetre along the arrow.
    px_per_mm: f64,
}

fn extrude_arrow(app: &App) -> Option<Arrow> {
    if let Some((plane, middle, text)) = crate::construction_view::offset_base(app) { return distance_arrow(app, plane, middle, &text); }
    let Dialog::Feature(f) = &app.dialog else { return None };
    if f.revolve || f.sweep.is_some() || f.through_all {
        return None;
    }
    let doc = app.doc();
    // From the middle of what is being extruded, straight out of its plane.
    let (plane, middle) = match (&f.face, f.sketch.and_then(|s| doc.sketch(s))) {
        (Some(face), _) => {
            let pts: Vec<DVec2> = face.loops.iter().flatten().copied().collect();
            (app.body_plane_world(face.body, face.plane?), pts.iter().sum::<DVec2>() / pts.len().max(1) as f64)
        }
        (None, Some(sk)) => {
            let pts: Vec<DVec2> = profiles(sk).iter().filter(|p| f.profiles.contains(&p.edges)).flat_map(|p| p.outer.clone()).collect();
            if pts.is_empty() {
                return None;
            }
            (app.session.built.sketch_plane(doc, f.sketch?)?, pts.iter().sum::<DVec2>() / pts.len() as f64)
        }
        _ => return None,
    };
    distance_arrow(app, plane, middle, &f.text)
}

fn distance_arrow(app: &App, plane: Plane, middle: DVec2, text: &str) -> Option<Arrow> {
    let text = text.split_once('=').map_or(text, |t| t.1);
    let dist = app.doc().eval(text, Kind::Length).unwrap_or(0.0);
    let (from, n) = (plane.to_world(middle), plane.normal());
    let base = to_screen(app, from);
    let along = to_screen(app, from + n) - base;
    let px_per_mm = along.length() as f64;
    // Seen end-on, the arrow has no length on screen to pull.
    if px_per_mm < app.cam.scale * 0.12 {
        return None;
    }
    let dir = along.normalized();
    let reach = (dist.abs() * px_per_mm).max(34.0) as f32 * if dist < 0.0 { -1.0 } else { 1.0 };
    Some(Arrow { dist, base, tip: base + dir * reach, dir, px_per_mm })
}

fn draw_arrow(app: &App, painter: &Painter, a: &Arrow, hot: bool) {
    let colors = ViewColors::new(painter.ctx());
    let color = if hot { colors.selected } else { colors.accent };
    let d = (a.tip - a.base).normalized();
    painter.line_segment([a.base, a.tip - d * 12.0], Stroke::new(if hot { 4.0 } else { 3.0 }, color));
    painter.add(Shape::convex_polygon(vec![a.tip + d * 6.0, a.tip - d * 12.0 + d.rot90() * 8.0, a.tip - d * 12.0 - d.rot90() * 8.0], color, Stroke::new(1.0, colors.paper)));
    painter.circle(a.base, 3.5, colors.paper, Stroke::new(1.5, color));
    let unit = app.doc().units;
    label(painter, a.tip + d * 26.0, &format!("{} {}", fr_core::units::fmt_len(a.dist, unit), unit.name()), color, None);
}

/// Adds a screen-space drag to the Extrude dialog's distance.
fn pull_arrow(app: &mut App, delta: Vec2) {
    let Some(a) = extrude_arrow(app) else { return };
    app.drag_value += delta.dot(a.dir) as f64 / a.px_per_mm;
    // Land on round numbers: the smallest tidy step that is a couple of pixels wide.
    let unit = app.doc().units;
    let step = [0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 50.0, 100.0].into_iter().map(|s| s * unit.mm()).find(|s| s * a.px_per_mm >= 2.0).unwrap_or(100.0 * unit.mm());
    let snapped = (app.drag_value / step).round() * step;
    let text = format!("{} {}", fr_core::units::fmt_len(snapped, unit), unit.name());
    match &mut app.dialog { Dialog::Feature(f) => f.text = text, Dialog::Plane(p) => p.text = text, _ => {} }
}

fn navigate(app: &mut App, ui: &Ui, resp: &egui::Response) {
    let d = |v: Vec2| DVec2::new(v.x as f64, v.y as f64);
    let (shift, alt, space) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt, i.key_down(egui::Key::Space)));
    let delta = d(resp.drag_delta());
    // In a sketch the left button belongs to the tools; in the model it turns the view.
    let free = app.sketch().is_none();
    if free && resp.drag_started_by(PointerButton::Primary) {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
        let grabbed = match &app.dialog {
            Dialog::Transform(t) => pick_body(app, app.shown(), origin).is_some_and(|b| b.0 == t.body),
            _ => false,
        };
        app.drag = match extrude_arrow(app) {
            Some(a) if seg_dist(origin, a.base, a.tip) < 12.0 || origin.distance(a.tip) < 18.0 => {
                app.drag_value = a.dist;
                Drag::Arrow
            }
            _ if grabbed => Drag::Body,
            _ => Drag::None,
        };
    }
    if resp.dragged_by(PointerButton::Primary) && app.drag == Drag::Arrow {
        pull_arrow(app, resp.drag_delta());
    } else if resp.dragged_by(PointerButton::Primary) && app.drag == Drag::Body {
        slide_body(app, delta);
    } else if resp.dragged_by(PointerButton::Primary) && space {
        app.cam.pan(delta);
    } else if resp.dragged_by(PointerButton::Secondary) || (resp.dragged_by(PointerButton::Primary) && (alt || free)) {
        if shift { app.cam.pan(delta) } else { app.cam.orbit(delta) }
    } else if resp.dragged_by(PointerButton::Middle) {
        if shift { app.cam.orbit(delta) } else { app.cam.pan(delta) }
    }
    if free && resp.drag_stopped_by(PointerButton::Primary) {
        app.drag = Drag::None;
    }
    let Some(pos) = resp.hover_pos() else { return };
    let at = d(pos - app.vp.center());
    let pinch = ui.input(|i| i.zoom_delta()) as f64;
    if pinch != 1.0 {
        app.cam.zoom(pinch, at);
    }
    // A wheel zooms; a trackpad's two-finger scroll pans, or orbits with Shift.
    for e in ui.input(|i| i.events.clone()) {
        if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e {
            if modifiers.command || modifiers.ctrl {
                continue;
            }
            match unit {
                egui::MouseWheelUnit::Point if modifiers.shift => app.cam.orbit(d(delta)),
                egui::MouseWheelUnit::Point => app.cam.pan(d(delta)),
                _ => app.cam.zoom((delta.y as f64 * 0.18).exp(), at),
            }
        }
    }
}

/// The plane the grid lies on and whose axes are drawn.
fn work_plane(app: &App) -> Plane {
    app.sketch().and_then(|(id, _)| app.session.built.sketch_plane(app.doc(), id)).unwrap_or_else(|| Plane::XY.transformed(app.shown().component_placement(app.doc().active_component)))
}

fn draw_grid(app: &App, painter: &Painter) {
    let plane = work_plane(app);
    let rect = app.vp;
    let corners: Option<Vec<DVec2>> = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()].iter().map(|c| plane.ray_hit(ray(app, *c).0, ray(app, *c).1)).collect();
    let Some(corners) = corners else { return };
    let lo = corners.iter().fold(DVec2::splat(f64::MAX), |a, b| a.min(*b));
    let hi = corners.iter().fold(DVec2::splat(f64::MIN), |a, b| a.max(*b));
    let px = |p: DVec2| to_screen(app, plane.to_world(p));
    if app.opts.grid && (hi - lo).max_element() * app.cam.scale < 30_000.0 {
        let step = grid_step(app);
        for major in [false, true] {
            let stroke = Stroke::new(1.0, if major { theme::choose(painter.ctx(), Color32::from_rgb(214, 218, 225), Color32::from_rgb(67, 75, 88)) } else { theme::choose(painter.ctx(), Color32::from_rgb(233, 235, 240), Color32::from_rgb(43, 49, 60)) });
            let s = if major { step * 10.0 } else { step };
            let mut x = (lo.x / s).floor() * s;
            while x <= hi.x {
                painter.line_segment([px(DVec2::new(x, lo.y)), px(DVec2::new(x, hi.y))], stroke);
                x += s;
            }
            let mut y = (lo.y / s).floor() * s;
            while y <= hi.y {
                painter.line_segment([px(DVec2::new(lo.x, y)), px(DVec2::new(hi.x, y))], stroke);
                y += s;
            }
        }
    }
    painter.line_segment([px(DVec2::new(lo.x, 0.0)), px(DVec2::new(hi.x, 0.0))], Stroke::new(1.2, Color32::from_rgb(226, 96, 96)));
    painter.line_segment([px(DVec2::new(0.0, lo.y)), px(DVec2::new(0.0, hi.y))], Stroke::new(1.2, Color32::from_rgb(92, 184, 100)));
    if app.sketch().is_none() {
        let reach = rect.size().max_elem() as f64 / app.cam.scale;
        painter.line_segment([to_screen(app, DVec3::ZERO), to_screen(app, DVec3::Z * reach)], Stroke::new(1.2, theme::choose(painter.ctx(), Color32::from_rgb(86, 124, 226), Color32::from_rgb(124, 157, 255))));
    }
}

/// The section view's plane as (normal, offset): everything with `normal . p > offset` is hidden.
fn section_plane(app: &App) -> Option<(DVec3, f64)> {
    let s = app.section;
    if !s.on { return None; }
    let flip = if s.flip { -1.0 } else { 1.0 };
    // A construction plane cuts along its own normal, offset from where it sits; a
    // plane that is not built (suppressed, rolled back, in error) falls back to the axis.
    if let Some(p) = s.plane.and_then(|id| app.shown().planes.get(&id)) {
        let n = p.plane.normal() * flip;
        return Some((n, n.dot(p.plane.origin) + s.offset * flip));
    }
    // Unflipped, each plane hides the side the home view looks at.
    let toward = [1.0, -1.0, 1.0][s.axis.min(2)] * flip;
    let n = [DVec3::X, DVec3::Y, DVec3::Z][s.axis.min(2)] * toward;
    Some((n, s.offset * toward))
}

fn draw_bodies(app: &mut App, ui: &Ui, painter: &Painter) {
    let rect = app.vp;
    let hidden = app.doc().hidden_bodies.clone();
    // The selected face is lit through the vertex data, so it is part of what was sent.
    let face = app.sel_face.clone().filter(|_| app.preview.is_none());
    let key = (app.session.rev, app.preview.as_ref().map(|p| p.0.clone()), hidden.clone(), face.as_ref().and_then(|f| f.tris.first().map(|first| (f.body, f.tris.len(), *first))));
    if app.scene.key.as_ref() != Some(&key) {
        let bodies: Vec<&Body> = app.shown().bodies.iter().filter(|b| !hidden.contains(&b.id) && app.body_visible(b)).collect();
        let verts = Arc::new(gpu::vertices(bodies.iter().copied(), face.as_ref().map(|f| (f.body, f.tris.as_slice()))));
        let bounds = bodies.iter().filter_map(|b| b.mesh.bbox()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        let big_key = (key.0, key.1.clone(), key.2.clone());
        if app.scene.big_key.as_ref() != Some(&big_key) {
            app.scene.big = Arc::new(bodies.iter().filter(|b| b.mesh.len() > gpu::BIG).map(|b| gpu::BigMesh::new(b)).collect());
            app.scene.big_key = Some(big_key);
        }
        app.scene.verts = verts;
        app.scene.bounds = bounds;
        app.scene.rev += 1;
        app.scene.key = Some(key);
    }
    if app.scene.verts.is_empty() && app.scene.big.is_empty() {
        // There is no model left to catch up with. Preserve the immediate empty
        // scene acknowledgement even though the GPU callback below releases its
        // old buffers. Nonempty scenes still require the actual GPU fence.
        app.timeline.software_done();
        if !app.gpu { return; }
    }
    // Submit an empty GPU frame too: it clears the last image and releases its
    // buffers after Hide All/New, rather than retaining a large deleted model.
    // While the camera is moving, meshes above the level-of-detail size draw their coarse copy;
    // a repaint shortly after it stops brings the full mesh back.
    let mut coarse = false;
    if app.scene.big.iter().any(|b| b.coarse.is_some()) {
        let now = std::time::Instant::now();
        if app.scene.last_cam != Some(app.cam) {
            app.scene.last_cam = Some(app.cam);
            app.scene.moved = Some(now);
        }
        if let Some(t) = app.scene.moved && now.duration_since(t) < std::time::Duration::from_millis(150) {
            coarse = true;
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(160));
        }
    }
    let ppp = ui.ctx().pixels_per_point();
    let size = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
    if app.gpu {
        // How far the scene reaches from the view's centre, for the depth range. Any of the
        // eight corners can be the farthest, not only the two that define the box.
        let reach = app.scene.bounds.map_or(100.0, |(lo, hi)| (0..8).map(|i| DVec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z }).distance(app.cam.target)).fold(0.0, f64::max)) * 1.05 + 1.0;
        painter.add(eframe::egui_wgpu::Callback::new_paint_callback(
            rect,
            gpu::Frame { frozen: app.timeline.preview.is_some(), painted: app.timeline.paint_signal(), rev: app.scene.rev, verts: app.scene.verts.clone(), big: app.scene.big.clone(), coarse, cam: app.cam, origin: [(rect.min.x * ppp).round(), (rect.min.y * ppp).round()], size, selected: app.sel_body, pixels_per_point: ppp, reach, section: section_plane(app) },
        ));
        return;
    }
    let key = (app.scene.rev, app.cam, [size[0] as usize, size[1] as usize], app.sel_body);
    if app.scene.cpu.as_ref().is_none_or(|c| c.0 != key) {
        let mut img = render::Image { w: key.2[0], h: key.2[1], rgba: vec![0; key.2[0] * key.2[1] * 4] };
        let cam = Camera { scale: app.cam.scale * ppp as f64, ..app.cam };
        render::draw_bodies(&mut img, app.shown().bodies.iter().filter(|b| !hidden.contains(&b.id) && app.body_visible(b)), &cam, app.sel_body);
        let tex = ui.ctx().load_texture("bodies", egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.rgba), egui::TextureOptions::NEAREST);
        app.scene.cpu = Some((key, tex));
    }
    if let Some((_, tex)) = &app.scene.cpu {
        painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    app.timeline.software_done();
}

/// Paints the small symbol for a constraint inside `r`.
pub fn constraint_icon(p: &Painter, r: Rect, kind: CKind, color: Color32) {
    let s = Stroke::new(1.4, color);
    let c = r.center();
    let u = r.width() * 0.5;
    let at = |x: f32, y: f32| c + vec2(x * u, y * u);
    let line = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment([at(a.0, a.1), at(b.0, b.1)], s);
    };
    match kind {
        CKind::Horizontal => line((-0.65, 0.0), (0.65, 0.0)),
        CKind::Vertical => line((0.0, -0.65), (0.0, 0.65)),
        CKind::Parallel => {
            line((-0.55, 0.6), (-0.05, -0.6));
            line((0.05, 0.6), (0.55, -0.6));
        }
        CKind::Perpendicular => {
            line((-0.6, 0.55), (0.6, 0.55));
            line((0.0, 0.55), (0.0, -0.6));
        }
        CKind::Tangent => {
            p.circle_stroke(at(0.0, 0.2), u * 0.42, s);
            line((-0.65, -0.24), (0.65, -0.24));
        }
        CKind::Equal => {
            line((-0.5, -0.22), (0.5, -0.22));
            line((-0.5, 0.22), (0.5, 0.22));
        }
        CKind::Coincident => {
            p.circle_stroke(c, u * 0.5, s);
            p.circle_filled(c, u * 0.2, color);
        }
        CKind::Midpoint => {
            line((-0.65, 0.3), (0.65, 0.3));
            p.add(Shape::convex_polygon(vec![at(0.0, -0.5), at(0.36, 0.2), at(-0.36, 0.2)], color, Stroke::NONE));
        }
        CKind::Concentric => {
            p.circle_stroke(c, u * 0.6, s);
            p.circle_stroke(c, u * 0.26, s);
        }
        CKind::Collinear => {
            line((-0.7, 0.35), (-0.15, 0.08));
            line((0.15, -0.08), (0.7, -0.35));
        }
        CKind::Symmetric => {
            line((0.0, -0.65), (0.0, 0.65));
            line((-0.65, -0.3), (-0.3, 0.0));
            line((-0.65, 0.3), (-0.3, 0.0));
            line((0.65, -0.3), (0.3, 0.0));
            line((0.65, 0.3), (0.3, 0.0));
        }
        CKind::Fix => {
            p.rect_filled(Rect::from_min_max(at(-0.45, -0.05), at(0.45, 0.6)), 1.5, color);
            p.circle_stroke(at(0.0, -0.1), u * 0.3, s);
        }
        CKind::Distance | CKind::Radius | CKind::Diameter | CKind::Angle | CKind::PositionX | CKind::PositionY => {
            line((-0.65, 0.0), (0.65, 0.0));
            line((-0.65, -0.4), (-0.65, 0.4));
            line((0.65, -0.4), (0.65, 0.4));
        }
    }
}

fn fill(app: &App, painter: &Painter, sk: &Sketch, p: &Profile, color: Color32) {
    let mut mesh = egui::Mesh::default();
    for t in fr_core::mesh::triangulate(p) {
        let i = mesh.vertices.len() as u32;
        for v in t {
            mesh.colored_vertex(on_screen(app, sk, v), color);
        }
        mesh.add_triangle(i, i + 1, i + 2);
    }
    painter.add(Shape::mesh(mesh));
}

fn label(painter: &Painter, at: Pos2, text: &str, color: Color32, outline: Option<Color32>) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(12.5), color);
    let r = Rect::from_center_size(at, galley.size() + vec2(8.0, 4.0));
    painter.rect_filled(r, 3.0, theme::Palette::from_ctx(painter.ctx()).panel);
    if let Some(c) = outline {
        painter.rect_stroke(r, 3.0, Stroke::new(1.2, c), StrokeKind::Outside);
    }
    painter.galley(r.min + vec2(4.0, 2.0), galley, color);
    r
}

/// Draws a dimension and returns its clickable label.
fn draw_dimension(app: &App, painter: &Painter, sk: &Sketch, c: &Constraint, away_from: Pos2, color: Color32, outline: Option<Color32>) -> Option<Rect> {
    let value = c.value.as_ref()?;
    let px = |p: DVec2| on_screen(app, sk, p);
    let thin = Stroke::new(1.0, color);
    let kinds: Vec<Ref> = c.refs.iter().filter_map(|r| sk.ref_kind(*r)).collect();
    let foot = |p: DVec2, l: (DVec2, DVec2)| l.0 + (l.1 - l.0) * ((p - l.0).dot(l.1 - l.0) / (l.1 - l.0).length_squared().max(1e-12));
    match c.kind {
        CKind::PositionX | CKind::PositionY => {
            let at = px(sk.pos(c.refs[0]));
            let (name, offset) = if c.kind == CKind::PositionX { ("X", vec2(38.0, -20.0)) } else { ("Y", vec2(38.0, 1.0)) };
            Some(label(painter, at + offset, &format!("{name}: {}", app.doc().show(value, Kind::Length)), color, outline))
        }
        CKind::Distance => {
            let (a, b) = match kinds.as_slice() {
                [Ref::Line] => sk.line(c.refs[0])?,
                [Ref::Point, Ref::Point] => (sk.pos(c.refs[0]), sk.pos(c.refs[1])),
                [Ref::Point, Ref::Line] => (sk.pos(c.refs[0]), foot(sk.pos(c.refs[0]), sk.line(c.refs[1])?)),
                _ => (sk.line(c.refs[1])?.0, foot(sk.line(c.refs[1])?.0, sk.line(c.refs[0])?)),
            };
            let (sa, sb) = (px(a), px(b));
            let dir = (sb - sa).normalized();
            let mut n = dir.rot90();
            let mid = sa + (sb - sa) / 2.0;
            if (mid + n).distance(away_from) < (mid - n).distance(away_from) {
                n = -n;
            }
            let off = n * 26.0;
            painter.line_segment([sa + n * 4.0, sa + off * 1.2], thin);
            painter.line_segment([sb + n * 4.0, sb + off * 1.2], thin);
            painter.line_segment([sa + off, sb + off], thin);
            for (end, d) in [(sa + off, dir), (sb + off, -dir)] {
                painter.add(Shape::convex_polygon(vec![end, end + d * 8.0 + n * 2.6, end + d * 8.0 - n * 2.6], color, Stroke::NONE));
            }
            Some(label(painter, mid + off, &app.doc().show(value, Kind::Length), color, outline))
        }
        CKind::Radius | CKind::Diameter => {
            let (centre, r) = sk.curve(c.refs[0])?;
            // Along the arc's middle, or up and to the right for a circle.
            let ang = sk.arc_angles(c.refs[0]).map_or(std::f64::consts::FRAC_PI_4, |(a0, sweep)| a0 + sweep / 2.0);
            let rim = px(centre + DVec2::from_angle(ang) * r);
            let out = (rim - px(centre)).normalized();
            let from = if c.kind == CKind::Diameter { px(centre - DVec2::from_angle(ang) * r) } else { px(centre) };
            painter.line_segment([from, rim + out * 16.0], thin);
            painter.add(Shape::convex_polygon(vec![rim, rim - out * 8.0 + out.rot90() * 2.6, rim - out * 8.0 - out.rot90() * 2.6], color, Stroke::NONE));
            let text = format!("{}{}", if c.kind == CKind::Radius { "R" } else { "\u{d8}" }, app.doc().show(value, Kind::Length));
            Some(label(painter, rim + out * 34.0, &text, color, outline))
        }
        CKind::Angle => {
            let at = match c.refs.as_slice() {
                [entity] => {
                    if let Some((a, b)) = sk.line(*entity) {
                        // Leave room for the length dimension on the other side of a line.
                        let mid = px((a + b) / 2.0);
                        let direction = (px(b) - px(a)).normalized();
                        mid + direction.rot90() * 48.0
                    } else {
                        let (center, radius) = sk.curve(*entity)?;
                        let (start, sweep) = sk.arc_angles(*entity)?;
                        px(center + DVec2::from_angle(start + sweep / 2.0) * radius) + vec2(0.0, -22.0)
                    }
                }
                [first, second] => {
                    let (a, b) = (sk.line(*first)?, sk.line(*second)?);
                    px((a.0 + a.1 + b.0 + b.1) / 4.0)
                }
                _ => return None,
            };
            Some(label(painter, at, &app.doc().show(value, Kind::Angle), color, outline))
        }
        _ => None,
    }
}

/// Draws a sketch. For the sketch being edited this also draws points,
/// constraint badges and dimensions, and collects their click targets.
fn draw_sketch(app: &App, painter: &Painter, sk: &Sketch, active: bool, hover: Hit, labels: &mut Vec<(Rect, Id)>) {
    let colors = ViewColors::new(painter.ctx());
    let solved = app.report.ok && app.report.dof == 0;
    let picked = |id: Id| app.sel.contains(&id) || app.dim_refs.contains(&id);
    if active {
        for p in profiles(sk) {
            fill(app, painter, sk, &p, Color32::from_rgba_unmultiplied(0, 120, 255, 22));
        }
    }
    for (id, e) in &sk.entities {
        let pts = path(app, sk, *id);
        let hot = active && (picked(*id) || hover == Hit::Entity(*id));
        let color = match (active, hot, e.construction) {
            (_, true, _) => colors.selected,
            (_, _, true) => colors.construction,
            // Projected from the model, and fixed.
            (true, _, _) if sk.fixed.contains(id) || sk.ent_points(*id).iter().all(|p| sk.fixed.contains(p)) => colors.fixed,
            (true, _, _) if app.report.bad.contains(id) => colors.error,
            (true, _, _) if solved => colors.ink,
            (true, _, _) => colors.sketch,
            _ => colors.sketch_dim,
        };
        let stroke = Stroke::new(if hot { 3.0 } else if active { 1.8 } else { 1.3 }, color);
        if e.construction {
            painter.extend(Shape::dashed_line(&pts, stroke, 7.0, 4.0));
        } else {
            painter.add(Shape::line(pts, stroke));
        }
    }
    if !active {
        return;
    }
    if app.opts.gaps && let Some((_, _, ends)) = &app.gap_cache {
        for id in ends {
            if let Some(p) = sk.points.get(id) {
                painter.circle_stroke(on_screen(app, sk, *p), 8.0, Stroke::new(2.0, colors.gap));
            }
        }
    }
    // Selecting a spline exposes its four interpolation points and their order.
    for (id, e) in &sk.entities {
        if let Geom::Spline { a, b, c, d } = e.geom
            && (picked(*id) || hover == Hit::Entity(*id) || [a,b,c,d].iter().any(|p| picked(*p) || hover == Hit::Point(*p)))
        {
            let pts: Vec<Pos2> = [a,b,c,d].iter().map(|p| on_screen(app, sk, sk.pos(*p))).collect();
            painter.extend(Shape::dashed_line(&pts, Stroke::new(1.0, colors.selected), 4.0, 4.0));
            for (i, at) in pts.iter().enumerate() {
                painter.text(*at + vec2(-10.0, -12.0), Align2::CENTER_CENTER, (i+1).to_string(), FontId::proportional(11.0), colors.sketch);
            }
        }
    }
    // A spline's fit points are its handles: drawn with a filled centre so they read as draggable.
    let fit: std::collections::HashSet<Id> = sk.entities.values().filter_map(|e| if let Geom::Spline { a, b, c, d } = e.geom { Some([a, b, c, d]) } else { None }).flatten().collect();
    for (id, p) in &sk.points {
        let at = on_screen(app, sk, *p);
        let hot = picked(*id) || hover == Hit::Point(*id);
        if *id == ORIGIN {
            painter.circle(at, 5.0, colors.paper, Stroke::new(1.5, colors.ink));
            painter.circle_filled(at, 2.2, if hot { colors.selected } else { colors.ink });
        } else if fit.contains(id) {
            painter.circle(at, if hot { 5.0 } else { 4.0 }, if hot { colors.selected } else { colors.paper }, Stroke::new(1.3, if solved { colors.ink } else { colors.sketch }));
            painter.circle_filled(at, 1.8, if hot { colors.paper } else if solved { colors.ink } else { colors.sketch });
        } else {
            painter.circle(at, if hot { 4.5 } else { 3.2 }, if hot { colors.selected } else { colors.paper }, Stroke::new(1.3, if solved { colors.ink } else { colors.sketch }));
        }
    }

    let centre = sk.bbox().map_or(app.vp.center(), |(lo, hi)| on_screen(app, sk, (lo + hi) / 2.0));
    if app.opts.constraints {
        // One badge per entity a constraint touches, set side by side next to it.
        let mut slots: HashMap<Id, Vec<(Id, CKind)>> = HashMap::new();
        for (cid, c) in &sk.constraints {
            if c.kind.value_kind().is_some() || (c.kind == CKind::Coincident && sk.ref_kind(c.refs[1]) == Some(Ref::Point)) {
                continue;
            }
            let ents: Vec<Id> = c.refs.iter().copied().filter(|r| sk.entities.contains_key(r)).collect();
            for anchor in if ents.is_empty() { vec![c.refs[0]] } else { ents } {
                slots.entry(anchor).or_default().push((*cid, c.kind));
            }
        }
        for (anchor, list) in slots {
            let (mid, dir) = match sk.ref_kind(anchor) {
                Some(Ref::Point) => (on_screen(app, sk, sk.pos(anchor)), vec2(1.0, 0.0)),
                _ => {
                    let pts = path(app, sk, anchor);
                    if pts.is_empty() { continue; }
                    let i = (pts.len() - 1) / 2;
                    let (a, b) = (pts[i], pts[(i + 1).min(pts.len() - 1)]);
                    (if pts.len() == 2 { a + (b - a) / 2.0 } else { a }, (b - a).normalized())
                }
            };
            let mut n = dir.rot90();
            if (mid + n).distance(centre) > (mid - n).distance(centre) {
                // Badges sit on the inside, leaving the outside for dimensions.
                n = -n;
            }
            for (i, (cid, kind)) in list.iter().enumerate() {
                let at = mid + n * 14.0 + dir * ((i as f32 - (list.len() - 1) as f32 / 2.0) * 18.0);
                let r = Rect::from_center_size(at, Vec2::splat(15.0));
                let hot = app.sel.contains(cid) || hover == Hit::Label(*cid);
                painter.rect(r, 3.0, colors.paper, Stroke::new(1.0, if hot { colors.selected } else { theme::Palette::from_ctx(painter.ctx()).disabled }), StrokeKind::Outside);
                constraint_icon(painter, r.shrink(1.5), *kind, if hot { colors.selected } else { colors.dim_ink });
                labels.push((r.expand(1.0), *cid));
            }
        }
    }
    if app.opts.dimensions {
        for (cid, c) in &sk.constraints {
            let hot = app.sel.contains(cid) || hover == Hit::Label(*cid);
            let bad = app.report.bad.contains(cid);
            let color = if bad { colors.error } else if hot { colors.accent } else { colors.dim_ink };
            if let Some(r) = draw_dimension(app, painter, sk, c, centre, color, hot.then_some(colors.selected)) {
                labels.push((r, *cid));
            }
        }
    }
}

fn toggle(list: &mut Vec<Id>, id: Id, add: bool) {
    if add {
        match list.iter().position(|x| *x == id) {
            Some(i) => {
                list.remove(i);
            }
            None => list.push(id),
        }
    } else {
        *list = vec![id];
    }
}

/// Run before painting: preview motion and a release outside the viewport must
/// still be handled by the gesture that started here, not by the hovered panel.
fn update_reference_drag(app: &mut App, ui: &Ui) -> bool {
    let dragging = app.reference_drag.is_dragging();
    let available = app.tool == Tool::Select && app.dialog == Dialog::None
        && app.reference_editor.sketch_id().is_none() && app.value_edit.is_none()
        && !app.show_params && !app.show_about && app.file_error.is_none() && app.rename.is_none();
    let sketch = app.sketch().filter(|(_, sk)| sk.reference.as_ref().is_some_and(|image| image.visible))
        .and_then(|(sid, _)| app.world_sketch(sid).map(|sk| (sid, sk)));
    if !available || sketch.is_none() {
        app.reference_drag.clear();
        return dragging;
    }
    if !dragging { return false; }
    let (focused, released, down, pointer) = ui.input(|i| (i.focused && !i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))), i.pointer.button_released(PointerButton::Primary), i.pointer.primary_down(), i.pointer.latest_pos()));
    if !focused || (!down && !released) {
        app.reference_drag.cancel_drag();
        return true;
    }
    let (sid, sk) = sketch.unwrap();
    if !app.reference_drag.selected(sid) {
        app.reference_drag.clear();
        return true;
    }
    let position = pointer.and_then(|p| sketch_pos(app, &sk, p)).filter(|p| p.is_finite());
    let Some(position) = position else {
        app.reference_drag.cancel_drag();
        app.toast("The pointer cannot be projected onto the sketch plane. Image movement was cancelled.");
        return true;
    };
    let invalid = app.reference_drag.update(position).is_err();
    if released {
        match app.reference_drag.finish() {
            Ok(Some(change)) if change.sketch == sid => {
                app.sketch_edit(|sk, _| { sk.reference = change.image; Ok(()) });
            }
            Err(error) => app.toast(error),
            _ => {}
        }
    } else {
        ui.ctx().set_cursor_icon(if invalid { egui::CursorIcon::NotAllowed } else { egui::CursorIcon::Grabbing });
    }
    ui.ctx().request_repaint();
    true
}

/// Returns true only when the image consumes this interaction. Blank canvas
/// outside the image retains ordinary box selection; sketch hits retain editing.
fn reference_select_tool(app: &mut App, ui: &Ui, resp: &egui::Response, sid: Id, sk: &Sketch, hover: Hit) -> bool {
    if app.dialog != Dialog::None || app.show_params || app.show_about || app.file_error.is_some() || app.rename.is_some() { return false; }
    let Some(image) = sk.reference.as_ref().filter(|i| i.visible) else { return false; };
    if ui.input(|i| i.modifiers.alt || i.key_down(egui::Key::Space)) { return false; }
    if let Some(pos) = resp.hover_pos()
        && let Some(handle) = app.reference_drag.hit(sid, image, sk.plane, app.cam, app.vp, pos, hover == Hit::None) {
        ui.ctx().set_cursor_icon(handle.cursor());
    }
    if resp.drag_started_by(PointerButton::Primary) {
        let Some(origin) = ui.input(|i| i.pointer.press_origin()) else { return false; };
        let handle = app.reference_drag.hit(sid, image, sk.plane, app.cam, app.vp, origin, hit(app, sk, origin) == Hit::None);
        if let Some(handle) = handle {
            let Some(start) = sketch_pos(app, sk, origin).filter(|p| p.is_finite()) else {
                app.toast("The pointer cannot be projected onto the sketch plane.");
                return true;
            };
            match app.reference_drag.begin(sid, image, handle, start) {
                Ok(()) => {
                    app.sel.clear();
                    app.dim_refs.clear();
                    app.drag = Drag::None;
                    // Include the threshold-crossing frame's movement as well.
                    let at = ui.input(|i| i.pointer.latest_pos()).and_then(|p| sketch_pos(app, sk, p));
                    if let Some(at) = at { let _ = app.reference_drag.update(at); }
                    ui.ctx().request_repaint();
                }
                Err(error) => app.toast(error),
            }
            return true;
        }
        app.reference_drag.clear();
    }
    if resp.clicked_by(PointerButton::Primary) {
        if let Some(pos) = resp.interact_pointer_pos()
            && app.reference_drag.hit(sid, image, sk.plane, app.cam, app.vp, pos, hover == Hit::None).is_some() {
            app.reference_drag.select(sid);
            app.sel.clear();
            app.dim_refs.clear();
            return true;
        }
        app.reference_drag.clear();
    }
    false
}

/// The Select tool, and the Move tool when `moving`: then a drag that starts on empty
/// space moves the selection instead of drawing a selection box.
fn select_tool(app: &mut App, ui: &Ui, resp: &egui::Response, painter: &Painter, sid: Id, sk: &Sketch, hover: Hit, moving: bool) {
    if reference_select_tool(app, ui, resp, sid, sk, hover) { return; }
    let colors = ViewColors::new(painter.ctx());
    let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    if resp.drag_started_by(PointerButton::Primary) && !ui.input(|i| i.modifiers.alt || i.key_down(egui::Key::Space)) {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
        let grab = sketch_pos(app, sk, origin).unwrap_or_default();
        app.drag = match hit(app, sk, origin) {
            Hit::Point(id) => Drag::Point(id),
            Hit::Entity(id) => {
                if !app.sel.contains(&id) {
                    toggle(&mut app.sel, id, add);
                }
                match sk.entities[&id].geom {
                    Geom::Circle { .. } => Drag::Radius(id),
                    _ => {
                        let mut pts: Vec<Id> = app.sel.iter().flat_map(|e| sk.ent_points(*e)).collect();
                        pts.sort();
                        pts.dedup();
                        Drag::Move(pts.into_iter().map(|p| (p, sk.pos(p) - grab)).collect())
                    }
                }
            }
            _ if moving && !app.sel.is_empty() => {
                let mut pts: Vec<Id> = app.sel.iter().flat_map(|e| if sk.points.contains_key(e) { vec![*e] } else { sk.ent_points(*e) }).collect();
                pts.sort();
                pts.dedup();
                Drag::Move(pts.into_iter().map(|p| (p, sk.pos(p) - grab)).collect())
            }
            _ => Drag::Box(origin),
        };
        if !matches!(app.drag, Drag::Box(_)) {
            app.session.snapshot();
        }
    }
    if resp.dragged_by(PointerButton::Primary)
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let at = sketch_pos(app, sk, pos);
        let drags: Vec<(Id, DVec2)> = match (&app.drag, at) {
            (Drag::Point(id), Some(t)) => vec![(*id, t)],
            (Drag::Move(list), Some(t)) => list.iter().map(|(id, off)| (*id, t + *off)).collect(),
            (Drag::Radius(id), Some(t)) => vec![(*id, DVec2::new(sk.curve(*id).map_or(1.0, |c| c.0.distance(t)).max(1e-3), 0.0))],
            (Drag::Box(start), _) => {
                let r = Rect::from_two_pos(*start, pos);
                painter.rect(r, 0.0, Color32::from_rgba_unmultiplied(0, 120, 255, 26), Stroke::new(1.0, colors.accent), StrokeKind::Inside);
                vec![]
            }
            _ => vec![],
        };
        if !drags.is_empty()
            && let Some(live) = app.session.doc.sketch_mut(sid)
        {
            let before = live.clone();
            let report = solver::solve(live, &drags);
            if report.ok && live.validate().is_ok() {
                app.report = report;
            } else {
                *live = before;
            }
        }
    }
    if resp.drag_stopped_by(PointerButton::Primary) {
        match std::mem::replace(&mut app.drag, Drag::None) {
            Drag::Box(start) => {
                let r = Rect::from_two_pos(start, resp.interact_pointer_pos().unwrap_or(start));
                let inside: Vec<Id> = sk
                    .entities
                    .keys()
                    .copied()
                    .filter(|e| path(app, sk, *e).iter().all(|p| r.contains(*p)))
                    .chain(sk.points.iter().filter(|(id, p)| **id != ORIGIN && r.contains(on_screen(app, sk, **p))).map(|(id, _)| *id))
                    .collect();
                if add {
                    app.sel.extend(inside.into_iter().filter(|i| !app.sel.contains(i)).collect::<Vec<_>>());
                } else {
                    app.sel = inside;
                }
            }
            Drag::None => {}
            _ => {
                // A drag that the constraints did not allow is not worth an undo step.
                let was = app.session.before().and_then(|d| d.sketch(sid));
                let moved = app.session.doc.sketch(sid).zip(was).is_none_or(|(now, was)| {
                    now.points.iter().any(|(id, p)| was.points.get(id).is_none_or(|q| q.distance(*p) > 1e-7)) || now.entities.keys().any(|e| (now.curve(*e).map_or(0.0, |c| c.1) - was.curve(*e).map_or(0.0, |c| c.1)).abs() > 1e-7)
                });
                if moved {
                    app.session.rebuild();
                } else {
                    app.session.abort();
                }
                app.refresh();
            }
        }
    }
    if resp.clicked_by(PointerButton::Primary) {
        match hover {
            Hit::None if !add => app.sel.clear(),
            Hit::None => {}
            Hit::Point(id) | Hit::Entity(id) | Hit::Label(id) => toggle(&mut app.sel, id, add),
        }
    }
    if resp.double_clicked_by(PointerButton::Primary) {
        match hover {
            Hit::Point(id) => app.open_point_coordinates(Some(id)),
            Hit::Label(cid) => if let Some(pos) = resp.interact_pointer_pos() { app.edit_dimension(cid, pos); },
            _ => {}
        }
    }
}

fn dimension_tool(app: &mut App, resp: &egui::Response, sk: &Sketch, hover: Hit) {
    if !resp.clicked_by(PointerButton::Primary) {
        return;
    }
    let Some(pos) = resp.interact_pointer_pos() else { return };
    let first = app.dim_refs.first().copied();
    let first_kind = first.and_then(|f| sk.ref_kind(f));
    match hover {
        Hit::Label(cid) => app.edit_dimension(cid, pos),
        Hit::Entity(e) => match (sk.ref_kind(e), first, first_kind) {
            (Some(Ref::Curve | Ref::Line), None, _) => app.dimension_selection(vec![e], pos),
            (Some(Ref::Line), Some(a), Some(Ref::Line)) if a != e => {
                let (l1, l2) = (sk.line(a).unwrap(), sk.line(e).unwrap());
                let (d1, d2) = ((l1.1 - l1.0).normalize_or_zero(), (l2.1 - l2.0).normalize_or_zero());
                let kind = if d1.perp_dot(d2).abs() < 0.02 { CKind::Distance } else { CKind::Angle };
                app.dim_refs.push(e);
                app.new_dimension(kind, vec![a, e], pos);
            }
            (Some(Ref::Line), Some(p), Some(Ref::Point)) => {
                app.dim_refs.push(e);
                app.new_dimension(CKind::Distance, vec![p, e], pos);
            }
            _ => app.dim_refs.clear(),
        },
        Hit::Point(p) => match (first, first_kind) {
            (None, _) => app.dim_refs = vec![p],
            (Some(q), Some(Ref::Point | Ref::Line)) if q != p => {
                app.dim_refs.push(p);
                app.new_dimension(CKind::Distance, vec![q, p], pos);
            }
            _ => app.dim_refs.clear(),
        },
        // A click on empty space places the length of the line just picked.
        Hit::None => match (first, first_kind) {
            (Some(l), Some(Ref::Line)) if app.dim_refs.len() == 1 => app.new_dimension(CKind::Distance, vec![l], pos),
            _ => app.dim_refs.clear(),
        },
    }
}

fn arc_points(c: DVec2, s: DVec2, e: DVec2) -> Vec<DVec2> {
    let r = c.distance(s);
    let (a0, a1) = ((s - c).to_angle(), (e - c).to_angle());
    // The short way round, as the arc tool will create it.
    let mut sweep = (a1 - a0).rem_euclid(std::f64::consts::TAU);
    if (s - c).perp_dot(e - c) < 0.0 {
        sweep -= std::f64::consts::TAU;
    }
    (0..=32).map(|i| c + DVec2::from_angle(a0 + sweep * i as f64 / 32.0) * r).collect()
}

/// Position the bulge of an endpoint-first arc without moving either endpoint.
/// The pointer picks the side and the nearer of the minor/major circular segments.
pub(crate) fn arc_bulge_for_diameter(start: DVec2, end: DVec2, pointer: DVec2, diameter: f64) -> Result<DVec2, String> {
    let chord = end - start;
    let half = chord.length() / 2.0;
    if half < 5e-7 { return Err("An arc needs two different endpoints.".into()); }
    if !diameter.is_finite() || diameter + 1e-9 < half * 2.0 {
        return Err("The diameter cannot be smaller than the distance between the endpoints.".into());
    }
    let normal = chord.normalize().perp();
    let mid = (start + end) / 2.0;
    let height = (pointer - mid).dot(normal);
    let radius = (diameter / 2.0).max(half);
    let offset = (radius * radius - half * half).max(0.0).sqrt();
    // The alternate formula avoids cancellation for a nearly straight small bulge.
    let minor = half * half / (radius + offset);
    let major = radius + offset;
    let sag = if (height.abs() - major).abs() < (height.abs() - minor).abs() { major } else { minor };
    Ok(mid + normal * sag * if height < 0.0 { -1.0 } else { 1.0 })
}

fn angle_delta(a: f64, b: f64) -> f64 { (a.rem_euclid(360.0) - b.rem_euclid(360.0) + 180.0).rem_euclid(360.0) - 180.0 }

/// Angular hysteresis: acquire within 3°, release beyond 6°.
fn sticky_angle(raw: f64, candidates: &[(f64, &'static str)], typed: &mut Typed) -> f64 {
    if let Some((angle, _)) = typed.guide && angle_delta(raw, angle).abs() <= 6.0 { return angle; }
    typed.guide = candidates.iter().copied().filter(|(angle, _)| angle_delta(raw, *angle).abs() <= 3.0)
        .min_by(|a, b| angle_delta(raw, a.0).abs().total_cmp(&angle_delta(raw, b.0).abs()));
    typed.guide.map_or(raw, |g| g.0)
}

fn update_angle_lock(typed: &mut Typed, shift: bool, current: f64) -> Option<f64> {
    if shift && !typed.shift_down { typed.locked = Some(current); }
    if !shift { typed.locked = None; }
    typed.shift_down = shift;
    typed.locked
}

pub(crate) fn line_drawing_snap(sk: &Sketch, start: DVec2, mut snap: Snap, typed: &mut Typed, scale: f64, shift: bool, angle: Option<f64>) -> Snap {
    let delta = snap.p - start;
    if delta.length() * scale < 2.0 { return snap; }
    let raw = delta.to_angle().to_degrees();
    let mut candidates: Vec<_> = (-4..=4).map(|i| (i as f64 * 45.0, if i % 2 == 0 { "Axis" } else { "45°" })).collect();
    for (&id, entity) in &sk.entities {
        if let Some((a, b)) = sk.line(id) {
            let a = (b - a).to_angle().to_degrees();
            for (offset, label) in [(0.0, "Parallel"), (90.0, "Perpendicular"), (180.0, "Parallel"), (270.0, "Perpendicular")] {
                candidates.push((a + offset, label));
            }
        } else if let Geom::Arc { s, e, .. } = entity.geom {
            for point in [s, e] {
                if sk.pos(point).distance(start) < 1e-6 && let Some(t) = sk.endpoint_tangent(id, point) {
                    candidates.push((t.to_angle().to_degrees(), "Tangent"));
                }
            }
        }
    }
    let on_axis = snap.axis[0] || snap.axis[1];
    let inferred = if angle.is_none() && snap.point.is_none() && snap.on.is_none() && !on_axis { sticky_angle(raw, &candidates, typed) } else { typed.guide = None; raw };
    let locked = update_angle_lock(typed, shift, angle.unwrap_or(inferred));
    let target = angle.or(locked).unwrap_or(inferred);
    if angle.is_some() || locked.is_some() || typed.guide.is_some() {
        let radians = target.rem_euclid(360.0).to_radians();
        snap.p = start + DVec2::from_angle(radians) * delta.length();
        if snap.p.distance(start + delta) > 1e-7 { (snap.point, snap.on, snap.axis) = (None, None, [false, false]); }
        snap.h = radians.sin().abs() < 1e-9;
        snap.v = radians.cos().abs() < 1e-9;
    }
    snap
}

/// A tangent arc's chord points halfway between its starting and ending tangents.
/// Locking this signed sweep preserves the turn while letting its radius change.
pub(crate) fn tangent_drawing_snap(start: DVec2, tangent: DVec2, mut snap: Snap, typed: &mut Typed, shift: bool, sweep: Option<f64>) -> Snap {
    let delta = snap.p - start;
    if delta.length() < 1e-6 { return snap; }
    let tangent = tangent.normalize_or(DVec2::X);
    let raw = 2.0 * delta.dot(tangent.perp()).atan2(delta.dot(tangent)).to_degrees();
    let candidates: Vec<_> = (-7..=7).filter(|i| *i != 0).map(|i| (i as f64 * 45.0, "Sweep")).collect();
    // Sweep values are signed and must not wrap: +315° and -45° are different arcs.
    let on_axis = snap.axis[0] || snap.axis[1];
    if sweep.is_some() || snap.point.is_some() || snap.on.is_some() || on_axis { typed.guide = None; }
    if typed.guide.is_some_and(|g| (raw - g.0).abs() > 6.0) { typed.guide = None; }
    if sweep.is_none() && typed.guide.is_none() && snap.point.is_none() && snap.on.is_none() && !on_axis {
        typed.guide = candidates.into_iter().filter(|(a, _)| (raw - a).abs() <= 3.0).min_by(|a, b| (raw - a.0).abs().total_cmp(&(raw - b.0).abs()));
    }
    let inferred = typed.guide.map_or(raw, |g| g.0);
    let sign = if raw < 0.0 { -1.0 } else { 1.0 };
    let current = sweep.map_or(inferred, |a| a * sign);
    // Shift pressed while the cursor is still on the straight tangent waits for
    // a usable arc, instead of creating an impossible zero-sweep lock.
    let usable = current.abs() > 1e-6 && current.abs() < 360.0 - 1e-6;
    let locked = if usable || typed.locked.is_some() || !shift { update_angle_lock(typed, shift, current) } else { None };
    let target = sweep.map(|a| a * locked.map_or(sign, |v| v.signum())).or(locked).unwrap_or(inferred);
    if target.abs() < 1e-6 || target.abs() >= 360.0 - 1e-6 { return snap; }
    if sweep.is_some() || locked.is_some() || typed.guide.is_some() {
        let half = (target / 2.0).to_radians();
        snap.p = start + (tangent * half.cos() + tangent.perp() * half.sin()) * delta.length();
        if snap.p.distance(start + delta) > 1e-7 { (snap.point, snap.on, snap.axis) = (None, None, [false, false]); }
    }
    snap
}

fn draw_tool(app: &mut App, ui: &Ui, resp: &egui::Response, painter: &Painter, sk: &Sketch) {
    let colors = ViewColors::new(painter.ctx());
    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    let tool = app.tool;
    let from = (tool == Tool::Line && app.clicks.len() == 1).then(|| app.clicks[0].p);
    // Sizes can be typed for these once their first point is down.
    let names: &[&str] = match (tool, app.clicks.len()) {
        (Tool::Rect, 1) => &["Width", "Height"],
        (Tool::Circle, 1) => &["Diameter"],
        (Tool::Line, 1) => &["Length", "Angle"],
        (Tool::Arc3, 2) => &["Diameter"],
        (Tool::TangentArc, 1) => &["Sweep"],
        _ => &[],
    };
    if names.is_empty() {
        app.typed = None;
    } else if app.typed.as_ref().is_none_or(|t| t.fields.len() != names.len()) {
        app.typed = Some(Typed { fields: vec![String::new(); names.len()], ..Default::default() });
    }
    let hovered = resp.hover_pos().and_then(|p| captured_snap(app, ui, sk, p, from));
    // Over the boxes themselves the pointer is not over the sketch; the shape stays where it was.
    let kept = app.typed.as_ref().and_then(|t| t.last).map(|(p, point, on)| Snap { p, point, on, h: false, v: false, axis: [false, false], mid: false });
    let Some(mut s) = hovered.or(kept) else { return };
    let attachment = s;
    if let Some(t) = &mut app.typed {
        t.last = Some((s.p, s.point, s.on));
    }
    let field_angle = |i: usize| tool == Tool::TangentArc || (tool == Tool::Line && i == 1);
    let parse = |i: usize, text: &str| if field_angle(i) { App::typed_angle(app.doc(), text, tool == Tool::TangentArc) } else { App::typed_size(app.doc(), text) };
    let sizes: Vec<Option<f64>> = app.typed.as_ref().map_or(Vec::new(), |t| t.fields.iter().enumerate().map(|(i, f)| parse(i, f)).collect());
    let shift = ui.input(|i| i.modifiers.shift);
    if let Some(t) = &mut app.typed {
        if tool == Tool::Line {
            s = line_drawing_snap(sk, app.clicks[0].p, s, t, app.cam.scale, shift, sizes.get(1).copied().flatten());
        } else if tool == Tool::TangentArc && let Some(start) = app.clicks[0].point
            && let Ok(source) = App::tangent_source(sk, start, &app.sel)
            && let Some(tangent) = sk.endpoint_tangent(source, start) {
            s = tangent_drawing_snap(app.clicks[0].p, tangent, s, t, shift, sizes.first().copied().flatten());
        }
    }
    let mut placement_error = None;
    if sizes.iter().any(Option::is_some) {
        // A typed size holds; the pointer still chooses the side or the direction.
        let a = app.clicks[0].p;
        let side = |d: f64| if d < 0.0 { -1.0 } else { 1.0 };
        match tool {
            Tool::Rect => {
                if let Some(w) = sizes[0] {
                    s.p.x = a.x + w * side(s.p.x - a.x);
                }
                if let Some(h) = sizes[1] {
                    s.p.y = a.y + h * side(s.p.y - a.y);
                }
            }
            Tool::Circle => s.p = a + (s.p - a).try_normalize().unwrap_or(DVec2::X) * sizes[0].unwrap_or(0.0) / 2.0,
            Tool::Line => if let Some(length) = sizes[0] { s.p = a + (s.p - a).normalize_or(DVec2::X) * length; },
            Tool::Arc3 => match arc_bulge_for_diameter(a, app.clicks[1].p, s.p, sizes[0].unwrap_or(0.0)) {
                Ok(p) => s.p = p,
                Err(e) => placement_error = Some(e),
            },
            _ => {},
        }
        (s.point, s.on) = if s.p.distance(attachment.p) <= 1e-7 { (attachment.point, attachment.on) } else { (None, None) };
    }
    let px = |p: DVec2| on_screen(app, sk, p);
    let stroke = Stroke::new(1.6, if app.opts.construction { colors.construction } else { colors.sketch });
    let unit = app.doc().units;
    let len = |mm: f64| format!("{} {}", fr_core::units::fmt_len(mm, unit), unit.name());
    let note = |at: Pos2, text: String| {
        label(painter, at + vec2(0.0, -18.0), &text, colors.dim_ink, None);
    };
    let c = &app.clicks;
    // Where the size boxes go, and what each would be if left to the pointer.
    let mut boxes: Option<(Pos2, Vec<f64>)> = None;
    match (tool, c.len()) {
        (Tool::Line, 1) => {
            painter.line_segment([px(c[0].p), px(s.p)], stroke);
            boxes = Some((px((c[0].p + s.p) / 2.0), vec![c[0].p.distance(s.p), (s.p - c[0].p).to_angle().to_degrees()]));
        }
        (Tool::Rect, 1) => {
            let (a, b) = (c[0].p, s.p);
            let pts = vec![px(a), px(DVec2::new(b.x, a.y)), px(b), px(DVec2::new(a.x, b.y))];
            painter.add(Shape::closed_line(pts, stroke));
            boxes = Some((px(DVec2::new((a.x + b.x) / 2.0, a.y.max(b.y))), vec![(b.x - a.x).abs(), (b.y - a.y).abs()]));
        }
        (Tool::Circle, 1) => {
            let r = c[0].p.distance(s.p);
            let pts = (0..=72).map(|i| px(c[0].p + DVec2::from_angle(i as f64 / 72.0 * std::f64::consts::TAU) * r)).collect();
            painter.add(Shape::line(pts, stroke));
            boxes = Some((px(c[0].p + DVec2::Y * r), vec![r * 2.0]));
        }
        (Tool::Polygon, 1) => {
            let (n, r, a0) = (app.opts.sides.clamp(3, 64), c[0].p.distance(s.p), (s.p - c[0].p).to_angle());
            let pts = (0..n).map(|i| px(c[0].p + DVec2::from_angle(a0 + std::f64::consts::TAU * i as f64 / n as f64) * r)).collect();
            painter.add(Shape::closed_line(pts, stroke));
            note(px(s.p), format!("R{}", len(r)));
        }
        (Tool::Arc, 1) => {
            painter.extend(Shape::dashed_line(&[px(c[0].p), px(s.p)], stroke, 5.0, 4.0));
            note(px(s.p), format!("R{}", len(c[0].p.distance(s.p))));
        }
        (Tool::Arc, 2) => {
            painter.add(Shape::line(arc_points(c[0].p, c[1].p, s.p).into_iter().map(px).collect(), stroke));
        }
        (Tool::Arc3, 1) => { painter.line_segment([px(c[0].p), px(s.p)], stroke); }
        (Tool::Arc3, 2) => {
            let mut preview = Sketch::new(Plane::XY);
            let a = preview.add_point(c[0].p);
            let e = preview.add_point(c[1].p);
            let through = preview.add_point(s.p);
            painter.extend(Shape::dashed_line(&[px(c[0].p), px(c[1].p)], Stroke::new(1.0, colors.dim_ink), 5.0, 4.0));
            let diameter = if let Ok(id) = preview.add_arc3(a, through, e, false) {
                painter.add(Shape::line(preview.polyline(id).into_iter().map(px).collect(), stroke));
                preview.curve(id).unwrap().1 * 2.0
            } else { c[0].p.distance(c[1].p) };
            boxes = Some((px(s.p), vec![diameter]));
        }
        (Tool::TangentArc, 1) => {
            if let Some(start) = c[0].point
                && let Ok(source) = App::tangent_source(sk, start, &app.sel)
            {
                let mut preview = sk.clone();
                let end = preview.add_point(s.p);
                if let Ok(id) = preview.add_tangent_arc(source, start, end, false) {
                    painter.add(Shape::line(preview.polyline(id).into_iter().map(px).collect(), stroke));
                    boxes = Some((px(s.p), vec![preview.measure(CKind::Angle, &[id])]));
                } else {
                    boxes = Some((px(s.p), vec![90.0]));
                }
            }
        }
        (Tool::Spline, n) if n > 0 => {
            let mut pts: Vec<DVec2> = c.iter().map(|c| c.p).collect();
            pts.push(s.p);
            painter.extend(Shape::dashed_line(&pts.iter().copied().map(px).collect::<Vec<_>>(), stroke, 4.0, 4.0));
            if pts.len() == 4 {
                let mut preview = Sketch::new(Plane::XY);
                let ids = [preview.add_point(pts[0]), preview.add_point(pts[1]), preview.add_point(pts[2]), preview.add_point(pts[3])];
                if let Ok(id) = preview.add_spline(ids, false) {
                    painter.add(Shape::line(preview.polyline(id).into_iter().map(px).collect(), stroke));
                }
            }
        }
        _ => {}
    }
    // Show both soft inference and explicit locks before placement.
    if let Some(t) = &app.typed {
        if let Some(angle) = t.locked {
            let value = if tool == Tool::TangentArc { angle.abs() } else { angle_delta(angle, 0.0) };
            note(px(s.p) + vec2(0.0, 56.0), format!("{} {}° locked", egui_phosphor::regular::LOCK_SIMPLE, fr_core::units::trim_num(value, 2)));
        } else if let Some((angle, guide)) = t.guide {
            let value = if tool == Tool::TangentArc { angle.abs() } else { angle_delta(angle, 0.0) };
            note(px(s.p) + vec2(0.0, 56.0), format!("{guide} · {}° · Shift to lock", fr_core::units::trim_num(value, 2)));
        }
    }
    if let Some(error) = &placement_error { note(px(s.p) + vec2(0.0, 78.0), error.clone()); }
    // Show what the click will attach to.
    let at = px(s.p);
    if s.point.is_some() {
        painter.rect_stroke(Rect::from_center_size(at, Vec2::splat(11.0)), 1.0, Stroke::new(1.6, colors.selected), StrokeKind::Outside);
    } else if let Some(entity) = s.on {
        painter.add(Shape::line(path(app,sk,entity),Stroke::new(2.4,colors.selected)));
        if s.mid {
            // A small triangle under the point says it is the midpoint.
            let tri = vec![at + vec2(0.0, 5.0), at + vec2(-6.0, 13.0), at + vec2(6.0, 13.0)];
            painter.add(Shape::convex_polygon(tri, colors.selected, Stroke::NONE));
            painter.text(at + vec2(0.0, 21.0), Align2::CENTER_CENTER, "mid", FontId::proportional(11.0), colors.selected);
        }
        if let Some(pointer) = resp.hover_pos().filter(|p|p.distance(at)>2.0) {
            painter.line_segment([pointer,at],Stroke::new(1.0,colors.selected));
        }
        painter.circle(at,4.5,colors.paper,Stroke::new(2.0,colors.selected));
        note(at+vec2(0.0,36.0),format!("On {} · Alt to release",if sk.line(entity).is_some(){"line"}else{"curve"}));
    } else if s.h || s.v {
        let r = Rect::from_center_size(at + vec2(16.0, 16.0), Vec2::splat(15.0));
        painter.rect(r, 3.0, colors.paper, Stroke::new(1.0, colors.selected), StrokeKind::Outside);
        constraint_icon(painter, r.shrink(1.5), if s.h { CKind::Horizontal } else { CKind::Vertical }, colors.selected);
    }
    // The size boxes: type a size to hold it, Tab to the next box, Enter to place the shape.
    let mut place = false;
    if let (Some((at, live)), Some(mut t)) = (boxes, app.typed.clone()) {
        let ctx = ui.ctx().clone();
        let mut focused = false;
        egui::Area::new("typed-sizes".into()).order(egui::Order::Foreground).pivot(Align2::CENTER_BOTTOM).fixed_pos(at + vec2(0.0, -10.0)).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, text) in t.fields.iter_mut().enumerate() {
                        ui.label(RichText::new(names[i]).small().color(colors.dim_ink));
                        // Empty, the box shows the size the pointer gives, and typing replaces it.
                        let r = ui.add(egui::TextEdit::singleline(text).id(egui::Id::new(("typed-size", i))).desired_width(74.0).hint_text(if field_angle(i) { format!("{}°", fr_core::units::trim_num(live[i], 2)) } else { len(live[i]) }));
                        if !text.is_empty() {
                            let held = sizes.get(i).copied().flatten().is_some();
                            ui.label(RichText::new(if held { egui_phosphor::regular::LOCK_SIMPLE } else { egui_phosphor::regular::WARNING }).color(if held { colors.selected } else { colors.error }));
                        }
                        if r.clicked() {
                            t.active = i;
                        }
                        focused |= r.has_focus() && t.active == i;
                        if t.active == i && !r.has_focus() {
                            r.request_focus();
                        }
                    }
                });
            });
        });
        let _ = focused;
        let key = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        if key(egui::Key::Tab) {
            t.active = (t.active + 1) % t.fields.len();
        }
        if key(egui::Key::Escape) {
            app.cancel_tool();
            return;
        }
        if key(egui::Key::Enter) {
            // With nothing typed, Enter does what it always did: ends the line, or drops the shape.
            if t.fields.iter().all(String::is_empty) {
                app.cancel_tool();
                return;
            }
            let bad = t.fields.iter().enumerate().find(|(i, f)| !f.trim().is_empty() && if field_angle(*i) { App::typed_angle(app.doc(), f, tool == Tool::TangentArc).is_none() } else { App::typed_size(app.doc(), f).is_none() });
            match bad {
                Some((i, bad)) => app.toast(format!("\"{bad}\" is not a valid {}. Use a length such as 20 mm, or an angle such as 30 deg.", names[i].to_lowercase())),
                None => place = true,
            }
        }
        app.typed = Some(t);
    }
    if (place || resp.clicked_by(PointerButton::Primary)) && !names.is_empty() {
        if let Some(t) = &app.typed {
            if let Some((i, bad)) = t.fields.iter().enumerate().find(|(i, f)| !f.trim().is_empty() && if field_angle(*i) { App::typed_angle(app.doc(), f, tool == Tool::TangentArc).is_none() } else { App::typed_size(app.doc(), f).is_none() }) {
                app.toast(format!("\"{bad}\" is not a valid {}.", names[i].to_lowercase()));
                return;
            }
        }
        // TextEdit can receive text and Enter in the same frame. Recompute from the
        // final text before committing, so a fresh dimension never pulls the first
        // endpoint away from the position the user already placed.
        if let Some(t) = &app.typed {
            let a = app.clicks[0].p;
            match tool {
                Tool::Line => {
                    let direction = t.fields.get(1).and_then(|f| App::typed_angle(app.doc(), f, false)).or(t.locked)
                        .map(|a| DVec2::from_angle(a.rem_euclid(360.0).to_radians())).unwrap_or_else(|| (s.p - a).normalize_or(DVec2::X));
                    let length = t.fields.first().and_then(|f| App::typed_size(app.doc(), f)).unwrap_or_else(|| a.distance(s.p));
                    s.p = a + direction * length;
                }
                Tool::Arc3 => if let Some(diameter) = t.fields.first().and_then(|f| App::typed_size(app.doc(), f)) {
                    match arc_bulge_for_diameter(a, app.clicks[1].p, s.p, diameter) {
                        Ok(p) => { s.p = p; placement_error = None; },
                        Err(error) => { app.toast(error); return; },
                    }
                },
                Tool::TangentArc => if let Some(sweep) = t.fields.first().and_then(|f| App::typed_angle(app.doc(), f, true))
                    && let Some(start) = app.clicks[0].point
                    && let Ok(source) = App::tangent_source(sk, start, &app.sel)
                    && let Some(tangent) = sk.endpoint_tangent(source, start) {
                    s = tangent_drawing_snap(a, tangent, s, &mut t.clone(), shift, Some(sweep));
                },
                _ => {},
            }
            if t.fields.iter().any(|f| !f.trim().is_empty()) {
                (s.point, s.on) = if s.p.distance(attachment.p) <= 1e-7 { (attachment.point, attachment.on) } else { (None, None) };
            }
        }
        if let Some(error) = placement_error { app.toast(error); return; }
    }
    if resp.double_clicked_by(PointerButton::Primary) || resp.secondary_clicked() {
        app.cancel_tool();
    } else if place {
        app.clicks.push(s);
        app.commit_clicks();
    } else if resp.clicked_by(PointerButton::Primary) {
        if tool == Tool::Line && app.clicks.len() == 1 && (app.clicks[0].p.distance(s.p) * app.cam.scale < 2.0 || (s.point.is_some() && s.point == app.clicks[0].point)) {
            // Clicking the same spot again ends the line.
            app.clicks.clear();
        } else {
            app.clicks.push(s);
            app.commit_clicks();
        }
    }
}

/// The floating box for typing a dimension.
fn value_box(app: &mut App, ui: &Ui) {
    let Some(mut edit) = app.value_edit.clone() else { return };
    let (mut commit, mut cancel) = (false, false);
    let kind = match &edit.target {
        EditTarget::New(kind, _) => Some(*kind),
        EditTarget::Existing(id) => app.sketch().and_then(|(_, sk)| sk.constraints.get(id)).map(|c| c.kind),
        _ => None,
    };
    let title = match kind {
        Some(CKind::Distance) => "Length / distance",
        Some(CKind::Radius) => "Radius",
        Some(CKind::Diameter) => "Diameter",
        Some(CKind::Angle) => "Angle",
        Some(CKind::PositionX) => "X position",
        Some(CKind::PositionY) => "Y position",
        _ => "Size",
    };
    egui::Area::new("value-edit".into()).order(egui::Order::Foreground).fixed_pos(edit.pos + vec2(8.0, 8.0)).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.label(title);
            let mut output = egui::TextEdit::singleline(&mut edit.text).desired_width(160.0).hint_text("10 mm, $w / 2, d = 5").show(ui);
            let r = &output.response;
            if edit.focus {
                r.request_focus();
                output.state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0), egui::text::CCursor::new(edit.text.chars().count()),
                )));
                output.state.store(ui.ctx(), r.id);
                edit.focus = false;
            }
            if let Some(e) = &edit.error {
                ui.set_max_width(240.0);
                ui.colored_label(theme::Palette::from_ctx(ui.ctx()).error, e);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            } else if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
        });
    });
    app.value_edit = (!cancel).then_some(edit);
    if cancel {
        app.dim_refs.clear();
    }
    if commit {
        app.commit_value();
    }
}

fn sketch_mode(app: &mut App, ui: &Ui, resp: &egui::Response, painter: &Painter, sid: Id, reference_consumed: bool) {
    if app.tool.clicks()==0 || app.dialog!=Dialog::None || app.value_edit.is_some() || !ui.input(|i|i.focused) { app.sketch_capture.clear(); }
    let Some(sk) = app.world_sketch(sid) else { return };
    if app.opts.gaps && app.gap_cache.as_ref().is_none_or(|(rev, cached, _)| *rev != app.session.rev || *cached != sid) {
        app.gap_cache = Some((app.session.rev, sid, sk.open_endpoints()));
    }
    let dim = Hit::None;
    for (f, _) in app.session.doc.sketches().filter(|(f, s)| f.id != sid && s.visible && !app.doc().is_suppressed(f.id) && !app.session.built.errors.contains_key(&f.id) && app.session.built.component_visible(f.owner) && app.doc().features.iter().take(app.doc().active()).any(|earlier| earlier.id == f.id)) {
        let Some(other) = app.world_sketch(f.id) else { continue };
        draw_sketch(app, painter, &other, false, dim, &mut Vec::new());
    }
    let hover = resp.hover_pos().map_or(Hit::None, |p| hit(app, &sk, p));
    let mut labels = Vec::new();
    draw_sketch(app, painter, &sk, true, if matches!(app.tool, Tool::Select | Tool::Dimension | Tool::Trim) { hover } else { Hit::None }, &mut labels);
    app.labels = labels;

    // Search owns input even on its opening and dismissal frames. Keep the
    // sketch visible without forwarding keys or clicks to an active tool.
    if app.command_search.block_input { return; }

    if app.reference_editor.sketch_id() == Some(sid) {
        let cursor = resp.hover_pos().and_then(|p| sketch_pos(app, &sk, p));
        app.reference_editor.paint_calibration(painter, sk.plane, app.cam, app.vp, cursor);
        if app.reference_editor.is_calibrating() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if resp.clicked_by(PointerButton::Primary) && let Some(p) = cursor { app.reference_editor.calibration_click(p); }
        }
        return;
    }
    if matches!(app.dialog, Dialog::PointCoordinates(_)) { return; }
    if app.value_edit.is_some() {
        // Keep invalid values visible so they can be corrected or explicitly cancelled.
        if resp.clicked() { app.commit_value(); }
    } else {
        match app.tool {
            Tool::Select | Tool::Move => if !reference_consumed { select_tool(app, ui, resp, painter, sid, &sk, hover, app.tool == Tool::Move); },
            Tool::Dimension => dimension_tool(app, resp, &sk, hover),
            Tool::Trim => {
                if let (true, Hit::Entity(e), Some(pos)) = (resp.clicked_by(PointerButton::Primary), hover, resp.interact_pointer_pos())
                    && let Some(at) = sketch_pos(app, &sk, pos)
                {
                    app.sketch_edit(|sk, _| sk.trim(e, at));
                }
            }
            Tool::Project => {
                if resp.clicked_by(PointerButton::Primary)
                    && let Some(pos) = resp.interact_pointer_pos()
                {
                    match pick_face(app, pos) {
                        Some(mut face) => {
                            let owner = app.doc().feature(sid).map_or(0, |f| f.owner);
                            let transform = app.session.built.component_placement(owner).inverse();
                            for ring in &mut face.outline { for point in ring { *point = transform.transform_point3(*point); } }
                            app.sketch_edit(|sk, _| if sk.project(&face.outline).is_empty() { Err("That outline is already in the sketch.".into()) } else { Ok(()) });
                        }
                        None => app.toast("Click a face of a body."),
                    }
                }
            }
            _ => draw_tool(app, ui, resp, painter, &sk),
        }
    }
    if app.tool == Tool::Select && let Some(image) = app.reference_drag.preview(sid).or(sk.reference.as_ref()) {
        app.reference_drag.paint(painter, sid, image, sk.plane, app.cam, app.vp);
    }
    value_box(app, ui);
}

/// The point halfway along a polyline.
pub fn midpoint(line: &[DVec3]) -> DVec3 {
    let mut left: f64 = line.windows(2).map(|w| w[0].distance(w[1])).sum::<f64>() / 2.0;
    for w in line.windows(2) {
        let d = w[0].distance(w[1]);
        if left <= d {
            return w[0].lerp(w[1], if d > 0.0 { left / d } else { 0.0 });
        }
        left -= d;
    }
    line.first().copied().unwrap_or_default()
}

/// Distance from a point to a polyline in space.
pub fn path_dist3(line: &[DVec3], p: DVec3) -> f64 {
    line.windows(2)
        .map(|w| {
            let d = w[1] - w[0];
            (w[0] + d * ((p - w[0]).dot(d) / d.length_squared().max(1e-18)).clamp(0.0, 1.0)).distance(p)
        })
        .fold(f64::MAX, f64::min)
}

/// What the Inspect tool would pick at a screen position: a corner or sketch point
/// if one is close, else an edge or sketch line, else the face under the pointer.
fn pick_item(app: &App, doc: &Document, pos: Pos2) -> Option<Picked> {
    let u = doc.units;
    let len = |mm: f64| format!("{} {}", fr_core::units::fmt_len(mm, u), u.name());
    let hidden = &doc.hidden_bodies;
    let bodies = || app.session.built.bodies.iter().filter(|b| !hidden.contains(&b.id) && app.body_visible(b));
    let sketches = || doc.sketches().filter(|(_, s)| s.visible).map(|(_, s)| s);
    let near = |p: DVec3| to_screen(app, p).distance(pos);
    let corners = bodies().flat_map(|b| b.edges.iter().flat_map(|e| [e.first().copied(), e.last().copied()])).flatten();
    let points = sketches().flat_map(|s| s.points.values().map(|p| s.plane.to_world(*p)));
    if let Some(p) = corners.chain(points).map(|p| (near(p), p)).filter(|h| h.0 <= 7.0).min_by(|a, b| a.0.total_cmp(&b.0)).map(|h| h.1) {
        let c = (p / u.mm()).to_array().map(|v| fr_core::units::trim_num(v, 3));
        return Some(Picked { item: Item::Point(p), label: format!("Point at {}, {}, {}", c[0], c[1], c[2]), outline: Vec::new() });
    }
    let lines = sketches().flat_map(|s| s.entities.iter().filter_map(|(id, e)| matches!(e.geom, Geom::Line { .. }).then(|| s.line(*id)).flatten().map(|(a, b)| vec![s.plane.to_world(a), s.plane.to_world(b)])));
    let edges = bodies().flat_map(|b| b.edges.iter().cloned());
    if let Some(line) = edges.chain(lines).map(|e| (path_dist(&e.iter().map(|p| to_screen(app, *p)).collect::<Vec<_>>(), pos), e)).filter(|h| h.0 <= 6.0).min_by(|a, b| a.0.total_cmp(&b.0)).map(|h| h.1) {
        let item = Item::Path(line);
        let what = if item.line().is_some() { "Straight edge" } else { "Curved edge" };
        return Some(Picked { label: format!("{what}, {} long", len(item.size().unwrap_or(0.0))), item, outline: Vec::new() });
    }
    let (id, at, tri) = pick_body(app, &app.session.built, pos)?;
    let body = app.session.built.body(id)?;
    let face = Face::pick(body, tri);
    let item = Item::Surface(face.tris.iter().map(|t| body.mesh.tri(*t)).collect());
    // A round face says how big round it is, which is how a hole or a rod is measured.
    let round = fr_core::exact::barrel(&body.solids, &[at, at]).ok().filter(|_| body.is_exact() && face.plane.is_none());
    let label = match round {
        Some(b) => format!("{}, \u{d8}{}", if b.internal { "Hole" } else { "Round face" }, len(b.radius * 2.0)),
        None => format!("{} face, {} {}\u{b2}", if face.plane.is_some() { "Flat" } else { "Curved" }, fr_core::units::trim_num(face.area / u.mm().powi(2), 3), u.name()),
    };
    Some(Picked { item, label, outline: face.outline })
}

/// The edge of an exact body nearest a screen position: the body and the edge's polyline.
fn pick_edge(app: &App, pos: Pos2, only: Option<Id>) -> Option<(Id, Vec<DVec3>)> {
    let hidden = &app.doc().hidden_bodies;
    app.session
        .built
        .bodies
        .iter()
        .filter(|b| b.is_exact() && !hidden.contains(&b.id) && only.is_none_or(|o| o == b.id))
        .flat_map(|b| b.edges.iter().map(move |e| (b.id, e)))
        .map(|(id, e)| (path_dist(&e.iter().map(|p| to_screen(app, *p)).collect::<Vec<_>>(), pos), id, e))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .filter(|hit| hit.0 <= 8.0)
        .map(|hit| (hit.1, hit.2.clone()))
}

fn draw_edge(app: &App, painter: &Painter, line: &[DVec3], color: Color32, width: f32) {
    painter.add(Shape::line(line.iter().map(|p| to_screen(app, *p)).collect(), Stroke::new(width, color)));
}

/// The body under a screen position: its id, the point hit and the triangle hit.
pub(crate) fn pick_body(app: &App, built: &fr_core::Built, pos: Pos2) -> Option<(Id, DVec3, usize)> {
    let (o, d) = ray(app, pos);
    let hidden = &app.doc().hidden_bodies;
    built.bodies.iter().filter(|b| !hidden.contains(&b.id) && app.body_visible(b)).filter_map(|b| b.mesh.ray(o, d).map(|(t, i)| (t, b.id, i))).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(t, id, i)| (id, o + d * t, i))
}

/// The face of a body under a screen position, in the document as it stands (not a dialog's preview).
pub(crate) fn pick_face(app: &App, pos: Pos2) -> Option<Face> {
    let built = app.modeling_source();
    let (id, at, tri) = pick_body(app, built, pos)?;
    let mut face = Face::pick(built.body(id)?, tri);
    face.at = at;
    Some(face)
}

/// The sketch line or curve under the pointer that could be a sweep's path: one in a
/// shown sketch other than the profile's.
fn pick_path(app: &App, doc: &Document, pos: Pos2, profile: Option<Id>, selected_path: Option<Id>) -> Option<(Id, Id)> {
    doc.sketches()
        .filter(|(f, s)| Some(f.id) != profile && (s.visible || selected_path == Some(f.id)) && !app.doc().is_suppressed(f.id) && !app.session.built.errors.contains_key(&f.id) && app.shown().component_visible(f.owner))
        .find_map(|(f, s)| match hit(app, s, pos) {
            Hit::Entity(e) => Some((f.id, e)),
            _ => None,
        })
}

/// The smallest profile under a screen position among the sketches a feature can use.
fn pick_profile(app: &App, doc: &Document, pos: Pos2, also: &[Id]) -> Option<(Id, Profile)> {
    doc.sketches()
        .filter(|(f, s)| !app.doc().is_suppressed(f.id) && !app.session.built.errors.contains_key(&f.id) && app.shown().component_visible(f.owner) && (s.visible || also.contains(&f.id)))
        .filter_map(|(f, s)| {
            let at = sketch_pos(app, s, pos)?;
            profiles(s).into_iter().filter(|p| p.contains(at)).min_by(|a, b| a.area().total_cmp(&b.area())).map(|p| (f.id, p))
        })
        .min_by(|a, b| a.1.area().total_cmp(&b.1.area()))
}

fn model_mode(app: &mut App, resp: &egui::Response, painter: &Painter, consumed: bool) {
    let colors = ViewColors::new(painter.ctx());
    let mut doc = app.session.doc.clone();
    let before = match &app.dialog { Dialog::Plane(d) => d.editing.and_then(|id| doc.features.iter().position(|f| f.id == id)).unwrap_or(doc.active()), _ => crate::body_ops_ui::editing(&app.dialog).and_then(|id|doc.features.iter().position(|f|f.id==id)).unwrap_or(doc.active()) };
    doc.features.truncate(before);
    for feature in &mut doc.features {
        if let FeatureKind::Sketch(sk) = &mut feature.kind {
            sk.plane = app.modeling_source().sketch_plane(app.doc(),feature.id).unwrap_or_else(||sk.plane.transformed(app.modeling_source().component_placement(feature.owner)));
            if app.doc().is_suppressed(feature.id) || app.session.built.errors.contains_key(&feature.id) || !app.shown().component_visible(feature.owner) { sk.visible = false; }
        }
    }
    // Sketches a dialog is using are drawn even when they were put away.
    let dlg_sketch: Vec<Id> = match &app.dialog {
        Dialog::Feature(f) => f.sketch.into_iter().collect(),
        Dialog::Loft(l) => l.sections.iter().map(|section| section.sketch).collect(),
        _ => Vec::new(),
    };
    for (f, sk) in doc.sketches().filter(|(f, s)| !app.doc().is_suppressed(f.id) && !app.session.built.errors.contains_key(&f.id) && app.shown().component_visible(f.owner) && (s.visible || dlg_sketch.contains(&f.id))) {
        let _ = f;
        draw_sketch(app, painter, sk, false, Hit::None, &mut Vec::new());
    }
    app.labels.clear();
    if app.command_search.block_input { return; }
    let hover = if consumed {None} else {resp.hover_pos()};
    let clicked = (!consumed && resp.clicked_by(PointerButton::Primary)).then(|| resp.interact_pointer_pos()).flatten();
    match app.dialog.clone() {
        Dialog::Remove(_) | Dialog::Split(_) => crate::body_ops_ui::interact(app,painter,hover,clicked),
        Dialog::Primitive(_) => {},
        Dialog::Plane(_) => {
            crate::construction_view::interact(app, painter, hover, clicked);
            if let Some(arrow) = extrude_arrow(app) {
                let hot = hover.is_some_and(|p| seg_dist(p, arrow.base, arrow.tip) < 12.0 || p.distance(arrow.tip) < 18.0);
                draw_arrow(app, painter, &arrow, hot || app.drag == Drag::Arrow);
            }
        }
        Dialog::PickPlane => {
            if let Some(id) = clicked.and_then(|p| crate::construction_view::pick_plane_before_face(app, p)) { app.create_sketch_on(id); return; }
            if let Some(pos) = clicked
                && let Some(face) = pick_face(app, pos)
            {
                match face.plane {
                    Some(plane) => app.create_sketch_world(plane),
                    None => app.toast("Sketches need a flat face."),
                }
            }
        }
        Dialog::Feature(mut f) => {
            let over = hover.and_then(|p| pick_profile(app, &doc, p, f.sketch.as_slice()));
            if let Some(sk) = f.sketch.and_then(|s| doc.sketch(s)) {
                for p in profiles(sk).iter().filter(|p| f.profiles.contains(&p.edges)) {
                    fill(app, painter, sk, p, Color32::from_rgba_unmultiplied(0, 120, 255, 84));
                }
                // Points across the revolve axis are ringed in red, matching the dialog's offer to fix them.
                if f.revolve && let Some((_, past, _)) = app.revolve_crossing() {
                    let red = Color32::from_rgb(214, 48, 48);
                    for p in past {
                        let at = on_screen(app, sk, sk.pos(p));
                        painter.circle_stroke(at, 8.0, Stroke::new(2.2, red));
                        painter.circle_filled(at, 2.5, red);
                    }
                }
                if f.revolve
                    && let Some((a, b)) = Document::axis_line(sk, f.axis)
                {
                    let d = (b - a).normalize_or(DVec2::X) * (app.vp.size().max_elem() as f64 / app.cam.scale);
                    painter.extend(Shape::dashed_line(&[on_screen(app, sk, a - d), on_screen(app, sk, a + d)], Stroke::new(1.6, theme::choose(painter.ctx(), Color32::from_rgb(214, 60, 160), Color32::from_rgb(244, 124, 208))), 10.0, 4.0));
                }
            }
            // The sweep's path, in the colour of a selection; dashed while it cannot be followed.
            let over_path = match (&f.sweep, hover) {
                (Some(w), Some(p)) => pick_path(app, &doc, p, f.sketch, w.path_sketch),
                _ => None,
            };
            if let Some(w) = &f.sweep {
                let stroke = Stroke::new(2.6, colors.selected);
                if let Some(sk) = w.path_sketch.and_then(|s| doc.sketch(s)) {
                    let chain = fr_core::profile::chain(sk, &w.path);
                    let ids: Vec<Id> = match &chain { Ok(c) => c.ids.clone(), Err(_) if w.path.is_empty() => sk.entities.iter().filter(|(_, e)| !e.construction).map(|(id, _)| *id).collect(), Err(_) => w.path.clone() };
                    // When only parts of the path are swept, the whole path is drawn faintly and the parts in full.
                    let partial = chain.as_ref().ok().filter(|_| !w.spans.is_empty());
                    for id in ids {
                        let line: Vec<Pos2> = sk.polyline(id).into_iter().map(|p| on_screen(app, sk, p)).collect();
                        if partial.is_some() { painter.add(Shape::line(line, Stroke::new(1.4, colors.selected.gamma_multiply(0.45)))); }
                        else if chain.is_ok() { painter.add(Shape::line(line, stroke)); }
                        else { painter.extend(Shape::dashed_line(&line, stroke, 8.0, 5.0)); }
                    }
                    if let Some(chain) = partial {
                        for part in crate::sweep_ui::covered(sk, chain, &w.spans) {
                            painter.add(Shape::line(part.into_iter().map(|p| on_screen(app, sk, p)).collect(), Stroke::new(3.4, colors.selected)));
                        }
                    }
                }
                if let Some((sid, _)) = over_path && w.path_sketch != Some(sid) && let Some(sk) = doc.sketch(sid) {
                    for id in sk.entities.iter().filter(|(_, e)| !e.construction).map(|(id, _)| *id) {
                        painter.add(Shape::line(sk.polyline(id).into_iter().map(|p| on_screen(app, sk, p)).collect(), Stroke::new(2.0, colors.selected.gamma_multiply(0.5))));
                    }
                }
            }
            if let Some(face) = &f.face
                && let Some(plane) = face.plane
            {
                for ring in &face.loops {
                    painter.add(Shape::closed_line(ring.iter().map(|p| to_screen(app, app.body_plane_world(face.body, plane).to_world(*p))).collect(), Stroke::new(2.6, colors.selected)));
                }
            }
            if let Some(a) = extrude_arrow(app) {
                let near = hover.is_some_and(|p| seg_dist(p, a.base, a.tip) < 12.0 || p.distance(a.tip) < 18.0);
                draw_arrow(app, painter, &a, near || app.drag == Drag::Arrow);
            }
            if let Some((sid, p)) = &over
                && !f.pick_axis
                && app.drag != Drag::Arrow
            {
                fill(app, painter, doc.sketch(*sid).unwrap(), p, Color32::from_rgba_unmultiplied(0, 120, 255, 40));
            }
            if let Some(pos) = clicked {
                // The nearest line of the sketch under the click, ignoring points: an axis is
                // often picked near where lines meet, and a construction line is as good an axis.
                let line = f.sketch.and_then(|s| doc.sketch(s)).and_then(|sk| {
                    sk.entities.keys().filter(|e| sk.line(**e).is_some()).map(|e| (*e, path_dist(&path(app, sk, *e), pos))).filter(|(_, d)| *d <= 6.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(e, _)| e)
                });
                if f.pick_to {
                    // Measure from the sketch (or face) plane to the face clicked, along the extrude direction.
                    let from = f.sketch.and_then(|s| doc.sketch(s)).map(|s| s.plane).or(f.face.as_ref().and_then(|face| face.plane.map(|plane| app.body_plane_world(face.body, plane))));
                    if let (Some(plane), Some((_, hit, _))) = (from, pick_body(app, &app.session.built, pos)) {
                        let d = (hit - plane.origin).dot(plane.normal());
                        f.text = format!("{} {}", fr_core::units::fmt_len(d, doc.units), doc.units.name());
                        f.through_all = false;
                        f.pick_to = false;
                    }
                } else if let (true, Some(l)) = (f.pick_axis || f.revolve, line) {
                    // Revolving: any line of the sketch clicked becomes the axis, no Line button needed.
                    f.axis = Axis::Line(l);
                    f.pick_axis = false;
                } else if let (Some(w), Some((sid, entity))) = (&mut f.sweep, over_path) {
                    // A plain click takes the sketch's whole run; Shift-click builds the path a piece at a time.
                    let extend = resp.ctx.input(|i| i.modifiers.shift);
                    if w.path_sketch != Some(sid) { w.path.clear(); }
                    w.path_sketch = Some(sid);
                    if extend {
                        match w.path.iter().position(|e| *e == entity) {
                            Some(i) => { w.path.remove(i); }
                            None => w.path.push(entity),
                        }
                    } else {
                        w.path.clear();
                    }
                } else if let Some((sid, p)) = over {
                    // Both sketches may be closed. Let a region click correct
                    // reversed defaults by releasing its previous path role.
                    if let Some(w) = &mut f.sweep && w.path_sketch == Some(sid) {
                        w.path_sketch = None;
                        w.path.clear();
                    }
                    let extend = resp.ctx.input(|i| i.modifiers.shift);
                    if !extend || f.sketch != Some(sid) { f.profiles.clear(); }
                    f.sketch = Some(sid);
                    f.face = None;
                    match f.profiles.iter().position(|e| *e == p.edges) {
                        Some(i) => { f.profiles.remove(i); }
                        None => f.profiles.push(p.edges),
                    }
                } else if !f.revolve
                    && f.sweep.is_none()
                    && f.editing.is_none()
                    && let Some(mut face) = pick_face(app, pos)
                {
                    if face.plane.is_some() {
                        if let Some(body) = app.session.built.body(face.body) { face.prepare_exact(body); }
                        f.profiles.clear();
                        f.sketch = None;
                        f.face_owner = app.session.built.body(face.body).map_or(0, |b| b.component);
                        f.face = Some(app.local_face(face));
                    } else {
                        app.toast("Only flat faces can be extruded.");
                    }
                }
                app.dialog = Dialog::Feature(f);
            }
        }
        Dialog::Loft(mut l) => {
            let over = hover.and_then(|p| pick_profile(app, &doc, p, &dlg_sketch));
            // Each section filled and numbered in the order it is joined.
            for (i, section) in l.sections.iter().enumerate() {
                let Some(sk) = doc.sketch(section.sketch) else { continue };
                let all = profiles(sk);
                let Some(p) = all.iter().find(|p| p.edges == section.profile) else { continue };
                fill(app, painter, sk, p, Color32::from_rgba_unmultiplied(0, 120, 255, 84));
                let at = on_screen(app, sk, p.centroid());
                painter.circle_filled(at, 10.0, colors.selected);
                painter.text(at, Align2::CENTER_CENTER, (i + 1).to_string(), FontId::proportional(12.0), Color32::WHITE);
            }
            if let Some((sid, p)) = &over {
                fill(app, painter, doc.sketch(*sid).unwrap(), p, Color32::from_rgba_unmultiplied(0, 120, 255, 40));
            }
            if clicked.is_some() && let Some((sid, p)) = over {
                // A sketch gives one section: clicking its chosen region drops it, another region replaces it.
                match l.sections.iter().position(|section| section.sketch == sid) {
                    Some(i) if l.sections[i].profile == p.edges => { l.sections.remove(i); }
                    Some(i) => l.sections[i].profile = p.edges,
                    None => l.sections.push(fr_core::doc::LoftSection { sketch: sid, profile: p.edges }),
                }
                app.dialog = Dialog::Loft(l);
            }
        }
        Dialog::Blend(mut b) => {
            let over = hover.and_then(|p| pick_edge(app, p, b.body));
            if let Some(body) = b.body.and_then(|id| app.session.built.body(id)) {
                for e in body.edges.iter().filter(|e| b.edges.iter().any(|p| path_dist3(e, body.placement.transform_point3(*p)) < 1e-4)) {
                    draw_edge(app, painter, e, colors.selected, 4.0);
                }
            }
            if let Some((_, line)) = &over {
                draw_edge(app, painter, line, colors.accent, 3.0);
            }
            if let (Some(_), Some((body, line))) = (clicked, over) {
                if b.body != Some(body) {
                    b.edges.clear();
                }
                b.body = Some(body);
                b.frame = app.session.built.frame(body);
                match b.edges.iter().position(|p| path_dist3(&line, app.body_point_world(Some(body), *p)) < 1e-4) {
                    Some(i) => {
                        b.edges.remove(i);
                    }
                    None => b.edges.push(app.session.built.body(body).map_or(midpoint(&line), |b| b.to_local(midpoint(&line)))),
                }
                app.dialog = Dialog::Blend(b);
            } else if clicked.is_some() && hover.and_then(|p| pick_body(app, &app.session.built, p)).is_some_and(|hit| app.session.built.body(hit.0).is_some_and(|x| !x.is_exact())) {
                app.toast("That body is a mesh. Only bodies made from sketches have edges that can be filleted or chamfered.");
            }
        }
        Dialog::Shell(mut sh) => {
            for (_, f) in &sh.faces {
                for ring in &f.outline {
                    painter.add(Shape::closed_line(ring.iter().map(|p| to_screen(app, *p)).collect(), Stroke::new(3.0, colors.selected)));
                }
            }
            if let Some(pos) = clicked
                && let Some(face) = pick_face(app, pos)
            {
                if !app.session.built.body(face.body).is_some_and(|b| b.is_exact()) {
                    app.toast("That body is a mesh. Only bodies made from sketches can be shelled.");
                } else {
                    if sh.body != Some(face.body) {
                        sh.faces.clear();
                    }
                    sh.body = Some(face.body);
                    sh.frame = app.session.built.frame(face.body);
                    match sh.faces.iter().position(|f| f.1.tris == face.tris) {
                        Some(i) => {
                            sh.faces.remove(i);
                        }
                        None => sh.faces.push((app.session.built.body(face.body).map_or(face.at, |b| b.to_local(face.at)), face)),
                    }
                    app.dialog = Dialog::Shell(sh);
                }
            }
        }
        Dialog::Hole(mut h) => {
            for at in &h.at {
                let c = to_screen(app, app.body_point_world(h.body, *at));
                painter.circle_stroke(c, 5.0, Stroke::new(2.0, colors.selected));
                painter.line_segment([c - vec2(8.0, 0.0), c + vec2(8.0, 0.0)], Stroke::new(1.0, colors.selected));
                painter.line_segment([c - vec2(0.0, 8.0), c + vec2(0.0, 8.0)], Stroke::new(1.0, colors.selected));
            }
            if let Some(pos) = clicked {
                // A click on a hole that is already there takes it away again.
                if let Some(i) = h.at.iter().position(|at| to_screen(app, app.body_point_world(h.body, *at)).distance(pos) < 9.0) {
                    h.at.remove(i);
                    app.dialog = Dialog::Hole(h);
                } else if let Some(face) = pick_face(app, pos) {
                    match face.plane {
                        None => app.toast("Holes start on a flat face. Click a flat face of the body."),
                        Some(plane) => {
                            let dir = app.session.built.body(face.body).map_or(-plane.normal(), |b| b.placement.inverse().transform_vector3(-plane.normal()));
                            if h.body != Some(face.body) || h.dir.dot(dir) < 1.0 - 1e-9 {
                                h.at.clear();
                            }
                            (h.body, h.dir) = (Some(face.body), dir);
                            // A sketch point under the cursor puts the hole exactly there, on this face.
                            let snapped = doc
                                .sketches()
                                .filter(|(_, s)| s.visible)
                                .flat_map(|(_, s)| s.points.values().map(|p| s.plane.to_world(*p)))
                                .map(|p| p - plane.normal() * (p - face.at).dot(plane.normal()))
                                .map(|p| (to_screen(app, p).distance(pos), p))
                                .filter(|(d, _)| *d < 9.0)
                                .min_by(|a, b| a.0.total_cmp(&b.0));
                            let point = snapped.map_or(face.at, |s| s.1);
                            h.at.push(app.session.built.body(face.body).map_or(point, |b| b.to_local(point)));
                            app.dialog = Dialog::Hole(h);
                        }
                    }
                }
            }
        }
        Dialog::Measure(mut m) => {
            let show = |p: &Picked, color: Color32, width: f32| match &p.item {
                Item::Point(at) => {
                    painter.circle_filled(to_screen(app, *at), width + 2.0, color);
                }
                Item::Path(line) => draw_edge(app, painter, line, color, width + 1.0),
                Item::Surface(_) => {
                    for ring in &p.outline {
                        painter.add(Shape::closed_line(ring.iter().map(|q| to_screen(app, *q)).collect(), Stroke::new(width, color)));
                    }
                }
            };
            for p in &m.picks {
                show(p, colors.selected, 3.0);
            }
            let over = hover.and_then(|p| pick_item(app, &doc, p));
            if let Some(p) = &over {
                show(p, colors.accent, 2.0);
            }
            if let Some(r) = m.result {
                let (a, b) = (to_screen(app, r.from), to_screen(app, r.to));
                painter.line_segment([a, b], Stroke::new(1.5, colors.dim_ink));
                for end in [a, b] {
                    painter.circle_filled(end, 3.0, colors.dim_ink);
                }
                let u = app.doc().units;
                label(painter, a + (b - a) / 2.0 + vec2(0.0, -14.0), &format!("{} {}", fr_core::units::fmt_len(r.apart.unwrap_or(r.distance), u), u.name()), colors.dim_ink, Some(colors.dim_ink));
            }
            if let (Some(_), Some(p)) = (clicked, over) {
                // A third pick starts a new measurement.
                if m.picks.len() >= 2 {
                    m = Default::default();
                }
                m.picks.push(p);
                m.result = (m.picks.len() == 2).then(|| fr_core::measure::between(&m.picks[0].item, &m.picks[1].item));
                app.dialog = Dialog::Measure(m);
            }
        }
        Dialog::Text(_) => {
            if let Some(origin) = app.text_baseline() {
                let at = to_screen(app, origin);
                painter.line_segment([at - vec2(6.0, 0.0), at + vec2(6.0, 0.0)], Stroke::new(2.0, colors.selected));
                painter.line_segment([at - vec2(0.0, 6.0), at + vec2(0.0, 6.0)], Stroke::new(2.0, colors.selected));
            }
            if let Some(face) = hover.and_then(|pos| pick_face(app, pos)) {
                for ring in &face.outline {
                    painter.add(Shape::closed_line(ring.iter().map(|q| to_screen(app, *q)).collect(), Stroke::new(2.0, colors.accent)));
                }
                if clicked.is_some() && let Err(e) = app.text_on_face(face) { app.toast(e); }
            }
        }
        Dialog::Sculpt(_) => {
            if let Some(pos) = clicked && let Some((id, at, _)) = pick_body(app, &app.session.built, pos) {
                app.sculpt_at(id, at);
            }
        }
        Dialog::Thread(mut t) => {
            if let Some(at) = t.face {
                painter.circle_stroke(to_screen(app, app.body_point_world(t.body, at)), 5.0, Stroke::new(2.0, colors.selected));
            }
            if let Some(pos) = clicked
                && let Some((id, at, _)) = pick_body(app, &app.session.built, pos)
                && let Some(body) = app.session.built.body(id)
            {
                if !body.is_exact() {
                    app.toast("That body is a mesh. Only bodies made from sketches have cylinders that can be threaded.");
                } else {
                    match fr_core::exact::barrel(&body.solids, &[at, at]) {
                        Ok(barrel) => {
                            let across = barrel.radius * 2.0;
                            (t.body, t.face, t.found, t.frame) = (Some(id), Some(body.to_local(at)), Some((across, barrel.internal)), app.session.built.frame(id));
                            t.thread = fr_core::threads::nearest(across, barrel.internal).name.to_owned();
                            app.dialog = Dialog::Thread(t);
                        }
                        Err(_) => app.toast("Click the round side of a rod, or the inside of a hole."),
                    }
                }
            }
        }
        Dialog::Pattern(p) => {
            // Show where the copies will land, so a wrong axis or spacing is obvious before OK.
            if let Some(source) = p.source {
                let key = (app.session.rev, source);
                if app.pattern_at.as_ref().is_none_or(|c| c.0 != key)
                    && let Some(c) = app.doc().tool_center(source, crate::body_ops_ui::source(app))
                {
                    app.pattern_at = Some((key, c));
                }
                if let (Some((_, c)), Ok(kind)) = (app.pattern_at.filter(|c| c.0 == key), p.pattern(&doc)) {
                    let places = fr_core::doc::Pattern { source, kind }.placements().unwrap_or_default();
                    let origin = to_screen(app, c);
                    painter.circle(origin, 5.0, colors.paper, Stroke::new(2.0, colors.accent));
                    let owner = app.doc().feature(source).map_or(0, |f| f.owner);
                    let frame = crate::body_ops_ui::source(app).component_placement(owner);
                    let mut points = vec![origin];
                    for place in places {
                        let next = to_screen(app, frame.transform_point3(place.point(frame.inverse().transform_point3(c))));
                        let index = points.len();
                        let previous = if p.kind == 1 && p.second && index % p.count as usize == 0 { index - p.count as usize } else { index - 1 };
                        painter.line_segment([points[previous], next], Stroke::new(1.5, colors.accent));
                        painter.circle(next, 5.0, colors.accent, Stroke::new(1.5, colors.paper));
                        points.push(next);
                    }
                }
            }
        }
        Dialog::Transform(mut t) => {
            // Clicking another body moves that one instead.
            if let Some(pos) = clicked
                && let Some((id, ..)) = pick_body(app, app.shown(), pos)
                && id != t.body
            {
                t.body = id;
                app.dialog = Dialog::Transform(t);
            }
        }
        Dialog::Combine(mut c) => {
            if let Some(pos) = clicked
                && let Some((id, ..)) = pick_body(app, &app.session.built, pos)
            {
                if c.target.is_none() {
                    c.target = Some(id);
                } else if c.target != Some(id) {
                    toggle(&mut c.tools, id, true);
                }
                app.dialog = Dialog::Combine(c);
            }
        }
        _ => {
            if !consumed && resp.double_clicked_by(PointerButton::Primary)
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let found = doc.sketches().filter(|(_, s)| s.visible).find(|(_, s)| !matches!(hit(app, s, pos), Hit::None)).map(|(f, _)| f.id);
                if let Some(id) = found {
                    app.edit_sketch(id);
                }
            } else if let Some(pos) = clicked {
                // A click picks the face under it; the body is picked in the browser.
                app.sel_face = pick_face(app, pos);
                app.sel_component = None;
                app.sel_body = None;
                app.sel_feature = app.sel_face.as_ref().map(|f| f.body);
            }
        }
    }
}

pub fn viewport(app: &mut App, ui: &mut Ui) {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.interact(rect, ui.id().with("viewport"), Sense::click_and_drag());
    app.vp = rect;
    let blocked = app.command_search.block_input;
    let reference_consumed = !blocked && update_reference_drag(app, ui);
    if app.fit_pending {
        app.fit_pending = false;
        app.fit();
    }
    let graphical_consumed = if !blocked && app.mode==Mode::Model && app.timeline.preview.is_none() {
        crate::primitives::interact(app,ui,&resp) || crate::model_drag::interact(app,ui,&resp) || crate::gizmo::interact(app,ui,&resp)
    } else {false};
    if !blocked && app.timeline.preview.is_none() && !app.reference_drag.is_dragging() && !reference_consumed && !graphical_consumed {
        navigate(app, ui, &resp);
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::Palette::from_ctx(ui.ctx()).background);
    if let Some((sid, sk)) = app.sketch() {
        let plane = app.session.built.sketch_plane(app.doc(), sid).unwrap_or(sk.plane);
        let image = app.reference_drag.preview(sid).or(app.reference_editor.calibration_image()).or(sk.reference.as_ref()).cloned();
        if let Some(image) = image && let Err(error) = app.reference_texture.paint(&painter, &image, plane, app.cam, rect) { app.toast(error); }
    }
    draw_grid(app, &painter);
    draw_bodies(app, ui, &painter);
    let hover = if blocked { None } else { resp.hover_pos() };
    crate::construction_view::draw(app, &painter, hover);
    match app.mode {
        Mode::Sketch(sid) => sketch_mode(app, ui, &resp, &painter, sid, reference_consumed),
        Mode::Model => {
            model_mode(app, &resp, &painter, graphical_consumed);
            crate::model_drag::draw(app,&painter,hover);
            crate::primitives::draw(app,&painter,hover);
            crate::gizmo::draw(app,&painter,hover);
        },
    }
    let hint = match (&app.dialog, app.mode) {
        (Dialog::PickPlane, _) => "Choose an origin plane, click a flat face, or click a construction plane.",
        (Dialog::Plane(d), _) => d.hint(),
        (Dialog::MoveComponent(_), _) => "Drag an axis arrow or rotation ring, or use the parent-relative values. OK applies the move.",
        (Dialog::Remove(_), _) => "Click bodies to remove them at this history step. Earlier copies stay.",
        (Dialog::Split(d), _) if d.picking_body || d.body.is_none() => "Choose the body to split.",
        (Dialog::Split(_), _) => "Choose a flat face or construction plane. XY, XZ and YZ use the body’s component axes.",
        (Dialog::DeleteComponent(_), _) => "Confirm deletion of the component and its contents, or cancel.",
        (Dialog::Feature(f), _) if f.pick_axis => "Click a sketch line to revolve around.",
        (Dialog::Feature(f), _) if f.revolve => "Click a line of the sketch to revolve around it, or a region to choose the profile.",
        (Dialog::Feature(f), _) if f.pick_to => "Click the face the extrude should reach.",
        (Dialog::Feature(f), _) if f.sweep.as_ref().is_some_and(|w| w.path_sketch.is_none()) => "Click a line or curve of the path, drawn in another sketch than the profile.",
        (Dialog::Feature(f), _) if f.sweep.is_some() && f.profiles.is_empty() => "Click a closed region for the profile.",
        (Dialog::Feature(f), _) if f.sweep.is_some() => "Click a region to change the profile or a curve to change the path. Shift-click curves to follow only part of a sketch.",
        (Dialog::Feature(f), _) if f.revolve => "Click a closed region to select it. Shift-click to add or remove regions.",
        (Dialog::Feature(_), _) => "Click a closed region or flat face. Shift-click to add or remove regions. Drag the arrow to set the distance.",
        (Dialog::Primitive(d), _) if d.pick_surface => "Click a flat face or construction plane for placement.",
        (Dialog::Primitive(d), _) if d.placing => "Click to place the primitive. Visible sketch geometry snaps; Alt releases. OK commits the feature.",
        (Dialog::Primitive(_), _) => "Use Place in view, colored arrows/rings, or Position fields. F fits the preview.",
        (Dialog::Loft(l), _) if l.sections.is_empty() => "Click a closed region in the first sketch, then one in each sketch after it, in order.",
        (Dialog::Loft(l), _) if l.sections.len() == 1 => "Click a closed region in the next sketch, drawn on another plane.",
        (Dialog::Loft(_), _) => "Click another region to add a section, or a numbered one to leave it out. Reorder them in the dialog.",
        (Dialog::Pattern(p), _) if p.kind==1 => "Drag a last-copy handle to set the span. Visible sketch geometry snaps along that axis; Alt releases.",
        (Dialog::Pattern(_), _) => "The dots show where each copy will go.",
        (Dialog::Blend(_), _) => "Click edges of a body to add or remove them.",
        (Dialog::Shell(_), _) => "Click the faces to leave open.",
        (Dialog::Hole(_), _) => "Click a flat face to put a hole there; click a hole to take it away. Sketch points snap.",
        (Dialog::Thread(_), _) => "Click the round side of a rod, or the inside of a hole.",
        (Dialog::Sculpt(_), _) => "Click the body to add a brush stroke.",
        (Dialog::Text(_), _) => "Click a flat face to place text, or choose XY, XZ or YZ. The cross marks its baseline origin.",
        (Dialog::Measure(_), _) => "Click two things to measure between: corners and sketch points, edges and sketch lines, or faces.",
        (Dialog::Transform(_), _) => "Drag colored arrows or rotation rings. Shift gives fine moves or 15° rotations. Click another body to move it.",
        (Dialog::Combine(c), _) if c.target.is_none() => "Click the body to keep.",
        (Dialog::Combine(_), _) => "Click the bodies to combine with it.",
        (_, Mode::Sketch(_)) => app.tool.hint(app.clicks.len()),
        _ => "Drag to orbit, Shift-drag or middle-drag to pan, scroll to zoom. Click a face to select it.",
    };
    let reference_hint = if let Mode::Sketch(sid) = app.mode { app.reference_drag.hint(sid) } else { None };
    let hint = app.reference_editor.calibration_hint().or(reference_hint).unwrap_or(hint);
    painter.text(rect.left_bottom() + vec2(12.0, -10.0), Align2::LEFT_BOTTOM, hint, FontId::proportional(12.5), theme::Palette::from_ctx(ui.ctx()).muted);
}
