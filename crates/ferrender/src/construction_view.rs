//! Plane overlays and picking. Guides are never part of geometric edge picking.
use egui::{Align2, Color32, FontId, Painter, Pos2, Shape, Stroke};
use fr_core::{FeatureKind, Id, Plane};
use fr_core::planes::{PlaneRef, PointRef};
use glam::DVec3;
use crate::app::{App, Dialog};
use crate::view::{pick_body, pick_face, ray, to_screen};

fn shown(app: &App, id: Id, component: Id) -> bool {
    app.shown().component_visible(component) && app.doc().feature(id).is_none_or(|f| matches!(&f.kind, FeatureKind::Plane(p) if p.visible))
}

pub fn pick_plane(app: &App, pos: Pos2) -> Option<(Id, f64)> {
    let (origin, direction) = ray(app, pos);
    let editing = if let Dialog::Plane(d) = &app.dialog { d.editing } else { None };
    app.plane_source().planes.iter().filter(|(id, p)| Some(**id) != editing && shown(app, **id, p.component)).filter_map(|(id, p)| {
        let normal = p.plane.normal();
        let denominator = direction.dot(normal);
        if denominator.abs() < 1e-9 { return None; }
        let distance = (p.plane.origin - origin).dot(normal) / denominator;
        if distance < 0.0 { return None; }
        let at = p.plane.to_local(origin + direction * distance);
        let a = p.plane.to_local(p.corners[0]);
        let b = p.plane.to_local(p.corners[2]);
        let (lo, hi) = (a.min(b), a.max(b));
        (at.cmpge(lo).all() && at.cmple(hi).all()).then_some((*id, distance))
    }).min_by(|a, b| a.1.total_cmp(&b.1))
}

/// A plane wins only if it lies in front of the body's hit, never through it.
pub fn pick_plane_before_face(app: &App, pos: Pos2) -> Option<Id> {
    let (id, distance) = pick_plane(app, pos)?;
    let (origin, direction) = ray(app, pos);
    let body_distance = pick_body(app, app.plane_source(), pos).map(|(_, p, _)| (p - origin).dot(direction));
    body_distance.is_none_or(|d| distance < d - 1e-7).then_some(id)
}

pub fn draw(app: &App, painter: &Painter, hover: Option<Pos2>) {
    let picking = matches!(app.dialog, Dialog::PickPlane | Dialog::Plane(_));
    let over = if picking { hover.and_then(|p| pick_plane_before_face(app, p)) } else { None };
    let editing = if let Dialog::Plane(d) = &app.dialog { d.editing } else { None };
    for (id, p) in &app.shown().planes {
        if !shown(app, *id, p.component) && editing != Some(*id) { continue; }
        let selected = over == Some(*id) || app.sel_feature == Some(*id) || app.doc().feature(*id).is_none() || editing == Some(*id);
        let front = app.cam.basis().0.dot(p.plane.normal()) >= 0.0;
        let color = if selected { Color32::from_rgb(225, 148, 53) } else { crate::theme::Palette::from_ctx(painter.ctx()).accent };
        let points: Vec<_> = p.corners.iter().map(|p| to_screen(app, *p)).collect();
        painter.add(Shape::convex_polygon(points.clone(), color.gamma_multiply(if front { 0.16 } else { 0.08 }), Stroke::new(if selected { 2.0 } else { 1.0 }, color.gamma_multiply(0.7))));
        let name = app.doc().feature(*id).map_or("Plane preview", |f| f.name.as_str());
        painter.text(points[3] + egui::vec2(5.0, -5.0), Align2::LEFT_BOTTOM, name, FontId::proportional(12.0), color);
    }
    let active = app.doc().active_component;
    if active != 0 && app.shown().component_visible(active) {
        let place = app.shown().component_placement(active);
        let origin = to_screen(app, place.transform_point3(DVec3::ZERO));
        let size = 30.0 / app.cam.scale;
        for (axis, name, color) in [(DVec3::X, "X", Color32::from_rgb(230, 80, 90)), (DVec3::Y, "Y", Color32::from_rgb(70, 180, 100)), (DVec3::Z, "Z", Color32::from_rgb(85, 135, 235))] {
            let end = to_screen(app, place.transform_point3(axis * size));
            painter.line_segment([origin, end], Stroke::new(2.0, color));
            painter.text(end, Align2::CENTER_BOTTOM, name, FontId::proportional(11.0), color);
        }
    }
}

pub fn offset_base(app: &App) -> Option<(Plane, glam::DVec2, String)> {
    let Dialog::Plane(d) = &app.dialog else { return None; };
    if d.kind != 0 { return None; }
    let owner = d.owner(app.doc());
    let (plane, points) = app.doc().plane_reference(d.base.as_ref()?, app.plane_source(), owner).ok()?;
    let middle = if points.is_empty() { glam::DVec2::ZERO } else { points.iter().map(|p| plane.to_local(*p)).sum::<glam::DVec2>() / points.len() as f64 };
    Some((plane.transformed(app.plane_source().component_placement(owner)), middle, d.text.clone()))
}

fn pick_point(app: &App, pos: Pos2) -> Option<(PointRef, DVec3)> {
    let mut candidates = Vec::new();
    for body in app.plane_source().bodies.iter().filter(|b| app.body_visible(b)) {
        for point in body.edges.iter().flat_map(|e| e.first().into_iter().chain(e.last())).copied() {
            let distance = to_screen(app, point).distance(pos);
            if distance <= 9.0 { candidates.push((distance, -app.cam.project(point).1, PointRef::Vertex { body: body.id, at: body.to_local(point), frame: body.local_frame() }, point)); }
        }
    }
    let before = match &app.dialog { Dialog::Plane(d) => d.editing.and_then(|id| app.doc().features.iter().position(|f| f.id == id)).unwrap_or(app.doc().active()), _ => app.doc().active() };
    for (feature, sketch) in app.doc().sketches().filter(|(f, sk)| sk.visible && app.plane_source().component_visible(f.owner) && app.doc().features.iter().take(before).any(|earlier| earlier.id == f.id)) {
        let Some(plane) = app.plane_source().sketch_plane(app.doc(), feature.id) else { continue; };
        for (id, p) in &sketch.points {
            let point = plane.to_world(*p);
            let distance = to_screen(app, point).distance(pos);
            if distance <= 9.0 { candidates.push((distance, -app.cam.project(point).1, PointRef::SketchPoint { sketch: feature.id, point: *id }, point)); }
        }
    }
    candidates.into_iter().min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))).map(|(_, _, reference, point)| (reference, point))
}

pub fn interact(app: &mut App, painter: &Painter, hover: Option<Pos2>, clicked: Option<Pos2>) {
    let Dialog::Plane(mut d) = app.dialog.clone() else { return; };
    if d.kind == 2 {
        let owner = d.owner(app.doc());
        let placement = app.plane_source().component_placement(owner);
        for (index, point) in d.points.iter().enumerate() {
            if let Some(point) = point && !d.typed[index]
                && let Ok(point) = app.doc().point_reference(point, app.plane_source(), d.editing.unwrap_or(0), owner) {
                let pos = to_screen(app, placement.transform_point3(point));
                painter.circle_stroke(pos, 6.0, Stroke::new(2.0, crate::theme::Palette::from_ctx(painter.ctx()).accent));
                painter.text(pos + egui::vec2(9.0, -9.0), Align2::LEFT_BOTTOM, format!("{}", index + 1), FontId::proportional(12.0), crate::theme::Palette::from_ctx(painter.ctx()).ink);
            }
        }
        if let Some((reference, point)) = hover.and_then(|p| pick_point(app, p)) {
            painter.circle_filled(to_screen(app, point), 5.0, crate::theme::Palette::from_ctx(painter.ctx()).accent);
            if clicked.is_some() {
                let row = d.pick_point.min(2);
                d.points[row] = Some(reference);
                d.typed[row] = false;
                d.coordinates[row] = placement.inverse().transform_point3(point).to_array().map(|v| format!("{v} mm"));
                d.pick_point = (row + 1).min(2);
            }
        } else if clicked.is_some() { app.toast("Choose an existing sketch point or body vertex, or type coordinates."); }
    } else if let Some(pos) = clicked {
        if d.kind == 0 && !d.pick_to && let Some(plane) = pick_plane_before_face(app, pos) {
            d.base = Some(PlaneRef::Plane(plane));
        } else if let Some(face) = pick_face(app, pos) {
            if face.plane.is_none() { app.toast("Choose a flat face."); return; }
            if d.kind == 0 && d.pick_to {
                if let Some((plane, _, _)) = offset_base(app) {
                    if face.plane.unwrap().normal().dot(plane.normal()).abs() < 1.0 - 1e-6 {
                        app.toast("Choose a face parallel to the base plane.");
                        return;
                    }
                    let distance = (face.at - plane.origin).dot(plane.normal());
                    d.text = format!("{distance} mm");
                    d.pick_to = false;
                }
            } else if let Some(body) = app.plane_source().body(face.body) {
                let reference = PlaneRef::Face { body: body.id, at: body.to_local(face.at), frame: body.local_frame() };
                if d.kind == 0 { d.base = Some(reference); }
                else {
                    if d.faces.len() == 2 { d.faces.clear(); }
                    let owner = d.owner(app.doc());
                    let duplicate = d.faces.iter().any(|old| app.doc().plane_reference(old, app.plane_source(), owner).ok().zip(app.doc().plane_reference(&reference, app.plane_source(), owner).ok()).is_some_and(|((a, _), (b, _))| a.normal().dot(b.normal()).abs() > 1.0 - 1e-7 && (a.origin - b.origin).dot(a.normal()).abs() < 1e-6));
                    if duplicate { app.toast("Choose two different flat faces."); } else { d.faces.push(reference); }
                }
            }
        } else { app.toast("Choose a flat face or visible construction plane."); }
    }
    app.dialog = Dialog::Plane(d);
}
