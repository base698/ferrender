//! Solid-primitive dialogs, component ownership, history and display regressions.
use super::*;
use fr_core::{FeatureKind, Id, Op};
use crate::primitives::PrimitiveDlg;

fn command(h: &mut H, value: serde_json::Value) -> serde_json::Value {
    h.state_mut().execute(&value).unwrap_or_else(|e| panic!("{value}: {e}"))
}

fn accept(h: &mut H) -> Id {
    h.run_steps(3);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "preview failed: {:?}", h.state().preview.as_ref().map(|p| &p.2));
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Dialog::None, "dialog failed: {:?}", h.state().toast);
    h.state().sel_feature.unwrap()
}

fn primitive(h: &mut H, kind: usize, size: &[&str], position: [&str; 3], op: Op) -> Id {
    run(h, Action::Primitive(kind));
    let Dialog::Primitive(d) = &mut h.state_mut().dialog else { panic!("expected primitive dialog") };
    for (field, value) in d.size.iter_mut().zip(size) { *field = (*value).into(); }
    d.position = position.map(str::to_owned);
    d.op = op;
    accept(h)
}

fn bounds(h: &H, id: Id) -> (DVec3, DVec3) { h.state().session.built.body(id).unwrap().mesh.bbox().unwrap() }
fn near(a: DVec3, b: DVec3) { assert!(a.distance(b) < 0.05, "{a:?} != {b:?}"); }

#[test]
fn all_five_primitives_open_from_the_model_menu_and_preview_without_editing() {
    let mut h = state_harness();
    for (kind, name) in ["Box", "Cylinder", "Sphere", "Cone", "Torus"].iter().enumerate() {
        let before = h.state().doc().clone();
        h.get_by_label("Model").click(); h.run_steps(2);
        h.get_by_label_contains("Primitives").click(); h.run_steps(2);
        h.get_by_label(name).click(); h.run_steps(2);
        let Dialog::Primitive(d) = &mut h.state_mut().dialog else { panic!("{name} should open its primitive dialog") };
        assert_eq!(d.kind, kind);
        assert_eq!(d.op, Op::New, "each primitive defaults to a separate solid");
        d.position[0] = format!("{} mm", kind * 50);
        h.run_steps(3);
        assert_eq!(h.state().doc(), &before, "the live primitive preview must not edit history");
        assert_eq!(h.state().shown().bodies.len(), kind + 1);
        let id = accept(&mut h);
        assert!(matches!(h.state().doc().feature(id).unwrap().kind, FeatureKind::Primitive(_)));
        assert_eq!(h.state().doc().features.len(), kind + 1, "a primitive is one feature with no hidden sketch");
        let body = h.state().session.built.body(id).unwrap();
        assert!(body.is_exact() && body.mesh.volume() > 0.0, "{name} must be an editable exact solid");
        assert_eq!(h.state().session.built.bodies.len(), kind + 1);
    }
}

#[test]
fn primitive_cancel_and_invalid_inputs_preserve_the_document() {
    let mut h = state_harness();
    primitive(&mut h, 0, &["20 mm", "20 mm", "20 mm"], ["0 mm";3], Op::New);
    for (kind, size) in [(0, ["0 mm", "20 mm", "20 mm"]), (1, ["-2 mm", "20 mm", "0 mm"]),
        (2, ["$missing", "0 mm", "0 mm"]), (3, ["0 mm", "0 mm", "20 mm"]), (4, ["5 mm", "5 mm", "0 mm"])] {
        let before = h.state().doc().clone();
        run(&mut h, Action::Primitive(kind));
        let Dialog::Primitive(d) = &mut h.state_mut().dialog else { panic!() };
        d.size = size.map(str::to_owned);
        h.run_steps(3);
        assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_some()), "invalid primitive {kind} must display a preview error");
        h.state_mut().apply_dialog(); h.step();
        assert_eq!(h.state().doc(), &before, "invalid primitive {kind} must leave bodies, parameters and history unchanged");
        assert!(matches!(h.state().dialog, Dialog::Primitive(_)));
        run(&mut h, Action::Cancel);
        assert_eq!(h.state().doc(), &before);
    }
    let before = h.state().doc().clone();
    run(&mut h, Action::Primitive(2));
    h.run_steps(3);
    assert_eq!(h.state().shown().bodies.len(), 2, "the sphere exists only in the preview before confirmation");
    run(&mut h, Action::Cancel);
    assert_eq!(h.state().doc(), &before);
    assert_eq!(h.state().shown().bodies.len(), 1);
}

#[test]
fn primitive_dimensions_parameters_rotation_history_and_save_round_trip() {
    let mut h = state_harness();
    run(&mut h, Action::Primitive(0));
    let Dialog::Primitive(d) = &mut h.state_mut().dialog else { panic!() };
    d.size = ["span = 1 in".into(), "10 mm".into(), "8 mm".into()];
    d.position = ["20 mm".into(), "30 mm".into(), "40 mm".into()];
    d.rotate[2] = "90 deg".into();
    let id = accept(&mut h);
    near(bounds(&h,id).0, DVec3::new(10.0,30.0,40.0));
    near(bounds(&h,id).1, DVec3::new(20.0,55.4,48.0));
    assert!(h.state().doc().params.iter().any(|p| p.name == "span"));
    command(&mut h, json!({"op":"set_parameter","name":"span","expr":"32 mm"}));
    near(bounds(&h,id).1, DVec3::new(20.0,62.0,48.0));
    let before = h.state().doc().clone();
    h.state_mut().edit_feature(id); h.run_steps(2);
    let Dialog::Primitive(d) = &mut h.state_mut().dialog else { panic!("editing the timeline feature must reopen Primitive") };
    assert_eq!(d.editing, Some(id));
    assert_eq!(d.kind, 0);
    d.size[1] = "12 mm".into();
    let edited = accept(&mut h);
    assert_eq!(edited,id);
    assert_eq!(h.state().doc().features.len(), before.features.len());
    near(bounds(&h,id).0, DVec3::new(8.0,30.0,40.0));
    let after = h.state().doc().clone();
    run(&mut h, Action::Undo);
    assert_eq!(h.state().doc(), &before);
    run(&mut h, Action::Redo);
    assert_eq!(h.state().doc(), &after);

    h.state_mut().edit_feature(id); h.run_steps(2);
    let Dialog::Primitive(d) = &mut h.state_mut().dialog else { panic!() };
    d.size[0] = "bad = 0 mm".into();
    h.state_mut().apply_dialog(); h.step();
    assert_eq!(h.state().doc(), &after, "invalid edits must also roll back parameter definitions");
    run(&mut h, Action::Cancel);
    let saved = fr_core::io::to_json(h.state().doc());
    std::fs::write(out_dir().join("primitive-parameter-box.ferr"), &saved).unwrap();
    let reopened = fr_core::Session::new(fr_core::io::from_json(&saved).unwrap());
    assert_eq!(reopened.doc, after);
    assert_eq!(reopened.built.bodies.len(),1);
    assert!(reopened.built.errors.is_empty());
    near(reopened.built.body(id).unwrap().mesh.bbox().unwrap().0, DVec3::new(8.0,30.0,40.0));
}

#[test]
fn primitive_operations_are_component_scoped_and_a_moved_primitive_can_be_patterned() {
    let mut h = state_harness();
    let root_box = primitive(&mut h, 0, &["20 mm", "20 mm", "20 mm"], ["0 mm";3], Op::New);
    let root_volume = h.state().session.built.body(root_box).unwrap().mesh.volume();
    let owner = command(&mut h, json!({"op":"create_component","name":"Insert","activate":true}))["component"].as_u64().unwrap() as Id;
    let insert = primitive(&mut h, 0, &["20 mm", "20 mm", "20 mm"], ["0 mm";3], Op::New);
    assert_eq!(h.state().session.built.bodies.len(),2, "overlapping New Body primitives remain separate");
    let joined = primitive(&mut h, 0, &["30 mm", "20 mm", "20 mm"], ["0 mm";3], Op::Join);
    assert_eq!(h.state().doc().feature(joined).unwrap().owner, owner);
    assert_eq!(h.state().session.built.bodies.len(),2, "Join combines only active-component bodies");
    let before_cut = h.state().session.built.body(insert).unwrap().mesh.volume();
    primitive(&mut h, 1, &["6 mm", "30 mm"], ["10 mm", "10 mm", "-5 mm"], Op::Cut);
    assert!(h.state().session.built.body(insert).unwrap().mesh.volume() < before_cut);
    assert!((h.state().session.built.body(root_box).unwrap().mesh.volume() - root_volume).abs() < 1e-7);
    command(&mut h, json!({"op":"move_component","id":owner,"translate":[50,0,0],"rotate":[0,0,90]}));
    let sphere = primitive(&mut h, 2, &["10 mm"], ["40 mm", "0 mm", "5 mm"], Op::New);
    near(bounds(&h,sphere).0, DVec3::new(45.0,35.0,0.0));
    near(bounds(&h,sphere).1, DVec3::new(55.0,45.0,10.0));
    run(&mut h, Action::Pattern);
    let Dialog::Pattern(p) = &mut h.state_mut().dialog else { panic!("a primitive must be an available pattern source") };
    assert_eq!(p.source,Some(sphere));
    p.kind=1; p.axis=0; p.count=2; p.text="20 mm".into(); p.second=false;
    let pattern = accept(&mut h);
    assert_eq!(h.state().doc().feature(pattern).unwrap().owner,owner);
    assert_eq!(h.state().session.built.bodies.len(),4);
    assert_eq!(h.state().session.built.bodies.iter().filter(|b| b.component==owner).count(),3);
    let copy = h.state().session.built.bodies.iter().find(|b| b.id != root_box && b.id != insert && b.id != sphere).unwrap();
    near(copy.mesh.bbox().unwrap().0, DVec3::new(45.0,55.0,0.0));
}

#[test]
fn primitive_shapes_and_live_dialog_render_together() {
    let mut h = harness();
    for (kind,x,z) in [(0,-60,0),(1,-25,0),(2,10,10),(3,45,0),(4,90,5)] {
        let d = PrimitiveDlg::new(kind);
        let values: Vec<&str> = d.size.iter().map(String::as_str).collect();
        primitive(&mut h,kind,&values,[&format!("{x} mm"),"0 mm",&format!("{z} mm")],Op::New);
    }
    std::fs::write(out_dir().join("primitive-gallery.ferr"),fr_core::io::to_json(h.state().doc())).unwrap();
    h.state_mut().fit();
    run(&mut h,Action::Primitive(4));
    let Dialog::Primitive(d) = &mut h.state_mut().dialog else { panic!() };
    d.position=["10 mm".into(),"50 mm".into(),"8 mm".into()];
    d.size[0]="18 mm".into(); d.size[1]="4 mm".into();
    h.run_steps(3);
    assert_eq!(h.state().shown().bodies.len(),6);
    h.state_mut().fit();
    let right = h.state().cam.basis().1;
    h.state_mut().cam.target += right * 28.0;
    h.state_mut().cam.scale *= 0.85;
    let img=save(&mut h,"primitives-dialog.png");
    assert_eq!(img.dimensions(),(1440,900));
    assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_none()));
    h.state_mut().set_appearance(Appearance::Dark);
    save(&mut h,"primitives-dialog-dark.png");
}
