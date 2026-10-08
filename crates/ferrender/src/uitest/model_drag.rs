//! Pattern span gestures exercise the real viewport and one-step history.
use super::*;
use fr_core::{Id,Kind,Sketch,FeatureKind};

fn setup<'a>()->(H<'a>,Id,Id) {
    let mut h=state_harness();
    let source=h.state_mut().execute(&json!({"op":"primitive","type":"sphere","diameter":6})).unwrap()["feature"].as_u64().unwrap() as Id;
    let sketch=h.state_mut().session.edit(|doc|{
        let mut sk=Sketch::new(Plane::XY);
        let a=sk.add_point(DVec2::new(50.,-20.));let b=sk.add_point(DVec2::new(50.,40.));sk.add_line(a,b);
        sk.add_point(DVec2::new(60.,30.));
        Ok(doc.add_feature(FeatureKind::Sketch(sk)))
    }).unwrap();
    h.state_mut().refresh();
    h.state_mut().sel_feature=Some(source);
    run(&mut h,Action::Pattern);
    let Dialog::Pattern(d)=&mut h.state_mut().dialog else {panic!()};
    d.kind=1;d.axis=0;d.count=3;d.text="10 mm".into();d.second=false;d.source=Some(source);
    run(&mut h,Action::View("top"));
    h.state_mut().cam.target=DVec3::new(20.,0.,0.);h.state_mut().cam.scale=5.;
    h.run_steps(3);
    (h,source,sketch)
}
fn span(h:&H,index:usize)->crate::model_drag::Handle {crate::model_drag::handles(h.state()).into_iter().find(|p|p.index==index).unwrap()}
fn spacing(h:&H,index:usize)->f64 {let Dialog::Pattern(p)=&h.state().dialog else{panic!()};h.state().doc().eval(if index==0{&p.text}else{&p.text2},Kind::Length).unwrap()}
#[test]
fn last_copy_snaps_to_a_sketch_edge_without_moving_camera_or_other_axes() {
    let (mut h,source,_)=setup();let before=h.state().doc().clone();let cam=h.state().cam;
    let start=span(&h,0).screen;
    let target=crate::view::to_screen(h.state(),DVec3::new(50.,12.,0.))+egui::vec2(3.,0.);
    drag(&mut h,&[start,(start+target.to_vec2())*0.5,target]);h.run_steps(3);
    assert!((spacing(&h,0)-25.).abs()<1e-6,"two equal gaps put the last center at X=50");
    assert_eq!(h.state().cam,cam);assert_eq!(h.state().doc(),&before);
    assert_eq!(h.state().shown().bodies.len(),3);
    let centers:Vec<_>=h.state().shown().bodies.iter().map(|b|{let(lo,hi)=b.mesh.bbox().unwrap();(lo+hi)*0.5}).collect();
    assert!(centers.iter().any(|c|c.distance(DVec3::new(50.,0.,0.))<0.05));
    assert!(centers.iter().all(|c|c.y.abs()<1e-6 && c.z.abs()<1e-6));
    h.get_by_label("OK").click();h.run_steps(2);assert_eq!(h.state().dialog,Dialog::None);
    assert!(h.state().session.built.body(source).is_some());
    run(&mut h,Action::Undo);assert_eq!(h.state().doc(),&before);
}
#[test]
fn two_grid_spans_align_the_last_corner_to_a_sketch_point_and_cancel_is_clean() {
    let (mut h,_,_)=setup();let before=h.state().doc().clone();
    let Dialog::Pattern(d)=&mut h.state_mut().dialog else{panic!()};d.second=true;d.axis2=1;d.count2=4;d.text2="5 mm".into();h.run_steps(3);
    let target=crate::view::to_screen(h.state(),DVec3::new(60.,30.,0.));
    for index in 0..2 {let start=span(&h,index).screen;drag(&mut h,&[start,target]);h.run_steps(3);}
    assert!((spacing(&h,0)-30.).abs()<1e-6);assert!((spacing(&h,1)-10.).abs()<1e-6);
    assert_eq!(h.state().shown().bodies.len(),12);
    assert!(h.state().shown().bodies.iter().any(|b|{let(lo,hi)=b.mesh.bbox().unwrap();((lo+hi)*0.5).distance(DVec3::new(60.,30.,0.))<0.05}));
    run(&mut h,Action::Cancel);assert_eq!(h.state().doc(),&before);assert_eq!(h.state().session.built.bodies.len(),1);
}
#[test]
fn hidden_future_and_off_plane_sketches_are_not_placement_targets() {
    let (mut h,source,sketch)=setup();
    let target=crate::view::to_screen(h.state(),DVec3::new(60.,30.,0.));
    assert!(crate::model_drag::sketch_target(h.state(),target,Some(Plane::XY)).is_some());
    let plane=Plane{origin:DVec3::Z*2.,..Plane::XY};
    assert!(crate::model_drag::sketch_target(h.state(),target,Some(plane)).is_none());
    h.state_mut().session.doc.sketch_mut(sketch).unwrap().visible=false;
    assert!(crate::model_drag::sketch_target(h.state(),target,None).is_none());
    h.state_mut().session.doc.sketch_mut(sketch).unwrap().visible=true;
    h.state_mut().edit_feature(source);h.run_steps(2);
    assert!(crate::model_drag::sketch_target(h.state(),target,None).is_none(),"primitive edits must not snap to a later sketch");
}
#[test]
fn replacing_dialog_mid_drag_does_not_leave_a_stale_pattern_gesture() {
    let (mut h,_,_)=setup();let start=span(&h,0).screen;
    h.hover_at(start);h.step();button(&h,start,true);h.step();
    let end=start+egui::vec2(100.,0.);h.hover_at(end);h.step();
    run(&mut h,Action::Cancel);run(&mut h,Action::Primitive(0));
    let before=h.state().dialog.clone();h.hover_at(end+egui::vec2(50.,0.));h.step();
    button(&h,end,false);h.step();assert_eq!(h.state().dialog,before,"release must not alter a replacement dialog");
}
#[test]
fn editing_a_pattern_uses_its_original_center_before_a_later_move() {
    let (mut h,source,_)=setup();
    h.get_by_label("OK").click();h.run_steps(2);let pattern=h.state().sel_feature.unwrap();
    h.state_mut().execute(&json!({"op":"transform","body":source,"translate":[100,0,0]})).unwrap();
    let before=h.state().doc().clone();h.state_mut().edit_feature(pattern);h.run_steps(3);
    assert!(span(&h,0).origin.length()<1e-6,"edit handle belongs at the source's pre-Move center");
    let start=span(&h,0).screen;let target=crate::view::to_screen(h.state(),DVec3::new(50.,10.,0.));
    drag(&mut h,&[start,target]);h.run_steps(3);assert!((spacing(&h,0)-25.).abs()<1e-6);
    h.get_by_label("OK").click();h.run_steps(2);assert_eq!(h.state().dialog,Dialog::None);
    assert!(h.state().session.built.errors.is_empty());
    let (lo,hi)=h.state().session.built.body(source).unwrap().mesh.bbox().unwrap();assert!(((lo+hi)*0.5-DVec3::X*100.).length()<0.01,"tessellated sphere center must remain at the later Move: {lo:?}..{hi:?}");
    run(&mut h,Action::Undo);assert_eq!(h.state().doc(),&before);
}
#[test]
fn negative_spans_and_alt_bypass_remain_numeric_until_confirmation() {
    let (mut h,_,_)=setup();let start=span(&h,0).screen;
    h.event(Event::ModifiersChanged(Modifiers::ALT));h.hover_at(start);h.step();
    h.event(Event::PointerButton{pos:start,button:PointerButton::Primary,pressed:true,modifiers:Modifiers::ALT});h.step();
    let target=crate::view::to_screen(h.state(),DVec3::new(49.,8.,0.));h.hover_at(target);h.step();
    h.event(Event::PointerButton{pos:target,button:PointerButton::Primary,pressed:false,modifiers:Modifiers::ALT});h.step();
    assert!((spacing(&h,0)-24.5).abs()<1e-6,"Alt ignores nearby X=50 edge");
    h.event(Event::ModifiersChanged(Modifiers::NONE));h.step();
    let start=span(&h,0).screen;let target=crate::view::to_screen(h.state(),DVec3::new(-20.,0.,0.));
    drag(&mut h,&[start,target]);h.run_steps(3);assert!((spacing(&h,0)+10.).abs()<1e-6);
    assert_eq!(h.state().doc().features.len(),2,"preview gestures do not write history");
}
#[test]
fn clicking_a_span_preserves_parameter_expression_and_inline_values_have_handles() {
    let (mut h,_,_)=setup();
    h.state_mut().session.doc.enter("gap = 10 mm",Kind::Length).unwrap();
    let Dialog::Pattern(p)=&mut h.state_mut().dialog else{panic!()};p.text="$gap".into();h.run_steps(2);
    let start=span(&h,0).screen;click(&mut h,start);
    let Dialog::Pattern(p)=&h.state().dialog else{panic!()};assert_eq!(p.text,"$gap","clicking without dragging must not replace a parameter link");
    let Dialog::Pattern(p)=&mut h.state_mut().dialog else{panic!()};p.text="new_gap = 12 mm".into();h.run_steps(2);
    assert!((span(&h,0).span-24.).abs()<1e-6);
    let start=span(&h,0).screen;h.hover_at(start);h.step();button(&h,start,true);h.step();
    let Dialog::Pattern(p)=&mut h.state_mut().dialog else{panic!()};p.text="15 mm".into();
    h.hover_at(start+egui::vec2(80.,0.));h.step();button(&h,start,false);h.step();
    assert!((spacing(&h,0)-15.).abs()<1e-6,"a field changed externally must cancel the in-flight drag");
}
