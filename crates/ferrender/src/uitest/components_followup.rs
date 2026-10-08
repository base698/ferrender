//! Nested component workflows across the app's dialogs, browser and file boundary.
use super::*;
use fr_core::{Id, Op};

fn command(h: &mut H, value: serde_json::Value) -> serde_json::Value {
    h.state_mut().execute(&value).unwrap_or_else(|e|panic!("{value}: {e}"))
}
fn near(a: f64,b: f64) {assert!((a-b).abs()<1e-5,"{a} != {b}");}
fn point(a: DVec3,b: DVec3) {assert!(a.distance(b)<1e-5,"{a:?} != {b:?}");}
fn label(h: &H,id: Id) -> String {format!("{} {}",egui_phosphor::regular::CUBE,h.state().doc().component_name(id))}
fn context(h: &mut H,id: Id,item: &str) {
    h.get_by_label(&label(h,id)).click_secondary();h.run_steps(2);
    h.get_by_label(item).click();h.run_steps(2);
}
fn component(h: &mut H,name: &str) -> Id {
    h.get_by_label("Model").click();h.run_steps(2);
    h.get_by_label("New Component").click();h.run_steps(2);
    let id=h.state().doc().active_component;
    assert_ne!(id,0);
    h.state_mut().rename=None;
    command(h,json!({"op":"edit_feature","feature":id,"name":name}));h.run_steps(2);
    id
}
fn accept(h: &mut H) -> Id {
    h.run_steps(3);
    assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_none()),"invalid preview: {:?}",h.state().preview.as_ref().map(|p|&p.2));
    h.get_by_label("OK").click();h.run_steps(2);
    assert_eq!(h.state().dialog,Dialog::None,"{:?}",h.state().toast);
    h.state().sel_feature.unwrap()
}
fn cube(h: &mut H,position:[f64;3],size:[f64;3],op:Op) -> Id {
    run(h,Action::Primitive(0));
    let Dialog::Primitive(d)=&mut h.state_mut().dialog else {panic!()};
    d.position=position.map(|v|format!("{v} mm"));d.size=size.map(|v|format!("{v} mm"));d.op=op;
    accept(h)
}
fn move_component(h: &mut H,id: Id,translate:[f64;3],rotate:[f64;3]) {
    h.state_mut().move_component_dialog(id);
    let Dialog::MoveComponent(d)=&mut h.state_mut().dialog else {panic!()};
    d.translate=translate.map(|v|format!("{v} mm"));d.rotate=rotate.map(|v|format!("{v} deg"));
    accept(h);
}
fn volume(h: &H,id: Id) -> f64 {h.state().session.built.body(id).unwrap().mesh.volume().abs()}
fn bounds(h: &H,id: Id) -> (DVec3,DVec3) {h.state().session.built.body(id).unwrap().mesh.bbox().unwrap()}

#[test]
fn nested_join_cut_and_selection_keep_active_component_ownership() {
    let mut h=state_harness();
    let root=cube(&mut h,[0.;3],[10.;3],Op::New);
    let parent=component(&mut h,"Parent");
    let parent_body=cube(&mut h,[0.;3],[10.;3],Op::Join);
    let child=component(&mut h,"Child");
    let child_body=cube(&mut h,[0.;3],[10.;3],Op::Join);
    assert_eq!(h.state().doc().feature(child).unwrap().owner,parent);
    assert_eq!(h.state().session.built.bodies.len(),3,"nested Join must not absorb ancestor bodies");
    for (body,owner) in [(root,0),(parent_body,parent),(child_body,child)] {
        assert_eq!(h.state().doc().feature(body).unwrap().owner,owner);
        near(volume(&h,body),1000.);
    }
    let cut=cube(&mut h,[0.;3],[5.,10.,10.],Op::Cut);
    assert_eq!(h.state().doc().feature(cut).unwrap().owner,child);
    near(volume(&h,child_body),500.);
    near(volume(&h,parent_body),1000.);near(volume(&h,root),1000.);
    run(&mut h,Action::Undo);near(volume(&h,child_body),1000.);
    run(&mut h,Action::Redo);near(volume(&h,child_body),500.);

    run(&mut h,Action::ActivateRoot);
    h.get_by_label(&label(&h,child)).click();h.run_steps(2);
    assert_eq!(h.state().sel_component,Some(child));
    assert_eq!(h.state().doc().active_component,0,"selecting a component must not activate it");
    let new_root=cube(&mut h,[30.,0.,0.],[2.;3],Op::New);
    assert_eq!(h.state().doc().feature(new_root).unwrap().owner,0);
    assert_eq!(h.state().session.built.body(new_root).unwrap().component,0);
    assert!(h.state().session.built.errors.is_empty());
}

#[test]
fn nested_rotation_face_pull_follows_target_owner_then_sketch_follows_active_owner() {
    let mut h=state_harness();
    let root=cube(&mut h,[-50.,0.,0.],[10.;3],Op::New);
    let parent=component(&mut h,"Assembly");
    let parent_body=cube(&mut h,[0.;3],[10.;3],Op::New);
    let child=component(&mut h,"Tilted child");
    let child_body=cube(&mut h,[0.;3],[10.;3],Op::New);
    move_component(&mut h,child,[20.,0.,0.],[0.,90.,0.]);
    move_component(&mut h,parent,[100.,0.,0.],[0.,0.,90.]);
    point(bounds(&h,child_body).0,DVec3::new(90.,20.,-10.));
    point(bounds(&h,child_body).1,DVec3::new(100.,30.,0.));
    let stable=[(root,bounds(&h,root)),(parent_body,bounds(&h,parent_body))];
    run(&mut h,Action::ActivateRoot);
    run(&mut h,Action::View("back"));run(&mut h,Action::Fit);
    h.state_mut().cam.target=DVec3::new(95.,25.,-5.);h.state_mut().cam.scale=20.;h.run_steps(2);
    let at=crate::view::to_screen(h.state(),DVec3::new(95.,30.,-5.));click(&mut h,at);
    assert_eq!(h.state().sel_face.as_ref().map(|f|f.body),Some(child_body));
    let before=h.state().doc().clone();
    run(&mut h,Action::Extrude);
    let Dialog::Feature(d)=&mut h.state_mut().dialog else {panic!()};
    assert!(d.face.is_some());assert_eq!(d.face_owner,child);
    d.text="3 mm".into();d.op=Op::Join;
    accept(&mut h);
    assert_eq!(h.state().doc().active_component,0);
    assert!(h.state().doc().features[before.features.len()..].iter().all(|f|f.owner==child),"both the face sketch and extrusion must follow the target body");
    point(bounds(&h,child_body).1,DVec3::new(100.,33.,0.));
    for (body,b) in stable {point(bounds(&h,body).0,b.0);point(bounds(&h,body).1,b.1);}
    run(&mut h,Action::Undo);assert_eq!(h.state().doc(),&before);
    run(&mut h,Action::Redo);near(bounds(&h,child_body).1.y,33.);

    h.state_mut().activate_component(child);h.run_steps(2);
    run(&mut h,Action::View("back"));
    let world=DVec3::new(95.,33.,-5.);
    let at=crate::view::to_screen(h.state(),world);click(&mut h,at);
    run(&mut h,Action::NewSketch);
    let Mode::Sketch(sketch)=h.state().mode else {panic!("moved child face should start a sketch: {:?}",h.state().toast)};
    assert_eq!(h.state().doc().feature(sketch).unwrap().owner,child);
    let plane=h.state().session.built.sketch_plane(h.state().doc(),sketch).unwrap();
    near((world-plane.origin).dot(plane.normal()),0.);
    let center=plane.to_local(world);
    command(&mut h,json!({"op":"add_geometry","sketch":sketch,"items":[{"type":"circle","center":center.to_array(),"radius":2}]}));
    run(&mut h,Action::Extrude);
    let Dialog::Feature(d)=&mut h.state_mut().dialog else {panic!()};
    d.text="2 mm".into();d.op=Op::New;
    let post=accept(&mut h);
    assert_eq!(h.state().session.built.body(post).unwrap().component,child);
    near(bounds(&h,post).0.y,33.);near(bounds(&h,post).1.y,35.);
    let local=h.state().doc().sketch(sketch).unwrap().plane;
    let post_before=bounds(&h,post);
    move_component(&mut h,parent,[110.,0.,0.],[0.,0.,90.]);
    point(bounds(&h,post).0,post_before.0+DVec3::X*10.);
    point(bounds(&h,post).1,post_before.1+DVec3::X*10.);
    assert_eq!(h.state().doc().sketch(sketch).unwrap().plane,local);
    run(&mut h,Action::Undo);point(bounds(&h,post).0,post_before.0);
    assert!(h.state().session.built.errors.is_empty());
}

#[test]
fn hidden_nested_state_survives_open_export_and_subtree_delete_undo_redo() {
    let mut h=state_harness();
    let root=cube(&mut h,[-50.,0.,0.],[10.;3],Op::New);
    let parent=component(&mut h,"Export assembly");
    let parent_body=cube(&mut h,[0.;3],[10.;3],Op::New);
    let child=component(&mut h,"Export child");
    let child_body=cube(&mut h,[20.,0.,0.],[4.;3],Op::New);
    move_component(&mut h,parent,[100.,0.,0.],[0.,0.,90.]);
    context(&mut h,parent,"Hide");
    assert_eq!(h.state().session.visible_bodies().map(|b|b.id).collect::<Vec<_>>(),vec![root]);
    assert_eq!(h.state().session.built.bodies.len(),3,"hidden parts remain built");
    let file=out_dir().join("component-followup-hidden.ferr");
    fr_core::io::save(h.state().doc(),&file).unwrap();
    h.state_mut().open_path(&file);h.run_steps(3);
    assert_eq!(h.state().doc().active_component,child);
    assert!(!h.state().session.dirty);
    assert!(!h.state().session.built.component_visible(parent));
    assert!(!h.state().session.built.component_visible(child));
    let stl=out_dir().join("component-followup-visible.stl");
    command(&mut h,json!({"op":"export_stl","path":stl}));
    let mesh=fr_core::io::parse_stl(&std::fs::read(&stl).unwrap(),fr_core::Unit::Mm).unwrap();
    point(mesh.bbox().unwrap().0,DVec3::new(-50.,0.,0.));
    point(mesh.bbox().unwrap().1,DVec3::new(-40.,10.,10.));
    near(mesh.volume().abs(),1000.);
    context(&mut h,parent,"Show");
    context(&mut h,child,"Hide");
    assert_eq!(h.state().session.visible_bodies().count(),2);
    let step=command(&mut h,json!({"op":"export_step","path":out_dir().join("component-followup-parent.step"),"component":parent}));
    assert_eq!(step["solids"],1,"hidden child must be absent from a filtered parent export");
    let before=h.state().doc().clone();
    let b=bounds(&h,parent_body);let c=bounds(&h,child_body);
    context(&mut h,parent,"Delete");
    assert_eq!(h.state().dialog,Dialog::DeleteComponent(parent));
    h.get(egui_kittest::kittest::By::new().role(egui::accesskit::Role::Button).label("Delete Component")).click();h.run_steps(2);
    assert_eq!(h.state().doc().active_component,0);
    assert!(h.state().doc().feature(parent).is_none() && h.state().doc().feature(child).is_none());
    assert_eq!(h.state().session.built.bodies.len(),1);
    assert!(h.state().session.built.body(root).is_some());
    run(&mut h,Action::Undo);
    assert_eq!(h.state().doc(),&before);
    assert_eq!(h.state().doc().active_component,child);
    assert!(h.state().session.built.component_visible(parent) && !h.state().session.built.component_visible(child));
    point(bounds(&h,parent_body).0,b.0);point(bounds(&h,child_body).1,c.1);
    run(&mut h,Action::Redo);
    assert_eq!(h.state().session.built.bodies.len(),1);
    assert_eq!(h.state().doc().active_component,0);
    assert!(h.state().session.built.errors.is_empty());
}
