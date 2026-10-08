//! Graphical primitive placement changes only preview fields until OK.
use super::*;
use fr_core::{Id,Kind};

fn command(h:&mut H,c:serde_json::Value)->serde_json::Value {h.state_mut().execute(&c).unwrap_or_else(|e|panic!("{c}: {e}"))}
fn accept(h:&mut H)->Id {
    h.run_steps(3);assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_none()),"{:?}",h.state().preview.as_ref().map(|p|&p.2));
    h.get_by_label("OK").click();h.run_steps(2);assert_eq!(h.state().dialog,Dialog::None);h.state().sel_feature.unwrap()
}
fn position(h:&H)->DVec3 {
    let Dialog::Primitive(d)=&h.state().dialog else {panic!("expected primitive dialog");};
    DVec3::from_array(d.position.each_ref().map(|s|h.state().doc().eval(s,Kind::Length).unwrap()))
}
fn world(h:&H,p:DVec3)->Pos2 {crate::view::to_screen(h.state(),p)}
fn hover(h:&mut H,p:DVec3) {let p=world(h,p);h.hover_at(p);h.run_steps(2);}
fn near(a:DVec3,b:DVec3) {assert!(a.distance(b)<0.03,"{a:?} != {b:?}");}
fn place_mode(h:&mut H) {h.run_steps(2);h.get_by_label("Place in view").click();h.run_steps(2);assert!(matches!(&h.state().dialog,Dialog::Primitive(d) if d.placing));}

#[test]
fn xy_pointer_placement_previews_then_click_holds_coordinates_until_ok() {
    let mut h=state_harness();run(&mut h,Action::Primitive(0));place_mode(&mut h);
    let before=h.state().doc().clone();let target=DVec3::new(14.0,9.0,0.0);
    hover(&mut h,target);near(position(&h),target);assert_eq!(h.state().doc(),&before);
    assert_eq!(h.state().shown().bodies.len(),1);
    let p=world(&h,target);click(&mut h,p);
    assert!(matches!(&h.state().dialog,Dialog::Primitive(d) if !d.placing && !d.pick_surface));
    hover(&mut h,DVec3::new(-12.0,-8.0,0.0));near(position(&h),target);
    let id=accept(&mut h);assert_eq!(h.state().doc().features.len(),1);
    let (lo,hi)=h.state().session.built.body(id).unwrap().mesh.bbox().unwrap();near(lo,target);near(hi,target+DVec3::splat(20.0));
}

#[test]
fn picking_a_moved_component_face_places_in_owner_coordinates_and_aligns_normal() {
    let mut h=state_harness();
    let owner=command(&mut h,json!({"op":"create_component","name":"Placed","activate":true}))["component"].as_u64().unwrap() as Id;
    run(&mut h,Action::Primitive(0));accept(&mut h);
    command(&mut h,json!({"op":"move_component","id":owner,"translate":[50,0,0],"rotate":[0,0,90]}));
    h.state_mut().fit();run(&mut h,Action::View("top"));
    run(&mut h,Action::Primitive(1));
    let Dialog::Primitive(d)=&mut h.state_mut().dialog else {panic!()};d.size[0]="8 mm".into();d.size[1]="6 mm".into();
    h.run_steps(2);h.get_by_label("Pick surface").click();h.run_steps(2);
    let top=world(&h,DVec3::new(40.0,10.0,20.0));click(&mut h,top);
    assert!(matches!(&h.state().dialog,Dialog::Primitive(d) if d.surface.is_some() && d.placing && !d.pick_surface));
    let target=DVec3::new(40.0,12.0,20.0);hover(&mut h,target);near(position(&h),DVec3::new(12.0,10.0,20.0));
    let p=world(&h,target);click(&mut h,p);let id=accept(&mut h);
    assert_eq!(h.state().doc().feature(id).unwrap().owner,owner);
    let (lo,hi)=h.state().session.built.body(id).unwrap().mesh.bbox().unwrap();near(lo,DVec3::new(36.0,8.0,20.0));near(hi,DVec3::new(44.0,16.0,26.0));
}

#[test]
fn construction_plane_placement_turns_local_z_and_can_keep_an_existing_rotation() {
    let mut h=state_harness();
    let plane=command(&mut h,json!({"op":"create_plane","kind":"offset","base":"YZ","distance":12}))["feature"].as_u64().unwrap() as Id;
    run(&mut h,Action::Primitive(1));
    let Dialog::Primitive(d)=&mut h.state_mut().dialog else {panic!()};d.size[0]="4 mm".into();d.size[1]="8 mm".into();d.pick_surface=true;
    h.state_mut().choose_construction_plane(plane);h.run_steps(2);
    assert!(matches!(&h.state().dialog,Dialog::Primitive(d) if d.surface.is_some() && d.placing));
    run(&mut h,Action::View("right"));let target=DVec3::new(12.0,7.0,9.0);hover(&mut h,target);
    let p=world(&h,target);click(&mut h,p);let id=accept(&mut h);
    let (lo,hi)=h.state().session.built.body(id).unwrap().mesh.bbox().unwrap();near(lo,DVec3::new(12.0,5.0,7.0));near(hi,DVec3::new(20.0,9.0,11.0));

    run(&mut h,Action::Primitive(0));
    let Dialog::Primitive(d)=&mut h.state_mut().dialog else {panic!()};d.align=false;d.rotate=["10 deg".into(),"20 deg".into(),"30 deg".into()];d.pick_surface=true;
    h.state_mut().choose_construction_plane(plane);h.run_steps(2);hover(&mut h,DVec3::new(12.0,-10.0,7.0));
    let Dialog::Primitive(d)=&h.state().dialog else {panic!()};assert_eq!(d.rotate,["10 deg","20 deg","30 deg"].map(str::to_owned),"Align off preserves the entered rotation");
}

#[test]
fn placement_snaps_to_sketch_points_and_lines_but_editing_ignores_future_geometry() {
    let mut h=state_harness();
    command(&mut h,json!({"op":"create_sketch","plane":"XY"}));
    command(&mut h,json!({"op":"add_geometry","items":[{"type":"line","from":[-20,4],"to":[20,4]}]}));
    run(&mut h,Action::Primitive(0));run(&mut h,Action::View("top"));h.state_mut().cam.scale=10.0;
    place_mode(&mut h);hover(&mut h,DVec3::new(-19.7,4.3,0.0));near(position(&h),DVec3::new(-20.0,4.0,0.0));
    hover(&mut h,DVec3::new(3.0,4.3,0.0));near(position(&h),DVec3::new(3.0,4.0,0.0));run(&mut h,Action::Cancel);

    let mut h=state_harness();run(&mut h,Action::Primitive(0));let first=accept(&mut h);
    command(&mut h,json!({"op":"create_sketch","plane":"XY"}));
    command(&mut h,json!({"op":"add_geometry","items":[{"type":"line","from":[-20,4],"to":[20,4]}]}));
    h.state_mut().edit_feature(first);h.run_steps(2);run(&mut h,Action::View("top"));h.state_mut().cam.scale=10.0;
    place_mode(&mut h);hover(&mut h,DVec3::new(3.0,4.3,0.0));near(position(&h),DVec3::new(3.0,4.3,0.0));
}

#[test]
fn cancelling_placement_edit_does_not_add_an_undo_step() {
    let mut h=state_harness();run(&mut h,Action::Primitive(0));let id=accept(&mut h);let before=h.state().doc().clone();
    h.state_mut().edit_feature(id);h.run_steps(2);place_mode(&mut h);hover(&mut h,DVec3::new(35.0,-9.0,0.0));
    assert_eq!(h.state().doc(),&before);run(&mut h,Action::Cancel);assert_eq!(h.state().doc(),&before);
    run(&mut h,Action::Undo);assert!(h.state().doc().features.is_empty(),"one Undo must still remove the original creation");
}
