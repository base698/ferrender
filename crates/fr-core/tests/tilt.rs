//! Tilt Face: a flat face of an exact body turned about a hinge lying in it.

use std::f64::consts::PI;

use fr_core::api::execute;
use fr_core::{Id, Session};
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    execute(s, &c, None).unwrap_or_else(|e| panic!("{c} failed: {e}"))
}

fn refused(s: &mut Session, c: J) -> String {
    execute(s, &c, None).expect_err("the command should be refused")
}

fn block(s: &mut Session) -> Id {
    cmd(s, json!({"op": "primitive", "type": "box", "width": 20, "depth": 10, "height": 8}))["changed_bodies"][0]["id"].as_u64().unwrap() as Id
}

fn volume(s: &Session, body: Id) -> f64 {
    s.built.body(body).unwrap().solids.iter().map(|x| x.volume()).sum()
}

fn top(s: &Session, body: Id) -> f64 {
    s.built.body(body).unwrap().mesh.bbox().unwrap().1.z
}

#[test]
fn a_box_face_tilts_about_its_middle_or_an_edge() {
    let wedge = 0.5 * 10.0 * (10.0 * (30f64).to_radians().tan()) * 20.0;
    // About the middle, what is cut from one side is carried out on the other.
    let mut s = Session::default();
    let b = block(&mut s);
    let out = cmd(&mut s, json!({"op": "tilt_face", "body": b, "face": [10, 5, 8], "axis": [1, 0, 0], "angle": 30}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!((volume(&s, b) - 1600.0).abs() < 1e-6, "{}", volume(&s, b));
    assert!((top(&s, b) - (8.0 + 5.0 * (30f64).to_radians().tan())).abs() < 1e-6, "{}", top(&s, b));
    assert_eq!(s.built.body(b).unwrap().mesh.open_edges(), 0);
    // Clockwise looking along +X: the far side, at greater Y, is the one that rises.
    let mesh = &s.built.body(b).unwrap().mesh;
    assert!(mesh.ray(glam::DVec3::new(10.0, 9.5, 30.0), -glam::DVec3::Z).is_some_and(|h| (30.0 - h.0) > 10.0), "the far side is the high one");
    // The tilted face is named, so later features find it again.
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": b}));
    assert!(info["topology"]["faces"].as_array().unwrap().iter().any(|f| f["made"].as_str().is_some_and(|m| m.contains("tilt:face"))), "{}", info["topology"]["faces"]);
    let feature = out["feature"].as_u64().unwrap();
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": feature}));
    assert_eq!(info["type"], "tilt");
    assert_eq!(info["pivot"], "middle");
    // A fillet on every edge of the tilted block still builds.
    cmd(&mut s, json!({"op": "fillet_edges", "body": b, "edges": "all", "radius": 0.5}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);

    // Hinged at the edge that only cuts, the top never rises; the other way it only adds.
    for (pivot, angle, want, high) in [("cut", 30, 1600.0 - wedge, 8.0), ("add", 30, 1600.0 + wedge, 8.0 + 10.0 * (30f64).to_radians().tan()), ("cut", -30, 1600.0 - wedge, 8.0)] {
        let mut s = Session::default();
        let b = block(&mut s);
        cmd(&mut s, json!({"op": "tilt_face", "body": b, "face": [10, 5, 8], "axis": [1, 0, 0], "angle": angle, "pivot": pivot}));
        assert!((volume(&s, b) - want).abs() < 1e-6, "{pivot} {angle}: {} is not {want}", volume(&s, b));
        assert!((top(&s, b) - high).abs() < 1e-6, "{pivot} {angle}: top at {}", top(&s, b));
    }

    // A side face tilts too; the hinge direction is taken along the face.
    let mut s = Session::default();
    let b = block(&mut s);
    cmd(&mut s, json!({"op": "tilt_face", "body": b, "face": [20, 5, 4], "axis": [0.3, 1, 0], "angle": 10, "pivot": "cut"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(volume(&s, b) < 1600.0 && volume(&s, b) > 1400.0);
}

#[test]
fn the_end_of_a_rod_tilts_and_what_cannot_be_tilted_is_refused() {
    let mut s = Session::default();
    let rod = cmd(&mut s, json!({"op": "primitive", "type": "cylinder", "diameter": 1, "height": 10}))["changed_bodies"][0]["id"].as_u64().unwrap() as Id;
    cmd(&mut s, json!({"op": "tilt_face", "body": rod, "face": [0, 0, 10], "axis": [0, 1, 0], "angle": 30}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!((volume(&s, rod) - PI * 0.25 * 10.0).abs() < 1e-7, "about its middle the rod holds what it did: {}", volume(&s, rod));
    assert!((top(&s, rod) - (10.0 + 0.5 * (30f64).to_radians().tan())).abs() < 1e-3);
    assert_eq!(s.built.body(rod).unwrap().mesh.open_edges(), 0);
    // It is a feature like any other: undone, redone, saved and reopened.
    let tilted = volume(&s, rod);
    cmd(&mut s, json!({"op": "undo"}));
    assert!((top(&s, rod) - 10.0).abs() < 1e-9);
    cmd(&mut s, json!({"op": "redo"}));
    let dir = std::env::temp_dir().join(format!("ferrender-tilt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rod.ferr");
    cmd(&mut s, json!({"op": "save", "path": path.to_str().unwrap(), "cache": false}));
    assert!(std::fs::read_to_string(&path).unwrap().contains("\"version\": 17"), "a tilt needs format 17");
    let again = Session::open(&path).unwrap();
    assert!(again.built.errors.is_empty(), "{:?}", again.built.errors);
    assert!((volume(&again, rod) - tilted).abs() < 1e-9);

    // The round side is not flat; a hinge straight out of the face is no hinge; nor is no angle.
    assert!(refused(&mut s, json!({"op": "tilt_face", "body": rod, "face": [0.5, 0, 5], "axis": [0, 1, 0], "angle": 10})).contains("flat"));
    assert!(refused(&mut s, json!({"op": "tilt_face", "body": rod, "face": [0, 0, 0], "axis": [0, 0, 1], "angle": 10})).contains("hinge"));
    assert!(refused(&mut s, json!({"op": "tilt_face", "body": rod, "face": [0, 0, 0], "axis": [0, 1, 0], "angle": 0})).contains("between 0 and 85"));
    assert!(refused(&mut s, json!({"op": "tilt_face", "body": rod, "face": [0, 0, 0], "axis": [0, 1, 0], "angle": 10, "pivot": "sideways"})).contains("middle, cut or add"));
    assert!((volume(&s, rod) - tilted).abs() < 1e-9, "refusals leave the body as it was");
}
