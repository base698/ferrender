//! A lens: two arcs meeting at the rim, turned about the line their centres lie on.

use std::f64::consts::PI;

use fr_core::Session;
use fr_core::api::execute;
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    execute(s, &c, None).unwrap_or_else(|e| panic!("{c} failed: {e}"))
}

fn volume(s: &Session) -> f64 {
    s.built.bodies.iter().flat_map(|b| b.solids.iter()).map(|x| x.volume()).sum()
}

/// Two caps of a 52 mm sphere, each 4 mm deep, 40 mm across.
const CAPS: f64 = 2.0 * PI * 16.0 * (3.0 * 52.0 - 4.0) / 3.0;

#[test]
fn two_arcs_closed_on_the_axis_revolve_into_a_lens() {
    // The through points are given to three places, as a hand would place them, which
    // leaves each arc's centre a hair to one side of the axis. Centres are not outline.
    for (through, slack) in [((52.0f64 * 52.0 - 100.0).sqrt() - 48.0, 1e-4), (3.029, 1.0)] {
        let mut s = Session::default();
        let sketch = cmd(&mut s, json!({"op": "create_sketch", "plane": "XZ"}))["sketch"].as_u64().unwrap();
        cmd(&mut s, json!({"op": "add_geometry", "sketch": sketch, "items": [
            {"type": "arc3", "start": [20, 0], "through": [10, through], "end": [0, 4]},
            {"type": "arc3", "start": [20, 0], "through": [10, -through], "end": [0, -4]},
            {"type": "line", "from": [0, 4], "to": [0, -4]},
        ]}));
        cmd(&mut s, json!({"op": "revolve", "sketch": sketch, "axis": "y", "angle": 360}));
        assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
        assert!((volume(&s) - CAPS).abs() < slack, "through {through}: {} is not {CAPS}", volume(&s));
        assert_eq!(s.built.bodies[0].mesh.open_edges(), 0);
    }
}

#[test]
fn two_spheres_intersect_into_the_same_lens() {
    let mut s = Session::default();
    let a = cmd(&mut s, json!({"op": "primitive", "type": "sphere", "diameter": 104, "position": [0, 0, -48]}))["changed_bodies"][0]["id"].as_u64().unwrap();
    let b = cmd(&mut s, json!({"op": "primitive", "type": "sphere", "diameter": 104, "position": [0, 0, 48]}))["changed_bodies"][0]["id"].as_u64().unwrap();
    cmd(&mut s, json!({"op": "combine", "target": a, "tools": [b], "operation": "intersect"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!((volume(&s) - CAPS).abs() < 1e-6, "{} is not {CAPS}", volume(&s));
}

#[test]
fn a_profile_whose_outline_is_past_the_axis_is_still_refused_by_its_points() {
    let mut s = Session::default();
    let sketch = cmd(&mut s, json!({"op": "create_sketch", "plane": "XZ"}))["sketch"].as_u64().unwrap();
    cmd(&mut s, json!({"op": "add_geometry", "sketch": sketch, "items": [{"type": "polyline", "points": [[-0.5, 0], [20, 0], [0, 10]], "closed": true}]}));
    let refused = execute(&mut s, &json!({"op": "revolve", "sketch": sketch, "axis": "y", "angle": 360}), None).expect_err("it crosses the axis");
    assert!(refused.contains("crosses the axis") && refused.contains("1 point lies"), "{refused}");
}
