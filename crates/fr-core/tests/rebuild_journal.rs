//! 0.4.1 rebuild behaviour: a failed feature leaves the bodies exactly as they were
//! without the rebuild copying every body per feature; edits that change nothing
//! geometric keep the built bodies; fillet "all" resolves edges that share a tag;
//! and a save says why a wanted geometry cache was not written.

use std::path::PathBuf;
use std::time::Instant;

use fr_core::api::execute;
use fr_core::doc::{CachePolicy, FeatureKind};
use fr_core::Session;
use glam::DVec3;
use serde_json::{Value as J, json};

fn run(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferrender-journal-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Body ids in order, with their exact volumes rounded past the kernel's noise.
fn shape(s: &Session) -> Vec<(u32, f64)> {
    s.built.bodies.iter().map(|b| (b.id, (b.solids.iter().map(|l| l.volume()).sum::<f64>() * 1e4).round() / 1e4)).collect()
}

/// The one body a creating command made.
fn made(out: &J) -> u32 {
    out["changed_bodies"][0]["id"].as_u64().unwrap_or_else(|| panic!("no new body in {out}")) as u32
}

fn box_at(s: &mut Session, x: f64) -> u32 {
    made(&run(s, json!({"op": "primitive", "type": "box", "width": 20, "depth": 20, "height": 10, "position": [x, 0, 0]})))
}

#[test]
fn a_feature_that_fails_after_changing_a_body_leaves_every_body_as_it_was() {
    let mut s = Session::default();
    let a = box_at(&mut s, 0.0);
    let b = box_at(&mut s, 40.0);
    let c = box_at(&mut s, 80.0);
    let before = shape(&s);
    assert_eq!(before.iter().map(|x| x.0).collect::<Vec<_>>(), vec![a, b, c]);

    // A hole that touches the middle box, then moved to where it touches nothing: the
    // drill is applied to the body before the "does not touch" check fails the feature.
    run(&mut s, json!({"op": "hole", "body": b, "at": [50, 10, 10], "diameter": 4, "through": true}));
    let hole = s.doc.features.last().unwrap().id;
    assert_ne!(shape(&s), before, "the hole changed the middle box");
    s.edit(|d| {
        let FeatureKind::Hole(h) = &mut d.feature_mut(hole).unwrap().kind else { panic!("not a hole") };
        h.at = vec![DVec3::new(500.0, 500.0, 10.0)];
        Ok(())
    }).unwrap();
    assert!(s.built.errors.get(&hole).is_some_and(|e| e.contains("does not touch")), "{:?}", s.built.errors);
    assert_eq!(shape(&s), before, "the failed hole left the bodies, their order and their volumes as before");

    // The same with the hole removed again: identical geometry either way.
    run(&mut s, json!({"op": "delete_feature", "feature": hole}));
    assert_eq!(shape(&s), before);
}

#[test]
fn rebuild_time_does_not_grow_with_the_square_of_the_body_count() {
    // 300 boxes rebuild in well under a second once the rebuild stops copying every
    // body per feature; with the copying it took seconds and grew 4x per doubling.
    let mut s = Session::default();
    for i in 0..300 {
        let (x, y) = ((i % 18) as f64 * 30.0, (i / 18) as f64 * 30.0);
        run(&mut s, json!({"op": "primitive", "type": "box", "width": 20, "depth": 20, "height": 20, "position": [x, y, 0]}));
    }
    assert_eq!(s.built.bodies.len(), 300);
    let t = Instant::now();
    let built = s.doc.rebuild();
    let ms = t.elapsed().as_millis();
    assert_eq!(built.bodies.len(), 300);
    assert!(ms < 2000, "300 trivial boxes rebuilt in {ms} ms");
}

#[test]
fn showing_hiding_activating_and_renaming_keep_the_built_bodies() {
    let mut s = Session::default();
    let a = box_at(&mut s, 0.0);
    let component = run(&mut s, json!({"op": "create_component", "name": "Inner", "activate": true}))["component"].as_u64().unwrap() as u32;
    let b = box_at(&mut s, 40.0);
    let path = dir().join("display.ferr");
    s.cache_policy = CachePolicy::Always;
    let saved = s.save(&path).unwrap();
    assert_eq!(saved.cached_bodies, 2, "{saved:?}");

    let mut s = Session::open(&path).unwrap();
    assert!(s.from_cache);
    s.cache_policy = CachePolicy::Always;
    let before = shape(&s);
    let rev = s.rev;

    run(&mut s, json!({"op": "set_visible", "id": a, "visible": false}));
    assert!(s.from_cache, "hiding a body did not rebuild");
    assert!(s.rev > rev, "views are told to redraw");
    assert!(s.doc.hidden_bodies.contains(&a));

    run(&mut s, json!({"op": "set_visible", "id": component, "visible": false}));
    assert!(s.from_cache && !s.built.component_visible(component), "component visibility is refreshed without a rebuild");
    assert_eq!(run(&mut s, json!({"op": "get_object_info", "id": b}))["body"]["visible"], json!(false));
    run(&mut s, json!({"op": "set_visible", "id": component, "visible": true}));
    assert!(s.from_cache && s.built.component_visible(component));

    run(&mut s, json!({"op": "activate_component", "id": 0}));
    run(&mut s, json!({"op": "activate_component", "id": component}));
    assert!(s.from_cache && s.doc.active_component == component);
    assert!(execute(&mut s, &json!({"op": "activate_component", "id": 999}), None).is_err());
    assert!(s.from_cache && s.doc.active_component == component, "a refused activation changes nothing and rebuilds nothing");

    run(&mut s, json!({"op": "edit_feature", "feature": component, "name": "Renamed"}));
    assert!(s.from_cache, "a new name alone does not rebuild");
    assert_eq!(s.doc.feature(component).unwrap().name, "Renamed");

    assert!(s.undo() && s.undo() && s.from_cache, "undoing display-only steps keeps the cached bodies");
    assert_eq!(s.doc.feature(component).unwrap().name, "Inner");
    assert!(s.redo() && s.from_cache);
    assert_eq!(shape(&s), before, "the bodies never changed");

    // A real edit still rebuilds, and undoing it rebuilds too.
    run(&mut s, json!({"op": "fillet_edges", "body": b, "edges": "all", "radius": 1}));
    assert!(!s.from_cache);
    assert_ne!(shape(&s), before);
    assert!(s.undo());
    assert_eq!(shape(&s), before);

    // Saved and reopened, the display state is what was set.
    let saved = s.save(&path).unwrap();
    assert_eq!(saved.cached_bodies, 2);
    let again = Session::open(&path).unwrap();
    assert!(again.doc.hidden_bodies.contains(&a) && again.doc.active_component == component);
}

#[test]
fn fillet_all_resolves_edges_that_share_a_tag() {
    // A slab cut through a torus: each flat face of the slab meets the torus along an
    // inner and an outer circle, so those curved edges carry the same tag pair.
    let mut s = Session::default();
    let torus = made(&run(&mut s, json!({"op": "primitive", "type": "torus", "major_radius": 30, "tube_radius": 8})));
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 100, "depth": 100, "height": 4, "position": [-50, -50, -2], "operation": "cut"}));
    let info = run(&mut s, json!({"op": "get_object_info", "id": torus}));
    let edges = info["topology"]["edges"].as_array().unwrap();
    let mut tags: Vec<String> = edges.iter().map(|e| e["tag"].to_string()).collect();
    tags.sort();
    assert!(tags.windows(2).any(|w| w[0] == w[1]), "the cut leaves edges sharing a tag: {tags:?}");
    let before = shape(&s);
    run(&mut s, json!({"op": "fillet_edges", "body": torus, "edges": "all", "radius": 0.5}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_ne!(shape(&s), before, "every edge was rounded");
    // The same by one edge of a shared pair, named by the point the body reports.
    assert!(s.undo());
    let shared = edges.iter().find(|e| tags.iter().filter(|t| **t == e["tag"].to_string()).count() > 1).unwrap();
    run(&mut s, json!({"op": "fillet_edges", "body": torus, "edges": [shared["point"].clone()], "radius": 0.5}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
}

#[test]
fn a_shell_opens_a_face_whose_edge_was_filleted() {
    // The kernel's thick solid keeps a face next to a fillet and seals an offset of it
    // underneath, which looked like a shell that did nothing: the cap stayed on the bottle.
    let mut s = Session::default();
    let rod = made(&run(&mut s, json!({"op": "primitive", "type": "cylinder", "diameter": 20, "height": 30})));
    run(&mut s, json!({"op": "fillet_edges", "body": rod, "edges": [[10, 0, 30]], "radius": 3}));
    let solid = s.built.body(rod).unwrap().solids[0].volume();
    run(&mut s, json!({"op": "shell", "body": rod, "open_faces": [[0, 0, 30]], "thickness": 2}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let hollow = s.built.body(rod).unwrap().solids[0].volume();
    assert!(hollow < solid * 0.6, "hollowed: {hollow} of {solid}");
    let down = s.built.body(rod).unwrap().mesh.ray(DVec3::new(0.0, 0.0, 40.0), -DVec3::Z);
    let faces = run(&mut s, json!({"op": "get_object_info", "id": rod}))["topology"]["faces"].as_array().unwrap().clone();
    let cap = std::f64::consts::PI * 7.0 * 7.0;
    let top_planes: Vec<f64> = faces.iter().filter(|f| f["shape"] == "plane" && (f["point"][2].as_f64().unwrap() - 30.0).abs() < 1e-6).map(|f| f["area"].as_f64().unwrap()).collect();
    assert!(top_planes.iter().all(|a| (a - cap).abs() > 1.0), "the picked face is gone or shrunk to a rim: {top_planes:?}");
    assert!(!faces.iter().any(|f| f["area"].as_f64().unwrap() < 0.0), "no inverted inner cap: {faces:?}");
    // A ray down the axis from above passes straight into the cavity.
    // From 10 mm above, a closed top is hit after 10 mm; the cavity floor is 38 mm down.
    assert!(down.is_some_and(|hit| hit.0 > 11.0), "nothing closes the top: {down:?}");
    // The bottom is still closed, and a plain cylinder still shells through the kernel alone.
    assert!(faces.iter().any(|f| f["shape"] == "plane" && f["point"][2].as_f64().unwrap().abs() < 1e-6 && (f["area"].as_f64().unwrap() - std::f64::consts::PI * 100.0).abs() < 1.0));
}

#[test]
fn a_save_says_why_a_wanted_cache_was_not_written() {
    let mut s = Session::default();
    let rod = made(&run(&mut s, json!({"op": "primitive", "type": "cylinder", "diameter": 10, "height": 30})));
    run(&mut s, json!({"op": "thread", "body": rod, "face": [5, 0, 15], "thread": "M10"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    s.cache_policy = CachePolicy::Always;
    let path = dir().join("threaded.ferr");
    let saved = s.save(&path).unwrap();
    assert_eq!(saved.cached_bodies, 0);
    assert!(saved.cache_skipped.as_deref().is_some_and(|r| r.contains("threads")), "{saved:?}");
    let out = run(&mut s, json!({"op": "save", "path": path.to_str().unwrap(), "cache": true}));
    assert!(out["cache_skipped"].as_str().is_some_and(|r| r.contains("threads")), "{out}");

    // Not wanted: nothing to explain.
    let out = run(&mut s, json!({"op": "save", "path": path.to_str().unwrap(), "cache": false}));
    assert!(out["cache_skipped"].is_null(), "{out}");

    // Wanted but the design has an error: said so.
    let mut s = Session::default();
    let b = box_at(&mut s, 0.0);
    run(&mut s, json!({"op": "hole", "body": b, "at": [10, 10, 10], "diameter": 4, "through": true}));
    let hole = s.doc.features.last().unwrap().id;
    s.edit(|d| {
        let FeatureKind::Hole(h) = &mut d.feature_mut(hole).unwrap().kind else { panic!() };
        h.at = vec![DVec3::new(500.0, 500.0, 10.0)];
        Ok(())
    }).unwrap();
    s.cache_policy = CachePolicy::Always;
    let saved = s.save(&dir().join("errors.ferr")).unwrap();
    assert!(saved.cache_skipped.as_deref().is_some_and(|r| r.contains("feature errors")), "{saved:?}");
}
