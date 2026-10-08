//! Construction-plane dialog inputs and component-aware sketch placement.
use egui::Context;
use fr_core::{Document, FeatureKind, Id, Kind, Plane, Sketch};
use fr_core::planes::{ConstructionPlane, OriginPlane, PlaneKind, PlaneRef, PointRef};
use glam::DVec3;
use crate::app::{App, Dialog};

#[derive(Clone, Debug, PartialEq)]
pub struct PlaneDlg {
    pub editing: Option<Id>,
    pub kind: usize,
    pub base: Option<PlaneRef>,
    pub faces: Vec<PlaneRef>,
    pub points: [Option<PointRef>; 3],
    pub coordinates: [[String; 3]; 3],
    pub typed: [bool; 3],
    pub pick_point: usize,
    pub text: String,
    pub pick_to: bool,
    pub flip: bool,
}

impl Default for PlaneDlg {
    fn default() -> Self {
        Self { editing: None, kind: 0, base: None, faces: Vec::new(), points: [None, None, None],
            coordinates: std::array::from_fn(|_| std::array::from_fn(|_| "0 mm".into())), typed: [false; 3],
            pick_point: 0, text: "10 mm".into(), pick_to: false, flip: false }
    }
}

impl PlaneDlg {
    pub fn from_plane(id: Id, plane: &ConstructionPlane) -> Self {
        let mut d = Self { editing: Some(id), ..Self::default() };
        match &plane.kind {
            PlaneKind::Offset { base, distance } => { d.base = Some(base.clone()); d.text = distance.expr.clone(); }
            PlaneKind::Midplane { a, b, flip } => { d.kind = 1; d.faces = vec![a.clone(), b.clone()]; d.flip = *flip; }
            PlaneKind::ThreePoint { points } => {
                d.kind = 2;
                for (i, p) in points.iter().enumerate() {
                    d.points[i] = Some(p.clone());
                    if let PointRef::World(p) = p { d.typed[i] = true; d.coordinates[i] = p.to_array().map(|v| format!("{v} mm")); }
                }
            }
        }
        d
    }

    pub fn owner(&self, doc: &Document) -> Id { self.editing.and_then(|id| doc.feature(id).map(|f| f.owner)).unwrap_or(doc.active_component) }

    pub fn apply(&self, doc: &mut Document) -> Result<Id, String> {
        let kind = match self.kind {
            0 => PlaneKind::Offset { base: self.base.clone().ok_or("Choose a flat face, origin plane, or construction plane.")?, distance: doc.enter(&self.text, Kind::Length)? },
            1 => {
                if self.faces.len() != 2 { return Err("Choose two different flat faces.".into()); }
                PlaneKind::Midplane { a: self.faces[0].clone(), b: self.faces[1].clone(), flip: self.flip }
            }
            _ => {
                let mut points = Vec::new();
                for i in 0..3 {
                    points.push(if self.typed[i] {
                        PointRef::World(DVec3::new(doc.eval(&self.coordinates[i][0], Kind::Length)?, doc.eval(&self.coordinates[i][1], Kind::Length)?, doc.eval(&self.coordinates[i][2], Kind::Length)?))
                    } else { self.points[i].clone().ok_or_else(|| format!("Choose point {} or type its coordinates.", i + 1))? });
                }
                PlaneKind::ThreePoint { points: points.try_into().unwrap() }
            }
        };
        let id = if let Some(id) = self.editing {
            let FeatureKind::Plane(plane) = &mut doc.feature_mut(id).ok_or("That plane no longer exists.")?.kind else { return Err("That feature is not a construction plane.".into()); };
            plane.kind = kind;
            id
        } else { doc.add_feature(FeatureKind::Plane(ConstructionPlane::new(kind))) };
        fr_core::validation::document(doc)?;
        // Suppression and rollback must not let an invalid edit bypass rebuild validation.
        let feature = doc.feature(id).unwrap();
        if self.editing.is_some() && (feature.suppressed || !doc.features.iter().take(doc.active()).any(|f| f.id == id) || !doc.component_available(feature.owner)) {
            let mut trial = doc.clone();
            let index = trial.features.iter().position(|f| f.id == id).unwrap();
            trial.roll_to(index + 1);
            trial.feature_mut(id).unwrap().suppressed = false;
            let mut owner = feature.owner;
            while owner != 0 {
                let parent = trial.feature_mut(owner).ok_or("The plane's component no longer exists.")?;
                parent.suppressed = false;
                owner = parent.owner;
            }
            let built = trial.rebuild();
            if let Some(error) = built.errors.get(&id) { return Err(error.clone()); }
            if !built.planes.contains_key(&id) { return Err("The plane cannot be resolved at its history position.".into()); }
        }
        Ok(id)
    }

    pub fn hint(&self) -> &'static str {
        match self.kind {
            0 if self.pick_to => "Click the flat face the offset plane should reach.",
            0 if self.base.is_none() => "Choose a flat face, an origin plane in the browser, or a construction plane.",
            0 => "Type a signed distance or drag the arrow. To face measures a gap.",
            1 if self.faces.is_empty() => "Click the first flat face for the midplane.",
            1 => "Click a different flat face. Flip chooses the other angular bisector.",
            _ => "Click three body vertices or sketch points, or type coordinates in the dialog.",
        }
    }
}

pub fn reference_name(doc: &Document, r: &PlaneRef) -> String {
    match r {
        PlaneRef::Origin(p) => format!("{p:?} origin plane"),
        PlaneRef::Plane(id) => doc.feature(*id).map_or(format!("Missing plane {id}"), |f| f.name.clone()),
        PlaneRef::Face { body, .. } => format!("Flat face of body {body}"),
    }
}

impl App {
    pub fn prepare_plane_source(&mut self) {
        let Dialog::Plane(d) = &self.dialog else { self.construction_source = None; return; };
        let Some(id) = d.editing else { self.construction_source = None; return; };
        if self.construction_source.as_ref().is_some_and(|(revision, plane, _)| *revision == self.session.rev && *plane == id) { return; }
        let Some(index) = self.doc().features.iter().position(|f| f.id == id) else { self.construction_source = None; return; };
        let mut before = self.doc().clone();
        before.roll_to(index);
        self.construction_source = Some((self.session.rev, id, before.rebuild()));
    }

    pub fn plane_source(&self) -> &fr_core::Built {
        match (&self.dialog, &self.construction_source) {
            (Dialog::Plane(d), Some((revision, plane, built))) if d.editing == Some(*plane) && *revision == self.session.rev => built,
            _ => crate::body_ops_ui::source(self),
        }
    }

    pub fn modeling_source(&self) -> &fr_core::Built {
        if matches!(self.dialog, Dialog::Plane(_)) { self.plane_source() } else if matches!(self.dialog, Dialog::Remove(_) | Dialog::Split(_) | Dialog::Primitive(_) | Dialog::Pattern(_)) { crate::body_ops_ui::source(self) } else { self.text_source() }
    }

    pub fn choose_plane_reference(&mut self, reference: PlaneRef) {
        let Dialog::Plane(d) = &mut self.dialog else { return; };
        if d.kind == 0 { d.base = Some(reference); d.pick_to = false; }
        else if d.kind == 1 && matches!(reference, PlaneRef::Face { .. }) {
            if d.faces.contains(&reference) { self.toast("Choose two different flat faces."); }
            else { if d.faces.len() == 2 { d.faces.clear(); } d.faces.push(reference); }
        }
    }

    pub fn choose_origin(&mut self, owner: Id, origin: OriginPlane) {
        if matches!(self.dialog, Dialog::Split(_)) {
            let target = if let Dialog::Split(d)=&self.dialog { d.body.and_then(|id|self.doc().body_owner(id)) } else {None};
            if target != Some(owner) {self.toast("Choose an origin plane in the body’s component.");return;}
            crate::body_ops_ui::choose_plane(self,PlaneRef::Origin(origin));
        } else if let Dialog::Plane(d) = &self.dialog {
            if owner != d.owner(self.doc()) { self.toast("Choose an origin plane of the component that owns this plane."); return; }
            self.choose_plane_reference(PlaneRef::Origin(origin));
        } else {
            self.finish_sketch();
            if self.doc().active_component != owner { self.activate_component(owner); }
            self.create_sketch(origin.plane());
        }
    }

    pub fn create_sketch_world(&mut self, plane: Plane) {
        let transform = self.session.built.component_placement(self.doc().active_component).inverse();
        self.create_sketch(plane.transformed(transform));
    }

    pub fn create_sketch_on(&mut self, plane: Id) {
        let Some(resolved) = self.session.built.planes.get(&plane) else { self.toast("That plane is not available at this history position."); return; };
        let local = resolved.plane.transformed(self.session.built.component_placement(self.doc().active_component).inverse());
        if !self.plane_offset.trim().is_empty() {
            match self.doc().eval(&self.plane_offset, Kind::Length) {
                Ok(distance) if distance.abs() < 1e-9 => {}
                Ok(_) => { self.toast("A sketch attaches directly to a construction plane. Clear Offset, or create another offset construction plane first."); return; }
                Err(error) => { self.toast(error); return; }
            }
        }
        self.plane_offset.clear();
        let mut sketch = Sketch::new(local);
        sketch.on = Some(plane);
        match self.session.edit(|doc| Ok(doc.add_feature(FeatureKind::Sketch(sketch)))) {
            Ok(id) => self.edit_sketch(id),
            Err(error) => self.toast(error),
        }
    }

    pub fn choose_construction_plane(&mut self, id: Id) {
        if matches!(self.dialog, Dialog::Plane(_)) { self.choose_plane_reference(PlaneRef::Plane(id)); }
        else if matches!(self.dialog,Dialog::Split(_)) { crate::body_ops_ui::choose_plane(self,PlaneRef::Plane(id)); }
        else if matches!(&self.dialog,Dialog::Primitive(d) if d.pick_surface) {crate::primitives::choose_plane(self,id);}
        else if self.dialog == Dialog::PickPlane { self.create_sketch_on(id); }
        else { self.sel_feature = Some(id); self.sel_component = None; self.sel_body = None; self.sel_face = None; }
    }

    pub fn open_plane_dialog(&mut self) {
        self.finish_sketch();
        let base = self.sel_face.as_ref().filter(|f| f.plane.is_some()).and_then(|f| {
            let body = self.session.built.body(f.body)?;
            Some(PlaneRef::Face { body: body.id, at: body.to_local(f.at), frame: body.local_frame() })
        });
        self.dialog = Dialog::Plane(PlaneDlg { base, ..PlaneDlg::default() });
    }
}

pub fn dialog(app: &mut App, ctx: &Context, mut d: PlaneDlg) {
    crate::panels::dialog_window(app, if d.editing.is_some() { "Edit Construction Plane" } else { "Construction Plane" }).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Kind");
            egui::ComboBox::from_id_salt("construction-kind").selected_text(["Offset", "Midplane", "Three Points"][d.kind.min(2)]).show_ui(ui, |ui| {
                for (kind, name) in ["Offset", "Midplane", "Three Points"].into_iter().enumerate() { ui.selectable_value(&mut d.kind, kind, name); }
            });
        });
        match d.kind {
            0 => {
                ui.label(d.base.as_ref().map_or("Choose a base in the viewport or browser.".into(), |r| reference_name(app.doc(), r)));
                egui::Grid::new("plane-offset").show(ui, |ui| crate::panels::value_row(app, ui, "Distance", &mut d.text, Kind::Length));
                if ui.add_enabled(d.base.is_some(), egui::Button::new("To face").selected(d.pick_to)).clicked() { d.pick_to = !d.pick_to; }
                if ui.button("Clear base").clicked() { d.base = None; d.pick_to = false; }
            }
            1 => {
                for (i, r) in d.faces.iter().enumerate() { ui.label(format!("Face {}: {}", i + 1, reference_name(app.doc(), r))); }
                if d.faces.len() < 2 { ui.label("Click two different flat faces."); }
                ui.checkbox(&mut d.flip, "Flip");
                if ui.button("Clear faces").clicked() { d.faces.clear(); }
            }
            _ => {
                ui.label("Coordinates use this component's X, Y and Z axes.");
                for row in 0..3 {
                    ui.push_id(row, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("Point {}", row + 1));
                            if ui.selectable_label(d.pick_point == row, "Pick").clicked() { d.pick_point = row; }
                            ui.small(if d.typed[row] { "coordinates" } else if d.points[row].is_some() { "linked reference" } else { "choose a point" });
                        });
                        ui.horizontal(|ui| for (axis, value) in ["X", "Y", "Z"].into_iter().zip(&mut d.coordinates[row]) {
                            ui.label(axis);
                            if ui.add(egui::TextEdit::singleline(value).desired_width(72.0)).changed() { d.typed[row] = true; d.points[row] = None; }
                        });
                        if ui.small_button("Use coordinates").clicked() { d.typed[row] = true; d.points[row] = None; }
                    });
                }
            }
        }
        app.dialog = Dialog::Plane(d.clone());
        crate::panels::confirm(app, ui, "OK");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppressed_or_rolled_back_plane_edits_still_reject_collinear_points() {
        for suppressed in [false, true] {
            let mut doc = fr_core::Session::default().doc;
            let kind = ConstructionPlane::new(PlaneKind::Offset { base: PlaneRef::Origin(OriginPlane::XY), distance: doc.value("5 mm", Kind::Length).unwrap() });
            let id = doc.add_feature(FeatureKind::Plane(kind));
            doc.feature_mut(id).unwrap().suppressed = suppressed;
            if !suppressed { doc.roll_to(0); }
            let mut session = fr_core::Session::new(doc);
            let before = session.doc.clone();
            let mut dialog = PlaneDlg { editing: Some(id), kind: 2, typed: [true; 3], ..PlaneDlg::default() };
            dialog.coordinates[1][0] = "10 mm".into();
            dialog.coordinates[2][0] = "20 mm".into();
            let result = session.edit_feature(|doc| dialog.apply(doc).map(|id| (id, id)));
            let error = result.unwrap_err();
            assert!(error.contains("in a line"), "unexpected refusal for suppressed={suppressed}: {error}");
            assert_eq!(session.doc, before, "a failed dialog edit must preserve history and values");
        }
    }

    #[test]
    fn typed_plane_coordinates_accept_units_and_create_a_finite_plane() {
        let mut session = fr_core::Session::default();
        let mut dialog = PlaneDlg { kind: 2, typed: [true; 3], ..PlaneDlg::default() };
        dialog.coordinates[0] = ["0 mm".into(), "0 mm".into(), "1 in".into()];
        dialog.coordinates[1] = ["10 mm".into(), "0 mm".into(), "1 in".into()];
        dialog.coordinates[2] = ["0 mm".into(), "10 mm".into(), "1 in".into()];
        let id = session.edit_feature(|doc| dialog.apply(doc).map(|id| (id, id))).unwrap();
        assert!(session.built.planes[&id].plane.origin.distance(DVec3::new(0.0, 0.0, 25.4)) < 1e-9);
    }
}
