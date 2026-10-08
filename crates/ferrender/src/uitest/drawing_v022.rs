//! Interaction regressions for the 0.2.2 endpoint-first arcs and lasting angle locks.
use super::*;

fn drawing_harness<'a>() -> H<'a> {
    let mut h = state_harness();
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().cam.scale = 10.0;
    h.run_steps(2);
    h
}

fn text(h: &mut H, value: &str) {
    h.event(Event::Text(value.into()));
    h.run_steps(2);
}

fn click_shift(h: &mut H, pos: Pos2) {
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.hover_at(pos);
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT });
        h.step();
    }
}

fn arc_endpoints(sk: &fr_core::Sketch, arc: fr_core::Id) -> [DVec2; 2] {
    let Geom::Arc { s, e, .. } = sk.entities[&arc].geom else { panic!("expected a circular arc") };
    [sk.pos(s), sk.pos(e)]
}

fn assert_arc_endpoints(sk: &fr_core::Sketch, arc: fr_core::Id, expected: [DVec2; 2]) {
    let ends = arc_endpoints(sk, arc);
    assert!(expected.iter().all(|p| ends.iter().any(|q| p.distance(*q) < 1e-5)), "endpoints moved: {ends:?}, expected {expected:?}");
}

#[test]
fn endpoint_first_arc_follows_bulge_above_and_below_without_moving_ends() {
    for side in [1.0, -1.0] {
        let mut h = drawing_harness();
        run(&mut h, Action::Tool(Tool::Arc3));
        let expected = [DVec2::new(-20.0, 0.0), DVec2::new(-10.0, 0.0)];
        for p in expected { let at = at(&h, p.x, p.y); click(&mut h, at); }
        assert_eq!(h.state().clicks.len(), 2);
        assert!(h.state().sketch().unwrap().1.entities.is_empty());
        // Both preview directions must retain the first two clicks as endpoints.
        for y in [5.0, -5.0, side * 5.0] {
            let p = at(&h, -15.0, y);
            h.hover_at(p);
            h.run_steps(2);
            assert!(h.state().clicks.iter().zip(expected).all(|(click, p)| click.p.distance(p) < 1e-5));
        }
        let p = at(&h, -15.0, side * 5.0);
        click(&mut h, p);
        let (_, sk) = h.state().sketch().unwrap();
        assert_eq!(sk.entities.len(), 1);
        let arc = *sk.entities.keys().next().unwrap();
        assert_arc_endpoints(sk, arc, expected);
        let fr_core::sketch::ArcGuide::Through { point } = sk.arc_guides[&arc] else { panic!("arc should retain its through point") };
        assert!(sk.pos(point).distance(DVec2::new(-15.0, side * 5.0)) < 1e-5);
        let points = sk.polyline(arc);
        assert!(points.iter().all(|p| p.y * side >= -1e-5), "arc chose the wrong side: {points:?}");
        assert!(points.iter().any(|p| p.y * side > 4.9));
        assert!((sk.curve(arc).unwrap().1 - 5.0).abs() < 1e-5);
        assert!(h.state().report.ok);
        let completed = h.state().doc().clone();
        run(&mut h, Action::Undo);
        assert!(h.state().sketch().unwrap().1.entities.is_empty());
        run(&mut h, Action::Redo);
        assert_eq!(h.state().doc(), &completed);
    }
}

#[test]
fn arc_diameter_refuses_less_than_chord_and_keeps_the_gesture_editable() {
    let mut h = drawing_harness();
    run(&mut h, Action::Tool(Tool::Arc3));
    let expected = [DVec2::new(-20.0, 0.0), DVec2::new(-10.0, 0.0)];
    for p in expected { let pos = at(&h, p.x, p.y); click(&mut h, pos); }
    let bulge = at(&h, -15.0, 4.0);
    h.hover_at(bulge);
    h.run_steps(3);
    assert_eq!(h.state().typed.as_ref().unwrap().fields.len(), 1);
    let before = h.state().doc().clone();
    text(&mut h, "8 mm");
    key(&mut h, Key::Enter);
    assert_eq!(h.state().doc(), &before, "an impossible diameter must not leave a partial arc or points");
    assert_eq!(h.state().clicks.len(), 2, "keep the endpoints so the user can correct the diameter");
    assert!(h.state().toast.as_ref().is_some_and(|(message, _)| message.contains("diameter") && message.contains("endpoints")), "expected visible chord/diameter error: {:?}", h.state().toast);
    h.state_mut().typed.as_mut().unwrap().fields[0] = "14 mm".into();
    h.run_steps(2);
    key(&mut h, Key::Enter);
    let (_, sk) = h.state().sketch().unwrap();
    assert_eq!(sk.entities.len(), 1, "correcting the input should finish the original arc");
    let arc = *sk.entities.keys().next().unwrap();
    assert_arc_endpoints(sk, arc, expected);
    assert!((sk.curve(arc).unwrap().1 - 7.0).abs() < 1e-5);
    assert!(sk.constraints.values().any(|c| c.kind == CKind::Diameter && c.refs == [arc] && (c.value.as_ref().unwrap().v - 14.0).abs() < 1e-6));
    assert!(h.state().report.ok);
}

#[test]
fn shift_line_lock_uses_pointer_modifiers_and_survives_drag_undo_and_save() {
    let mut h = drawing_harness();
    run(&mut h, Action::Tool(Tool::Line));
    let start = at(&h, -20.0, -10.0);
    click(&mut h, start);
    let initial = at(&h, -6.0, -3.0);
    h.hover_at(initial);
    h.run_steps(2);
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.step();
    let held = h.state().typed.as_ref().unwrap().locked.expect("pressing Shift should capture the current direction");
    assert!((held - (7.0_f64 / 14.0).atan().to_degrees()).abs() < 1e-5);
    let changed = at(&h, -2.0, 4.0);
    h.hover_at(changed);
    h.run_steps(2);
    assert_eq!(h.state().typed.as_ref().unwrap().locked, Some(held));
    click_shift(&mut h, changed);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    run(&mut h, Action::Tool(Tool::Select));
    let (sid, sk) = h.state().sketch().unwrap();
    let line = *sk.entities.keys().next().unwrap();
    assert_eq!(sk.entities.len(), 1);
    assert!(sk.constraints.values().any(|c| c.kind == CKind::Angle && c.refs == [line] && (c.value.as_ref().unwrap().v - held).abs() < 1e-5));
    assert!((sk.measure(CKind::Angle, &[line]) - held).abs() < 1e-5);
    let completed = h.state().doc().clone();
    run(&mut h, Action::Undo);
    assert!(h.state().sketch().unwrap().1.entities.is_empty());
    run(&mut h, Action::Redo);
    assert_eq!(h.state().doc(), &completed);
    let (a, b) = h.state().sketch().unwrap().1.line(line).unwrap();
    let from = at(&h, b.x, b.y);
    let to = at(&h, b.x + 3.0, b.y + 6.0);
    drag(&mut h, &[from, from + egui::vec2(12.0, -12.0), to]);
    let (_, sk) = h.state().sketch().unwrap();
    assert!((sk.measure(CKind::Angle, &[line]) - held).abs() < 1e-4, "dragging may resize or translate the line, but cannot release its angle");
    let new_line = sk.line(line).unwrap();
    assert!(new_line.0.distance(a) > 1e-4 || new_line.1.distance(b) > 1e-4, "the test must exercise a real constrained drag");
    let encoded = fr_core::io::validated_json(h.state().doc()).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&encoded).unwrap()["version"], 4);
    let restored = fr_core::io::from_json(&encoded).unwrap();
    let saved_sketch = restored.sketch(sid).unwrap();
    let live_sketch = h.state().sketch().unwrap().1;
    assert!((saved_sketch.measure(CKind::Angle, &[line]) - held).abs() < 1e-4);
    assert_eq!(saved_sketch.constraints, live_sketch.constraints);
    assert_eq!(saved_sketch.entities, live_sketch.entities);
    assert_eq!(saved_sketch.points.len(), live_sketch.points.len());
    for (id, point) in &live_sketch.points {
        assert!(saved_sketch.pos(*id).distance(*point) < 1e-9, "native roundtrip moved point {id}");
    }
}

#[test]
fn typed_line_direction_accepts_zero_and_signed_angles_as_lasting_dimensions() {
    for (angle, value) in [("0 deg", 0.0_f64), ("-30 deg", -30.0_f64)] {
        let mut h = drawing_harness();
        run(&mut h, Action::Tool(Tool::Line));
        let origin = at(&h, 0.0, 0.0);
        click(&mut h, origin);
        let pointer = at(&h, 15.0, 8.0);
        h.hover_at(pointer);
        h.run_steps(3);
        text(&mut h, "20 mm");
        key(&mut h, Key::Tab);
        h.run_steps(2);
        assert_eq!(h.state().typed.as_ref().unwrap().active, 1);
        text(&mut h, angle);
        key(&mut h, Key::Enter);
        let (_, sk) = h.state().sketch().unwrap();
        assert_eq!(sk.entities.len(), 1, "typed {angle} should place the line: {:?}", h.state().toast);
        let line = *sk.entities.keys().next().unwrap();
        let (a, b) = sk.line(line).unwrap();
        assert!(a.length() < 1e-6, "typing must not move the placed start point");
        assert!(b.distance(DVec2::from_angle(value.to_radians()) * 20.0) < 1e-5, "{angle}: end {b:?}");
        assert!(sk.constraints.values().any(|c| c.kind == CKind::Distance && c.refs == [line]));
        assert!(sk.constraints.values().any(|c| c.kind == CKind::Angle && c.refs == [line] && (c.value.as_ref().unwrap().v - value).abs() < 1e-6));
        assert!(!sk.constraints.values().any(|c| matches!(c.kind, CKind::Horizontal | CKind::Vertical)), "an explicit angle should not acquire a competing auto-axis constraint");
        assert!(h.state().report.ok);
        let encoded = fr_core::io::validated_json(h.state().doc()).unwrap();
        assert_eq!(fr_core::io::from_json(&encoded).unwrap(), *h.state().doc());
    }
}

#[test]
fn typed_tangent_arc_sweep_keeps_both_turn_directions_and_rejects_full_circle() {
    for side in [1.0, -1.0] {
        let mut h = drawing_harness();
        run(&mut h, Action::Tool(Tool::Line));
        for (x, y) in [(-20.0, -10.0), (-10.0, -10.0)] { let pos = at(&h, x, y); click(&mut h, pos); }
        run(&mut h, Action::Tool(Tool::Select));
        let line = *h.state().sketch().unwrap().1.entities.keys().next().unwrap();
        h.state_mut().sel = vec![line];
        run(&mut h, Action::Tool(Tool::TangentArc));
        h.run_steps(30);
        let start = at(&h, -10.0, -10.0);
        click(&mut h, start);
        let target = at(&h, 0.0, -10.0 + side * 10.0);
        h.hover_at(target);
        h.run_steps(3);
        assert_eq!(h.state().typed.as_ref().unwrap().fields.len(), 1);
        let before = h.state().doc().clone();
        text(&mut h, "360 deg");
        key(&mut h, Key::Enter);
        assert_eq!(h.state().doc(), &before);
        assert!(h.state().toast.as_ref().is_some_and(|(message, _)| message.contains("sweep")), "expected a visible invalid sweep error: {:?}", h.state().toast);
        h.state_mut().typed.as_mut().unwrap().fields[0] = "90 deg".into();
        h.run_steps(2);
        key(&mut h, Key::Enter);
        let (_, sk) = h.state().sketch().unwrap();
        assert_eq!(sk.entities.len(), 2, "typed sweep should place an arc: {:?}", h.state().toast);
        let arc = *sk.entities.iter().find(|(_, e)| matches!(e.geom, Geom::Arc { .. })).unwrap().0;
        assert!(sk.constraints.values().any(|c| c.kind == CKind::Tangent));
        assert!(sk.constraints.values().any(|c| c.kind == CKind::Angle && c.refs == [arc] && (c.value.as_ref().unwrap().v - 90.0).abs() < 1e-6));
        assert!((sk.measure(CKind::Angle, &[arc]) - 90.0).abs() < 1e-5);
        assert!(sk.polyline(arc).iter().all(|p| (p.y + 10.0) * side >= -1e-5), "typed positive sweep should retain the pointer's turn direction");
        let source = sk.line(line).unwrap();
        assert!(source.0.distance(DVec2::new(-20.0, -10.0)) < 1e-5);
        assert!(source.1.distance(DVec2::new(-10.0, -10.0)) < 1e-5);
        assert!(h.state().report.ok);
        let completed = h.state().doc().clone();
        run(&mut h, Action::Undo);
        assert_eq!(h.state().doc(), &before);
        run(&mut h, Action::Redo);
        assert_eq!(h.state().doc(), &completed);
    }
}

#[test]
fn shift_tangent_sweep_persists_only_when_held_through_placement() {
    for keep_shift in [true, false] {
        let mut h = drawing_harness();
        run(&mut h, Action::Tool(Tool::Line));
        for (x, y) in [(-20.0, -10.0), (-10.0, -10.0)] { let p = at(&h, x, y); click(&mut h, p); }
        run(&mut h, Action::Tool(Tool::Select));
        let line = *h.state().sketch().unwrap().1.entities.keys().next().unwrap();
        h.state_mut().sel = vec![line];
        run(&mut h, Action::Tool(Tool::TangentArc));
        h.run_steps(30);
        let start = at(&h, -10.0, -10.0);
        click(&mut h, start);
        let quarter_turn = at(&h, 0.0, 0.0);
        h.hover_at(quarter_turn);
        h.run_steps(2);
        h.event(Event::ModifiersChanged(Modifiers::SHIFT));
        h.step();
        let held = h.state().typed.as_ref().unwrap().locked.expect("Shift should lock the displayed tangent sweep");
        assert!((held - 90.0).abs() < 1e-5);
        let moved = at(&h, 3.0, -6.0);
        h.hover_at(moved);
        h.run_steps(2);
        assert_eq!(h.state().typed.as_ref().unwrap().locked, Some(held));
        if keep_shift {
            click_shift(&mut h, moved);
        } else {
            h.event(Event::ModifiersChanged(Modifiers::NONE));
            h.step();
            assert_eq!(h.state().typed.as_ref().unwrap().locked, None);
            click(&mut h, moved);
        }
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.step();
        let (sid, sk) = h.state().sketch().unwrap();
        assert_eq!(sk.entities.len(), 2, "the gesture must commit one tangent arc: {:?}", h.state().toast);
        let arc = *sk.entities.iter().find(|(_, e)| matches!(e.geom, Geom::Arc { .. })).unwrap().0;
        assert!(sk.constraints.values().any(|c| c.kind == CKind::Tangent));
        let angle = sk.constraints.values().find(|c| c.kind == CKind::Angle && c.refs == [arc]);
        assert_eq!(angle.is_some(), keep_shift, "releasing Shift before placement must release the lasting lock too");
        if keep_shift {
            assert!((angle.unwrap().value.as_ref().unwrap().v - held).abs() < 1e-5);
            assert!((sk.measure(CKind::Angle, &[arc]) - held).abs() < 1e-5);
        } else {
            assert!((sk.measure(CKind::Angle, &[arc]) - held).abs() > 20.0, "after release the arc should follow the new pointer direction");
        }
        assert!(h.state().report.ok);
        let encoded = fr_core::io::validated_json(h.state().doc()).unwrap();
        let restored = fr_core::io::from_json(&encoded).unwrap();
        assert_eq!(restored, *h.state().doc());
        assert_eq!(restored.sketch(sid).unwrap().constraints.values().filter(|c| c.kind == CKind::Angle && c.refs == [arc]).count(), usize::from(keep_shift));
        let complete = h.state().doc().clone();
        run(&mut h, Action::Undo);
        assert_eq!(h.state().sketch().unwrap().1.entities.len(), 1);
        run(&mut h, Action::Redo);
        assert_eq!(h.state().doc(), &complete);
    }
}

#[test]
fn endpoint_first_arc_dark_screenshot_and_native_fixture() {
    let mut h = harness();
    h.state_mut().set_appearance(Appearance::Dark);
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().cam.scale = 14.0;
    h.run_steps(2);
    run(&mut h, Action::Tool(Tool::Arc3));
    for (x, y) in [(-15.0, 0.0), (15.0, 0.0), (0.0, 15.0)] { let pos = at(&h, x, y); click(&mut h, pos); }
    run(&mut h, Action::Tool(Tool::Select));
    let (_, sk) = h.state().sketch().unwrap();
    let arc = *sk.entities.keys().next().unwrap();
    assert_arc_endpoints(sk, arc, [DVec2::new(-15.0, 0.0), DVec2::new(15.0, 0.0)]);
    assert!(sk.polyline(arc).iter().all(|p| p.y >= -1e-5));
    assert!(h.state().report.ok);
    let fixture = out_dir().join("endpoint-first-arc-022.ferr");
    fr_core::io::save(h.state().doc(), &fixture).unwrap();
    assert_eq!(fr_core::io::load(&fixture).unwrap(), *h.state().doc());
    let image = save(&mut h, "endpoint-first-arc-dark-022.png");
    let background = at(&h, 0.0, -15.0) + egui::vec2(7.0, 7.0);
    let pixel = image.get_pixel(background.x as u32, background.y as u32);
    assert!(pixel[0] < 70 && pixel[1] < 70 && pixel[2] < 80, "the screenshot must use the dark canvas: {pixel:?}");
}
