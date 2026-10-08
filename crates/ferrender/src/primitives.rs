//! Editable, component-local solid primitives, using the same feature preview
//! and transaction path as extrusion and revolve.
use egui::{Align2, Context, FontId, Painter, PointerButton, Pos2, Response, Shape, Stroke, Ui};
use fr_core::{Document, FeatureKind, Id, Kind, Op, Plane};
use fr_core::primitives::{Primitive, PrimitiveShape};
use glam::{DMat3,DQuat,DVec3,EulerRot};
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
    pub placing: bool,
    pub pick_surface: bool,
    /// 0 XY, 1 XZ, 2 YZ, in the primitive owner's frame.
    pub place_plane: usize,
    /// A picked plane in world coordinates; placement is a numeric snapshot.
    pub surface: Option<Plane>,
    pub align: bool,
    cursor: Option<(DVec3,bool)>,
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
            rotate: std::array::from_fn(|_| "0 deg".into()), op: Op::New,
            placing:false,pick_surface:false,place_plane:0,surface:None,align:true,cursor:None }
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
        ui.horizontal(|ui| {
            if ui.selectable_label(d.placing && !d.pick_surface,"Place in view").clicked() {
                d.placing=!d.placing;d.pick_surface=false;d.cursor=None;
            }
            if ui.selectable_label(d.pick_surface,"Pick surface").clicked() {
                d.pick_surface=!d.pick_surface;d.placing=false;d.cursor=None;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Plane");
            for (index,name) in ["XY","XZ","YZ"].into_iter().enumerate() {
                if ui.selectable_label(d.surface.is_none() && d.place_plane==index,name).clicked() {
                    d.place_plane=index;d.surface=None;d.pick_surface=false;d.cursor=None;
                }
            }
            if d.surface.is_some() { ui.label("Picked surface"); }
        });
        ui.checkbox(&mut d.align,"Align to plane").on_hover_text("Point the primitive's +Z along the plane normal. Turn off to keep the current rotation.");
        if d.pick_surface { ui.label("Click a flat face or construction plane, then click a position."); }
        else if d.placing { ui.label("Move to preview; click to keep the position. Alt bypasses sketch snapping."); }
        ui.label("Picked surfaces set coordinates once; they do not stay attached.");
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

fn owner(app:&App,d:&PrimitiveDlg)->Id { d.editing.and_then(|id|app.doc().feature(id).map(|f|f.owner)).unwrap_or(app.doc().active_component) }

fn placement_plane(app:&App,d:&PrimitiveDlg)->Plane {
    d.surface.unwrap_or_else(||[Plane::XY,Plane::XZ,Plane::YZ][d.place_plane.min(2)].transformed(crate::body_ops_ui::source(app).component_placement(owner(app,d))))
}

fn set_position(app:&App,d:&mut PrimitiveDlg,world:DVec3,plane:Plane,snapped:bool)->Result<(),String> {
    let frame=crate::body_ops_ui::source(app).component_placement(owner(app,d));
    let local=frame.inverse().transform_point3(world);
    if !local.is_finite() || local.abs().max_element()>1_000_000.0 { return Err("Choose a position within 1000000 mm of the component origin.".into()); }
    d.position=local.to_array().map(|v|format!("{:.8} mm",if v.abs()<1e-9 {0.0}else{v}));
    if d.align {
        let local=plane.transformed(frame.inverse());
        let basis=DMat3::from_cols(local.x.normalize(),local.y.normalize(),local.normal().normalize());
        let (z,y,x)=DQuat::from_mat3(&basis).normalize().to_euler(EulerRot::ZYX);
        d.rotate=[x,y,z].map(|v|format!("{:.8} deg",if v.abs()<1e-10 {0.0}else{v.to_degrees()}));
    }
    d.cursor=Some((world,snapped));
    Ok(())
}

fn picked_surface(app:&App,pos:Pos2)->Option<Plane> {
    let source=crate::body_ops_ui::source(app);
    let (origin,direction)=crate::view::ray(app,pos);
    let face=crate::view::pick_body(app,source,pos).map(|(id,at,triangle)| {
        let mut face=fr_core::face::Face::pick(source.body(id).unwrap(),triangle);face.at=at;face
    });
    let body_distance=face.as_ref().map(|f|(f.at-origin).dot(direction));
    let plane=source.planes.iter().filter(|(id,p)|source.component_visible(p.component) && app.doc().feature(**id).is_some_and(|f|matches!(&f.kind,FeatureKind::Plane(p) if p.visible))).filter_map(|(_,p)| {
        let normal=p.plane.normal();let denominator=direction.dot(normal);
        if denominator.abs()<1e-9 {return None;}
        let distance=(p.plane.origin-origin).dot(normal)/denominator;
        if distance<0.0 || body_distance.is_some_and(|d|distance>=d-1e-7) {return None;}
        let point=p.plane.to_local(origin+direction*distance);let a=p.plane.to_local(p.corners[0]);let b=p.plane.to_local(p.corners[2]);
        (point.cmpge(a.min(b)).all() && point.cmple(a.max(b)).all()).then_some((distance,p.plane))
    }).min_by(|a,b|a.0.total_cmp(&b.0));
    plane.map(|(_,p)|p).or_else(||face.and_then(|f|f.plane))
}

/// Browser plane picking uses the same prefix scene as viewport face picking.
pub fn choose_plane(app:&mut App,id:Id) {
    let Dialog::Primitive(mut d)=app.dialog.clone() else {return;};
    let Some(plane)=crate::body_ops_ui::source(app).planes.get(&id).map(|p|p.plane) else {app.toast("That plane is unavailable before this primitive.");return;};
    d.surface=Some(plane);d.pick_surface=false;d.placing=true;d.cursor=None;
    app.dialog=Dialog::Primitive(d);
}

/// Update only the dialog preview. A canvas click holds the coordinates; OK
/// remains the one document edit. Secondary/middle navigation stays available.
pub fn interact(app:&mut App,ui:&Ui,resp:&Response)->bool {
    let Dialog::Primitive(mut d)=app.dialog.clone() else {return false;};
    if !d.placing && !d.pick_surface {return false;}
    let consumed=resp.hovered() && ui.input(|i|i.pointer.primary_down() || i.pointer.button_released(PointerButton::Primary));
    let Some(pos)=resp.hover_pos() else {return consumed;};
    let clicked=resp.clicked_by(PointerButton::Primary);
    if d.pick_surface {
        if clicked {
            if let Some(plane)=picked_surface(app,pos) {
                d.surface=Some(plane);d.pick_surface=false;d.placing=true;
                let (o,v)=crate::view::ray(app,pos);
                if let Some(p)=plane.ray_hit(o,v) {let _=set_position(app,&mut d,plane.to_world(p),plane,false);}
            } else {app.toast("Choose a visible flat face or construction plane.");}
        }
    } else {
        let plane=placement_plane(app,&d);let (o,v)=crate::view::ray(app,pos);
        if let Some(p)=plane.ray_hit(o,v) {
            let snapped=(!ui.input(|i|i.modifiers.alt)).then(||crate::model_drag::sketch_target(app,pos,Some(plane))).flatten();
            let world=snapped.unwrap_or_else(||plane.to_world(p));
            match set_position(app,&mut d,world,plane,snapped.is_some()) {
                Ok(())=>if clicked {d.placing=false;},
                Err(error)=>if clicked {app.toast(error);},
            }
        } else if clicked {app.toast("The placement plane is edge-on. Orbit the view before placing the primitive.");}
    }
    let next=Dialog::Primitive(d);
    if app.dialog!=next {app.dialog=next;ui.ctx().request_repaint();}
    if resp.hovered() {ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);}
    consumed || clicked
}

pub fn draw(app:&App,painter:&Painter,hover:Option<Pos2>) {
    let Dialog::Primitive(d)=&app.dialog else {return;};
    if !d.placing && !d.pick_surface {return;}
    let color=crate::theme::Palette::from_ctx(painter.ctx()).accent;
    if d.pick_surface {
        if let Some(pos)=hover && let Some(plane)=picked_surface(app,pos) {
            let (o,v)=crate::view::ray(app,pos);
            if let Some(p)=plane.ray_hit(o,v) {
                let at=plane.to_world(p);let radius=24.0/app.cam.scale;
                let ring=[(-1.0,-1.0),(1.0,-1.0),(1.0,1.0),(-1.0,1.0)].map(|(x,y)|crate::view::to_screen(app,at+plane.x*x*radius+plane.y*y*radius));
                painter.add(Shape::closed_line(ring.to_vec(),Stroke::new(2.0,color)));
            }
        }
    } else if let Some((world,snapped))=d.cursor {
        let plane=placement_plane(app,d);let point=crate::view::to_screen(app,world);
        for axis in [plane.x,plane.y] {painter.line_segment([crate::view::to_screen(app,world-axis*12.0/app.cam.scale),crate::view::to_screen(app,world+axis*12.0/app.cam.scale)],Stroke::new(1.5,color));}
        painter.circle_stroke(point,5.0,Stroke::new(2.0,color));
        painter.text(point+egui::vec2(12.0,14.0),Align2::LEFT_TOP,if snapped {"On sketch · click to place"}else{"Click to place"},FontId::proportional(12.0),color);
    }
}
