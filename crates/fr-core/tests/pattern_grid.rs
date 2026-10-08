use fr_core::{api, io, Body, Document, FeatureKind, Kind, Session};
use fr_core::doc::{LinearDirection, Pattern, PatternKind};
use fr_core::exact::Place;
use glam::DVec3;
use serde_json::{json, Value};

fn run(s: &mut Session, command: Value) -> Value {
    api::execute(s, &command, None).unwrap_or_else(|error| panic!("{command}: {error}"))
}
fn id(reply: &Value, key: &str) -> u32 { reply[key].as_u64().unwrap() as u32 }
fn cylinder(s: &mut Session, center: [f64;2], z: f64) -> u32 {
    run(s, json!({"op":"create_sketch", "plane":"XY", "offset":z}));
    run(s, json!({"op":"add_geometry", "items":[{"type":"circle", "center":center, "radius":3}]}));
    id(&run(s, json!({"op":"extrude", "distance":6, "operation":"new"})), "feature")
}
fn near(actual: f64, expected: f64) { assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}"); }
fn point_near(actual: DVec3, expected: DVec3) { assert!(actual.distance(expected) < 1e-5, "{actual:?} != {expected:?}"); }
fn center(body: &Body) -> DVec3 { let (lo, hi) = body.mesh.bbox().unwrap(); (lo + hi) / 2. }
fn volume(body: &Body) -> f64 { body.solids.iter().map(|solid| solid.volume()).sum() }

#[test]
fn two_by_two_cylinders_fill_plate_corners_without_repeating_or_joining_the_plate() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"create_sketch"}));
    run(&mut s, json!({"op":"add_geometry", "items":[{"type":"rect", "from":[0,0], "to":[100,60]}]}));
    let plate = id(&run(&mut s, json!({"op":"extrude", "distance":2, "operation":"new"})), "feature");
    let source = cylinder(&mut s, [10.,10.], 2.);
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":2, "spacing":80, "axis2":"y", "count2":2, "spacing2":40})), "feature");
    assert!(s.built.errors.is_empty());
    assert_eq!(s.built.bodies.len(), 5, "one plate and four separate cylinders must remain");
    near(volume(s.built.body(plate).unwrap()), 100. * 60. * 2.);
    point_near(center(s.built.body(plate).unwrap()), DVec3::new(50.,30.,1.));
    for (body, expected) in [
        (source, DVec3::new(10.,10.,5.)),
        (pattern * 1000 + 1, DVec3::new(90.,10.,5.)),
        (pattern * 1000 + 2, DVec3::new(10.,50.,5.)),
        (pattern * 1000 + 3, DVec3::new(90.,50.,5.)),
    ] {
        let body = s.built.body(body).unwrap();
        point_near(center(body), expected);
        near(volume(body), 54. * std::f64::consts::PI);
    }
    let info = run(&mut s, json!({"op":"get_object_info", "id":pattern}));
    assert_eq!(info["pattern"]["axis2"], "y");
    assert_eq!(info["pattern"]["count2"], 2);
    assert_eq!(info["pattern"]["spacing2"], "40 mm");
    let encoded = io::validated_json(&s.doc).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap()["version"], 6);
    let reopened = Session::new(io::from_json(&encoded).unwrap());
    assert_eq!(s.doc, reopened.doc);
    assert_eq!(reopened.built.bodies.len(), 5);
    for old in &s.built.bodies {
        point_near(center(reopened.built.body(old.id).unwrap()), center(old));
        near(volume(reopened.built.body(old.id).unwrap()), volume(old));
    }
}

#[test]
fn negative_spacing_and_parameter_changes_use_the_sources_component_frame() {
    let mut s = Session::default();
    let owner = id(&run(&mut s, json!({"op":"create_component", "name":"Corner posts"})), "component");
    let source = cylinder(&mut s, [10.,20.], 2.);
    run(&mut s, json!({"op":"move_component", "id":owner, "translate":[100,200,300], "rotate":[0,0,90]}));
    run(&mut s, json!({"op":"set_parameter", "name":"row_gap", "expr":"5 mm"}));
    run(&mut s, json!({"op":"activate_component", "id":0}));
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"rectangular", "axis":"x", "count":3, "spacing":-8, "axis2":"y", "count2":2, "spacing2":"$row_gap"})), "feature");
    assert_eq!(s.doc.feature(pattern).unwrap().owner, owner);
    assert_eq!(s.built.bodies.len(), 6);
    for (copy, world) in [
        (source, DVec3::new(80.,210.,305.)),
        (pattern * 1000 + 1, DVec3::new(80.,202.,305.)),
        (pattern * 1000 + 2, DVec3::new(80.,194.,305.)),
        (pattern * 1000 + 3, DVec3::new(75.,210.,305.)),
        (pattern * 1000 + 4, DVec3::new(75.,202.,305.)),
        (pattern * 1000 + 5, DVec3::new(75.,194.,305.)),
    ] {
        let body = s.built.body(copy).unwrap();
        assert_eq!(body.component, owner);
        point_near(center(body), world);
    }
    run(&mut s, json!({"op":"set_parameter", "name":"row_gap", "expr":"-9 mm"}));
    point_near(center(s.built.body(pattern * 1000 + 3).unwrap()), DVec3::new(89.,210.,305.));
    point_near(center(s.built.body(pattern * 1000 + 5).unwrap()), DVec3::new(89.,194.,305.));
    point_near(center(s.built.body(pattern * 1000 + 1).unwrap()), DVec3::new(80.,202.,305.));
    assert!(s.built.errors.is_empty());
}

#[test]
fn legacy_single_direction_keeps_its_ids_placements_and_file_schema() {
    let mut s = Session::default();
    let source = cylinder(&mut s, [0.,0.], 0.);
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"y", "count":3, "spacing":-10})), "feature");
    assert_eq!(s.built.bodies.len(), 3);
    point_near(center(s.built.body(pattern * 1000 + 1).unwrap()), DVec3::new(0.,-10.,3.));
    point_near(center(s.built.body(pattern * 1000 + 2).unwrap()), DVec3::new(0.,-20.,3.));
    let text = io::validated_json(&s.doc).unwrap();
    assert!(!text.contains("\"second\""));
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["version"], 1);
    let reopened = Session::new(io::from_json(&text).unwrap());
    assert_eq!(s.doc, reopened.doc);
    assert_eq!(reopened.built.bodies.len(), 3);
    let info = run(&mut s, json!({"op":"get_object_info", "id":pattern}));
    assert!(info["pattern"].get("axis2").is_none());
}

#[test]
fn legacy_zero_spacing_single_direction_roundtrips_with_coincident_copies() {
    let mut s = Session::default();
    let source = cylinder(&mut s, [4.,5.], 0.);
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":3, "spacing":0})), "feature");
    let text = io::validated_json(&s.doc).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["version"], 1);
    assert!(!text.contains("\"second\""));
    let reopened = Session::new(io::from_json(&text).unwrap());
    assert_eq!(reopened.doc, s.doc);
    assert!(reopened.built.errors.is_empty());
    assert_eq!(reopened.built.bodies.len(), 3);
    for id in [source, pattern * 1000 + 1, pattern * 1000 + 2] {
        point_near(center(reopened.built.body(id).unwrap()), DVec3::new(4.,5.,3.));
        near(volume(reopened.built.body(id).unwrap()), 54. * std::f64::consts::PI);
    }
}

#[test]
fn native_pattern_expressions_are_bounded_before_evaluation() {
    let mut s = Session::default();
    let source = cylinder(&mut s, [0.,0.], 0.);
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":2, "spacing":10, "axis2":"y", "count2":2, "spacing2":20})), "feature");
    for field in ["spacing", "spacing2", "angle"] {
        for bytes in [4096, 4097] {
            let mut document = s.doc.clone();
            let FeatureKind::Pattern(pattern) = &mut document.feature_mut(pattern).unwrap().kind else { panic!() };
            if field == "angle" {
                pattern.kind = PatternKind::Circular { axis:2, count:3, angle:fr_core::Value { expr:"360 deg".into(), v:360. } };
            }
            let value = match &mut pattern.kind {
                PatternKind::Linear { spacing, second, .. } => if field == "spacing" { spacing } else { &mut second.as_mut().unwrap().spacing },
                PatternKind::Circular { angle, .. } => angle,
                _ => unreachable!(),
            };
            value.expr.push_str(&" ".repeat(bytes - value.expr.len()));
            let saved = io::validated_json(&document);
            let loaded = io::from_json(&io::to_json(&document));
            if bytes == 4096 {
                assert!(saved.is_ok() && loaded.is_ok(), "maximum-size {field} expression should remain valid");
            } else {
                assert!(saved.unwrap_err().contains("4096"), "oversized {field} was accepted on save");
                assert!(loaded.unwrap_err().contains("4096"), "oversized {field} reached document evaluation on load");
            }
        }
    }
}

#[test]
fn malformed_grid_requests_fail_without_changing_the_document() {
    let mut s = Session::default();
    let source = cylinder(&mut s, [0.,0.], 0.);
    let command = json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":2, "spacing":10, "axis2":"y", "count2":2, "spacing2":20});
    for (key, value) in [
        ("count", json!(0)), ("count", json!(-1)), ("count", json!(2.5)),
        ("count", json!(4294967298u64)), ("count2", json!(u64::MAX)),
        ("count2", json!(501)), ("count2", json!(1)),
        ("axis", json!(7)), ("axis2", json!("x")), ("axis2", json!("q")),
        ("spacing", json!(0)), ("spacing2", json!(0)), ("spacing2", json!("1e40 mm")),
        ("axis2", Value::Null), ("count2", Value::Null), ("spacing2", Value::Null),
        ("type", json!("circular")),
    ] {
        let mut invalid = command.clone();
        invalid[key] = value;
        let before = s.doc.clone();
        assert!(api::execute(&mut s, &invalid, None).is_err(), "invalid command succeeded: {invalid}");
        assert_eq!(s.doc, before, "rejected command altered history: {invalid}");
        assert_eq!(s.built.bodies.len(), 1);
    }
}

#[test]
fn grid_copy_limit_is_total_including_the_source_and_is_enforced_on_native_files() {
    let d = Document::default();
    let value = |text| d.value(text, Kind::Length).unwrap();
    let mut pattern = Pattern { source:1, kind:PatternKind::Linear {
        axis:0, count:2, spacing:value("10 mm"),
        second:Some(LinearDirection { axis:1, count:500, spacing:value("-3 mm") }),
    }};
    let places = pattern.placements().unwrap();
    assert_eq!(places.len(), 999);
    let Place::Shift(last) = places.last().unwrap() else { panic!("grid copy was not translated") };
    point_near(*last, DVec3::new(10.,-1497.,0.));
    let unique: std::collections::BTreeSet<_> = places.iter().map(|place| {
        let Place::Shift(point) = place else { panic!() };
        [point.x.to_bits(),point.y.to_bits(),point.z.to_bits()]
    }).collect();
    assert_eq!(unique.len(), 999);
    let PatternKind::Linear { count, second, .. } = &mut pattern.kind else { unreachable!() };
    *count = 1000; *second = None;
    assert_eq!(pattern.placements().unwrap().len(), 999);

    let mut s = Session::default();
    let source = cylinder(&mut s, [0.,0.], 0.);
    let feature = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":2, "spacing":10, "axis2":"y", "count2":2, "spacing2":20})), "feature");
    for problem in ["count", "axis", "spacing", "overflow"] {
        let mut bad = s.doc.clone();
        let FeatureKind::Pattern(Pattern { kind:PatternKind::Linear { count, second:Some(second), .. }, .. }) = &mut bad.feature_mut(feature).unwrap().kind else { panic!() };
        match problem {
            "count" => second.count = 501,
            "axis" => second.axis = 0,
            "spacing" => second.spacing.v = 0.,
            "overflow" => { *count = u32::MAX; second.count = u32::MAX; },
            _ => unreachable!(),
        }
        assert!(io::validated_json(&bad).is_err(), "save accepted invalid {problem}");
        assert!(io::from_json(&io::to_json(&bad)).is_err(), "load accepted invalid {problem}");
    }
    let mut future: Value = serde_json::from_str(&io::to_json(&s.doc)).unwrap();
    future["version"] = json!(io::FORMAT_VERSION + 1);
    assert!(io::from_json(&future.to_string()).unwrap_err().contains("newer version"));
}
