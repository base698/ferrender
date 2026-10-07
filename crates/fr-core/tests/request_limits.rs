use fr_core::{Session, api};
use serde_json::json;

#[test]
fn oversized_or_deep_batches_fail_before_any_mutation() {
    let mut s = Session::default();
    let before = fr_core::io::to_json(&s.doc);
    let mut commands = vec![json!({"op":"create_sketch"}); 1000];
    assert!(api::execute(&mut s, &json!({"op":"batch","commands":commands}), None).is_err());
    assert_eq!(before, fr_core::io::to_json(&s.doc));
    let mut deep = json!({"op":"create_sketch"});
    for _ in 0..17 { deep = json!({"op":"batch","commands":[deep]}); }
    commands = vec![json!({"op":"create_sketch"}), deep];
    assert!(api::execute(&mut s, &json!({"op":"batch","commands":commands}), None).is_err());
    assert_eq!(before, fr_core::io::to_json(&s.doc));
    let result = api::execute(&mut s, &json!({"op":"batch","commands":[{"op":"create_sketch"},{"op":"batch","commands":[{"op":"get_scene_info"}]}]}), None).unwrap();
    assert!(result[0]["sketch"].is_number());
}
