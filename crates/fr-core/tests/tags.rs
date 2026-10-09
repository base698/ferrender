//! Face and edge tags: picks follow the faces they were made on when upstream
//! geometry changes, and report how they were found.

use fr_core::api::execute;
use fr_core::tag::{Level, Origin};
use fr_core::{FeatureKind, Session};
use serde_json::{Value as J, json};

fn run(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

fn last(s: &Session) -> u32 {
    s.doc.features.last().unwrap().id
}

/// The kernel's volume of a body, exact where the mesh's is not.
fn volume(s: &Session, body: u32) -> f64 {
    s.built.body(body).unwrap().solids.iter().map(|l| l.volume()).sum()
}

/// A block whose width is a parameter, with a fillet on the edge at its far (+X) end.
fn block_with_fillet() -> (Session, u32, u32) {
    let mut s = Session::default();
    run(&mut s, json!({"op": "set_parameter", "name": "w", "expr": "20 mm"}));
    run(&mut s, json!({"op": "create_sketch", "plane": "XY"}));
    let items = run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [20, 10]}]}));
    // The rect's right side is the line from (20,0) to (20,10); its length is the width parameter.
    let lines = items["items"][0]["entities"].as_array().unwrap().clone();
    // Dimension the bottom edge (first line) to w so the block grows along X.
    run(&mut s, json!({"op": "add_constraint", "kind": "distance", "refs": [lines[0]], "value": "$w"}));
    run(&mut s, json!({"op": "extrude", "distance": 5, "operation": "new"}));
    let body = last(&s);
    // The top edge at the far end, x = 20: where the end face meets the top cap.
    run(&mut s, json!({"op": "fillet_edges", "body": body, "edges": [[20, 5, 5]], "radius": 1}));
    let fillet = last(&s);
    (s, body, fillet)
}

#[test]
fn a_sweep_names_its_faces_by_sketch_entity_and_caps() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "create_sketch", "plane": "XY"}));
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [20, 10]}]}));
    run(&mut s, json!({"op": "extrude", "distance": 5, "operation": "new"}));
    let body_id = s.built.bodies[0].id;
    let body_tags = s.built.bodies[0].tags.clone();
    let tags: Vec<_> = body_tags.iter().flatten().flatten().collect();
    assert_eq!(tags.len(), 6, "a box has six tagged faces: {tags:?}");
    assert_eq!(tags.iter().filter(|t| matches!(t.origin, Origin::Cap { .. })).count(), 2);
    assert_eq!(tags.iter().filter(|t| matches!(t.origin, Origin::Swept { .. })).count(), 4);
    let swept: std::collections::BTreeSet<u32> = tags.iter().filter_map(|t| if let Origin::Swept { entity, .. } = t.origin { Some(entity) } else { None }).collect();
    assert_eq!(swept.len(), 4, "each side comes from a different sketch line");
    let info = run(&mut s, json!({"op": "get_object_info", "id": body_id}));
    assert!(info["topology"]["faces"].as_array().unwrap().iter().all(|f| !f["tag"].is_null() && f["made"].as_str().is_some()));
    assert!(info["topology"]["edges"].as_array().unwrap().iter().all(|e| !e["tag"].is_null()));
}

#[test]
fn a_fillet_follows_its_edge_when_the_block_grows() {
    let (mut s, body, fillet) = block_with_fillet();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let before = volume(&s, body);
    let FeatureKind::Blend(b) = &s.doc.feature(fillet).unwrap().kind else { panic!() };
    assert_eq!(b.tags.len(), 1, "the first build learns the edge's tag");
    let tag = b.tags[0].clone().expect("the edge between a swept side and the end cap has a tag");
    assert!(tag.faces.iter().any(|t| matches!(t.origin, Origin::Cap { end: true, .. })), "one face is the top cap: {tag:?}");
    assert!(tag.faces.iter().any(|t| matches!(t.origin, Origin::Swept { .. })), "the other is a swept side: {tag:?}");
    assert_eq!(s.built.resolutions.get(&fillet), Some(&Level::Tag), "the command learned the edge's tag when it was picked");

    // Grow the block to 60 mm. By position, (20,5,5) would now be in the middle of the top face,
    // nearest some unrelated edge; by tag the fillet stays on the far end.
    run(&mut s, json!({"op": "set_parameter", "name": "w", "expr": "60 mm"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.resolutions.get(&fillet), Some(&Level::Tag), "found by tag after the edit");
    let b = s.built.body(body).unwrap();
    let (lo, hi) = b.mesh.bbox().unwrap();
    assert!((hi.x - 60.0).abs() < 1e-6, "the block grew: {lo} {hi}");
    // A 1 mm fillet along a 10 mm edge removes (1 - pi/4) mm^3 per mm of edge.
    let removed = 60.0 * 10.0 * 5.0 - volume(&s, body);
    assert!((removed - 10.0 * (1.0 - std::f64::consts::FRAC_PI_4)).abs() < 1e-6, "exactly one 10 mm edge is filleted: removed {removed}");
    // The round face is at the far end, not the near one.
    let round = b.mesh.tris().filter(|t| { let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero(); n.x > 0.2 && n.z > 0.2 }).count();
    assert!(round > 0, "the fillet's curved face faces +X and +Z");
    let _ = before;
    let info = run(&mut s, json!({"op": "get_object_info", "id": fillet}));
    assert_eq!(info["resolved"], "tag");
}

#[test]
fn fillet_commands_accept_edge_tags() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 20, "depth": 10, "height": 5}));
    let body = last(&s);
    let info = run(&mut s, json!({"op": "get_object_info", "id": body}));
    let edge = info["topology"]["edges"].as_array().unwrap().iter().find(|e| e["length"] == 20.0).unwrap();
    let before = volume(&s, body);
    run(&mut s, json!({"op": "fillet_edges", "body": body, "edges": [{"tag": edge["tag"]}], "radius": 1}));
    let fillet = last(&s);
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.resolutions.get(&fillet), Some(&Level::Tag));
    let removed = before - volume(&s, body);
    assert!((removed - 20.0 * (1.0 - std::f64::consts::FRAC_PI_4)).abs() < 1e-6, "one 20 mm edge filleted: {removed}");
    let bad = execute(&mut s, &json!({"op": "fillet_edges", "body": body, "edges": [{"tag": {"faces": [{"origin": {"made": {"feature": 99, "n": 0}}, "kind": "plane"}, {"origin": {"made": {"feature": 99, "n": 1}}, "kind": "plane"}]}}], "radius": 1}), None);
    assert!(bad.as_ref().is_err_and(|e| e.contains("no edge of the body has that tag")), "{bad:?}");
}

#[test]
fn a_shell_keeps_its_open_face_after_a_cut_splits_it() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 40, "depth": 20, "height": 10}));
    let body = last(&s);
    // Open the top face.
    run(&mut s, json!({"op": "shell", "body": body, "open_faces": [[20, 10, 10]], "thickness": 1}));
    let shell = last(&s);
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let FeatureKind::Shell(sh) = &s.doc.feature(shell).unwrap().kind else { panic!() };
    assert!(sh.tags.len() == 1 && sh.tags[0].is_some());
    let hollow = s.built.body(body).unwrap().mesh.volume();
    assert!(hollow < 40.0 * 20.0 * 10.0 * 0.5);
    // Insert, before the shell, a slot through the top that splits the top face in two.
    run(&mut s, json!({"op": "rollback", "to": body}));
    run(&mut s, json!({"op": "create_sketch", "plane": "XY", "offset": 10}));
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [19, -1], "to": [21, 21]}]}));
    run(&mut s, json!({"op": "extrude", "distance": -4, "operation": "cut"}));
    run(&mut s, json!({"op": "rollback", "to": "end"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    // The picked point (20,10,10) is now in the slot; by position the shell would open the slot's floor or fail.
    // By tag family it opens a piece of the original top face.
    let level = s.built.resolutions.get(&shell).copied();
    assert!(matches!(level, Some(Level::Origin) | Some(Level::Tag)), "resolved by the face's origin, got {level:?}");
    let b = s.built.body(body).unwrap();
    // A shell open at a top piece: the wall thickness is 1, so the cavity is well under half the volume.
    assert!(b.mesh.volume() < 40.0 * 20.0 * 10.0 * 0.5, "the body is hollow: {}", b.mesh.volume());
}

#[test]
fn text_stays_on_its_face_when_a_hole_halves_it() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 40, "depth": 20, "height": 5}));
    let body = last(&s);
    run(&mut s, json!({"op": "text", "text": "A", "operation": "join", "body": body, "face": [30, 10, 5], "height": "6 mm", "depth": "1 mm"}));
    let text = last(&s);
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let FeatureKind::Text(t) = &s.doc.feature(text).unwrap().kind else { panic!() };
    assert!(t.tag.is_some(), "the text learned its face");
    let raised = s.built.body(body).unwrap().mesh.volume();
    // A slot through the whole depth before the text, leaving the text's end of the face intact.
    run(&mut s, json!({"op": "rollback", "to": body}));
    run(&mut s, json!({"op": "create_sketch", "plane": "XY", "offset": 5}));
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [14, -1], "to": [16, 21]}]}));
    run(&mut s, json!({"op": "extrude", "distance": -5, "operation": "cut"}));
    run(&mut s, json!({"op": "rollback", "to": "end"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let level = s.built.resolutions.get(&text).copied();
    assert!(matches!(level, Some(Level::Origin) | Some(Level::Tag)), "{level:?}");
    let now = s.built.body(body).unwrap().mesh.volume();
    // The slot took 2 x 20 x 5 out, the letter is still there.
    assert!((raised - now - 200.0).abs() < 1e-3, "raised {raised} now {now}");
}

#[test]
fn a_cut_that_removes_the_tagged_face_fails_honestly() {
    let (mut s, body, fillet) = block_with_fillet();
    // Cut the whole far end off before the fillet: its edge no longer exists anywhere.
    run(&mut s, json!({"op": "rollback", "to": body}));
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 30, "depth": 40, "height": 40, "position": [15, -10, -10], "operation": "cut"}));
    run(&mut s, json!({"op": "rollback", "to": "end"}));
    let err = s.built.errors.get(&fillet).cloned();
    assert!(err.as_ref().is_some_and(|e| e.contains("no longer there")), "the fillet must fail rather than move: {err:?}");
    let _ = body;
}

#[test]
fn tags_survive_save_and_load_and_old_files_learn_them() {
    let (s, _, fillet) = block_with_fillet();
    let dir = std::env::temp_dir().join(format!("ferrender-tags-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tagged.ferr");
    let text = fr_core::io::to_json(&s.doc);
    assert!(text.contains("\"tags\""), "tags are written");
    assert_eq!(fr_core::io::design_version(&s.doc), 10);
    std::fs::write(&path, &text).unwrap();
    let again = Session::open(&path).unwrap();
    assert_eq!(again.doc, s.doc);
    assert_eq!(again.built.resolutions.get(&fillet), Some(&Level::Tag));
    // Strip the tags, as a 0.3 file has none: the reopen finds the edge by position and learns the tag back.
    let mut v: J = serde_json::from_str(&text).unwrap();
    for f in v["features"].as_array_mut().unwrap() {
        // Indexing a missing key mutably would insert it; only touch blends.
        if let Some(o) = f["kind"].get_mut("blend").and_then(|b| b.as_object_mut()) { o.remove("tags"); }
    }
    v["version"] = json!(8);
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let old = Session::open(&path).unwrap();
    assert!(old.built.errors.is_empty());
    assert_eq!(old.built.resolutions.get(&fillet), Some(&Level::Position));
    let FeatureKind::Blend(b) = &old.doc.feature(fillet).unwrap().kind else { panic!() };
    assert_eq!(b.tags.len(), 1);
    assert!(!old.dirty, "learning tags on open is not an edit");
}

#[test]
fn deleting_a_primitives_picked_face_does_not_select_another_face_of_that_primitive() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"primitive","type":"box","width":40,"depth":20,"height":10}));
    let body = last(&s);
    run(&mut s, json!({"op":"shell","body":body,"open_faces":[[20,10,10]],"thickness":1}));
    let shell = last(&s);
    run(&mut s, json!({"op":"rollback","to":body}));
    run(&mut s, json!({"op":"primitive","type":"box","width":60,"depth":40,"height":10,"position":[-10,-10,5],"operation":"cut"}));
    run(&mut s, json!({"op":"rollback","to":"end"}));
    assert!(s.built.errors.get(&shell).is_some_and(|e|e.contains("no longer there")), "{:?}", s.built.errors);
}

#[test]
fn a_tagged_text_face_cannot_migrate_to_a_replacement_cut_face() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"primitive","type":"box","width":40,"depth":20,"height":10}));
    let body = last(&s);
    run(&mut s, json!({"op":"text","text":"A","operation":"join","body":body,"face":[20,10,10],"height":4,"depth":1}));
    let text = last(&s);
    run(&mut s, json!({"op":"rollback","to":body}));
    run(&mut s, json!({"op":"primitive","type":"box","width":60,"depth":40,"height":10,"position":[-10,-10,5],"operation":"cut"}));
    run(&mut s, json!({"op":"rollback","to":"end"}));
    assert!(s.built.errors.get(&text).is_some_and(|e|e.contains("no longer there")), "{:?}", s.built.errors);
}

#[test]
fn tag_families_keep_face_ordinals_and_copy_identity() {
    use fr_core::tag::{Tag, Kind};
    let a = Tag::new(Origin::Made {feature:1,n:0}, Kind::Plane);
    let b = Tag::new(Origin::Made {feature:1,n:1}, Kind::Plane);
    assert_ne!(a.family(), b.family());
    assert_ne!(a.family(), a.copy(1).family());
    assert_ne!(a.copy(1).family(), a.copy(2).family());
    assert_eq!(a.copy(1).split(2).family(), a.split(3).copy(1).family());
}

#[test]
fn a_tagged_construction_plane_errors_if_its_face_was_replaced() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"primitive","type":"box","width":40,"depth":20,"height":10}));
    let body = last(&s);
    run(&mut s, json!({"op":"create_plane","kind":"offset","base":{"face":{"body":body,"point":[20,10,10]}},"distance":2}));
    let plane = last(&s);
    run(&mut s, json!({"op":"rollback","to":body}));
    run(&mut s, json!({"op":"primitive","type":"box","width":60,"depth":40,"height":10,"position":[-10,-10,5],"operation":"cut"}));
    run(&mut s, json!({"op":"rollback","to":"end"}));
    assert!(s.built.errors.get(&plane).is_some_and(|e|e.contains("gone")), "{:?}", s.built.errors);
}

#[test]
fn first_copies_and_separate_patterns_have_distinct_face_tags() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"primitive","type":"box","width":10,"depth":10,"height":10}));
    let body = last(&s);
    run(&mut s, json!({"op":"pattern","feature":body,"type":"linear","axis":"x","count":2,"spacing":20}));
    run(&mut s, json!({"op":"pattern","feature":body,"type":"linear","axis":"y","count":2,"spacing":20}));
    assert_eq!(s.built.bodies.len(), 3);
    let tags: Vec<std::collections::BTreeSet<_>> = s.built.bodies.iter().map(|b| b.tags.iter().flatten().flatten().cloned().collect()).collect();
    assert!(tags[0].is_disjoint(&tags[1]), "the first copy must not reuse the original face tags");
    assert!(tags[1].is_disjoint(&tags[2]), "different patterns must not share copy identities");
}

#[test]
fn a_thread_keeps_its_cylindrical_face_when_the_rod_length_changes() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"set_parameter","name":"h","expr":"20 mm"}));
    run(&mut s, json!({"op":"primitive","type":"cylinder","diameter":6,"height":"$h"}));
    let body = last(&s);
    run(&mut s, json!({"op":"thread","body":body,"face":[3,0,10],"thread":"M6","length":5,"allowance":0.2}));
    let thread = last(&s);
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    run(&mut s, json!({"op":"set_parameter","name":"h","expr":"40 mm"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.resolutions.get(&thread), Some(&Level::Tag));
    let b = s.built.body(body).unwrap();
    assert_eq!(b.threads.len(), 1);
    assert_eq!(b.threads[0].open_edges(), 0);
    let (lo, hi) = b.mesh.bbox().unwrap();
    assert!((hi.z - lo.z - 40.0).abs() < 1e-6);
}
