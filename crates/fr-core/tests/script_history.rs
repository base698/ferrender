//! Script-owned features retain normal history semantics.
use fr_core::{Session, FeatureKind, api::execute, doc::ScriptRun};
use serde_json::{json, Value};
fn cmd(s: &mut Session, c: Value) -> Value { execute(s, &c, None).unwrap_or_else(|e| panic!("{c}: {e}")) }
fn run_with_boxes() -> (Session, u32, Vec<u32>) {
    let mut s = Session::default();
    let chip = s.doc.add_feature(FeatureKind::ScriptRun(ScriptRun { script_name: "Boxes".into(), source: "const META = #{name: \"Boxes\"}; fn run(i) {}".into(), source_hash: 0, inputs: json!({}) }));
    let mut ids = Vec::new();
    for x in [0, 20] {
        cmd(&mut s, json!({"op":"primitive", "type":"box", "width":10, "depth":10, "height":10, "position":[x,0,0]}));
        let f = s.doc.features.last_mut().unwrap();
        f.made_by = Some(chip); ids.push(f.id);
    }
    s.rebuild(); (s, chip, ids)
}
#[test]
fn suppressing_a_script_is_reversible_and_preserves_individual_suppression() {
    let (mut s, chip, ids) = run_with_boxes();
    cmd(&mut s, json!({"op":"edit_feature", "feature":ids[1], "suppressed":true}));
    cmd(&mut s, json!({"op":"edit_feature", "feature":chip, "suppressed":true}));
    assert!(s.built.bodies.is_empty());
    assert!(!s.doc.feature(ids[0]).unwrap().suppressed, "inherited suppression must not overwrite the child's own state");
    assert!(s.doc.is_suppressed(ids[0]));
    cmd(&mut s, json!({"op":"edit_feature", "feature":chip, "suppressed":false}));
    assert_eq!(s.built.bodies.len(), 1);
    assert_eq!(s.built.bodies[0].id, ids[0]);
    assert!(s.doc.feature(ids[1]).unwrap().suppressed);
    assert!(s.undo()); assert!(s.built.bodies.is_empty());
    assert!(s.redo()); assert_eq!(s.built.bodies.len(), 1);
    let text = fr_core::io::to_json(&s.doc);
    let d: fr_core::Document = serde_json::from_str(&text).unwrap();
    assert!(!d.feature(ids[0]).unwrap().suppressed);
}
#[test]
fn deleting_a_script_through_the_api_removes_its_children_and_keeps_a_valid_document() {
    let (mut s, chip, _) = run_with_boxes();
    let before = s.doc.clone();
    let result = cmd(&mut s, json!({"op":"delete_feature", "feature":chip}));
    assert_eq!(result["removed_features"].as_array().unwrap().len(), 3);
    assert!(s.doc.features.is_empty());
    fr_core::validation::document(&s.doc).unwrap();
    assert!(s.undo()); assert_eq!(s.doc, before); assert_eq!(s.built.bodies.len(), 2);
}

#[test]
fn a_saved_suppressed_script_can_be_reenabled_after_reopening() {
    let (mut s, chip, _) = run_with_boxes();
    cmd(&mut s, json!({"op":"edit_feature", "feature":chip, "suppressed":true}));
    let doc: fr_core::Document = serde_json::from_str(&fr_core::io::to_json(&s.doc)).unwrap();
    let mut reopened = Session::new(doc);
    assert!(reopened.built.bodies.is_empty());
    cmd(&mut reopened, json!({"op":"edit_feature", "feature":chip, "suppressed":false}));
    assert_eq!(reopened.built.bodies.len(), 2);
}
