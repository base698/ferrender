//! Bodies can be named by hand; the name is kept with the design.

use fr_core::api::execute;
use fr_core::{Id, Session};
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    execute(s, &c, None).unwrap_or_else(|e| panic!("{c} failed: {e}"))
}

fn name(s: &Session, body: Id) -> String {
    s.built.body(body).unwrap().name.clone()
}

#[test]
fn a_body_keeps_the_name_it_is_given() {
    let mut s = Session::default();
    let a = cmd(&mut s, json!({"op": "primitive", "type": "box", "width": 10, "depth": 10, "height": 10}))["changed_bodies"][0]["id"].as_u64().unwrap() as Id;
    let b = cmd(&mut s, json!({"op": "primitive", "type": "sphere", "diameter": 10, "position": [40, 0, 0]}))["changed_bodies"][0]["id"].as_u64().unwrap() as Id;
    assert_eq!((name(&s, a).as_str(), name(&s, b).as_str()), ("Body1", "Body2"));

    let out = cmd(&mut s, json!({"op": "rename_body", "body": b, "name": "  Top lens  "}));
    assert_eq!(out["name"], "Top lens");
    assert_eq!((name(&s, a).as_str(), name(&s, b).as_str()), ("Body1", "Top lens"));
    let scene = cmd(&mut s, json!({"op": "get_scene_info"}));
    assert!(scene["bodies"].as_array().unwrap().iter().any(|x| x["name"] == "Top lens"), "{}", scene["bodies"]);

    // The name follows the body through later features, and through undo and redo.
    cmd(&mut s, json!({"op": "transform", "body": b, "translate": [0, 20, 0]}));
    assert_eq!(name(&s, b), "Top lens");
    cmd(&mut s, json!({"op": "undo"}));
    cmd(&mut s, json!({"op": "undo"}));
    assert_eq!(name(&s, b), "Body2");
    cmd(&mut s, json!({"op": "redo"}));
    assert_eq!(name(&s, b), "Top lens");

    // It is saved with the design, which then needs format 18.
    let dir = std::env::temp_dir().join(format!("ferrender-names-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("named.ferr");
    cmd(&mut s, json!({"op": "save", "path": path.to_str().unwrap(), "cache": false}));
    assert!(std::fs::read_to_string(&path).unwrap().contains("\"version\": 18"));
    let again = Session::open(&path).unwrap();
    assert_eq!(name(&again, b), "Top lens");

    // An empty name gives back the automatic one; a design without names keeps its older format.
    cmd(&mut s, json!({"op": "rename_body", "body": b, "name": ""}));
    assert_eq!(name(&s, b), "Body2");
    cmd(&mut s, json!({"op": "save", "path": path.to_str().unwrap(), "cache": false}));
    assert!(!std::fs::read_to_string(&path).unwrap().contains("\"version\": 18"));

    assert!(execute(&mut s, &json!({"op": "rename_body", "body": 999, "name": "x"}), None).unwrap_err().contains("no body"));
    assert!(execute(&mut s, &json!({"op": "rename_body", "body": b}), None).unwrap_err().contains("name"));
}
