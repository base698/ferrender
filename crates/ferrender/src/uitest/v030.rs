//! Candidate acceptance regressions for components and persistent construction planes.
use super::*;
use fr_core::{FeatureKind, Id, Op};
use fr_core::planes::{OriginPlane, PlaneRef, PointRef};

fn command(h: &mut H, value: serde_json::Value) -> serde_json::Value {
    h.state_mut().execute(&value).unwrap_or_else(|e| panic!("{value}: {e}"))
}
fn near(a: f64, b: f64) { assert!((a-b).abs() < 1e-5, "{a} != {b}"); }
fn accept(h: &mut H) {
    h.run_steps(3);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "dialog preview failed: {:?}", h.state().preview.as_ref().map(|p| &p.2));
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Dialog::None, "dialog failed to apply: {:?}", h.state().toast);
}
fn component_label(h: &H, id: Id) -> String {
    format!("{} {}", egui_phosphor::regular::CUBE, h.state().doc().component_name(id))
}
fn new_component(h: &mut H) -> Id {
    h.get_by_label("Model").click();
    h.run_steps(2);
    h.get_by_label("New Component").click();
    h.run_steps(2);
    let id = h.state().doc().active_component;
    assert_ne!(id, 0);
    assert!(matches!(&h.state().doc().feature(id).unwrap().kind, FeatureKind::Component(_)));
    h.state_mut().rename = None;
    h.run_steps(2);
    id
}
fn set_plane_distance(h: &mut H, text: &str) {
    let Dialog::Plane(mut d) = h.state().dialog.clone() else { panic!("expected construction-plane dialog") };
    d.text = text.into();
    h.state_mut().dialog = Dialog::Plane(d);
}
fn plane_distance(h: &H) -> f64 {
    let Dialog::Plane(d) = &h.state().dialog else { panic!("expected construction-plane dialog") };
    h.state().doc().eval(&d.text, fr_core::Kind::Length).unwrap()
}
fn post_on_plane(h: &mut H, plane: Id) -> (Id, Id) {
    h.state_mut().create_sketch_on(plane);
    let Mode::Sketch(sketch) = h.state().mode else { panic!("plane should start an attached sketch") };
    command(h, json!({"op":"add_geometry","sketch":sketch,"items":[{"type":"circle","center":[20,10],"radius":3}]}));
    run(h, Action::Extrude);
    let Dialog::Feature(mut d) = h.state().dialog.clone() else { panic!("expected extrude dialog") };
    d.text = "3 mm".into(); d.op = Op::New;
    h.state_mut().dialog = Dialog::Feature(d);
    accept(h);
    (sketch, h.state().doc().features.last().unwrap().id)
}

#[test]
fn component_menu_browser_activation_visibility_and_delete_are_scoped() {
    let mut h = state_harness();
    let a = new_component(&mut h);
    plate(&mut h);
    let first = h.state().session.built.bodies[0].id;
    assert_eq!(h.state().session.built.bodies[0].component, a);
    run(&mut h, Action::ActivateRoot);
    let b = new_component(&mut h);
    plate(&mut h);
    assert_eq!(h.state().session.built.bodies.len(), 2, "overlapping parts in different components must not merge");
    assert!(h.state().session.built.bodies.iter().all(|body| body.name == "Body1"));
    assert_eq!(h.state().session.built.bodies.iter().filter(|body| body.component == a).count(), 1);
    assert_eq!(h.state().session.built.bodies.iter().filter(|body| body.component == b).count(), 1);

    let label = component_label(&h, a);
    h.get_by_label(&label).click_secondary(); h.run_steps(2);
    h.get_by_label("Activate").click(); h.run_steps(2);
    assert_eq!(h.state().doc().active_component, a);
    let child = new_component(&mut h);
    assert_eq!(h.state().doc().feature(child).unwrap().owner, a);
    h.get_by_label(&label).click_secondary(); h.run_steps(2);
    h.get_by_label("Hide").click(); h.run_steps(2);
    assert!(h.state().session.visible_bodies().all(|body| body.id != first));
    assert!(!h.state().session.built.component_visible(child), "hiding the parent hides its child too");
    assert_eq!(h.state().session.visible_bodies().count(), 1);
    h.get_by_label(&label).click_secondary(); h.run_steps(2);
    h.get_by_label("Show").click(); h.run_steps(2);
    assert_eq!(h.state().session.visible_bodies().count(), 2);

    h.get_by_label(&label).click_secondary(); h.run_steps(2);
    h.get_by_label("Delete").click(); h.run_steps(2);
    assert_eq!(h.state().dialog, Dialog::DeleteComponent(a));
    h.get(egui_kittest::kittest::By::new().role(egui::accesskit::Role::Button).label("Delete Component")).click(); h.run_steps(2);
    assert!(h.state().doc().feature(a).is_none() && h.state().doc().feature(child).is_none());
    assert!(h.state().doc().feature(b).is_some());
    assert_eq!(h.state().session.built.bodies.len(), 1);
    run(&mut h, Action::Undo);
    assert!(h.state().doc().feature(a).is_some() && h.state().doc().feature(child).is_some());
    assert_eq!(h.state().session.built.bodies.len(), 2);
}

#[test]
fn moved_component_face_picking_creates_local_sketch_and_world_positioned_extrusion() {
    let mut h = state_harness();
    let owner = new_component(&mut h);
    plate(&mut h);
    let before_features = h.state().doc().features.len();
    let label = component_label(&h, owner);
    h.get_by_label(&label).click(); h.run_steps(2);
    run(&mut h, Action::Transform);
    let Dialog::MoveComponent(mut d) = h.state().dialog.clone() else { panic!("Move must choose component placement when a component is selected") };
    d.translate = ["50 mm".into(), "0 mm".into(), "5 mm".into()];
    d.rotate = ["0 deg".into(), "0 deg".into(), "90 deg".into()];
    h.state_mut().dialog = Dialog::MoveComponent(d);
    accept(&mut h);
    assert_eq!(h.state().doc().features.len(), before_features, "component movement edits placement, without a new Transform feature");
    let bounds = h.state().session.built.bodies[0].mesh.bbox().unwrap();
    for (got, want) in bounds.0.to_array().into_iter().zip([30.0, 0.0, 5.0]) { near(got, want); }
    for (got, want) in bounds.1.to_array().into_iter().zip([50.0, 40.0, 15.0]) { near(got, want); }
    run(&mut h, Action::View("top")); run(&mut h, Action::Fit);
    run(&mut h, Action::NewSketch);
    let target = crate::view::to_screen(h.state(), DVec3::new(40.0, 20.0, 15.0));
    click(&mut h, target);
    let Mode::Sketch(sketch) = h.state().mode else { panic!("clicking the moved top face should start a sketch: {:?}", h.state().toast) };
    assert_eq!(h.state().doc().feature(sketch).unwrap().owner, owner);
    let plane = h.state().session.built.sketch_plane(h.state().doc(), sketch).unwrap();
    near((DVec3::new(40.0, 20.0, 15.0) - plane.origin).dot(plane.normal()), 0.0);
    let center = plane.to_local(DVec3::new(40.0, 20.0, 15.0));
    command(&mut h, json!({"op":"add_geometry","sketch":sketch,"items":[{"type":"circle","center":center.to_array(),"radius":2}]}));
    run(&mut h, Action::Extrude);
    let Dialog::Feature(mut d) = h.state().dialog.clone() else { panic!() };
    d.text = "3 mm".into(); d.op = Op::Join;
    h.state_mut().dialog = Dialog::Feature(d); accept(&mut h);
    assert_eq!(h.state().session.built.bodies.len(), 1);
    let body = &h.state().session.built.bodies[0];
    assert_eq!(body.component, owner);
    near(body.mesh.bbox().unwrap().1.z, 18.0);
    assert!(h.state().session.built.errors.is_empty());
}

#[test]
fn offset_plane_dialog_face_pick_parameter_follow_and_visibility_pin() {
    let mut h = state_harness();
    plate(&mut h);
    let base = h.state().session.built.bodies[0].id;
    command(&mut h, json!({"op":"set_parameter","name":"gap","expr":"5 mm"}));
    run(&mut h, Action::View("top")); run(&mut h, Action::Fit);
    run(&mut h, Action::Plane);
    let target = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 10.0));
    click(&mut h, target);
    let Dialog::Plane(d) = &h.state().dialog else { panic!() };
    assert!(matches!(d.base, Some(PlaneRef::Face { body, .. }) if body == base), "the dialog should pick the source face");
    set_plane_distance(&mut h, "$gap"); accept(&mut h);
    let plane = h.state().doc().features.last().unwrap().id;
    near(h.state().session.built.planes[&plane].plane.origin.z, 15.0);
    // A value left in New Sketch's quick offset must never silently detach a
    // construction-plane sketch from its parametric support.
    let before = h.state().doc().clone();
    h.state_mut().plane_offset = "2 mm".into();
    h.state_mut().create_sketch_on(plane);
    assert_eq!(h.state().doc(), &before);
    assert!(!matches!(h.state().mode, Mode::Sketch(_)));
    h.state_mut().plane_offset = "0 mm".into();
    let (sketch, post) = post_on_plane(&mut h, plane);
    assert_eq!(h.state().doc().sketch(sketch).unwrap().on, Some(plane));
    near(h.state().session.built.body(post).unwrap().mesh.bbox().unwrap().0.z, 15.0);
    let FeatureKind::Plane(p) = &h.state().doc().feature(plane).unwrap().kind else { panic!() };
    assert!(!p.visible && !p.visibility_pinned, "finishing an attached sketch should auto-hide an untouched plane eye");
    command(&mut h, json!({"op":"set_parameter","name":"gap","expr":"8 mm"}));
    near(h.state().session.built.planes[&plane].plane.origin.z, 18.0);
    near(h.state().session.built.body(post).unwrap().mesh.bbox().unwrap().0.z, 18.0);
    command(&mut h, json!({"op":"edit_feature","feature":base,"distance":20}));
    near(h.state().session.built.planes[&plane].plane.origin.z, 28.0);
    near(h.state().session.built.body(post).unwrap().mesh.bbox().unwrap().0.z, 28.0);
    command(&mut h, json!({"op":"set_visible","id":plane,"visible":true}));
    h.state_mut().edit_sketch(sketch); h.state_mut().finish_sketch(); h.step();
    let FeatureKind::Plane(p) = &h.state().doc().feature(plane).unwrap().kind else { panic!() };
    assert!(p.visible && p.visibility_pinned, "an explicit eye choice must survive finishing a sketch");
    let encoded = fr_core::io::validated_json(h.state().doc()).unwrap();
    let restored = fr_core::Session::new(fr_core::io::from_json(&encoded).unwrap());
    assert_eq!(restored.doc.sketch(sketch).unwrap().on, Some(plane));
    near(restored.built.body(post).unwrap().mesh.bbox().unwrap().0.z, 28.0);
}

#[test]
fn three_point_plane_dialog_refuses_collinearity_and_keeps_inputs_editable() {
    let mut h = state_harness();
    run(&mut h, Action::Plane);
    let Dialog::Plane(mut d) = h.state().dialog.clone() else { panic!() };
    d.kind = 2; d.typed = [true; 3];
    d.coordinates = [["0 mm".into(), "0 mm".into(), "0 mm".into()], ["10 mm".into(), "0 mm".into(), "0 mm".into()], ["20 mm".into(), "0 mm".into(), "0 mm".into()]];
    h.state_mut().dialog = Dialog::Plane(d.clone()); h.run_steps(3);
    let before = h.state().doc().clone();
    assert!(h.state().preview.as_ref().and_then(|p| p.2.as_ref()).is_some_and(|e| e.contains("line")));
    h.state_mut().apply_dialog(); h.step();
    assert_eq!(h.state().doc(), &before);
    assert!(matches!(h.state().dialog, Dialog::Plane(_)));
    d.coordinates[2] = ["0 mm".into(), "10 mm".into(), "10 mm".into()];
    h.state_mut().dialog = Dialog::Plane(d); accept(&mut h);
    let plane = h.state().doc().features.last().unwrap().id;
    let resolved = h.state().session.built.planes[&plane].plane;
    for p in [DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(0.0, 10.0, 10.0)] { near((p-resolved.origin).dot(resolved.normal()), 0.0); }
    run(&mut h, Action::NewSketch);
    h.state_mut().choose_construction_plane(plane); h.step();
    assert_eq!(h.state().sketch().unwrap().1.on, Some(plane));
    assert!(h.state().session.built.errors.is_empty());
}

#[test]
fn midplane_dialog_handles_opposite_faces_and_edits_the_angular_bisector() {
    let mut h = state_harness();
    plate(&mut h);
    let body = &h.state().session.built.bodies[0];
    let make_face = |at| PlaneRef::Face { body: body.id, at, frame: body.local_frame(), tag: None };
    let top = make_face(DVec3::new(20.0, 10.0, 10.0));
    let bottom = make_face(DVec3::new(20.0, 10.0, 0.0));
    let side = make_face(DVec3::new(40.0, 10.0, 5.0));
    run(&mut h, Action::Plane);
    let Dialog::Plane(mut d) = h.state().dialog.clone() else { panic!() };
    d.kind = 1; d.faces = vec![top.clone(), bottom];
    h.state_mut().dialog = Dialog::Plane(d); accept(&mut h);
    let id = h.state().doc().features.last().unwrap().id;
    near(h.state().session.built.planes[&id].plane.origin.z, 5.0);
    h.state_mut().edit_feature(id);
    let Dialog::Plane(mut d) = h.state().dialog.clone() else { panic!() };
    assert_eq!(d.editing, Some(id));
    d.faces = vec![top, side];
    h.state_mut().dialog = Dialog::Plane(d); accept(&mut h);
    let first = h.state().session.built.planes[&id].plane;
    near(first.normal().dot(DVec3::Z).abs(), std::f64::consts::FRAC_1_SQRT_2);
    h.state_mut().edit_feature(id); h.run_steps(2);
    h.get_by_label("Flip").click(); h.run_steps(2);
    accept(&mut h);
    let second = h.state().session.built.planes[&id].plane;
    near(first.normal().dot(second.normal()), 0.0);
    near((DVec3::new(40.0, 0.0, 10.0)-second.origin).dot(second.normal()), 0.0);
    run(&mut h, Action::Undo);
    assert!(h.state().session.built.planes[&id].plane.normal().distance(first.normal()) < 1e-6);
    run(&mut h, Action::Redo);
    assert!(h.state().session.built.planes[&id].plane.normal().distance(second.normal()) < 1e-6);
}

#[test]
fn editing_plane_picks_faces_before_a_later_body_transform() {
    let mut h = state_harness();
    plate(&mut h);
    let body = h.state().session.built.bodies[0].id;
    run(&mut h, Action::Plane);
    h.state_mut().choose_origin(0, OriginPlane::XY);
    set_plane_distance(&mut h, "15 mm");
    accept(&mut h);
    let plane = h.state().doc().features.last().unwrap().id;
    command(&mut h, json!({"op":"transform","body":body,"translate":[1000,0,0]}));
    near(h.state().session.built.body(body).unwrap().mesh.bbox().unwrap().0.x, 1000.0);
    h.state_mut().edit_feature(plane);
    h.run_steps(3);
    near(h.state().plane_source().body(body).unwrap().mesh.bbox().unwrap().0.x, 0.0);
    near(h.state().preview.as_ref().unwrap().1.body(body).unwrap().mesh.bbox().unwrap().0.x, 0.0);
    run(&mut h, Action::View("top"));
    run(&mut h, Action::Fit);
    h.get_by_label("Clear base").click();
    h.run_steps(2);
    let target = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 10.0));
    click(&mut h, target);
    let Dialog::Plane(d) = &h.state().dialog else { panic!() };
    assert!(matches!(d.base, Some(PlaneRef::Face { body: picked, at, .. }) if picked == body && (at.z - 10.0).abs() < 1e-5), "editing must pick the source face at this plane's history position");
    set_plane_distance(&mut h, "5 mm");
    accept(&mut h);
    near(h.state().session.built.planes[&plane].plane.origin.z, 15.0);
    near(h.state().session.built.body(body).unwrap().mesh.bbox().unwrap().0.x, 1000.0);
    assert!(h.state().session.built.errors.is_empty());
}

#[test]
fn offset_plane_arrow_drag_updates_distance_and_preview_without_orbiting() {
    let mut h = state_harness();
    plate(&mut h);
    run(&mut h, Action::Plane);
    h.state_mut().choose_origin(0, OriginPlane::XY);
    set_plane_distance(&mut h, "10 mm");
    h.run_steps(3);
    run(&mut h, Action::View("front"));
    run(&mut h, Action::Fit);
    h.state_mut().cam.scale *= 0.6;
    h.run_steps(2);
    let cam = h.state().cam;
    let (plane, middle, _) = crate::construction_view::offset_base(h.state()).unwrap();
    let from = plane.to_world(middle);
    let base = crate::view::to_screen(h.state(), from);
    let along = crate::view::to_screen(h.state(), from + plane.normal()) - base;
    assert!(along.length() * 10.0 > 34.0, "the 10 mm arrow should exceed its minimum display size");
    let tip = crate::view::to_screen(h.state(), from + plane.normal() * 10.0);
    drag_with(&mut h, PointerButton::Primary, tip, along * 15.0);
    h.run_steps(2);
    assert_eq!(h.state().cam, cam, "dragging the plane arrow must not orbit the view");
    assert!((plane_distance(&h) - 25.0).abs() <= 0.6);
    near(h.state().shown().planes.values().next().unwrap().plane.origin.z, plane_distance(&h));
    let tip = crate::view::to_screen(h.state(), from + plane.normal() * plane_distance(&h));
    drag_with(&mut h, PointerButton::Primary, tip, -along * 40.0);
    h.run_steps(2);
    assert!((plane_distance(&h) + 15.0).abs() <= 0.6, "pulling through the base must produce a negative offset");
    near(h.state().shown().planes.values().next().unwrap().plane.origin.z, plane_distance(&h));
    accept(&mut h);
    assert_eq!(h.state().session.built.planes.len(), 1);
}

#[test]
fn offset_to_face_uses_signed_world_gap_on_a_rotated_component() {
    let mut h = state_harness();
    let owner = new_component(&mut h);
    plate(&mut h);
    command(&mut h, json!({"op":"move_component","id":owner,"translate":[50,20,30],"rotate":[0,90,0]}));
    run(&mut h, Action::Plane);
    h.state_mut().choose_origin(owner, OriginPlane::XY);
    set_plane_distance(&mut h, "3 mm");
    h.run_steps(3);
    run(&mut h, Action::View("top"));
    run(&mut h, Action::Fit);
    h.get_by_label("To face").click(); h.run_steps(2);
    let perpendicular = crate::view::to_screen(h.state(), DVec3::new(55.0, 30.0, 30.0));
    click(&mut h, perpendicular);
    near(plane_distance(&h), 3.0);
    assert!(matches!(&h.state().dialog, Dialog::Plane(d) if d.pick_to));
    assert!(h.state().toast.as_ref().is_some_and(|t| t.0.contains("parallel")), "a nonparallel target must explain why it cannot set the offset");
    run(&mut h, Action::View("right"));
    let top = crate::view::to_screen(h.state(), DVec3::new(60.0, 30.0, 10.0));
    click(&mut h, top);
    near(plane_distance(&h), 10.0);
    accept(&mut h);
    let id = h.state().doc().features.last().unwrap().id;
    near(h.state().session.built.planes[&id].plane.origin.x, 60.0);
    h.state_mut().edit_feature(id); h.run_steps(3);
    h.get_by_label("Clear base").click(); h.run_steps(2);
    let top = crate::view::to_screen(h.state(), DVec3::new(60.0, 30.0, 10.0));
    click(&mut h, top);
    h.get_by_label("To face").click(); h.run_steps(2);
    run(&mut h, Action::View("left"));
    let bottom = crate::view::to_screen(h.state(), DVec3::new(50.0, 30.0, 10.0));
    click(&mut h, bottom);
    near(plane_distance(&h), -10.0);
    accept(&mut h);
    near(h.state().session.built.planes[&id].plane.origin.x, 50.0);
    assert!(h.state().session.built.errors.is_empty());
}

#[test]
fn three_point_plane_viewport_picks_vertices_and_a_linked_sketch_point() {
    let mut h = state_harness();
    plate(&mut h);
    let body = h.state().session.built.bodies[0].id;
    command(&mut h, json!({"op":"create_sketch","plane":"XY"}));
    let source = h.state().doc().features.last().unwrap().id;
    command(&mut h, json!({"op":"add_geometry","sketch":source,"items":[{"type":"point","at":[60,30]}]}));
    let point = *h.state().doc().sketch(source).unwrap().points.iter().find(|(_, p)| p.distance(DVec2::new(60.0,30.0)) < 1e-6).unwrap().0;
    run(&mut h, Action::Plane);
    let Dialog::Plane(mut d) = h.state().dialog.clone() else { panic!() };
    d.kind = 2;
    h.state_mut().dialog = Dialog::Plane(d);
    run(&mut h, Action::View("iso")); run(&mut h, Action::Fit);
    h.state_mut().cam.scale *= 0.6; h.run_steps(2);
    let points = [DVec3::new(0.0, 0.0, 10.0), DVec3::new(40.0, 0.0, 10.0), DVec3::new(0.0, 20.0, 0.0)];
    for (index, point) in points.into_iter().enumerate() {
        let pos = crate::view::to_screen(h.state(), point);
        click(&mut h, pos);
        let Dialog::Plane(d) = &h.state().dialog else { panic!() };
        assert!(matches!(d.points[index], Some(PointRef::Vertex { body: picked, at, .. }) if picked == body && at.distance(point) < 1e-5), "vertex click {index} selected {:?}", d.points[index]);
    }
    accept(&mut h);
    let id = h.state().doc().features.last().unwrap().id;
    h.state_mut().edit_feature(id); h.run_steps(3);
    let Dialog::Plane(mut d) = h.state().dialog.clone() else { panic!() };
    d.pick_point = 2;
    h.state_mut().dialog = Dialog::Plane(d); h.step();
    let pos = crate::view::to_screen(h.state(), DVec3::new(60.0,30.0,0.0));
    click(&mut h, pos);
    let Dialog::Plane(d) = &h.state().dialog else { panic!() };
    assert_eq!(d.points[2], Some(PointRef::SketchPoint { sketch: source, point }));
    accept(&mut h);
    let before = h.state().session.built.planes[&id].plane;
    command(&mut h, json!({"op":"point_coordinates","sketch":source,"point":point,"x":60,"y":50}));
    let after = h.state().session.built.planes[&id].plane;
    assert!(before.normal().distance(after.normal()) > 0.01, "moving the picked sketch point must tilt the plane");
    near((DVec3::new(60.0,50.0,0.0) - after.origin).dot(after.normal()), 0.0);
}

#[test]
fn viewport_plane_picking_uses_nearest_visible_plane_and_respects_body_occlusion() {
    let mut h = state_harness();
    plate(&mut h);
    let body = h.state().session.built.bodies[0].id;
    let mut planes = Vec::new();
    for distance in [5, 15, 25] {
        command(&mut h, json!({"op":"create_plane","kind":"offset","base":"XY","distance":distance}));
        planes.push(h.state().doc().features.last().unwrap().id);
    }
    run(&mut h, Action::View("top")); run(&mut h, Action::Fit);
    let select_at_center = |h: &mut H| {
        run(h, Action::NewSketch);
        let pos = crate::view::to_screen(h.state(), DVec3::new(20.0,10.0,10.0));
        click(h, pos);
        let Mode::Sketch(id) = h.state().mode else { panic!("click should start a sketch: {:?}", h.state().toast) };
        let on = h.state().doc().sketch(id).unwrap().on;
        let world = h.state().session.built.sketch_plane(h.state().doc(), id).unwrap();
        h.state_mut().finish_sketch(); h.run_steps(2);
        (on, world.origin.z)
    };
    let (on, height) = select_at_center(&mut h);
    assert_eq!(on, Some(planes[2])); near(height, 25.0);
    // Finishing the sketch auto-hides its plane, exposing the next plane.
    let (on, height) = select_at_center(&mut h);
    assert_eq!(on, Some(planes[1])); near(height, 15.0);
    // The remaining visible plane is behind the box's top face.
    let (on, height) = select_at_center(&mut h);
    assert_eq!(on, None); near(height, 10.0);
    command(&mut h, json!({"op":"set_visible","id":body,"visible":false}));
    let (on, height) = select_at_center(&mut h);
    assert_eq!(on, Some(planes[0])); near(height, 5.0);
}

#[test]
fn component_planes_dark_screenshot_and_native_candidate_fixture() {
    let mut h = harness();
    h.state_mut().set_appearance(Appearance::Dark);
    let owner = new_component(&mut h);
    command(&mut h, json!({"op":"edit_feature","feature":owner,"name":"Bracket"}));
    plate(&mut h);
    run(&mut h, Action::Plane);
    h.state_mut().choose_origin(owner, OriginPlane::XY);
    set_plane_distance(&mut h, "15 mm"); accept(&mut h);
    let plane = h.state().doc().features.last().unwrap().id;
    command(&mut h, json!({"op":"edit_feature","feature":plane,"name":"Post support"}));
    let (_, post) = post_on_plane(&mut h, plane);
    command(&mut h, json!({"op":"set_visible","id":plane,"visible":true}));
    h.state_mut().move_component_dialog(owner);
    let Dialog::MoveComponent(mut d) = h.state().dialog.clone() else { panic!() };
    d.translate[0] = "15 mm".into(); d.rotate[2] = "20 deg".into();
    h.state_mut().dialog = Dialog::MoveComponent(d); accept(&mut h);
    assert_eq!(h.state().session.built.body(post).unwrap().component, owner);
    run(&mut h, Action::ActivateRoot);
    let insert = new_component(&mut h);
    command(&mut h, json!({"op":"edit_feature","feature":insert,"name":"Insert"}));
    command(&mut h, json!({"op":"create_sketch","plane":"XY"}));
    command(&mut h, json!({"op":"add_geometry","items":[{"type":"circle","center":[0,0],"radius":5}]}));
    command(&mut h, json!({"op":"extrude","distance":8}));
    h.state_mut().move_component_dialog(insert);
    let Dialog::MoveComponent(mut d) = h.state().dialog.clone() else { panic!() };
    d.translate[0] = "-12 mm".into();
    h.state_mut().dialog = Dialog::MoveComponent(d); accept(&mut h);
    run(&mut h, Action::ActivateRoot);
    run(&mut h, Action::View("iso")); run(&mut h, Action::Fit);
    assert_eq!(h.state().session.built.bodies.len(), 3);
    assert!(h.state().session.built.errors.is_empty());
    let file = out_dir().join("components-and-planes-0.3.0.ferr");
    fr_core::io::save(h.state().doc(), &file).unwrap();
    let restored = fr_core::Session::new(fr_core::io::load(&file).unwrap());
    assert_eq!(restored.built.bodies.len(), 3);
    assert_eq!(restored.built.planes.len(), 1);
    assert!(restored.built.errors.is_empty());
    let image = save(&mut h, "components-and-planes-dark-0.3.0.png");
    assert_eq!((image.width(), image.height()), (1440, 900));
    h.state_mut().set_appearance(Appearance::Light);
    let image = save(&mut h, "components-and-planes-light-0.3.0.png");
    assert_eq!((image.width(), image.height()), (1440, 900));
}
