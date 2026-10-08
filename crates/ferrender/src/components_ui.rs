//! Component browser, activation, and rigid placement controls.
use egui::{Color32, Context, RichText, Ui};
use egui_phosphor::regular as icon;
use fr_core::{Document, FeatureKind, Id, Kind};
use fr_core::components::Placement;
use fr_core::planes::OriginPlane;
use serde_json::json;
use crate::app::{Action, App, Dialog, Mode};

#[derive(Clone, Debug, PartialEq)]
pub struct MoveDlg {
    pub component: Id,
    pub translate: [String; 3],
    pub rotate: [String; 3],
}

impl MoveDlg {
    pub fn apply(&self, doc: &mut Document) -> Result<Id, String> {
        let mut value = |s: &str, kind| doc.enter(if s.trim().is_empty() { "0" } else { s }, kind);
        let translate = [value(&self.translate[0], Kind::Length)?, value(&self.translate[1], Kind::Length)?, value(&self.translate[2], Kind::Length)?];
        let rotate = [value(&self.rotate[0], Kind::Angle)?, value(&self.rotate[1], Kind::Angle)?, value(&self.rotate[2], Kind::Angle)?];
        doc.move_component(self.component, Placement { translate, rotate })?;
        Ok(self.component)
    }
}

impl App {
    /// Display copies carry a world plane; their 2D coordinates remain local.
    pub fn world_sketch(&self, id: Id) -> Option<fr_core::Sketch> {
        let mut sketch = self.doc().sketch(id)?.clone();
        let owner = self.doc().feature(id)?.owner;
        sketch.plane = self.session.built.sketch_plane(self.doc(), id).unwrap_or_else(|| sketch.plane.transformed(self.session.built.component_placement(owner)));
        Some(sketch)
    }

    pub fn local_face(&self, mut face: fr_core::face::Face) -> fr_core::face::Face {
        if let Some(body) = self.text_source().body(face.body) {
            face.plane = face.plane.map(|p| body.plane_to_local(p));
            face.at = body.to_local(face.at);
            for ring in &mut face.outline { for point in ring { *point = body.to_local(*point); } }
        }
        face
    }

    pub fn body_point_world(&self, body: Option<Id>, point: glam::DVec3) -> glam::DVec3 {
        body.and_then(|id| self.session.built.body(id)).map_or(point, |b| b.placement.transform_point3(point))
    }

    pub fn body_plane_world(&self, body: Id, plane: fr_core::Plane) -> fr_core::Plane {
        self.session.built.body(body).map_or(plane, |b| plane.transformed(b.placement))
    }

    pub fn body_visible(&self, body: &fr_core::Body) -> bool {
        !self.doc().hidden_bodies.contains(&body.id) && self.shown().component_visible(body.component)
    }

    pub fn activate_component(&mut self, id: Id) {
        self.finish_sketch();
        self.cancel_tool();
        self.dialog = Dialog::None;
        match self.session.edit(|doc| doc.activate_component(id)) {
            Ok(()) => { self.sel_component = Some(id); self.sel_body = None; self.sel_face = None; self.refresh(); }
            Err(error) => self.toast(error),
        }
    }

    pub fn new_component(&mut self, parent: Id) {
        self.finish_sketch();
        self.cancel_tool();
        self.dialog = Dialog::None;
        match self.session.edit(|doc| doc.create_component(None, parent, true)) {
            Ok(id) => {
                self.sel_component = Some(id);
                self.sel_feature = Some(id);
                self.sel_body = None;
                self.sel_face = None;
                self.rename = self.doc().feature(id).map(|f| (id, f.name.clone()));
                self.refresh();
            }
            Err(error) => self.toast(error),
        }
    }

    pub fn move_component_dialog(&mut self, id: Id) {
        if id == 0 { self.toast("The root component cannot be moved."); return; }
        self.finish_sketch();
        let Some(feature) = self.doc().feature(id) else { return; };
        let FeatureKind::Component(component) = &feature.kind else { return; };
        self.dialog = Dialog::MoveComponent(MoveDlg { component: id,
            translate: component.placement.translate.each_ref().map(|v| v.expr.clone()),
            rotate: component.placement.rotate.each_ref().map(|v| v.expr.clone()) });
    }

    pub fn delete_component_confirmed(&mut self, id: Id) {
        self.dialog = Dialog::None;
        self.cancel_tool();
        match self.session.edit(|doc| doc.delete_feature(id)) {
            Ok(_) => { self.sel_component = None; self.sel_feature = None; self.sel_body = None; self.refresh(); }
            Err(error) => self.toast(error),
        }
    }
}

pub fn move_dialog(app: &mut App, ctx: &Context, mut d: MoveDlg) {
    crate::panels::dialog_window(app, "Move Component").show(ctx, |ui| {
        ui.label(app.doc().component_name(d.component));
        ui.label("Position and rotation relative to its parent component.");
        egui::Grid::new("component-placement").show(ui, |ui| {
            for (axis, value) in ["Translate X", "Translate Y", "Translate Z"].into_iter().zip(&mut d.translate) { crate::panels::value_row(app, ui, axis, value, Kind::Length); }
            for (axis, value) in ["Rotate X", "Rotate Y", "Rotate Z"].into_iter().zip(&mut d.rotate) { crate::panels::value_row(app, ui, axis, value, Kind::Angle); }
        });
        app.dialog = Dialog::MoveComponent(d.clone());
        crate::panels::confirm(app, ui, "OK");
    });
}

pub fn delete_dialog(app: &mut App, ctx: &Context, id: Id) {
    crate::panels::dialog_window(app, "Delete Component").show(ctx, |ui| {
        let features = app.doc().features.iter().filter(|f| f.id != id && app.doc().component_contains(id, f.owner)).count();
        let bodies = app.session.built.bodies.iter().filter(|b| app.doc().component_contains(id, b.component)).count();
        ui.label(format!("Delete {} and its {features} features and {bodies} bodies?", app.doc().component_name(id)));
        ui.label("Undo restores the component and everything it contains.");
        ui.horizontal(|ui| {
            if ui.button("Delete Component").clicked() { app.delete_component_confirmed(id); }
            if ui.button("Cancel").clicked() { app.dialog = Dialog::None; }
        });
    });
}

pub fn color(doc: &Document, owner: Id) -> Color32 {
    let colors = [Color32::from_rgb(65, 148, 214), Color32::from_rgb(219, 145, 57), Color32::from_rgb(78, 168, 123), Color32::from_rgb(176, 112, 193), Color32::from_rgb(208, 100, 118), Color32::from_rgb(79, 169, 176)];
    let index = if owner == 0 { 0 } else { 1 + doc.features.iter().filter(|f| matches!(f.kind, FeatureKind::Component(_))).position(|f| f.id == owner).unwrap_or(0) };
    colors[index % colors.len()]
}

fn component_menu(app: &mut App, ui: &mut Ui, id: Id) {
    if ui.button("New Component").clicked() { app.new_component(id); ui.close(); }
    if id == 0 { return; }
    if ui.button("Activate").clicked() { app.activate_component(id); ui.close(); }
    if ui.button("Rename").clicked() { app.rename = app.doc().feature(id).map(|f| (id, f.name.clone())); ui.close(); }
    if ui.button("Move").clicked() { app.move_component_dialog(id); ui.close(); }
    let visible = app.doc().feature(id).is_some_and(|f| matches!(&f.kind, FeatureKind::Component(c) if c.visible));
    if ui.button(if visible { "Hide" } else { "Show" }).clicked() { let _ = app.execute(&json!({"op":"set_visible","id":id,"visible":!visible})); ui.close(); }
    if ui.button("Delete").clicked() { app.delete_feature(id); ui.close(); }
}

pub fn browser(app: &mut App, ui: &mut Ui) {
    let colors = crate::theme::Palette::from_ctx(ui.ctx());
    ui.label(RichText::new("BROWSER").size(10.0).color(colors.muted));
    egui::ScrollArea::vertical().show(ui, |ui| { component_node(app, ui, 0); });
}

fn component_node(app: &mut App, ui: &mut Ui, id: Id) {
    let colors = crate::theme::Palette::from_ctx(ui.ctx());
    let name = if id == 0 { app.doc_name() } else { app.doc().component_name(id) };
    let active = app.doc().active_component == id;
    let visible = id == 0 || app.doc().feature(id).is_some_and(|f| matches!(&f.kind, FeatureKind::Component(c) if c.visible));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(("component", id)), true)
        .show_header(ui, |ui| {
            if ui.radio(active, "").on_hover_text(format!("Activate {name}")).clicked() { app.activate_component(id); }
            if id != 0 && crate::panels::eye(ui, visible) { let _ = app.execute(&json!({"op":"set_visible","id":id,"visible":!visible})); }
            let text = RichText::new(format!("{} {name}", icon::CUBE));
            let response = ui.selectable_label(app.sel_component == Some(id), if active { text.strong() } else { text });
            if response.clicked() { app.sel_component = Some(id); app.sel_feature = (id != 0).then_some(id); app.sel_body = None; app.sel_face = None; }
            if response.double_clicked() { app.activate_component(id); }
            response.context_menu(|ui| component_menu(app, ui, id));
        }).body(|ui| {
            ui.push_id(id, |ui| {
                if id == 0 {
                    egui::CollapsingHeader::new(format!("{} Document Settings", icon::GEAR)).default_open(true).show(ui, |ui| {
                        ui.label(format!("Units: {}", app.doc().units.name()));
                        if ui.link(format!("Parameters ({})", app.doc().params.len())).clicked() { app.show_params = true; }
                    });
                }
                egui::CollapsingHeader::new(format!("{} Origin", icon::FOLDER)).show(ui, |ui| {
                    for (name, origin) in [("XY plane (top)", OriginPlane::XY), ("XZ plane (front)", OriginPlane::XZ), ("YZ plane (right)", OriginPlane::YZ)] {
                        if ui.link(name).on_hover_text("Use this component's origin plane").clicked() { app.choose_origin(id, origin); }
                    }
                });
                egui::CollapsingHeader::new(format!("{} Construction", icon::FOLDER)).default_open(true).show(ui, |ui| {
                    let planes: Vec<_> = app.doc().features.iter().filter_map(|f| if f.owner == id { if let FeatureKind::Plane(p) = &f.kind { Some((f.id, f.name.clone(), p.visible, f.suppressed)) } else { None } } else { None }).collect();
                    if planes.is_empty() { ui.small(RichText::new("None yet").color(colors.muted)); }
                    for (plane, name, visible, suppressed) in planes {
                        ui.horizontal(|ui| {
                            if crate::panels::eye(ui, visible) { let _ = app.execute(&json!({"op":"set_visible","id":plane,"visible":!visible})); }
                            let error = app.session.built.errors.get(&plane).cloned();
                            let label = RichText::new(name).color(if error.is_some() { colors.error } else { colors.ink });
                            let response = ui.selectable_label(app.sel_feature == Some(plane), label).on_hover_text(error.unwrap_or("Use as a sketch plane, or double-click to edit".into()));
                            if response.double_clicked() { app.edit_feature(plane); } else if response.clicked() { app.choose_construction_plane(plane); }
                            response.context_menu(|ui| crate::panels::feature_menu(app, ui, plane, suppressed));
                        });
                    }
                });
                egui::CollapsingHeader::new(format!("{} Sketches", icon::FOLDER)).default_open(true).show(ui, |ui| {
                    let sketches: Vec<_> = app.doc().sketches().filter(|(f, _)| f.owner == id).map(|(f, sk)| (f.id, f.name.clone(), sk.visible, f.suppressed)).collect();
                    if sketches.is_empty() { ui.small(RichText::new("None yet").color(colors.muted)); }
                    for (sid, name, visible, suppressed) in sketches {
                        ui.horizontal(|ui| {
                            if crate::panels::eye(ui, visible) { let _ = app.execute(&json!({"op":"set_visible","id":sid,"visible":!visible})); }
                            let response = ui.selectable_label(app.mode == Mode::Sketch(sid), name).on_hover_text("Double-click to edit");
                            if response.double_clicked() { app.edit_sketch(sid); } else if response.clicked() { app.sel_feature = Some(sid); app.sel_component = None; }
                            response.context_menu(|ui| crate::panels::feature_menu(app, ui, sid, suppressed));
                        });
                    }
                });
                egui::CollapsingHeader::new(format!("{} Bodies", icon::FOLDER)).default_open(true).show(ui, |ui| {
                    let bodies: Vec<_> = app.session.built.bodies.iter().filter(|b| b.component == id).map(|b| (b.id, b.name.clone(), !app.doc().hidden_bodies.contains(&b.id))).collect();
                    if bodies.is_empty() { ui.small(RichText::new("None yet").color(colors.muted)); }
                    for (body, name, visible) in bodies {
                        ui.horizontal(|ui| {
                            if crate::panels::eye(ui, visible) { let _ = app.execute(&json!({"op":"set_visible","id":body,"visible":!visible})); }
                            let response = ui.selectable_label(app.sel_body == Some(body), name);
                            if response.clicked() { app.sel_body = Some(body); app.sel_feature = app.doc().feature(body).map(|f| f.id); app.sel_component = None; app.sel_face = None; }
                            response.context_menu(|ui| { if ui.button("Move").clicked() { app.sel_body = Some(body); app.sel_component = None; let ctx = ui.ctx().clone(); app.run(&ctx, Action::Transform); ui.close(); } });
                        });
                    }
                });
                let children: Vec<_> = app.doc().features.iter().filter(|f| f.owner == id && matches!(f.kind, FeatureKind::Component(_)) && app.session.built.components.contains_key(&f.id)).map(|f| f.id).collect();
                for child in children { component_node(app, ui, child); }
            });
        });
}
