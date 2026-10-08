//! Linear-pattern span handles and shared, visible-sketch placement targets.
use egui::{Painter, Pos2, Stroke, Color32, PointerButton, Response, Ui, Vec2};
use fr_core::{Id, Kind, Plane};
use glam::DVec3;
use crate::app::{App, Dialog, Mode, PatternDlg};

/// One-time numeric target: points have priority over the visible extent of a
/// line/arc/circle. Hidden, failed and future sketches cannot steal a snap.
pub fn sketch_target(app:&App,pos:Pos2,on_plane:Option<Plane>)->Option<DVec3> {
    let source=crate::body_ops_ui::source(app);
    let before=crate::body_ops_ui::editing(&app.dialog).and_then(|id|app.doc().features.iter().position(|f|f.id==id)).unwrap_or(app.doc().active());
    let mut points=Vec::new();let mut edges=Vec::new();
    for feature in app.doc().features.iter().take(before) {
        let fr_core::FeatureKind::Sketch(sk)=&feature.kind else {continue};
        if feature.suppressed || !sk.visible || source.errors.contains_key(&feature.id) || !source.component_visible(feature.owner) {continue;}
        let Some(plane)=source.sketch_plane(app.doc(),feature.id) else {continue};
        let accept=|p:DVec3|on_plane.is_none_or(|plane|(p-plane.origin).dot(plane.normal()).abs()<1e-5);
        for p in sk.points.values() {let world=plane.to_world(*p);let distance=crate::view::to_screen(app,world).distance(pos);if distance<=10. && accept(world) {points.push((distance,world));}}
        for entity in sk.entities.keys() {
            if let Some(c)=crate::sketch_capture::project_entity(sk,*entity,pos,|p|crate::view::to_screen(app,plane.to_world(p))) {
                let world=plane.to_world(c.point);if c.distance<=10. && accept(world){edges.push((c.distance,world));}
            }
        }
    }
    points.into_iter().min_by(|a,b|a.0.total_cmp(&b.0)).or_else(||edges.into_iter().min_by(|a,b|a.0.total_cmp(&b.0))).map(|(_,p)|p)
}

#[derive(Clone, Debug, PartialEq)]
struct Key { revision:u64, editing:Option<Id>,source:Option<Id>,axis:usize,count:u32,second:bool,axis2:usize,count2:u32 }
fn key(app:&App,p:&PatternDlg)->Key {Key{revision:app.session.rev,editing:p.editing,source:p.source,axis:p.axis,count:p.count,second:p.second,axis2:p.axis2,count2:p.count2}}
#[derive(Clone,Debug)]
struct Gesture {key:Key,index:usize,start:Pos2,origin:DVec3,axis:DVec3,screen_axis:Vec2,span:f64,count:u32,camera:fr_core::render::Camera,expected:[String;2],moved:bool}
#[derive(Default,Debug)]
pub struct PatternDrag {gesture:Option<Gesture>,target:Option<DVec3>,blocked:bool}
impl PatternDrag {pub fn clear(&mut self){let blocked=self.blocked||self.gesture.is_some();*self=Self{blocked,..Self::default()};}}
#[derive(Clone,Debug)]
pub(crate) struct Handle {pub index:usize,pub origin:DVec3,pub axis:DVec3,pub screen:Pos2,pub span:f64,pub count:u32,pub screen_axis:Vec2,pub label:&'static str}
pub(crate) fn handles(app:&App)->Vec<Handle> {
    let Dialog::Pattern(p)=&app.dialog else{return vec![]};
    if p.kind!=1{return vec![];}
    let Some(source)=p.source else{return vec![]};
    let built=crate::body_ops_ui::source(app);
    let center=app.pattern_at.filter(|(k,_)|*k==(app.session.rev,source)).map(|(_,c)|c).or_else(||app.doc().tool_center(source,built));
    let Some(origin)=center else{return vec![]};
    let owner=app.doc().feature(source).map_or(0,|f|f.owner);
    let frame=built.component_placement(owner);
    let choices=[(p.axis,p.count,&p.text),(p.axis2,p.count2,&p.text2)];
    choices.into_iter().take(if p.second{2}else{1}).enumerate().filter_map(|(index,(axis,count,text))|{
        if count<2 || axis>2 {return None;}
        let expression=text.split_once('=').map_or(text.as_str(),|(_,rhs)|rhs);
        let spacing=app.doc().eval(expression,Kind::Length).ok()?;
        let axis_world=frame.transform_vector3([DVec3::X,DVec3::Y,DVec3::Z][axis]);
        let a=crate::view::to_screen(app,origin);let screen_axis=crate::view::to_screen(app,origin+axis_world)-a;
        if screen_axis.length()<0.15 {return None;}
        let span=spacing*(count-1) as f64;
        let screen_span=screen_axis*span as f32;
        let screen=a+if screen_span.length()<32. {screen_axis.normalized()*32.*if span<0.{-1.}else{1.}}else{screen_span};
        Some(Handle{index,origin,axis:axis_world,screen,span,count,screen_axis,label:["X","Y","Z"][axis]})
    }).collect()
}
pub fn interact(app:&mut App,ui:&Ui,resp:&Response)->bool {
    let (pos,pressed,down,released,alt)=ui.input(|i|(i.pointer.interact_pos(),i.pointer.button_pressed(PointerButton::Primary),i.pointer.button_down(PointerButton::Primary),i.pointer.button_released(PointerButton::Primary),i.modifiers.alt));
    if app.pattern_drag.blocked {if !down{app.pattern_drag.blocked=false;}return down||released;}
    let valid=matches!(&app.dialog,Dialog::Pattern(p) if p.kind==1) && app.mode==Mode::Model && app.timeline.preview.is_none();
    if !valid {let active=app.pattern_drag.gesture.is_some();app.pattern_drag.clear();return active;}
    let current_key=if let Dialog::Pattern(p)=&app.dialog{key(app,p)}else{unreachable!()};
    if app.pattern_drag.gesture.as_ref().is_some_and(|g|g.key!=current_key || g.camera!=app.cam || matches!(&app.dialog,Dialog::Pattern(p) if g.expected != [p.text.clone(),p.text2.clone()])){app.pattern_drag.clear();return down||released;}
    if app.pattern_drag.gesture.is_none() && pressed && resp.hovered() && let Some(pos)=pos {
        if let Some(h)=handles(app).into_iter().filter(|h|h.screen.distance(pos)<=13.).min_by(|a,b|a.screen.distance(pos).total_cmp(&b.screen.distance(pos))) {
            app.pattern_drag.gesture=Some(Gesture{key:current_key,index:h.index,start:pos,origin:h.origin,axis:h.axis,screen_axis:h.screen_axis,span:h.span,count:h.count,camera:app.cam,expected:if let Dialog::Pattern(p)=&app.dialog{[p.text.clone(),p.text2.clone()]}else{unreachable!()},moved:false});
            app.drag=crate::app::Drag::None;
        }
    }
    let Some(mut g)=app.pattern_drag.gesture.take() else{return false};
    if let Some(pos)=pos && (g.moved || pos.distance(g.start)>=3.) {
        g.moved=true;
        let target=(!alt).then(||sketch_target(app,pos,None)).flatten();
        let span=target.map_or(g.span+(pos-g.start).dot(g.screen_axis) as f64/g.screen_axis.length_sq() as f64,|target|(target-g.origin).dot(g.axis));
        let spacing=span/(g.count-1) as f64;
        if spacing.is_finite() && spacing.abs()<=1_000_000. {
            if let Dialog::Pattern(p)=&mut app.dialog {let text=format!("{spacing:.9} mm");if g.index==0{p.text=text;}else{p.text2=text;}g.expected=[p.text.clone(),p.text2.clone()];}
            app.pattern_drag.target=target;
        }
    }
    if released || !down {app.pattern_drag.target=None;}else{app.pattern_drag.gesture=Some(g);}
    ui.ctx().request_repaint();
    true
}
pub fn draw(app:&App,painter:&Painter,hover:Option<Pos2>) {
    let color=Color32::from_rgb(241,159,44);
    for h in handles(app) {
        let start=crate::view::to_screen(app,h.origin);
        let hot=hover.is_some_and(|p|p.distance(h.screen)<13.) || app.pattern_drag.gesture.as_ref().is_some_and(|g|g.index==h.index);
        painter.line_segment([start,h.screen],Stroke::new(if hot{3.}else{2.},color));
        painter.rect_filled(egui::Rect::from_center_size(h.screen,egui::vec2(13.,13.)),2.,color);
        painter.text(h.screen+egui::vec2(10.,-10.),egui::Align2::LEFT_BOTTOM,format!("{} last · drag",h.label),egui::FontId::proportional(12.),color);
        if app.pattern_drag.gesture.as_ref().is_some_and(|g|g.index==h.index) && let Some(target)=app.pattern_drag.target {
            let at=crate::view::to_screen(app,target);
            painter.circle_stroke(at,7.,Stroke::new(2.,color));
            painter.extend(egui::Shape::dashed_line(&[at,crate::view::to_screen(app,h.origin+h.axis*h.span)],Stroke::new(1.5,color),5.,3.));
            painter.text(at+egui::vec2(12.,14.),egui::Align2::LEFT_TOP,format!("{} aligned to sketch · Alt releases",h.label),egui::FontId::proportional(12.),color);
        }
    }
}
