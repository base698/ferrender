//! Pointer-driven placement handles: coordinate frames and transaction boundaries.
use super::*;
use egui::Vec2;
use fr_core::Id;

fn command(h: &mut H, value: serde_json::Value) -> serde_json::Value { h.state_mut().execute(&value).unwrap() }
fn close(a: DVec3,b: DVec3) { assert!(a.distance(b)<0.02,"{a:?} != {b:?}"); }
fn vector(h: &H, fields: &[String;3], kind: fr_core::Kind) -> DVec3 {
    DVec3::from_array(fields.each_ref().map(|s| h.state().doc().eval(s,kind).unwrap()))
}
fn begin_body(h: &mut H) -> Id {
    plate(h);
    let body = h.state().session.built.bodies[0].id;
    h.state_mut().sel_component = None;
    h.state_mut().sel_body = Some(body);
    run(h,Action::Transform);
    run(h,Action::View("iso")); run(h,Action::Fit);
    h.state_mut().cam.scale *= 0.65;
    h.run_steps(3);
    body
}
fn move_axis(h: &mut H, axis: usize, world_axis: DVec3, amount: f64) {
    let tip = crate::gizmo::test_handle(h.state(),axis,false).expect("translation handle");
    let screen = crate::view::to_screen(h.state(),world_axis)-crate::view::to_screen(h.state(),DVec3::ZERO);
    drag_with(h,PointerButton::Primary,tip,screen*amount as f32);
    h.run_steps(3);
}
fn accept(h: &mut H) {
    assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_none()),"{:?}",h.state().preview.as_ref().map(|p|&p.2));
    h.get_by_label("OK").click(); h.run_steps(2);
    assert_eq!(h.state().dialog,Dialog::None);
}

#[test]
fn each_translation_arrow_changes_only_its_axis_and_cancel_preserves_history() {
    for (axis,world) in [DVec3::X,DVec3::Y,DVec3::Z].into_iter().enumerate() {
        let mut h=state_harness();
        let body=begin_body(&mut h);
        let Dialog::Transform(d)=&mut h.state_mut().dialog else {panic!()};
        d.translate[(axis+1)%3]="0 in".into();
        h.run_steps(2);
        let before=h.state().doc().clone();
        let rev=h.state().session.rev;
        let cam=h.state().cam;
        move_axis(&mut h,axis,world,8.0);
        let Dialog::Transform(d)=&h.state().dialog else {panic!()};
        close(vector(&h,&d.translate,fr_core::Kind::Length),world*8.0);
        assert_eq!(d.translate[(axis+1)%3],"0 in","an unrelated input expression must be retained");
        assert_eq!(h.state().cam,cam,"a handle must not orbit the camera");
        assert_eq!(h.state().doc(),&before);
        assert_eq!(h.state().session.rev,rev,"dragging previews without adding undo entries");
        close(h.state().shown().body(body).unwrap().mesh.bbox().unwrap().0,world*8.0);
        assert!(!h.state().gizmo.is_active());
        run(&mut h,Action::Cancel);
        assert_eq!(h.state().doc(),&before);
    }
}

#[test]
fn body_ring_rotates_about_its_center_and_ok_is_one_undo_action() {
    let mut h=state_harness();
    let body=begin_body(&mut h);
    run(&mut h,Action::View("top"));
    let before=h.state().doc().clone();
    let cam=h.state().cam;
    let center=crate::view::to_screen(h.state(),DVec3::new(20.0,10.0,5.0));
    let from=crate::gizmo::test_handle(h.state(),2,true).unwrap();
    let v=from-center;
    let points:Vec<_>=(0..=8).map(|i| {
        let angle=-(i as f32)*std::f32::consts::FRAC_PI_2/8.0;
        center+Vec2::new(v.x*angle.cos()-v.y*angle.sin(),v.x*angle.sin()+v.y*angle.cos())
    }).collect();
    drag(&mut h,&points); h.run_steps(3);
    let bounds=h.state().shown().body(body).unwrap().mesh.bbox().unwrap();
    close((bounds.0+bounds.1)*0.5,DVec3::new(20.0,10.0,5.0));
    close(bounds.1-bounds.0,DVec3::new(20.0,40.0,10.0));
    assert_eq!(h.state().cam,cam);
    assert_eq!(h.state().doc(),&before);
    accept(&mut h);
    assert_eq!(h.state().doc().features.len(),before.features.len()+1);
    run(&mut h,Action::Undo);
    assert_eq!(h.state().doc(),&before);
}

#[test]
fn nested_component_arrows_use_parent_frame_and_primitive_arrows_use_owner_frame() {
    let mut h=state_harness();
    let parent=command(&mut h,json!({"op":"create_component","name":"Parent"}))["component"].as_u64().unwrap() as Id;
    command(&mut h,json!({"op":"move_component","id":parent,"translate":[100,0,0],"rotate":[0,0,90]}));
    let child=command(&mut h,json!({"op":"create_component","name":"Child"}))["component"].as_u64().unwrap() as Id;
    command(&mut h,json!({"op":"move_component","id":child,"rotate":[0,0,30]}));
    plate(&mut h);
    h.state_mut().move_component_dialog(child);h.run_steps(3);
    run(&mut h,Action::View("iso"));run(&mut h,Action::Fit);
    h.state_mut().cam.scale*=0.5;h.run_steps(2);
    let before=h.state().shown().bodies[0].mesh.bbox().unwrap();
    move_axis(&mut h,0,DVec3::Y,12.0);
    let Dialog::MoveComponent(d)=&h.state().dialog else {panic!()};
    close(vector(&h,&d.translate,fr_core::Kind::Length),DVec3::new(12.0,0.0,0.0));
    let after=h.state().shown().bodies[0].mesh.bbox().unwrap();
    close(after.0-before.0,DVec3::Y*12.0);
    close(after.1-before.1,DVec3::Y*12.0);
    accept(&mut h);
    let body=h.state().session.built.bodies[0].id;
    let world_axis=h.state().session.built.component_placement(child).transform_vector3(DVec3::X);
    h.state_mut().sel_component=None;h.state_mut().sel_body=Some(body);
    run(&mut h,Action::Transform);
    let before=h.state().shown().body(body).unwrap().mesh.bbox().unwrap();
    move_axis(&mut h,0,world_axis,4.0);
    let Dialog::Transform(d)=&h.state().dialog else {panic!()};
    close(vector(&h,&d.translate,fr_core::Kind::Length),DVec3::X*4.0);
    close(h.state().shown().body(body).unwrap().mesh.bbox().unwrap().0-before.0,world_axis*4.0);
    run(&mut h,Action::Cancel);
    command(&mut h,json!({"op":"activate_component","id":parent}));
    run(&mut h,Action::Primitive(0));h.run_steps(3);
    run(&mut h,Action::Fit);h.state_mut().cam.scale*=0.5;h.run_steps(2);
    let original=h.state().doc().clone();
    move_axis(&mut h,0,DVec3::Y,9.0);
    let Dialog::Primitive(d)=&h.state().dialog else {panic!()};
    close(vector(&h,&d.position,fr_core::Kind::Length),DVec3::new(9.0,0.0,0.0));
    assert_eq!(h.state().doc(),&original);
    run(&mut h,Action::Cancel);
    assert_eq!(h.state().doc(),&original);
}

#[test]
fn end_on_translation_and_edge_on_rotation_have_usable_handles() {
    let mut h=state_harness();
    begin_body(&mut h);
    run(&mut h,Action::View("top"));
    let cam=h.state().cam;
    let from=crate::gizmo::test_handle(h.state(),2,false).unwrap();
    let by=Vec2::new(-0.65,-0.76).normalized()*h.state().cam.scale as f32*6.0;
    drag_with(&mut h,PointerButton::Primary,from,by);h.run_steps(2);
    let Dialog::Transform(d)=&h.state().dialog else {panic!()};
    close(vector(&h,&d.translate,fr_core::Kind::Length),DVec3::Z*6.0);
    let from=crate::gizmo::test_handle(h.state(),0,true).unwrap();
    drag_with(&mut h,PointerButton::Primary,from,Vec2::new(30.0,0.0));h.run_steps(2);
    let Dialog::Transform(d)=&h.state().dialog else {panic!()};
    close(vector(&h,&d.rotate,fr_core::Kind::Angle),DVec3::X*30.0);
    assert_eq!(h.state().cam,cam);
}

#[test]
fn clicking_or_jittering_handles_preserves_linked_position_and_rotation() {
    let mut h=state_harness();
    command(&mut h,json!({"op":"set_parameter","name":"offset","expr":"12 mm"}));
    command(&mut h,json!({"op":"set_parameter","name":"angle","expr":"180 deg"}));
    run(&mut h,Action::Primitive(0));
    let Dialog::Primitive(d)=&mut h.state_mut().dialog else {panic!()};
    d.position[0]="$offset".into();
    // This equivalent Euler representation must not replace the expression
    // with rotations about the other axes on a motionless ring click.
    d.rotate[1]="$angle".into();
    h.run_steps(3);
    run(&mut h,Action::View("iso"));run(&mut h,Action::Fit);
    h.state_mut().cam.scale*=0.65;h.run_steps(2);
    let original=h.state().dialog.clone();
    let doc=h.state().doc().clone();
    let cam=h.state().cam;
    for turn in [false,true] {
        let start=crate::gizmo::test_handle(h.state(),0,turn).unwrap();
        click(&mut h,start);h.run_steps(2);
        assert_eq!(h.state().dialog,original,"a handle click must retain parameter expressions");
        drag(&mut h,&[start,start+Vec2::new(1.0,1.0)]);h.run_steps(2);
        assert_eq!(h.state().dialog,original,"sub-threshold pointer jitter must retain parameter expressions");
        assert!(!h.state().gizmo.is_active());
    }
    let start=crate::gizmo::test_handle(h.state(),0,false).unwrap();
    drag(&mut h,&[start,start+Vec2::new(25.0,0.0),start]);h.run_steps(2);
    assert_eq!(h.state().dialog,original,"returning a gesture to zero restores its original expression");
    assert_eq!(h.state().doc(),&doc);
    assert_eq!(h.state().cam,cam);
}

#[test]
fn replacement_dialog_cancels_an_inflight_gesture_and_screenshots_show_both_themes() {
    let mut h=harness();
    begin_body(&mut h);
    let start=crate::gizmo::test_handle(h.state(),0,false).unwrap();
    h.hover_at(start);h.step();button(&h,start,true);h.step();
    assert!(h.state().gizmo.is_active());
    run(&mut h,Action::Primitive(0));
    let dialog=h.state().dialog.clone();
    let moved=start+Vec2::new(40.0,20.0);
    h.hover_at(moved);h.step();button(&h,moved,false);h.step();
    assert_eq!(h.state().dialog,dialog,"an old handle must not change a new dialog");
    assert!(!h.state().gizmo.is_active());
    run(&mut h,Action::Cancel);
    let body=h.state().session.built.bodies[0].id;
    h.state_mut().sel_component=None;h.state_mut().sel_body=Some(body);
    run(&mut h,Action::Transform);run(&mut h,Action::View("iso"));run(&mut h,Action::Fit);
    h.state_mut().cam.scale*=0.6;h.run_steps(3);
    for (theme,name) in [(Appearance::Light,"move-gizmo-light.png"),(Appearance::Dark,"move-gizmo-dark.png")] {
        h.state_mut().set_appearance(theme);
        let image=save(&mut h,name);
        assert_eq!((image.width(),image.height()),(1440,900));
    }
}
