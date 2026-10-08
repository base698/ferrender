//! Timeline body operations use the geometry immediately before the edited step.
use super::*;
use fr_core::{FeatureKind, Id, Op};
use fr_core::planes::{OriginPlane, PlaneRef};

fn command(h:&mut H,c:serde_json::Value)->serde_json::Value {h.state_mut().execute(&c).unwrap_or_else(|e|panic!("{c}: {e}"))}
fn feature(c:&serde_json::Value)->Id {c["feature"].as_u64().unwrap() as Id}
fn block(h:&mut H,position:[f64;3],size:[f64;3])->Id {
    feature(&command(h,json!({"op":"primitive","type":"box","width":size[0],"depth":size[1],"height":size[2],"position":position})))
}
fn plane(h:&mut H,distance:f64)->Id {feature(&command(h,json!({"op":"create_plane","kind":"offset","base":"XY","distance":distance})))}
fn near(a:f64,b:f64){assert!((a-b).abs()<1e-5,"{a} != {b}");}
fn volume(h:&H,id:Id)->f64 {h.state().session.built.body(id).unwrap().solids.iter().map(|s|s.volume()).sum()}
fn accept(h:&mut H)->Id {
    h.run_steps(3);
    assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_none()),"preview failed: {:?}",h.state().preview.as_ref().map(|p|&p.2));
    h.get_by_label("OK").click();h.run_steps(2);
    assert_eq!(h.state().dialog,Dialog::None,"apply failed: {:?}",h.state().toast);
    h.state().sel_feature.unwrap()
}
fn top(h:&mut H) {run(h,Action::View("top"));h.state_mut().cam.target=DVec3::ZERO;h.state_mut().cam.scale=8.;h.run_steps(2);}
fn world_click(h:&mut H,p:[f64;3]) {let p=crate::view::to_screen(h.state(),DVec3::from_array(p));click(h,p);}
fn menu(h:&mut H,label:&str) {h.get_by_label("Model").click();h.run_steps(2);h.get_by_label(label).click();h.run_steps(2);}

#[test]
fn remove_body_menu_preserves_mirrored_cone_preview_history_and_save() {
    let mut h=state_harness();
    let cone=feature(&command(&mut h,json!({"op":"primitive","type":"cone","bottom_diameter":10,"height":15,"position":[20,0,0]})));
    let mirror=feature(&command(&mut h,json!({"op":"pattern","feature":cone,"type":"mirror","normal":"x"})));
    let copy=mirror*1000+1;let expected=volume(&h,copy);let before=h.state().doc().clone();
    h.state_mut().sel_body=Some(cone);
    menu(&mut h,"Remove Body");
    assert!(matches!(&h.state().dialog,Dialog::Remove(d) if d.bodies==[cone]));
    assert_eq!(h.state().doc(),&before);
    assert!(h.state().shown().body(cone).is_none() && h.state().shown().body(copy).is_some());
    let removed=accept(&mut h);
    assert!(matches!(&h.state().doc().feature(removed).unwrap().kind,FeatureKind::Remove(r) if r.bodies==[cone]));
    assert!(h.state().doc().feature(cone).is_some() && h.state().doc().feature(mirror).is_some());
    assert_eq!(h.state().session.built.bodies.len(),1);near(volume(&h,copy),expected);
    let saved=fr_core::io::validated_json(h.state().doc()).unwrap();
    let reopened=fr_core::Session::new(fr_core::io::from_json(&saved).unwrap());
    assert!(reopened.built.body(cone).is_none() && reopened.built.body(copy).is_some());
    assert_eq!(reopened.doc,h.state().session.doc);
    run(&mut h,Action::Undo);assert_eq!(h.state().doc(),&before);
    run(&mut h,Action::Redo);assert!(h.state().session.built.body(cone).is_none());
}

#[test]
fn editing_remove_can_select_an_original_body_before_later_transforms() {
    let mut h=state_harness();
    let a=block(&mut h,[-25.,-5.,0.],[10.;3]);let b=block(&mut h,[15.,-5.,0.],[10.;3]);
    let remove=feature(&command(&mut h,json!({"op":"remove_body","body":a})));
    command(&mut h,json!({"op":"transform","body":b,"translate":[100,0,0]}));
    let before=h.state().doc().clone();
    h.state_mut().edit_feature(remove);top(&mut h);
    assert!(h.state().modeling_source().body(a).is_some(),"the removed original must be available while editing its removal");
    near(h.state().modeling_source().body(b).unwrap().mesh.bbox().unwrap().0.x,15.);
    world_click(&mut h,[-20.,0.,10.]);
    assert!(matches!(&h.state().dialog,Dialog::Remove(d) if d.bodies.is_empty()),"a second click must unselect the original");
    world_click(&mut h,[20.,0.,10.]);
    assert!(matches!(&h.state().dialog,Dialog::Remove(d) if d.bodies==[b]),"the other body must be selectable at its pre-transform location");
    assert_eq!(h.state().doc(),&before);
    assert_eq!(accept(&mut h),remove);
    assert!(h.state().session.built.body(a).is_some() && h.state().session.built.body(b).is_none());
    assert_eq!(h.state().doc().features.len(),before.features.len());
    run(&mut h,Action::Undo);assert_eq!(h.state().doc(),&before);
}

#[test]
fn split_origin_pieces_are_individually_selectable_and_join_bodies_restores_the_solid() {
    let mut h=state_harness();let body=block(&mut h,[-10.,-10.,-10.],[20.;3]);
    h.state_mut().sel_body=Some(body);menu(&mut h,"Split Body");
    h.get_by_label("XY").click();h.run_steps(2);
    let split=accept(&mut h);let copy=split*1000+1;
    near(volume(&h,body),4000.);near(volume(&h,copy),4000.);
    run(&mut h,Action::View("front"));h.state_mut().cam.target=DVec3::ZERO;h.state_mut().cam.scale=12.;h.run_steps(2);
    world_click(&mut h,[0.,-10.,5.]);
    assert_eq!(h.state().sel_face.as_ref().map(|f|f.body),Some(copy),"the positive split piece must pick independently");
    menu(&mut h,"Join Bodies");
    assert!(matches!(&h.state().dialog,Dialog::Combine(c) if c.target==Some(copy) && c.op==Op::Join));
    world_click(&mut h,[0.,-10.,-5.]);
    assert!(matches!(&h.state().dialog,Dialog::Combine(c) if c.tools==[body]));
    accept(&mut h);
    assert_eq!(h.state().session.built.bodies.len(),1);near(volume(&h,copy),8000.);
    run(&mut h,Action::Undo);assert_eq!(h.state().session.built.bodies.len(),2);
}

#[test]
fn split_picks_a_distant_flat_face_and_editing_uses_the_face_before_later_moves() {
    let mut h=state_harness();let body=block(&mut h,[-5.,-5.,0.],[10.;3]);let cutter=block(&mut h,[15.,-5.,4.],[10.,10.,1.]);
    h.state_mut().sel_body=Some(body);run(&mut h,Action::SplitBody);top(&mut h);
    world_click(&mut h,[20.,0.,5.]);
    assert!(matches!(&h.state().dialog,Dialog::Split(d) if matches!(d.plane,Some(PlaneRef::Face{body:id,..}) if id==cutter)));
    let split=accept(&mut h);near(volume(&h,body),500.);near(volume(&h,split*1000+1),500.);
    command(&mut h,json!({"op":"transform","body":cutter,"translate":[0,0,50]}));
    let before=h.state().doc().clone();h.state_mut().edit_feature(split);top(&mut h);
    near(h.state().modeling_source().body(cutter).unwrap().mesh.bbox().unwrap().1.z,5.);
    h.get_by_label("Pick face / plane").click();h.run_steps(2);world_click(&mut h,[20.,0.,5.]);
    assert!(matches!(&h.state().dialog,Dialog::Split(d) if matches!(d.plane,Some(PlaneRef::Face{body:id,at,..}) if id==cutter && (at.z-5.).abs()<1e-6)));
    assert_eq!(accept(&mut h),split);assert_eq!(h.state().doc(),&before);
}

#[test]
fn split_edit_plane_picking_excludes_planes_created_later_in_history() {
    let mut h=state_harness();let body=block(&mut h,[-5.,-5.,0.],[10.;3]);let earlier=plane(&mut h,5.);
    let split=feature(&command(&mut h,json!({"op":"split_body","body":body,"plane":{"plane":earlier}})));
    let later=plane(&mut h,7.);assert!(h.state().session.built.planes.contains_key(&later));
    h.state_mut().edit_feature(split);top(&mut h);
    h.get_by_label("Pick face / plane").click();h.run_steps(2);
    world_click(&mut h,[-5.5,0.,5.]);
    assert!(matches!(&h.state().dialog,Dialog::Split(d) if d.plane==Some(PlaneRef::Plane(earlier))),"picking must use the older plane despite the later plane being in front in the final model: {:?}",h.state().dialog);
    assert_eq!(accept(&mut h),split);
}

#[test]
fn split_rejects_no_cut_edits_while_suppressed_or_rolled_back_and_preserves_history() {
    let mut h=state_harness();let body=block(&mut h,[0.;3],[10.;3]);let cut=plane(&mut h,5.);
    h.state_mut().sel_body=Some(body);run(&mut h,Action::SplitBody);
    h.get_by_label("XY").click();h.run_steps(2);
    let before=h.state().doc().clone();
    assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_some()));
    h.state_mut().apply_dialog();assert_eq!(h.state().doc(),&before);run(&mut h,Action::Cancel);
    let split=feature(&command(&mut h,json!({"op":"split_body","body":body,"plane":{"plane":cut}})));
    for suppressed in [true,false] {
        command(&mut h,json!({"op":"edit_feature","feature":split,"suppressed":suppressed}));
        if !suppressed {command(&mut h,json!({"op":"rollback","to":cut}));}
        let before=h.state().doc().clone();h.state_mut().edit_feature(split);h.run_steps(2);
        let Dialog::Split(d)=&mut h.state_mut().dialog else {panic!()};d.plane=Some(PlaneRef::Origin(OriginPlane::XY));
        h.run_steps(2);assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_some()));
        h.state_mut().apply_dialog();assert_eq!(h.state().doc(),&before,"an unbuilt operation must still reject a plane that cuts no material");
        assert!(matches!(h.state().dialog,Dialog::Split(_)));run(&mut h,Action::Cancel);
    }
}

#[test]
fn body_dialogs_follow_the_target_owner_and_reject_reparenting_during_edits() {
    let mut h=state_harness();let root_body=block(&mut h,[-5.;3],[10.;3]);
    let owner=command(&mut h,json!({"op":"create_component","name":"Other","activate":true}))["component"].as_u64().unwrap() as Id;
    let other=block(&mut h,[-5.;3],[10.;3]);
    h.state_mut().sel_body=Some(root_body);run(&mut h,Action::SplitBody);h.run_steps(3);
    h.get_by_label("XY").click();h.run_steps(2);let split=accept(&mut h);
    assert_eq!(h.state().doc().active_component,owner);
    assert_eq!(h.state().doc().feature(split).unwrap().owner,0);
    assert_eq!(h.state().session.built.body(split*1000+1).unwrap().component,0);
    let before=h.state().doc().clone();h.state_mut().edit_feature(split);h.run_steps(2);
    crate::body_ops_ui::choose_body(h.state_mut(),other);h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_some()));
    h.state_mut().apply_dialog();assert_eq!(h.state().doc(),&before);
    run(&mut h,Action::Cancel);
    h.state_mut().sel_body=Some(root_body);run(&mut h,Action::RemoveBody);let remove=accept(&mut h);
    assert_eq!(h.state().doc().feature(remove).unwrap().owner,0);
    let before=h.state().doc().clone();h.state_mut().edit_feature(remove);h.run_steps(2);
    crate::body_ops_ui::choose_body(h.state_mut(),root_body);
    crate::body_ops_ui::choose_body(h.state_mut(),other);h.run_steps(2);
    h.state_mut().apply_dialog();assert_eq!(h.state().doc(),&before);
}
