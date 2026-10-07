use fr_core::{Session, api::execute, io};
use glam::DVec3;
use serde_json::{Value, json};

fn run(session: &mut Session, command: Value) -> Value {
    execute(session, &command, None).unwrap_or_else(|error| panic!("{command}: {error}"))
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-6 * expected.abs().max(1.0), "{actual} != {expected}");
}

fn volume(session: &Session, body: u32) -> f64 {
    session.built.body(body).unwrap().solids.iter().map(|solid| solid.volume()).sum()
}

fn plate(session: &mut Session) -> u32 {
    run(session, json!({"op":"create_sketch"}));
    run(session, json!({"op":"add_geometry","items":[{"type":"rect","from":[-20,-10],"to":[20,10]}]}));
    run(session, json!({"op":"extrude","distance":4,"operation":"new"}))["feature"].as_u64().unwrap() as u32
}

#[test]
fn text_defaults_and_plane_coordinates_are_reported_and_editable() {
    let mut s = Session::default();
    let id = run(&mut s, json!({"op":"text","text":"H"}))["feature"].as_u64().unwrap();
    let info = run(&mut s, json!({"op":"get_object_info","id":id}));
    assert_eq!(info["type"], "text");
    assert_eq!(info["text"], "H");
    assert_eq!(info["height"]["value"], 6.0);
    assert_eq!(info["depth"]["value"], 1.0);
    assert_eq!(info["align"], "left");
    assert_eq!(info["operation"], "new");
    assert_eq!(info["body"]["kind"], "exact");

    let info = run(&mut s, json!({"op":"edit_feature","feature":id,"plane":"YZ","origin":[10,20,30],"x":2,"y":3,"height":6,"depth":2,"align":"center","name":"Label"}));
    assert_eq!(info["plane"]["origin"], json!([10.0,20.0,30.0]));
    assert_eq!(info["name"], "Label");
    let (lo, hi) = s.built.body(id as u32).unwrap().mesh.bbox().unwrap();
    close(lo.x, 10.0);
    close(hi.x, 12.0);
    close((lo.y + hi.y) / 2.0, 22.0);
    close(lo.z, 33.0);
    close(hi.z, 39.0);
    let old_width = hi.y - lo.y;
    run(&mut s, json!({"op":"edit_feature","feature":id,"angle":90,"text":"H"}));
    let (lo, hi) = s.built.body(id as u32).unwrap().mesh.bbox().unwrap();
    close(hi.y - lo.y, 6.0);
    close(hi.z - lo.z, old_width);
    close(hi.x - lo.x, 2.0);
    assert!(s.built.errors.is_empty());
}

#[test]
fn text_dimensions_survive_unit_changes_and_save_open() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"set_parameter","name":"label_height","expr":"6 mm"}));
    let id = run(&mut s, json!({"op":"text","text":"HI","height":"$label_height","depth":"2 mm","spacing":"1 mm","x":"3 mm","y":"4 mm"}))["feature"].as_u64().unwrap() as u32;
    let bounds = s.built.body(id).unwrap().mesh.bbox().unwrap();
    let before = volume(&s, id);
    for units in ["in", "cm", "mm"] {
        run(&mut s, json!({"op":"set_units","units":units}));
        let now = s.built.body(id).unwrap().mesh.bbox().unwrap();
        assert!(now.0.distance(bounds.0) < 1e-7 && now.1.distance(bounds.1) < 1e-7);
        close(volume(&s, id), before);
    }
    run(&mut s, json!({"op":"set_parameter","name":"label_height","expr":"8 mm"}));
    let (lo, hi) = s.built.body(id).unwrap().mesh.bbox().unwrap();
    close(hi.z - lo.z, 2.0);
    close(hi.y - lo.y, 8.0);
    let serialized = io::to_json(&s.doc);
    assert_eq!(serde_json::from_str::<Value>(&serialized).unwrap()["version"], 2);
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("ferrender-text-api-{}-{nonce}.ferr", std::process::id()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
    let _cleanup = Cleanup(path.clone());
    run(&mut s, json!({"op":"save","path":path}));
    assert!(!s.dirty);
    run(&mut s, json!({"op":"new"}));
    run(&mut s, json!({"op":"open","path":path}));
    assert_eq!(io::to_json(&s.doc), serialized);
    assert!(s.built.errors.is_empty());
    let info = run(&mut s, json!({"op":"get_object_info","id":id}));
    assert!(info["height"]["expr"].as_str().unwrap().contains("$label_height"));
    assert_eq!(info["text"], "HI");
}

#[test]
fn default_text_size_is_physical_even_in_an_inch_document() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"new","units":"in"}));
    let id = run(&mut s, json!({"op":"text","text":"H"}))["feature"].as_u64().unwrap() as u32;
    let (lo, hi) = s.built.body(id).unwrap().mesh.bbox().unwrap();
    close(hi.y - lo.y, 6.0);
    close(hi.z - lo.z, 1.0);
}

#[test]
fn rejected_text_and_wrong_types_preserve_geometry_and_redo() {
    let mut s = Session::default();
    let id = run(&mut s, json!({"op":"text","text":"H"}))["feature"].as_u64().unwrap();
    run(&mut s, json!({"op":"edit_feature","feature":id,"text":"HI"}));
    assert!(s.undo());
    let before = io::to_json(&s.doc);
    let before_volume = volume(&s, id as u32);
    let before_dirty = s.dirty;
    for command in [
        json!({"op":"text","text":"\u{10ffff}"}),
        json!({"op":"edit_feature","feature":id,"text":"\u{10ffff}"}),
        json!({"op":"edit_feature","feature":id,"text":123}),
        json!({"op":"edit_feature","feature":id,"height":null}),
        json!({"op":"edit_feature","feature":id,"depth":false}),
        json!({"op":"edit_feature","feature":id,"spacing":[]}),
        json!({"op":"edit_feature","feature":id,"align":123}),
        json!({"op":"edit_feature","feature":id,"operation":"intersect"}),
        json!({"op":"text","text":"H","plane":false}),
        json!({"op":"text","text":"H","origin":null}),
        json!({"op":"text","text":"H","height":0}),
        json!({"op":"text","text":"H","depth":0}),
    ] {
        assert!(execute(&mut s, &command, None).is_err(), "accepted {command}");
        assert_eq!(io::to_json(&s.doc), before, "rejected command changed document: {command}");
        close(volume(&s, id as u32), before_volume);
        assert_eq!(s.dirty, before_dirty);
        assert!(s.can_redo());
    }
    assert!(s.redo());
    assert_eq!(run(&mut s, json!({"op":"get_object_info","id":id}))["text"], "HI");
}

#[test]
fn attached_text_modifies_only_its_selected_body_and_operation_can_change() {
    let mut s = Session::default();
    let target = plate(&mut s);
    // A second body occupies the same space. A generic join/cut would affect
    // both; attached lettering must honor the explicitly selected target.
    let other = plate(&mut s);
    let unchanged = s.built.body(other).unwrap().mesh.clone();
    let base = volume(&s, target);
    let output = run(&mut s, json!({"op":"text","text":"H","body":target,"face":[-10,0,4],"operation":"join","height":4,"depth":1}));
    let feature = output["feature"].as_u64().unwrap();
    assert_eq!(s.built.bodies.len(), 2);
    assert!(volume(&s, target) > base);
    assert_eq!(s.built.body(other).unwrap().mesh.tris, unchanged.tris);
    assert_eq!(output["changed_bodies"].as_array().unwrap().len(), 1);
    let info = run(&mut s, json!({"op":"edit_feature","feature":feature,"operation":"cut","depth":"0.5 mm"}));
    assert_eq!(info["body"], target);
    assert_eq!(info["face"], json!([-10.0,0.0,4.0]));
    assert!(volume(&s, target) < base);
    assert_eq!(s.built.body(other).unwrap().mesh.tris, unchanged.tris);
    assert!(s.built.errors.is_empty());

    run(&mut s, json!({"op":"edit_feature","feature":feature,"operation":"new"}));
    close(volume(&s, target), base);
    assert_eq!(s.built.bodies.len(), 3);
    let info = run(&mut s, json!({"op":"edit_feature","feature":feature,"operation":"join","body":target,"face":[-10,0,4]}));
    assert_eq!(info["body"], target);
    assert_eq!(s.built.bodies.len(), 2);
    assert_eq!(s.built.body(other).unwrap().mesh.tris, unchanged.tris);
}

#[test]
fn invalid_attachment_or_overhanging_text_is_rejected_atomically() {
    let mut s = Session::default();
    let target = plate(&mut s);
    let before = io::to_json(&s.doc);
    let geometry = s.built.body(target).unwrap().mesh.clone();
    for command in [
        json!({"op":"text","text":"H","operation":"join"}),
        json!({"op":"text","text":"H","operation":"new","body":target,"face":[0,0,4]}),
        json!({"op":"text","text":"H","operation":"join","body":target,"face":[0,0,4],"plane":"XY"}),
        json!({"op":"text","text":"H","operation":"join","body":target,"face":[0,0,50]}),
        json!({"op":"text","text":"H","operation":"join","body":target,"face":[19,0,4],"height":6}),
        json!({"op":"text","text":"H","operation":"join","body":u64::MAX,"face":[0,0,4]}),
    ] {
        assert!(execute(&mut s, &command, None).is_err(), "accepted {command}");
        assert_eq!(io::to_json(&s.doc), before);
        assert_eq!(s.built.body(target).unwrap().mesh.tris, geometry.tris);
    }
    run(&mut s, json!({"op":"create_sketch"}));
    run(&mut s, json!({"op":"add_geometry","items":[{"type":"circle","center":[60,0],"radius":10}]}));
    let cylinder = run(&mut s, json!({"op":"extrude","distance":4,"operation":"new"}))["feature"].as_u64().unwrap();
    let before = io::to_json(&s.doc);
    assert!(execute(&mut s, &json!({"op":"text","text":"H","operation":"cut","body":cylinder,"face":[70,0,2]}), None).is_err());
    assert_eq!(io::to_json(&s.doc), before);
    assert!(s.built.bodies.iter().all(|b| b.mesh.bbox().is_some_and(|(lo, hi)| lo.is_finite() && hi.is_finite() && (hi - lo).cmpgt(DVec3::ZERO).all())));
}

#[test]
fn face_edits_reject_downstream_coordinates_until_the_timeline_is_rolled_back() {
    let mut s = Session::default();
    let target = plate(&mut s);
    let feature = run(&mut s, json!({"op":"text","text":"H","body":target,"face":[-10,0,4],"operation":"join","height":4,"depth":1}))["feature"].as_u64().unwrap();
    run(&mut s, json!({"op":"transform","body":target,"translate":[2,0,0]}));
    let before = io::to_json(&s.doc);
    let before_mesh = s.built.body(target).unwrap().mesh.tris.clone();
    let error = execute(&mut s, &json!({"op":"edit_feature","feature":feature,"body":target,"face":[0,0,4]}), None).unwrap_err();
    assert!(error.contains("rollback"), "{error}");
    assert_eq!(io::to_json(&s.doc), before);
    assert_eq!(s.built.body(target).unwrap().mesh.tris, before_mesh);
    assert!(s.doc.rollback.is_none(), "checking placement must not change the real timeline");

    // Ordinary dimensions/text edits keep their existing attachment and remain
    // available even when a later transform changes the displayed coordinates.
    run(&mut s, json!({"op":"edit_feature","feature":feature,"text":"H","height":3}));
    assert!(s.built.errors.is_empty());
    run(&mut s, json!({"op":"rollback","to":feature}));
    run(&mut s, json!({"op":"edit_feature","feature":feature,"body":target,"face":[0,0,4]}));
    let fr_core::FeatureKind::Text(text) = &s.doc.feature(feature as u32).unwrap().kind else { panic!("text feature missing") };
    assert_eq!(text.face, Some(DVec3::new(0.0, 0.0, 4.0)));
    close(text.frame.unwrap()[1].z, 4.0); // capture the base body, not raised text
    run(&mut s, json!({"op":"rollback","to":target}));
    let before = io::to_json(&s.doc);
    assert!(execute(&mut s, &json!({"op":"edit_feature","feature":feature,"body":target,"face":[19,0,4]}), None).is_err(), "a feature outside the active timeline still needs placement validation");
    assert_eq!(io::to_json(&s.doc), before);
    run(&mut s, json!({"op":"edit_feature","feature":feature,"body":target,"face":[0,0,4]}));
    run(&mut s, json!({"op":"rollback","to":"end"}));
    assert!(s.built.errors.is_empty());
    let raised_min_x = s.built.body(target).unwrap().mesh.tris.iter().flatten().filter(|p| p.z > 4.5).map(|p| p.x).fold(f64::INFINITY, f64::min);
    close(raised_min_x, 2.0); // the later move is applied exactly once
}

#[test]
fn edited_text_cannot_attach_to_its_own_raised_face() {
    let mut s = Session::default();
    let target = plate(&mut s);
    let feature = run(&mut s, json!({"op":"text","text":"H","body":target,"face":[-10,0,4],"operation":"join","height":4,"depth":1}))["feature"].as_u64().unwrap();
    let body = s.built.body(target).unwrap();
    let top = body.mesh.tris.iter().find(|tri| tri.iter().all(|p| (p.z - 5.0).abs() < 1e-7)).unwrap();
    let at = top.iter().copied().sum::<DVec3>() / 3.0;
    let before = io::to_json(&s.doc);
    assert!(execute(&mut s, &json!({"op":"edit_feature","feature":feature,"body":target,"face":at.to_array()}), None).is_err());
    assert_eq!(io::to_json(&s.doc), before);
    let base_point = DVec3::new(at.x, at.y, 4.0);
    run(&mut s, json!({"op":"edit_feature","feature":feature,"body":target,"face":base_point.to_array()}));
    assert!(s.built.errors.is_empty());
}

#[test]
fn unrelated_downstream_body_does_not_block_text_face_edits() {
    let mut s = Session::default();
    let target = plate(&mut s);
    let feature = run(&mut s, json!({"op":"text","text":"H","body":target,"face":[-10,0,4],"operation":"join","height":4,"depth":1}))["feature"].as_u64().unwrap();
    let unrelated = plate(&mut s);
    let unchanged = s.built.body(unrelated).unwrap().mesh.tris.clone();
    run(&mut s, json!({"op":"edit_feature","feature":feature,"body":target,"face":[0,0,4]}));
    assert!(s.doc.rollback.is_none());
    assert_eq!(s.built.body(unrelated).unwrap().mesh.tris, unchanged);
    assert!(s.built.errors.is_empty());
}
