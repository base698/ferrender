//! Editable, component-local solid primitives, using the same feature preview
//! and transaction path as extrusion and revolve.
use egui::Context;
use fr_core::{Document, FeatureKind, Id, Kind, Op};
use fr_core::primitives::{Primitive, PrimitiveShape};
use crate::app::{App, Dialog};

pub const NAMES: [&str; 5] = ["Box", "Cylinder", "Sphere", "Cone", "Torus"];

#[derive(Clone, Debug, PartialEq)]
pub struct PrimitiveDlg {
    pub editing: Option<Id>,
    pub kind: usize,
    pub size: [String; 3],
    pub position: [String; 3],
    pub rotate: [String; 3],
    pub op: Op,
}

impl PrimitiveDlg {
    pub fn new(kind: usize) -> Self {
        let kind = kind.min(4);
        let size = match kind {
            3 => ["20 mm", "0 mm", "20 mm"],
            4 => ["15 mm", "5 mm", "0 mm"],
            _ => ["20 mm", "20 mm", "20 mm"],
        }.map(str::to_owned);
        Self { editing: None, kind, size, position: std::array::from_fn(|_| "0 mm".into()),
            rotate: std::array::from_fn(|_| "0 deg".into()), op: Op::New }
    }

    pub fn from_feature(id: Id, primitive: &Primitive) -> Self {
        let (kind, values): (usize, Vec<_>) = match &primitive.shape {
            PrimitiveShape::Box { width, depth, height } => (0, vec![width, depth, height]),
            PrimitiveShape::Cylinder { diameter, height } => (1, vec![diameter, height]),
            PrimitiveShape::Sphere { diameter } => (2, vec![diameter]),
            PrimitiveShape::Cone { bottom_diameter, top_diameter, height } => (3, vec![bottom_diameter, top_diameter, height]),
            PrimitiveShape::Torus { major_radius, tube_radius } => (4, vec![major_radius, tube_radius]),
        };
        let mut dialog = Self::new(kind);
        dialog.editing = Some(id);
        for (field, value) in dialog.size.iter_mut().zip(values) { *field = value.expr.clone(); }
        dialog.position = primitive.position.clone().map(|v| v.expr);
        dialog.rotate = primitive.rotate.clone().map(|v| v.expr);
        dialog.op = primitive.op;
        dialog
    }

    pub fn apply(&self, doc: &mut Document) -> Result<Id, String> {
        let length = |doc: &mut Document, i: usize| doc.enter(self.size[i].as_str(), Kind::Length);
        let shape = match self.kind {
            0 => PrimitiveShape::Box { width: length(doc, 0)?, depth: length(doc, 1)?, height: length(doc, 2)? },
            1 => PrimitiveShape::Cylinder { diameter: length(doc, 0)?, height: length(doc, 1)? },
            2 => PrimitiveShape::Sphere { diameter: length(doc, 0)? },
            3 => PrimitiveShape::Cone { bottom_diameter: length(doc, 0)?, top_diameter: length(doc, 1)?, height: length(doc, 2)? },
            4 => PrimitiveShape::Torus { major_radius: length(doc, 0)?, tube_radius: length(doc, 1)? },
            _ => return Err("Choose a primitive shape.".into()),
        };
        let vector = |doc: &mut Document, fields: &[String; 3], kind| -> Result<_, String> {
            Ok([doc.enter(&fields[0], kind)?, doc.enter(&fields[1], kind)?, doc.enter(&fields[2], kind)?])
        };
        let primitive = Primitive { shape, position: vector(doc, &self.position, Kind::Length)?, rotate: vector(doc, &self.rotate, Kind::Angle)?, op: self.op };
        primitive.validate()?;
        if let Some(id) = self.editing {
            let feature = doc.feature_mut(id).ok_or("That primitive no longer exists.")?;
            let FeatureKind::Primitive(previous) = &feature.kind else { return Err("That feature is not a primitive.".into()); };
            if previous.shape.name() != primitive.shape.name() { return Err("Create a new primitive to use a different shape.".into()); }
            feature.kind = FeatureKind::Primitive(primitive);
            Ok(id)
        } else { Ok(doc.add_feature(FeatureKind::Primitive(primitive))) }
    }
}

pub fn dialog(app: &mut App, ctx: &Context, mut d: PrimitiveDlg) {
    crate::panels::dialog_window(app, if d.editing.is_some() { "Edit Primitive" } else { "Primitive" }).show(ctx, |ui| {
        ui.set_max_width(400.0);
        let owner = d.editing.and_then(|id| app.doc().feature(id).map(|f| f.owner)).unwrap_or(app.doc().active_component);
        ui.label(format!("Component: {}", app.doc().component_name(owner)));
        egui::Grid::new("primitive-dimensions").show(ui, |ui| {
            ui.label("Shape");
            let previous = d.kind;
            if d.editing.is_some() {
                ui.label(NAMES[d.kind.min(4)]).on_hover_text("Create a new primitive to use a different shape.");
            } else {
                egui::ComboBox::from_id_salt("primitive-shape").width(110.0).selected_text(NAMES[d.kind.min(4)]).show_ui(ui, |ui| {
                    for (kind, name) in NAMES.into_iter().enumerate() { ui.selectable_value(&mut d.kind, kind, name); }
                });
            }
            if d.kind != previous { d.size = PrimitiveDlg::new(d.kind).size; }
            ui.end_row();
            let labels: &[&str] = match d.kind {
                0 => &["Width", "Depth", "Height"],
                1 => &["Diameter", "Height"],
                2 => &["Diameter"],
                3 => &["Bottom diameter", "Top diameter", "Height"],
                _ => &["Major radius", "Tube radius"],
            };
            for (label, value) in labels.iter().zip(&mut d.size) { crate::panels::value_row(app, ui, label, value, Kind::Length); }
            crate::panels::op_row(ui, &mut d.op, &Op::ALL);
        });
        ui.separator();
        ui.label(match d.kind {
            0 => "Position is the box's minimum corner before rotation.",
            1 | 3 => "Position is the base center. Height runs along +Z before rotation.",
            2 => "Position is the sphere's center.",
            _ => "Position is the ring's center. Its axis is +Z before rotation.",
        });
        egui::Grid::new("primitive-position").show(ui, |ui| {
            for (label, value) in ["Position X", "Position Y", "Position Z"].into_iter().zip(&mut d.position) {
                crate::panels::value_row(app, ui, label, value, Kind::Length);
            }
        });
        ui.collapsing("Rotation", |ui| {
            egui::Grid::new("primitive-rotation").show(ui, |ui| {
                for (label, value) in ["Rotate X", "Rotate Y", "Rotate Z"].into_iter().zip(&mut d.rotate) {
                    crate::panels::value_row(app, ui, label, value, Kind::Angle);
                }
            });
            ui.label("Rotate X, then Y, then Z around the primitive's position.");
        });
        if d.kind == 3 { ui.label("Set either diameter to 0 for a pointed cone."); }
        if d.kind == 4 { ui.label("Major radius goes to the tube's center; it must exceed tube radius."); }
        ui.label("Uses the component's axes. Sizes accept units and parameters.");
        app.dialog = Dialog::Primitive(d);
        crate::panels::confirm(app, ui, "OK");
    });
}
