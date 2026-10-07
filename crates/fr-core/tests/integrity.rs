use fr_core::{Document, Session, Unit, api::execute, expr};
use serde_json::{Value, json};

fn run(s: &mut Session, command: Value) -> Value {
    execute(s, &command, None).unwrap_or_else(|error| panic!("{command}: {error}"))
}

fn rect(s: &mut Session, from: [f64; 2], to: [f64; 2], distance: Value, operation: &str) -> u64 {
    run(s, json!({"op":"create_sketch"}));
    run(
        s,
        json!({"op":"add_geometry","items":[{"type":"rect","from":from,"to":to}]}),
    );
    run(
        s,
        json!({"op":"extrude","distance":distance,"operation":operation}),
    )["feature"]
        .as_u64()
        .unwrap()
}

#[test]
fn display_units_preserve_mixed_expressions_and_parameter_updates() {
    let mut s = Session::default();
    run(
        &mut s,
        json!({"op":"set_parameter","name":"w","expr":"10 mm"}),
    );
    run(
        &mut s,
        json!({"op":"set_parameter","name":"clearance","expr":"$w + 2"}),
    );
    rect(
        &mut s,
        [0., 0.],
        [10., 10.],
        json!("$clearance + 3 * 2"),
        "new",
    );
    for units in ["in", "cm", "mm"] {
        run(&mut s, json!({"op":"set_units","units":units}));
        assert!((s.built.bodies[0].mesh.bbox().unwrap().1.z - 18.).abs() < 1e-8);
        assert_eq!(s.doc.eval("$clearance", expr::Kind::Length).unwrap(), 12.);
    }
    run(
        &mut s,
        json!({"op":"set_parameter","name":"w","expr":"20 mm"}),
    );
    assert!((s.built.bodies[0].mesh.bbox().unwrap().1.z - 28.).abs() < 1e-8);
}

#[test]
fn pinning_preserves_nested_arithmetic_and_dimensionless_factors() {
    let mut d = Document::new(Unit::Cm);
    d.set_param("w", "10 mm").unwrap();
    d.set_param("factor", "2").unwrap();
    for source in [
        "$w + 2",
        "2 + $w",
        "1 + 2 + $w",
        "($w + 2) / $factor",
        "abs($w - 2)",
        "(($w + 2) / 1 mm + 2) * 1 mm",
        "(2 + $w) / (2 + $w)",
    ] {
        let original = d.quantity(source).unwrap();
        let kind = if original.dim == expr::Dim::None {
            expr::Kind::Scalar
        } else {
            expr::Kind::Length
        };
        let pinned = d.value(source, kind).unwrap();
        d.units = Unit::In;
        assert!(
            (d.eval(&pinned.expr, kind).unwrap() - original.v).abs() < 1e-8,
            "{source} -> {}",
            pinned.expr
        );
        d.units = Unit::Cm;
    }
    d.units = Unit::In;
    assert_eq!(d.quantity("$factor").unwrap().v, 2.);
}

#[test]
fn legacy_documents_pin_their_existing_unit_context_on_open() {
    let mut s = Session::default();
    rect(&mut s, [0., 0.], [10., 10.], json!(12), "new");
    let mut legacy = serde_json::to_value(&s.doc).unwrap();
    legacy["format"] = json!("ferrender");
    legacy["version"] = json!(1);
    legacy["features"][1]["kind"]["extrude"]["distance"]["expr"] = json!("10 mm + 2");
    let doc = fr_core::io::from_json(&legacy.to_string()).unwrap();
    let mut reopened = Session::new(doc);
    run(&mut reopened, json!({"op":"set_units","units":"in"}));
    assert!((reopened.built.bodies[0].mesh.bbox().unwrap().1.z - 12.).abs() < 1e-8);
    let round_trip = fr_core::io::from_json(&fr_core::io::to_json(&reopened.doc)).unwrap();
    let again = Session::new(round_trip);
    assert!((again.built.bodies[0].mesh.bbox().unwrap().1.z - 12.).abs() < 1e-8);
}

#[test]
fn failed_multi_hole_feature_has_no_partial_effect() {
    let mut s = Session::default();
    let body = rect(&mut s, [0., 0.], [20., 10.], json!(20), "new");
    let cut = rect(&mut s, [10., 0.], [20., 10.], json!(15), "cut");
    let hole = run(&mut s, json!({"op":"hole","body":body,"at":[[5,5,20],[15,5,20]],"thread":"M3","fit":"tapped","modeled":true,"type":"counterbore","head_depth":3,"through":true}))["feature"].as_u64().unwrap();
    run(
        &mut s,
        json!({"op":"edit_feature","feature":cut,"distance":18}),
    );
    assert!(s.built.errors.contains_key(&(hole as u32)));
    let failed = s.built.bodies[0].mesh.clone();
    assert!(
        (s.built.bodies[0]
            .solids
            .iter()
            .map(|solid| solid.volume())
            .sum::<f64>()
            - 2200.)
            .abs()
            < 1e-5
    );
    run(
        &mut s,
        json!({"op":"edit_feature","feature":hole,"suppressed":true}),
    );
    assert_eq!(failed.tris, s.built.bodies[0].mesh.tris);
}

#[test]
fn move_reports_bodies_even_when_volume_and_triangle_count_are_unchanged() {
    let mut s = Session::default();
    let body = rect(&mut s, [10., 10.], [20., 20.], json!(10), "new");
    let result = run(
        &mut s,
        json!({"op":"move","sketch":1,"ids":[5,7,9,11],"by":[10,0]}),
    );
    assert_eq!(result["changed_bodies"][0]["id"], body);
    assert_eq!(result["changed_bodies"][0]["min"][0], 20.);
    let unchanged = run(
        &mut s,
        json!({"op":"move","sketch":1,"ids":[5,7,9,11],"by":[0,0]}),
    );
    assert_eq!(unchanged["changed_bodies"], json!([]));
}

#[test]
fn rejected_feature_and_cancelled_drag_preserve_redo_and_clean_state() {
    let mut s = Session::default();
    rect(&mut s, [0., 0.], [10., 10.], json!(10), "new");
    run(&mut s, json!({"op":"set_parameter","name":"n","expr":"7"}));
    assert!(s.undo());
    // Equivalent to saving the currently displayed state.
    s.dirty = false;
    let doc = s.doc.clone();
    assert!(execute(&mut s, &json!({"op":"extrude","distance":0}), None).is_err());
    assert_eq!(s.doc, doc);
    assert!(!s.dirty);
    assert!(s.can_redo());
    s.snapshot();
    s.doc.set_param("drag", "2").unwrap();
    s.abort();
    assert_eq!(s.doc, doc);
    assert!(!s.dirty);
    assert!(s.redo());
    assert_eq!(s.doc.quantity("$n").unwrap().v, 7.);
}

#[test]
fn rejected_edit_at_history_limit_does_not_discard_oldest_undo() {
    let mut s = Session::default();
    for n in 0..200 {
        s.edit(|doc| doc.set_param("n", &n.to_string())).unwrap();
    }
    assert!(s.edit::<()>(|_| Err("rejected".into())).is_err());
    for _ in 0..200 {
        assert!(s.undo());
    }
    assert!(s.doc.params.is_empty());
    assert!(!s.can_undo());
}

#[test]
fn expressions_reject_excessive_size_nesting_and_nonfinite_conversions() {
    let d = Document::new(Unit::Mm);
    let small = d.value("0.0000001", expr::Kind::Length).unwrap();
    assert_eq!(d.eval(&small.expr, expr::Kind::Length).unwrap(), 0.0000001);
    for source in [
        " ".repeat(4097),
        format!("{}1{}", "(".repeat(64), ")".repeat(64)),
        format!("{}1", "-".repeat(64)),
    ] {
        assert!(d.quantity(&source).is_err());
    }
    let at_limit = format!("{}1{}", "(".repeat(31), ")".repeat(31));
    assert!(d.quantity(&at_limit).is_ok());
    assert!(
        d.value(&at_limit, expr::Kind::Length).is_err(),
        "a stored unit wrapper must not create an unevaluable expression"
    );
    assert!(
        expr::to_kind(
            expr::Quantity {
                v: f64::MAX,
                dim: expr::Dim::None
            },
            expr::Kind::Length,
            Unit::In
        )
        .is_err()
    );
    let mut d = Document::new(Unit::Mm);
    d.set_param("p0", "1").unwrap();
    for n in 1..23 {
        d.set_param(&format!("p{n}"), &format!("$p{} + $p{}", n - 1, n - 1))
            .unwrap();
    }
    assert_eq!(d.quantity("$p22").unwrap().v, 2f64.powi(22));
    assert!(d.set_param("cycle", "$cycle").is_err());
}
