//! Structural estimates through the same command API used by Rhai and MCP.
use fr_core::{Id, Session, api, io, script};
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    api::execute(s, &c, None).unwrap_or_else(|e| panic!("{c}: {e}"))
}
fn number(v: &J) -> f64 {
    v.as_f64().unwrap()
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6 * b.abs().max(1.0), "{a} != {b}");
}

fn beam(s: &mut Session, x: f64, y: f64, length: J) -> Id {
    let sketch = cmd(s, json!({"op":"create_sketch","plane":"YZ","offset":x}))["sketch"].clone();
    cmd(
        s,
        json!({"op":"add_geometry","sketch":sketch,"items":[{"type":"rect","from":[y-19.05,-92.075],"to":[y+19.05,92.075]}]}),
    );
    cmd(
        s,
        json!({"op":"extrude","sketch":sketch,"distance":length,"operation":"new"}),
    )["feature"]
        .as_u64()
        .unwrap() as Id
}

fn tag() -> J {
    json!({"species":"Test species","grade":"Test grade","nominal_size":"2x8",
        "design_values":{"e_mpa":10000,"fb_mpa":8,"fv_mpa":1,"source":"Synthetic test values; not a lumber table",
            "factors":{"bending":{"CD":1.25,"CM":0.8},"shear":{"CD":1.25},"elasticity":{"CM":0.9}}},
        "density_kg_m3":500,"end_bearing_mm":[38.1,38.1]})
}
fn tagged(s: &mut Session) -> Id {
    let body = beam(s, 0.0, 0.0, json!(3000));
    cmd(
        s,
        json!({"op":"set_structural_member","body":body,"member":tag()}),
    );
    body
}
fn request(body: Id) -> J {
    json!({"op":"analyze_structure","analysis":{"plane":"XZ","bodies":[body],
        "supports":[{"location":{"body":body,"at":0},"fix":[true,true,false]},
            {"location":{"body":body,"at":1},"fix":[false,true,false]}],
        "uniform_loads":[{"body":body,"force_n_per_mm":[0,-1]}],"include_self_weight":false,"deformation_scale":20}})
}

#[test]
fn geometry_stress_and_sag_use_actual_section_and_adjusted_properties() {
    let mut s = Session::default();
    let body = tagged(&mut s);
    let out = cmd(&mut s, request(body));
    let m = &out["members"][0];
    near(number(&m["section"]["width_mm"]), 38.1);
    near(number(&m["section"]["depth_mm"]), 184.15);
    near(number(&m["geometry"]["length_mm"]), 3000.0);
    near(number(&m["geometry"]["clear_span_mm"]), 2923.8);
    near(number(&m["adjusted_design_values"]["e_mpa"]), 9000.0);
    near(number(&m["adjusted_design_values"]["fb_mpa"]), 8.0);
    near(number(&m["adjusted_design_values"]["fv_mpa"]), 1.25);
    let inertia = 38.1 * 184.15_f64.powi(3) / 12.0;
    near(number(&m["max_moment_n_mm"]), 1_125_000.0);
    near(
        number(&m["bending_utilization"]),
        1_125_000.0 / (inertia / (184.15 / 2.0)) / 8.0,
    );
    near(
        number(&m["shear_utilization"]),
        1.5 * 1500.0 / (38.1 * 184.15) / 1.25,
    );
    let sag = -5.0 * 3000.0_f64.powi(4) / (384.0 * 9000.0 * inertia);
    near(number(&m["samples"][16]["displacement_mm"][2]), sag);
    near(number(&m["samples"][16]["deformed_mm"][2]), sag * 20.0);
    assert_eq!(out["design_checks_complete"], false);
    assert_eq!(m["axial_design_required"], false);
    near(number(&out["nodes"][0]["reaction_n"][1]), 1500.0);
    let undo_depth = s.undo_depth();
    cmd(&mut s, request(body));
    assert_eq!(s.undo_depth(), undo_depth, "analysis is read-only");
}

#[test]
fn tags_roundtrip_undo_and_export_without_changing_old_file_versions() {
    let mut s = Session::default();
    let body = beam(&mut s, 0.0, 0.0, json!(3000));
    let old_version = io::design_version(&s.doc);
    let before = s.doc.clone();
    cmd(
        &mut s,
        json!({"op":"set_structural_member","body":body,"member":tag()}),
    );
    assert!(s.doc.same_geometry(&before));
    assert_eq!(io::design_version(&s.doc), 19);
    let loaded = io::from_json(&io::to_json(&s.doc)).unwrap();
    assert_eq!(loaded.structural_members, s.doc.structural_members);
    let export = script::export_timeline(&s).unwrap();
    let mut replay = Session::default();
    script::run(&mut replay, &script::Request::new(export.source)).unwrap();
    assert_eq!(replay.doc.structural_members, s.doc.structural_members);
    cmd(&mut s, json!({"op":"undo"}));
    assert!(s.doc.structural_members.is_empty());
    assert_eq!(io::design_version(&s.doc), old_version);
    cmd(&mut s, json!({"op":"redo"}));
    cmd(&mut s, json!({"op":"remove_structural_member","body":body}));
    assert_eq!(io::design_version(&s.doc), old_version);
}

#[test]
fn parameters_and_display_units_do_not_leave_stale_or_rescaled_results() {
    let mut s = Session::default();
    cmd(
        &mut s,
        json!({"op":"set_parameter","name":"span","expr":"3000 mm"}),
    );
    let body = beam(&mut s, 0.0, 0.0, json!("$span"));
    cmd(
        &mut s,
        json!({"op":"set_structural_member","body":body,"member":tag()}),
    );
    let first = cmd(&mut s, request(body));
    cmd(
        &mut s,
        json!({"op":"set_parameter","name":"span","expr":"4000 mm"}),
    );
    let second = cmd(&mut s, request(body));
    near(
        number(&second["members"][0]["bending_utilization"])
            / number(&first["members"][0]["bending_utilization"]),
        (4.0_f64 / 3.0).powi(2),
    );
    near(
        number(&second["members"][0]["sampled_max_displacement_mm"])
            / number(&first["members"][0]["sampled_max_displacement_mm"]),
        (4.0_f64 / 3.0).powi(4),
    );
    cmd(&mut s, json!({"op":"set_units","units":"in"}));
    let inches = cmd(&mut s, request(body));
    near(
        number(&inches["members"][0]["bending_utilization"]),
        number(&second["members"][0]["bending_utilization"]),
    );
}

#[test]
fn self_weight_area_load_and_interior_point_load_balance_reactions() {
    let mut s = Session::default();
    let body = tagged(&mut s);
    let mut r = request(body);
    r["analysis"]["include_self_weight"] = json!(true);
    r["analysis"]["area_loads"] =
        json!([{"body":body,"pressure_pa":2000,"tributary_width_mm":406.4,"direction":[0,-1]}]);
    r["analysis"]["point_loads"] = json!([{"location":{"body":body,"at":0.3},"force_n":[0,-1000]}]);
    let out = cmd(&mut s, r);
    let total: f64 = out["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| number(&n["reaction_n"][1]))
        .sum();
    let self_weight = 500.0 * 38.1 * 184.15 * 1e-9 * 9.80665;
    near(total, (1.0 + 0.8128 + self_weight) * 3000.0 + 1000.0);
    near(
        number(&out["members"][0]["uniform_load_n_per_mm"][1]),
        -1.0 - 0.8128 - self_weight,
    );
}

#[test]
fn rigid_connections_are_explicit_and_transfer_load_between_members() {
    let mut s = Session::default();
    let a = tagged(&mut s);
    let b = beam(&mut s, 3000.0, 0.0, json!(3000));
    cmd(
        &mut s,
        json!({"op":"set_structural_member","body":b,"member":tag()}),
    );
    let mut r = request(a);
    r["analysis"]["bodies"] = json!([a, b]);
    r["analysis"]["supports"][1]["location"]["body"] = json!(b);
    r["analysis"]["uniform_loads"] =
        json!([{"body":a,"force_n_per_mm":[0,-1]},{"body":b,"force_n_per_mm":[0,-1]}]);
    assert!(
        api::execute(&mut s, &r, None)
            .unwrap_err()
            .contains("unstable")
    );
    r["analysis"]["connections"] = json!([[{"body":a,"at":1},{"body":b,"at":0}]]);
    let out = cmd(&mut s, r);
    near(
        number(&out["members"][0]["max_moment_n_mm"]),
        6000.0_f64.powi(2) / 8.0,
    );
    near(
        number(&out["members"][1]["max_moment_n_mm"]),
        6000.0_f64.powi(2) / 8.0,
    );
}

#[test]
fn inspection_measures_spacing_without_inventing_tributary_width_or_clear_span() {
    let mut s = Session::default();
    let a = tagged(&mut s);
    let b = beam(&mut s, 0.0, 406.4, json!(3000));
    let mut t = tag();
    t.as_object_mut().unwrap().remove("end_bearing_mm");
    cmd(
        &mut s,
        json!({"op":"set_structural_member","body":b,"member":t}),
    );
    let out = cmd(&mut s, json!({"op":"get_structural_members"}));
    assert_eq!(out["parallel_spacing"][0]["bodies"], json!([a, b]));
    near(
        number(&out["parallel_spacing"][0]["centerline_distance_mm"]),
        406.4,
    );
    assert!(out["members"][1]["geometry"]["clear_span_mm"].is_null());
}

#[test]
fn signed_symmetric_extrusions_and_component_placements_are_resolved() {
    let mut s = Session::default();
    let component = cmd(
        &mut s,
        json!({"op":"create_component","name":"Rotated joist"}),
    )["component"]
        .clone();
    let body = tagged(&mut s);
    cmd(
        &mut s,
        json!({"op":"edit_feature","feature":body,"distance":-3000,"symmetric":true}),
    );
    cmd(
        &mut s,
        json!({"op":"move_component","id":component,"translate":[100,200,300],"rotate":[0,0,90]}),
    );
    let out = cmd(&mut s, json!({"op":"get_structural_members"}));
    let g = &out["members"][0]["geometry"];
    near(number(&g["start_mm"][0]), 100.0);
    near(number(&g["start_mm"][1]), 1700.0);
    near(number(&g["end_mm"][1]), -1300.0);
    let mut r = request(body);
    r["analysis"]["plane"] = json!("YZ");
    let out = cmd(&mut s, r);
    near(number(&out["members"][0]["max_moment_n_mm"]), 1_125_000.0);
}

#[test]
fn unsafe_or_unsupported_inputs_are_errors_not_partial_results() {
    let mut s = Session::default();
    let body = tagged(&mut s);
    let mut t = tag();
    t["design_values"]["factors"]["elasticity"]["CD"] = json!(1.6);
    assert!(
        api::execute(
            &mut s,
            &json!({"op":"set_structural_member","body":body,"member":t}),
            None
        )
        .is_err()
    );
    let mut r = request(body);
    r["analysis"]["plane"] = json!("YZ");
    assert!(
        api::execute(&mut s, &r, None)
            .unwrap_err()
            .contains("out of")
    );
    r = request(body);
    r["analysis"]["supports"][0]["location"]["at"] = json!(1.1);
    assert!(api::execute(&mut s, &r, None).is_err());
    r = request(body);
    r["analysis"]
        .as_object_mut()
        .unwrap()
        .remove("include_self_weight");
    assert!(api::execute(&mut s, &r, None).is_err());
    r = request(body);
    r["analysis"]["load_typo"] = json!(1000);
    assert!(api::execute(&mut s, &r, None).is_err());
    cmd(
        &mut s,
        json!({"op":"transform","body":body,"translate":[10,0,0]}),
    );
    assert!(
        api::execute(&mut s, &request(body), None)
            .unwrap_err()
            .contains("later transform")
    );
    let inspection = cmd(&mut s, json!({"op":"get_structural_members"}));
    assert!(inspection["members"][0]["error"].is_string());
    cmd(&mut s, json!({"op":"undo"}));
    cmd(
        &mut s,
        json!({"op":"edit_feature","feature":body,"suppressed":true}),
    );
    assert!(api::execute(&mut s, &request(body), None).is_err());
}

#[test]
fn laying_a_board_flat_changes_stiffness_and_biaxial_rotation_is_rejected() {
    let mut s = Session::default();
    let component = cmd(
        &mut s,
        json!({"op":"create_component","name":"Orientation"}),
    )["component"]
        .clone();
    let body = tagged(&mut s);
    let upright = cmd(&mut s, request(body));
    cmd(
        &mut s,
        json!({"op":"move_component","id":component,"rotate":[90,0,0]}),
    );
    let flat = cmd(&mut s, request(body));
    near(number(&flat["members"][0]["section"]["depth_mm"]), 38.1);
    near(
        number(&flat["members"][0]["bending_utilization"])
            / number(&upright["members"][0]["bending_utilization"]),
        184.15 / 38.1,
    );
    near(
        number(&flat["members"][0]["sampled_max_displacement_mm"])
            / number(&upright["members"][0]["sampled_max_displacement_mm"]),
        (184.15_f64 / 38.1).powi(2),
    );
    cmd(
        &mut s,
        json!({"op":"move_component","id":component,"rotate":[45,0,0]}),
    );
    assert!(
        api::execute(&mut s, &request(body), None)
            .unwrap_err()
            .contains("biaxial")
    );
}

#[test]
fn example_runs_via_rhai_host_functions() {
    let mut s = Session::default();
    let source = include_str!("../../../examples/structural-joist.rhai");
    script::run(&mut s, &script::Request::new(source)).unwrap();
    assert_eq!(s.doc.structural_members.len(), 1);
    assert!(s.built.errors.is_empty());
}
