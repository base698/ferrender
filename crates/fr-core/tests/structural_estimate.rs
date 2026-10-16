//! The quick load case: one deck pressure over a row of tagged joists.

use fr_core::api::execute;
use fr_core::{Id, Session};
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    execute(s, &c, None).unwrap_or_else(|e| panic!("{c} failed: {e}"))
}

const E: f64 = 10000.0;
const LENGTH: f64 = 3000.0;

fn tag() -> J {
    json!({"species": "test", "grade": "test", "nominal_size": "2x6", "design_values": {"e_mpa": E, "fb_mpa": 8.0, "fv_mpa": 1.0, "source": "test"}, "density_kg_m3": 500})
}

/// A 40 by 140 joist along X at each of these positions along Y.
fn joists(s: &mut Session, at: &[f64]) -> Vec<Id> {
    at.iter().map(|y| {
        let sketch = cmd(s, json!({"op": "create_sketch", "plane": "YZ"}))["sketch"].clone();
        cmd(s, json!({"op": "add_geometry", "sketch": sketch, "items": [{"type": "rect", "center": [y, 0], "size": [40, 140]}]}));
        let body = cmd(s, json!({"op": "extrude", "sketch": sketch, "distance": LENGTH, "operation": "new"}))["feature"].as_u64().unwrap() as Id;
        cmd(s, json!({"op": "set_structural_member", "body": body, "member": tag()}));
        body
    }).collect()
}

fn member<'a>(out: &'a J, body: Id) -> &'a J {
    out["members"].as_array().unwrap().iter().find(|m| m["body"] == body).unwrap()
}

#[test]
fn a_row_of_joists_shares_a_deck_load_by_its_spacing() {
    let mut s = Session::default();
    let ids = joists(&mut s, &[0.0, 400.0, 800.0, 1400.0]);
    let out = cmd(&mut s, json!({"op": "estimate_structure", "case": {"live_pa": 1500, "dead_pa": 500}}));
    // Half the gap each side; an end joist carries as much again on its open side.
    let widths: Vec<f64> = ids.iter().map(|id| out["tributary_mm"][id.to_string()].as_f64().unwrap()).collect();
    assert_eq!(widths, [400.0, 400.0, 500.0, 600.0]);
    let inertia = 40.0 * 140f64.powi(3) / 12.0;
    for (id, width) in ids.iter().zip(&widths) {
        let q = 2000.0 * width * 1e-6;
        let m = member(&out, *id);
        assert!((m["max_moment_n_mm"].as_f64().unwrap() - q * LENGTH * LENGTH / 8.0).abs() < 1e-3, "{m}");
        assert!((m["sampled_max_displacement_mm"].as_f64().unwrap() - 5.0 * q * LENGTH.powi(4) / (384.0 * E * inertia)).abs() < 1e-6);
    }
    assert_eq!(out["analysis_axes"][0], json!([1.0, 0.0, 0.0]));

    // A width given by hand replaces the one from spacing; a trial section is analysed
    // in place of the measured one, and says so.
    let out = cmd(&mut s, json!({"op": "estimate_structure", "case": {"live_pa": 2000, "tributary_mm": {ids[0].to_string(): 250.0}, "trial_section_mm": [40, 280]}}));
    assert_eq!(out["tributary_mm"][ids[0].to_string()], 250.0);
    assert_eq!(out["trial_section_mm"], json!([40.0, 280.0]));
    let m = member(&out, ids[0]);
    let q = 2000.0 * 250.0 * 1e-6;
    assert_eq!(m["section"]["depth_mm"], 280.0);
    assert!((m["sampled_max_displacement_mm"].as_f64().unwrap() - 5.0 * q * LENGTH.powi(4) / (384.0 * E * inertia * 8.0)).abs() < 1e-6, "twice as deep is eight times as stiff");
    assert!((m["bending_stress_mpa"].as_f64().unwrap() - q * LENGTH * LENGTH / 8.0 / (40.0 * 280.0 * 280.0 / 6.0)).abs() < 1e-9);
    // The design itself is as it was.
    assert_eq!(cmd(&mut s, json!({"op": "get_structural_members"}))["members"][0]["geometry"]["section_mm"], json!([40.0, 140.0]));

    // Self-weight follows the trial section too.
    let own = cmd(&mut s, json!({"op": "estimate_structure", "case": {"live_pa": 0, "include_self_weight": true, "trial_section_mm": [40, 280]}}));
    let reactions: f64 = own["nodes"].as_array().unwrap().iter().map(|n| n["reaction_n"][1].as_f64().unwrap()).sum();
    assert!((reactions - 4.0 * 500.0 * 40.0 * 280.0 * 1e-9 * 9.80665 * LENGTH).abs() < 1e-6, "{reactions}");
}

#[test]
fn the_case_is_kept_with_the_design_and_supports_sit_on_the_bearings() {
    let mut s = Session::default();
    let ids = joists(&mut s, &[0.0, 600.0]);
    let mut bearing = tag();
    bearing["end_bearing_mm"] = json!([100, 100]);
    cmd(&mut s, json!({"op": "set_structural_member", "body": ids[0], "member": bearing}));
    assert!(execute(&mut s, &json!({"op": "estimate_structure"}), None).unwrap_err().contains("needs a \"case\""));
    let first = cmd(&mut s, json!({"op": "estimate_structure", "case": {"live_pa": 1000}, "remember": true}));
    // Supported at the middle of each bearing, the span is shorter than the board.
    let span = LENGTH - 100.0;
    let q = 1000.0 * 600.0 * 1e-6;
    let moment = member(&first, ids[0])["max_moment_n_mm"].as_f64().unwrap();
    assert!((moment - (q * span * span / 8.0 - q * 50.0 * 50.0 / 2.0)).abs() < 1e-3, "{moment}");
    // Remembered: it runs again without being given, undoes, and is saved as format 19.
    let again = cmd(&mut s, json!({"op": "estimate_structure"}));
    assert_eq!(again["members"], first["members"]);
    let dir = std::env::temp_dir().join(format!("ferrender-case-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("case.ferr");
    cmd(&mut s, json!({"op": "save", "path": path.to_str().unwrap(), "cache": false}));
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"version\": 19") && raw.contains("structural_case"));
    let mut back = Session::open(&path).unwrap();
    assert_eq!(cmd(&mut back, json!({"op": "estimate_structure"}))["members"], first["members"]);
    cmd(&mut s, json!({"op": "undo"}));
    assert!(s.doc.structural_case.is_none());
}

#[test]
fn what_cannot_be_estimated_is_refused_or_reported() {
    let mut s = Session::default();
    assert!(execute(&mut s, &json!({"op": "estimate_structure", "case": {"live_pa": 1000}}), None).unwrap_err().contains("tag a rectangular extrusion"));
    let ids = joists(&mut s, &[0.0, 600.0]);
    for (case, word) in [(json!({"live_pa": -1}), "live load"), (json!({"live_pa": 1, "trial_section_mm": [0, 10]}), "trial width"), (json!({"live_pa": 1, "tributary_mm": {"5": -2.0}}), "tributary"), (json!({"live_pa": 1, "bogus": 2}), "invalid structural case")] {
        let refused = execute(&mut s, &json!({"op": "estimate_structure", "case": case}), None).unwrap_err();
        assert!(refused.contains(word), "{refused}");
    }
    // A member spoiled by a later feature is passed over and named; the rest are estimated.
    cmd(&mut s, json!({"op": "tilt_face", "body": ids[1], "face": [LENGTH, 600, 0], "axis": [0, 1, 0], "angle": 10}));
    let out = cmd(&mut s, json!({"op": "estimate_structure", "case": {"live_pa": 1000}}));
    assert_eq!(out["members"].as_array().unwrap().len(), 1);
    assert!(out["skipped"][0]["error"].as_str().unwrap().contains("later tilt"), "{}", out["skipped"]);
    // Alone, the remaining joist has no spacing to take a width from and carries nothing.
    assert!(out["tributary_mm"].as_object().unwrap().is_empty());
    assert_eq!(member(&out, ids[0])["max_moment_n_mm"], 0.0);

    // A post is not a level member.
    let mut s = Session::default();
    let sketch = cmd(&mut s, json!({"op": "create_sketch", "plane": "XY"}))["sketch"].clone();
    cmd(&mut s, json!({"op": "add_geometry", "sketch": sketch, "items": [{"type": "rect", "center": [0, 0], "size": [90, 90]}]}));
    let post = cmd(&mut s, json!({"op": "extrude", "sketch": sketch, "distance": 2400, "operation": "new"}))["feature"].clone();
    cmd(&mut s, json!({"op": "set_structural_member", "body": post, "member": tag()}));
    assert!(execute(&mut s, &json!({"op": "estimate_structure", "case": {"live_pa": 1000}}), None).unwrap_err().contains("level members"));
}
