//! Body operations select from the scene immediately before their history step.
use egui::{Context, Painter, Pos2, Stroke};
use fr_core::{Built, Document, FeatureKind, Id};
use fr_core::body_ops::{Remove, Split};
use fr_core::planes::{OriginPlane, PlaneRef};
use crate::app::{App, Dialog};

#[derive(Clone, Debug, PartialEq)]
pub struct RemoveDlg { pub editing: Option<Id>, pub bodies: Vec<Id> }
#[derive(Clone, Debug, PartialEq)]
pub struct SplitDlg { pub editing: Option<Id>, pub body: Option<Id>, pub plane: Option<PlaneRef>, pub picking_body: bool }

fn put(doc: &mut Document, id: Option<Id>, kind: FeatureKind, owner: Id) -> Result<Id,String> {
    if let Some(id) = id {
        let f = doc.feature_mut(id).ok_or("That feature no longer exists.")?;
        if std::mem::discriminant(&f.kind) != std::mem::discriminant(&kind) { return Err("The feature type changed.".into()); }
        if f.owner != owner { return Err("Choose a body in the feature's original component, or create a new operation.".into()); }
        f.kind = kind;
        fr_core::validation::document(doc)?;
        let mut trial=doc.clone();
        let index=trial.features.iter().position(|f|f.id==id).unwrap();
        trial.roll_to(index+1);
        trial.feature_mut(id).unwrap().suppressed=false;
        let mut component=owner;
        while component!=0 {let feature=trial.feature_mut(component).ok_or("The component no longer exists.")?;feature.suppressed=false;component=feature.owner;}
        let built=trial.rebuild();
        if let Some(error)=built.errors.get(&id){return Err(error.clone());}
        Ok(id)
    } else { Ok(doc.add_feature(kind)) }
}
impl RemoveDlg {
    pub fn apply(&self, doc: &mut Document) -> Result<Id,String> {
        let op = Remove { bodies: self.bodies.clone() }; op.validate()?;
        let owner = doc.body_owner(self.bodies[0]).ok_or("The selected body no longer exists.")?;
        put(doc,self.editing,FeatureKind::Remove(op),owner)
    }
}
impl SplitDlg {
    pub fn apply(&self, doc: &mut Document) -> Result<Id,String> {
        let body = self.body.ok_or("Choose a body to split.")?;
        let op = Split { body, plane: self.plane.clone().ok_or("Choose a flat face or a plane to split with.")? }; op.validate()?;
        let owner = doc.body_owner(body).ok_or("The selected body no longer exists.")?;
        put(doc,self.editing,FeatureKind::Split(op),owner)
    }
}
pub fn editing(d: &Dialog) -> Option<Id> { match d { Dialog::Remove(d)=>d.editing,Dialog::Split(d)=>d.editing,Dialog::Primitive(d)=>d.editing,Dialog::Pattern(d)=>d.editing,_=>None } }
pub fn prepare(app: &mut App) {
    let Some(id) = editing(&app.dialog) else { if app.body_ops_source.is_some(){app.pattern_at=None;} app.body_ops_source=None; return; };
    if app.body_ops_source.as_ref().is_some_and(|(rev,f,_)| *rev==app.session.rev && *f==id) { return; }
    let Some(index) = app.doc().features.iter().position(|f|f.id==id) else { app.body_ops_source=None; return; };
    app.pattern_at=None;
    let mut doc=app.doc().clone(); doc.roll_to(index);
    app.body_ops_source=Some((app.session.rev,id,doc.rebuild()));
}
pub fn source(app: &App) -> &Built {
    if let Some((rev,id,built))=&app.body_ops_source && *rev==app.session.rev && editing(&app.dialog)==Some(*id) { built } else { &app.session.built }
}
fn name(app: &App,id:Id)->String { source(app).body(id).map_or(format!("Missing body {id}"),|b|b.name.clone()) }
pub fn choose_body(app: &mut App,id:Id) {
    if source(app).body(id).is_none() { app.toast("That body is not available before this feature."); return; }
    match &mut app.dialog {
        Dialog::Remove(d)=>{ if let Some(i)=d.bodies.iter().position(|b|*b==id) {d.bodies.remove(i);} else {d.bodies.push(id);} }
        Dialog::Split(d)=>{d.body=Some(id);d.picking_body=false;}
        _=>{}
    }
}
pub fn choose_plane(app: &mut App, plane: PlaneRef) { if let Dialog::Split(d)=&mut app.dialog {d.plane=Some(plane);d.picking_body=false;} }
pub fn remove_dialog(app:&mut App,ctx:&Context,mut d:RemoveDlg) {
    crate::panels::dialog_window(app,if d.editing.is_some(){"Edit Remove Body"}else{"Remove Body"}).show(ctx,|ui| {
        ui.set_max_width(380.);
        ui.label("Select bodies in the view or browser. Click again to unselect.");
        ui.label("Earlier features stay in history. Copies made before this step are preserved.");
        let mut remove=None;
        for (i,id) in d.bodies.iter().enumerate() {ui.horizontal(|ui|{ui.label(name(app,*id));if ui.small_button("Unselect").clicked(){remove=Some(i);}});}
        if let Some(i)=remove {d.bodies.remove(i);}
        if d.bodies.is_empty(){ui.label("No bodies selected.");}
        app.dialog=Dialog::Remove(d);
        crate::panels::confirm(app,ui,"OK");
    });
}
pub fn split_dialog(app:&mut App,ctx:&Context,mut d:SplitDlg) {
    crate::panels::dialog_window(app,if d.editing.is_some(){"Edit Split Body"}else{"Split Body"}).show(ctx,|ui| {
        ui.set_max_width(380.);
        ui.horizontal(|ui|{ui.label(d.body.map_or("Choose a body.".into(),|id|name(app,id)));if ui.selectable_label(d.picking_body,"Pick body").clicked(){d.picking_body=true;}});
        ui.label(d.plane.as_ref().map_or("Choose a flat face or construction plane.".into(),|r|crate::construction::reference_name(app.doc(),r)));
        ui.horizontal(|ui| { for (label,p) in [("XY",OriginPlane::XY),("XZ",OriginPlane::XZ),("YZ",OriginPlane::YZ)] {if ui.button(label).clicked(){d.plane=Some(PlaneRef::Origin(p));d.picking_body=false;}} });
        if ui.button("Pick face / plane").clicked(){d.picking_body=false;d.plane=None;}
        ui.label("Origin planes use the body's component axes. The whole flat plane cuts the body; its displayed size does not limit the cut.");
        ui.label("Each piece becomes a separate body. Use Join Bodies to join pieces again.");
        app.dialog=Dialog::Split(d);
        crate::panels::confirm(app,ui,"OK");
    });
}
pub fn interact(app:&mut App,painter:&Painter,hover:Option<Pos2>,clicked:Option<Pos2>) {
    let selected=match &app.dialog {Dialog::Remove(d)=>d.bodies.clone(),Dialog::Split(d)=>d.body.into_iter().collect(),_=>return};
    let color=crate::theme::Palette::from_ctx(painter.ctx()).accent;
    for id in selected { if let Some(body)=source(app).body(id) {for edge in &body.edges {painter.add(egui::Shape::line(edge.iter().map(|p|crate::view::to_screen(app,*p)).collect(),Stroke::new(2.,color)));}} }
    if let Dialog::Split(d)=&app.dialog && let (Some(body),Some(reference))=(d.body,&d.plane)
        && let Some(body)=source(app).body(body)
        && let Ok((plane,_))=app.doc().plane_reference(reference,source(app),body.component) {
        let plane=plane.transformed(source(app).component_placement(body.component));
        let (lo,hi)=body.mesh.bbox().unwrap_or((glam::DVec3::ZERO,glam::DVec3::ONE));
        let center=plane.to_local((lo+hi)*0.5);let size=(hi-lo).length().max(10.)*0.65;
        let points=[(-1.,-1.),(1.,-1.),(1.,1.),(-1.,1.)].map(|(x,y)|crate::view::to_screen(app,plane.to_world(center+glam::DVec2::new(x,y)*size))).to_vec();
        painter.add(egui::Shape::convex_polygon(points,color.gamma_multiply(0.1),Stroke::new(1.5,color)));
    }
    if let Some(pos)=clicked {
        let pick_body=matches!(&app.dialog,Dialog::Remove(_)) || matches!(&app.dialog,Dialog::Split(d) if d.picking_body || d.body.is_none());
        if pick_body {
            if let Some((id,..))=crate::view::pick_body(app,source(app),pos){choose_body(app,id);}
        } else if let Some(id)=crate::construction_view::pick_plane_before_face(app,pos) {choose_plane(app,PlaneRef::Plane(id));}
        else if let Some(face)=crate::view::pick_face(app,pos) {
            if face.plane.is_none(){app.toast("Choose a flat face or construction plane.");return;}
            if let Some(body)=source(app).body(face.body){let r=PlaneRef::Face{body:body.id,at:body.to_local(face.at),frame:body.local_frame()};choose_plane(app,r);}
        }
    } else if let Some(pos)=hover && let Some((id,..))=crate::view::pick_body(app,source(app),pos) && let Some(body)=source(app).body(id) {
        for edge in &body.edges {painter.add(egui::Shape::line(edge.iter().map(|p|crate::view::to_screen(app,*p)).collect(),Stroke::new(1.,color)));}
    }
}
