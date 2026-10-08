//! Regressions for projecting a face, choosing one new profile, and repeating it.
use super::*;
use fr_core::{FeatureKind, Id, Op};

fn command(h: &mut H, value: serde_json::Value) -> serde_json::Value {
    h.state_mut().execute(&value).unwrap_or_else(|e| panic!("{value}: {e}"))
}

fn extrusion_profiles(h: &H) -> Vec<Vec<Id>> {
    let Dialog::Feature(d) = &h.state().dialog else { panic!("expected extrusion dialog") };
    d.profiles.clone()
}

fn shift_click(h: &mut H, pos: Pos2) {
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.hover_at(pos);
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT });
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn accept(h: &mut H) {
    h.run_steps(3);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "preview failed: {:?}", h.state().preview.as_ref().map(|p| &p.2));
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Dialog::None, "dialog failed: {:?}", h.state().toast);
}

/// A synthetic version of the reported workflow: a component plate, a root
/// sketch projected from its face, and a circle crossing the outer corner.
/// Projected outlines stay ordinary, fixed geometry that users can extrude.
fn projected_plate_and_circle(h: &mut H) -> (Id, Id, Id) {
    projected_plate_and_circle_at(h, DVec2::new(0.0, 50.0))
}

fn projected_plate_and_circle_at(h: &mut H, center_mm: DVec2) -> (Id, Id, Id) {
    command(h, json!({"op":"create_component","name":"Plate","activate":true}));
    command(h, json!({"op":"create_sketch","plane":"XY"}));
    command(h, json!({"op":"add_geometry","items":[
        {"type":"rect","from":[0,0],"to":[60,50]},
        {"type":"rect","from":[28,19],"to":[31,27]}
    ]}));
    let plate = command(h, json!({"op":"extrude","distance":21,"operation":"new"}))["feature"].as_u64().unwrap() as Id;
    run(h, Action::ActivateRoot);
    h.state_mut().fit();
    h.state_mut().create_sketch(Plane::XY);
    h.run_steps(2);
    let Mode::Sketch(sketch) = h.state().mode else { panic!("expected new sketch") };
    run(h, Action::Tool(Tool::Project));
    let face = at(h, 15.0, 20.0);
    click(h, face);
    assert_eq!(h.state().doc().sketch(sketch).unwrap().entities.len(), 8, "the outer plate and its hole should both project");
    run(h, Action::Tool(Tool::Circle));
    let (center, radius) = (at(h, center_mm.x, center_mm.y), at(h, center_mm.x - 11.0, center_mm.y));
    click(h, center);
    click(h, radius);
    let sk = h.state().doc().sketch(sketch).unwrap();
    let (&circle, _) = sk.entities.iter().find(|(_, e)| matches!(e.geom, Geom::Circle { .. })).expect("circle should be drawn");
    (plate, sketch, circle)
}

#[test]
fn projected_outline_and_corner_circle_require_an_explicit_extrusion_profile() {
    let mut h = state_harness();
    let (_, sketch, _) = projected_plate_and_circle(&mut h);
    assert_eq!(fr_core::profile::profiles(h.state().doc().sketch(sketch).unwrap()).iter().filter(|p| p.depth % 2 == 0).count(), 2);
    run(&mut h, Action::Extrude);
    let Dialog::Feature(d) = &h.state().dialog else { panic!("expected extrusion dialog") };
    assert!(d.profiles.is_empty(), "opening Extrude on a projected plate plus a circle must not silently select both: {:?}", d.profiles);
    assert!(d.sketch.is_none() || d.sketch == Some(sketch));
    let before = h.state().doc().clone();
    h.state_mut().apply_dialog();
    assert_eq!(h.state().doc(), &before, "unselected profiles must not create an accidental plate copy");
    assert!(matches!(h.state().dialog, Dialog::Feature(_)));
}

#[test]
fn profile_click_replaces_shift_toggles_and_new_body_contains_only_the_circle() {
    let mut h = state_harness();
    let (plate, sketch, circle) = projected_plate_and_circle(&mut h);
    let plate_volume = h.state().session.built.body(plate).unwrap().mesh.volume();
    let plate_bounds = h.state().session.built.body(plate).unwrap().mesh.bbox().unwrap();
    run(&mut h, Action::Extrude);
    run(&mut h, Action::View("top"));
    let (plate_at, circle_at) = (at(&h, 15.0, 20.0), at(&h, -5.0, 50.0));
    click(&mut h, plate_at);
    assert_eq!(extrusion_profiles(&h).len(), 1, "the projected outline remains intentionally selectable");
    assert_ne!(extrusion_profiles(&h), vec![vec![circle]]);
    click(&mut h, circle_at);
    assert_eq!(extrusion_profiles(&h), vec![vec![circle]], "a plain circle click replaces the projected outline selection");
    shift_click(&mut h, plate_at);
    assert_eq!(extrusion_profiles(&h).len(), 2, "Shift can intentionally select multiple profiles");
    shift_click(&mut h, plate_at);
    assert_eq!(extrusion_profiles(&h), vec![vec![circle]], "Shift can remove the added outline");

    h.get_by_value("Join").click();
    h.run_steps(2);
    h.get_by_label("New Body").click();
    h.run_steps(2);
    let Dialog::Feature(d) = &mut h.state_mut().dialog else { panic!() };
    d.text = "21 mm".into();
    accept(&mut h);
    let cylinder = h.state().doc().features.last().unwrap().id;
    let FeatureKind::Extrude(e) = &h.state().doc().feature(cylinder).unwrap().kind else { panic!() };
    assert_eq!((e.sketch, &e.profiles, e.op), (sketch, &vec![vec![circle]], Op::New));
    assert_eq!(h.state().session.built.bodies.len(), 2);
    let unchanged = h.state().session.built.body(plate).unwrap();
    assert_eq!(unchanged.mesh.bbox().unwrap(), plate_bounds);
    assert!((unchanged.mesh.volume() - plate_volume).abs() < 1e-7, "New Body must preserve the original plate");
    let body = h.state().session.built.body(cylinder).unwrap();
    let (lo, hi) = body.mesh.bbox().unwrap();
    assert!(lo.distance(DVec3::new(-11.0, 39.0, 0.0)) < 0.05 && hi.distance(DVec3::new(11.0, 61.0, 21.0)) < 0.05, "new body must have cylinder bounds, not plate bounds: {lo:?} {hi:?}");
    let expected = std::f64::consts::PI * 11.0 * 11.0 * 21.0;
    assert!((body.mesh.volume() - expected).abs() / expected < 0.005, "new body must have cylinder volume");

    // Selecting the cylinder face must choose that feature when Pattern opens.
    let face = crate::view::to_screen(h.state(), DVec3::new(-5.0, 50.0, 21.0));
    click(&mut h, face);
    assert_eq!(h.state().sel_face.as_ref().map(|f| f.body), Some(cylinder));
    run(&mut h, Action::Pattern);
    let Dialog::Pattern(p) = &h.state().dialog else { panic!() };
    assert_eq!(p.source, Some(cylinder), "Pattern must use the selected cylinder, not the projected plate");
}

#[test]
fn previous_circle_edge_selection_does_not_replace_explicit_region_choice() {
    let mut h = state_harness();
    let (_, _, circle) = projected_plate_and_circle(&mut h);
    run(&mut h, Action::Tool(Tool::Select));
    let edge = at(&h, -11.0, 50.0);
    click(&mut h, edge);
    assert!(h.state().sel.contains(&circle), "clicking the circle edge should select the circle entity");
    run(&mut h, Action::Extrude);
    assert!(extrusion_profiles(&h).is_empty(), "an edge left selected from sketch editing must not preselect an extrusion region");
}

#[test]
fn nested_circle_inside_projected_outline_also_requires_explicit_region_choice() {
    let mut h = state_harness();
    let (_, sketch, circle) = projected_plate_and_circle_at(&mut h, DVec2::new(15.0, 35.0));
    let profiles = fr_core::profile::profiles(h.state().doc().sketch(sketch).unwrap());
    assert_eq!(profiles.iter().filter(|p| p.depth % 2 == 0).count(), 1, "the circle is entirely inside the projected plate");
    assert_eq!(profiles.iter().find(|p| p.edges == [circle]).unwrap().depth, 1);
    run(&mut h, Action::Extrude);
    assert!(extrusion_profiles(&h).is_empty(), "one outer region must not auto-select the plate when other closed regions exist inside it");
    run(&mut h, Action::View("top"));
    let center = at(&h, 15.0, 35.0);
    click(&mut h, center);
    assert_eq!(extrusion_profiles(&h), vec![vec![circle]], "the inner circle must be picked independently of the surrounding projected region");
}

#[test]
fn selected_circle_patterns_in_two_directions_and_edits_as_one_feature() {
    let mut h = harness();
    let (plate, _, circle) = projected_plate_and_circle(&mut h);
    let plate_volume = h.state().session.built.body(plate).unwrap().mesh.volume();
    run(&mut h, Action::Extrude);
    run(&mut h, Action::View("top"));
    let circle_at = at(&h, -5.0, 50.0);
    click(&mut h, circle_at);
    assert_eq!(extrusion_profiles(&h), vec![vec![circle]]);
    let Dialog::Feature(d) = &mut h.state_mut().dialog else { panic!() };
    d.text = "21 mm".into(); d.op = Op::New;
    accept(&mut h);
    let cylinder = h.state().doc().features.last().unwrap().id;
    let face = crate::view::to_screen(h.state(), DVec3::new(-5.0, 50.0, 21.0));
    click(&mut h, face);
    run(&mut h, Action::Pattern);
    h.run_steps(3); // Let the new dialog settle before hitting its controls.
    h.get_by_label("Linear").click(); h.run_steps(3);
    assert!(matches!(&h.state().dialog, Dialog::Pattern(p) if p.kind == 1));
    h.get_by_label("Second direction").click(); h.run_steps(3);
    let Dialog::Pattern(p) = &mut h.state_mut().dialog else { panic!() };
    assert_eq!(p.source, Some(cylinder));
    assert_eq!(p.kind, 1);
    assert!(p.second);
    p.axis = 0; p.count = 2; p.text = "60 mm".into();
    p.axis2 = 1; p.count2 = 2; p.text2 = "-50 mm".into();
    save(&mut h, "projected-circle-grid-dialog.png");
    accept(&mut h);
    let pattern = h.state().doc().features.last().unwrap().id;
    let FeatureKind::Pattern(p) = &h.state().doc().feature(pattern).unwrap().kind else { panic!() };
    assert_eq!(p.source, cylinder);
    let fr_core::doc::PatternKind::Linear { count, second: Some(second), .. } = &p.kind else { panic!("two directions must be saved as one linear pattern") };
    assert_eq!((*count, second.axis, second.count), (2, 1, 2));
    let centers = |h: &H| {
        let mut found: Vec<(i32, i32, i32)> = h.state().session.built.bodies.iter().filter(|b| b.id != plate).map(|b| {
            let (lo, hi) = b.mesh.bbox().unwrap();
            assert!((hi.x-lo.x-22.0).abs() < 0.05 && (hi.y-lo.y-22.0).abs() < 0.05 && (hi.z-lo.z-21.0).abs() < 0.05, "every pattern body must be just the cylinder");
            let c = (lo + hi) * 0.5;
            (c.x.round() as i32, c.y.round() as i32, (c.z*10.0).round() as i32)
        }).collect();
        found.sort_unstable(); found
    };
    assert_eq!(h.state().session.built.bodies.len(), 5, "one plate plus a 2×2 set of cylinders");
    assert_eq!(centers(&h), vec![(0,0,105), (0,50,105), (60,0,105), (60,50,105)]);
    run(&mut h, Action::View("iso"));
    h.state_mut().fit();
    save(&mut h, "projected-circle-grid-result.png");
    assert!((h.state().session.built.body(plate).unwrap().mesh.volume() - plate_volume).abs() < 1e-7);
    run(&mut h, Action::Undo);
    assert_eq!(h.state().session.built.bodies.len(), 2, "one Undo removes the whole grid");
    run(&mut h, Action::Redo);
    assert_eq!(h.state().session.built.bodies.len(), 5);

    let count = h.state().doc().features.len();
    h.state_mut().edit_feature(pattern); h.run_steps(2);
    let Dialog::Pattern(p) = &mut h.state_mut().dialog else { panic!("timeline editing should reopen Pattern") };
    assert_eq!(p.editing, Some(pattern));
    assert!(p.second);
    p.text2 = "-80 mm".into();
    accept(&mut h);
    assert_eq!(h.state().doc().features.len(), count, "editing the grid must update its existing feature");
    assert_eq!(centers(&h), vec![(0,-30,105), (0,50,105), (60,-30,105), (60,50,105)]);
    let saved = fr_core::io::to_json(h.state().doc());
    std::fs::write(out_dir().join("projected-circle-grid.ferr"), &saved).unwrap();
    let reopened = fr_core::Session::new(fr_core::io::from_json(&saved).unwrap());
    assert_eq!(reopened.built.bodies.len(), 5);
    assert!(reopened.built.errors.is_empty());
    assert_eq!(reopened.doc.feature(pattern), h.state().doc().feature(pattern));
}

#[test]
fn editing_an_old_extrusion_removes_accidentally_selected_projected_outline() {
    let mut h = state_harness();
    let (plate, sketch, circle) = projected_plate_and_circle(&mut h);
    let all = fr_core::profile::profiles(h.state().doc().sketch(sketch).unwrap());
    let old_profiles: Vec<_> = all.iter().filter(|p| p.depth % 2 == 0).map(|p| p.edges.clone()).collect();
    assert_eq!(old_profiles.len(), 2);
    run(&mut h, Action::Extrude);
    let Dialog::Feature(d) = &mut h.state_mut().dialog else { panic!() };
    d.profiles = old_profiles; d.op = Op::New; d.text = "21 mm".into();
    accept(&mut h);
    let feature = h.state().doc().features.last().unwrap().id;
    let before = h.state().doc().clone();
    let plate_volume = h.state().session.built.body(plate).unwrap().mesh.volume();
    assert!(h.state().session.built.body(feature).unwrap().mesh.bbox().unwrap().1.x > 59.0);
    h.state_mut().edit_feature(feature); h.run_steps(2);
    run(&mut h, Action::View("top"));
    let circle_at = at(&h, -5.0, 50.0);
    click(&mut h, circle_at);
    assert_eq!(extrusion_profiles(&h), vec![vec![circle]]);
    accept(&mut h);
    assert_eq!(h.state().doc().features.len(), before.features.len());
    assert_eq!(h.state().session.built.bodies.len(), 2);
    assert!(h.state().session.built.body(feature).unwrap().mesh.bbox().unwrap().1.x < 11.05);
    assert!((h.state().session.built.body(plate).unwrap().mesh.volume() - plate_volume).abs() < 1e-7);
    run(&mut h, Action::Undo);
    assert_eq!(h.state().doc(), &before);
    run(&mut h, Action::Redo);
    assert!(h.state().session.built.body(feature).unwrap().mesh.bbox().unwrap().1.x < 11.05);
}
