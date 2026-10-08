use fr_core::{api::execute, io, solver, CKind, Document, FeatureKind, Geom, Plane, Session, Sketch, Unit, Value};
use glam::DVec2;
use serde_json::json;

fn angle(v: f64) -> Value { Value { expr: format!("{v} deg"), v } }
fn near(a: f64, b: f64) { assert!((a-b).abs() < 2e-5, "{a} != {b}"); }

#[test]
fn single_line_angle_persists_during_drag_including_signed_zero_and_wrapped_values() {
    for degrees in [-179.0_f64, -45.0, 0.0, 90.0, 179.0, 270.0, 720.0, 1e12, -1e12, 1e308] {
        let mut sk = Sketch::new(Plane::XY);
        let end = sk.add_point(DVec2::from_angle(degrees.rem_euclid(360.0).to_radians()) * 10.0);
        let line = sk.add_line(0, end);
        sk.add_constraint(CKind::Angle, &[line], Some(angle(degrees))).unwrap();
        let direction = DVec2::from_angle(degrees.rem_euclid(360.0).to_radians());
        let target = direction * 15.0 + direction.perp() * 7.0;
        assert!(solver::solve(&mut sk, &[(end, target)]).ok);
        let direction = sk.pos(end).normalize();
        near(direction.dot(DVec2::from_angle(degrees.rem_euclid(360.0).to_radians())), 1.0);
        sk.validate().unwrap();
        let clip = sk.copy(&[line]);
        clip.validate().unwrap();
        let mut copy = Sketch::new(Plane::XY);
        let pasted = copy.paste(&clip, DVec2::new(3.0, 2.0));
        assert!(solver::solve(&mut copy, &[]).ok);
        near(DVec2::from_angle(copy.measure(CKind::Angle, &[pasted[0]]).to_radians())
            .dot(DVec2::from_angle(degrees.rem_euclid(360.0).to_radians())), 1.0);
    }
}

#[test]
fn tangent_arc_sweep_lock_preserves_clockwise_counterclockwise_and_major_arcs() {
    for sign in [-1.0, 1.0] {
        for sweep in [45.0_f64, 90.0, 180.0, 270.0] {
            let mut sk = Sketch::new(Plane::XY);
            let back = sk.add_point(DVec2::new(-10.0, 0.0));
            sk.fixed.insert(back);
            let source = sk.add_line(back, 0);
            let end = sk.add_point(DVec2::from_angle(sign * sweep.to_radians() / 2.0) * 10.0);
            let arc = sk.add_tangent_arc(source, 0, end, false).unwrap();
            near(sk.measure(CKind::Angle, &[arc]), sweep);
            sk.add_constraint(CKind::Angle, &[arc], Some(angle(sweep))).unwrap();
            let target = sk.pos(end) * 1.7 + DVec2::new(0.0, sign * 2.0);
            assert!(solver::solve(&mut sk, &[(end, target)]).ok);
            near(sk.measure(CKind::Angle, &[arc]), sweep);
            near(sk.endpoint_tangent(source, 0).unwrap().dot(-sk.endpoint_tangent(arc, 0).unwrap()), 1.0);
            sk.validate().unwrap();
        }
    }
}

#[test]
fn single_ref_angles_roundtrip_with_new_schema_and_old_shapes_keep_their_schema() {
    let mut doc = Document::new(Unit::Mm);
    let mut sk = Sketch::new(Plane::XY);
    let end = sk.add_point(DVec2::new(10.0, 10.0));
    let line = sk.add_line(0, end);
    let sid = doc.add_feature(FeatureKind::Sketch(sk));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&io::to_json(&doc)).unwrap()["version"], 1);
    doc.sketch_mut(sid).unwrap().add_constraint(CKind::Angle, &[line], Some(angle(45.0))).unwrap();
    let data = io::validated_json(&doc).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&data).unwrap()["version"], 4);
    assert_eq!(io::from_json(&data).unwrap(), doc);
}

#[test]
fn angle_api_accepts_signed_line_values_and_rejects_invalid_arc_sweeps_atomically() {
    let mut s = Session::default();
    execute(&mut s, &json!({"op":"create_sketch"}), None).unwrap();
    let items = execute(&mut s, &json!({"op":"add_geometry","items":[{"type":"line","from":[0,0],"to":[10,0]}]}), None).unwrap();
    let line = items["items"][0]["entities"][0].as_u64().unwrap();
    let c = execute(&mut s, &json!({"op":"add_constraint","kind":"angle","refs":[line],"value":0}), None).unwrap()["id"].as_u64().unwrap();
    for value in [-30.0, 0.0, 179.0, -179.0] {
        execute(&mut s, &json!({"op":"set_dimension","constraint":c,"value":value}), None).unwrap();
        near(s.doc.sketch(1).unwrap().measure(CKind::Angle, &[line as u32]), value);
    }
    let mut sk = Sketch::new(Plane::XY);
    let start = sk.add_point(DVec2::X * 10.0);
    let end = sk.add_point(DVec2::Y * 10.0);
    let arc = sk.add(Geom::Arc { c: 0, s: start, e: end }, false);
    let circle = sk.add(Geom::Circle { c: 0, r: 3.0 }, false);
    assert!(sk.add_constraint(CKind::Angle, &[circle], Some(angle(90.0))).is_err());
    for bad in [0.0, -90.0, 360.0, f64::NAN] {
        let before = sk.clone();
        assert!(sk.add_constraint(CKind::Angle, &[arc], Some(angle(bad))).is_err());
        assert_eq!(sk, before);
    }
    let mut s = Session::default();
    let sid = s.doc.add_feature(FeatureKind::Sketch(sk));
    let c = execute(&mut s, &json!({"op":"add_constraint","sketch":sid,"kind":"angle","refs":[arc],"value":90}), None).unwrap()["id"].as_u64().unwrap();
    let before = io::to_json(&s.doc);
    for bad in [0.0, -90.0, 360.0] {
        assert!(execute(&mut s, &json!({"op":"set_dimension","sketch":sid,"constraint":c,"value":bad}), None).is_err());
        assert_eq!(io::to_json(&s.doc), before);
    }
    let mut corrupt_doc = s.doc.clone();
    corrupt_doc.sketch_mut(sid).unwrap().constraints.get_mut(&(c as u32)).unwrap().value = Some(angle(360.0));
    assert!(io::validated_json(&corrupt_doc).is_err());
}

#[test]
fn trimming_copies_line_direction_to_surviving_fragments_and_removes_old_arc_sweep() {
    let mut sk = Sketch::new(Plane::XY);
    let end = sk.add_point(DVec2::splat(10.0));
    let line = sk.add_line(0, end);
    sk.add_constraint(CKind::Angle, &[line], Some(angle(45.0))).unwrap();
    for x in [3.0, 7.0] {
        let a = sk.add_point(DVec2::new(x, -5.0));
        let b = sk.add_point(DVec2::new(x, 15.0));
        sk.add(Geom::Line { a, b }, true);
    }
    sk.trim(line, DVec2::splat(5.0)).unwrap();
    let fragments: Vec<_> = sk.entities.iter().filter(|(_, e)| !e.construction).map(|(id, _)| *id).collect();
    assert_eq!(fragments.len(), 2);
    for fragment in fragments {
        let value = sk.constraints.values().find(|c| c.kind == CKind::Angle && c.refs == [fragment]).unwrap().value.as_ref().unwrap();
        near(value.v, 45.0);
        assert_eq!(value.expr, "45 deg");
    }
    sk.validate().unwrap();
    assert!(solver::solve(&mut sk, &[]).ok);

    let mut sk = Sketch::new(Plane::XY);
    let start = sk.add_point(DVec2::X * 10.0);
    let end = sk.add_point(DVec2::Y * 10.0);
    let arc = sk.add(Geom::Arc { c: 0, s: start, e: end }, false);
    sk.add_constraint(CKind::Angle, &[arc], Some(angle(90.0))).unwrap();
    let a = sk.add_point(DVec2::new(5.0, -5.0));
    let b = sk.add_point(DVec2::new(5.0, 15.0));
    sk.add(Geom::Line { a, b }, true);
    sk.trim(arc, DVec2::new(9.0, 1.0)).unwrap();
    assert!(sk.constraints.values().all(|c| c.kind != CKind::Angle));
    near(sk.measure(CKind::Angle, &[arc]), 30.0);
    sk.validate().unwrap();
    assert!(solver::solve(&mut sk, &[]).ok);
    near(sk.measure(CKind::Angle, &[arc]), 30.0);
}
