//! Exported generators must survive a fresh session and preserve document meaning.
use fr_core::{Session, FeatureKind, api::execute, script::{self, Request}};
use serde_json::{json, Value};
fn cmd(s: &mut Session, c: Value) -> Value { execute(s, &c, None).unwrap_or_else(|e| panic!("{c}: {e}")) }
fn replay(s: &Session, inputs: Value) -> Session {
    let source = script::export_timeline(s).unwrap();
    script::meta(&source).unwrap();
    let mut req = Request::new(source); req.inputs = inputs;
    let mut target = Session::default();
    script::run(&mut target, &req).unwrap();
    target
}
#[test]
fn parameter_expressions_keep_units_dependencies_and_keyword_names() {
    let mut s = Session::default();
    for (name, expr) in [("ratio", "2"), ("if", "10 mm"), ("turn", "(pi / 2) rad"), ("angle", "$turn / 2"), ("width", "$if * $ratio"), ("expr", "3")] {
        s.doc.set_param(name, expr).unwrap();
    }
    // A valid saved parameter table can refer forward after edits/reordering.
    s.doc.params.reverse();
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":"$width", "depth":6, "height":7}));
    let again = replay(&s, json!({"ratio":"3", "if":"12 mm"}));
    assert!(again.built.errors.is_empty(), "{:?}", again.built.errors);
    assert_eq!(again.doc.quantity("$ratio").unwrap().dim, fr_core::expr::Dim::None);
    assert_eq!(again.doc.quantity("$angle").unwrap().dim, fr_core::expr::Dim::Angle);
    assert!((again.doc.quantity("$angle").unwrap().v - 45.).abs() < 1e-8);
    assert!((again.doc.quantity("$width").unwrap().v - 36.).abs() < 1e-8);
    assert_eq!(again.doc.quantity("$expr").unwrap().v, 3.);
}
#[test]
fn components_script_ownership_suppression_and_hidden_bodies_survive() {
    let mut s = Session::default();
    cmd(&mut s, json!({"op":"create_component", "name":"Bracket"}));
    let component = s.doc.active_component;
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":10, "depth":6, "height":7}));
    let body = s.built.bodies[0].id;
    let chip = s.doc.add_feature(FeatureKind::ScriptRun(fr_core::doc::ScriptRun {
        script_name:"Kept as data".into(), source:"not executable script source".into(), source_hash:1, inputs:json!({})
    }));
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":2, "depth":2, "height":2, "position":[20,0,0]}));
    let output = s.doc.features.last_mut().unwrap(); output.made_by = Some(chip); output.suppressed=true;
    s.rebuild();
    cmd(&mut s, json!({"op":"set_visible", "id":body, "visible":false}));
    let again = replay(&s, json!({}));
    assert_eq!(again.doc.features, s.doc.features);
    assert_eq!(again.doc.active_component, component);
    assert_eq!(again.doc.hidden_bodies, s.doc.hidden_bodies);
    assert_eq!(again.built.bodies.len(), 1);
    assert_eq!(again.built.bodies[0].component, component);
}
#[test]
fn unsupported_replay_states_are_refused_before_saving_a_script() {
    let mut s = Session::default();
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":10, "depth":6, "height":7}));
    s.doc.rollback = Some(0); s.rebuild();
    assert!(script::export_timeline(&s).unwrap_err().contains("timeline"));
    s.doc.rollback = None; s.doc.hidden_bodies.push(999); s.rebuild();
    assert!(script::export_timeline(&s).unwrap_err().contains("hidden"));
    s.doc.hidden_bodies.clear(); s.read_only = true;
    assert!(script::export_timeline(&s).unwrap_err().contains("read-only"));
}
#[test]
fn large_embedded_mesh_is_refused_with_an_actionable_message() {
    let mut m = fr_core::mesh::Mesh::default();
    for _ in 0..25_000 { m.push([glam::DVec3::ZERO, glam::DVec3::X, glam::DVec3::Y]); }
    let mut s = Session::default(); s.doc.add_feature(FeatureKind::Import(m));
    let err = script::export_timeline(&s).err().expect("oversized export must fail before allocating the source");
    assert!(err.contains("1 MB") && err.contains("mesh"), "{err}");
}
#[test]
fn oversized_non_mesh_data_and_unsupported_integer_literals_are_refused() {
    let mut s = Session::default();
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":2, "depth":2, "height":2}));
    s.doc.features[0].name = "x".repeat(script::MAX_SOURCE_BYTES + 1);
    assert!(script::export_timeline(&s).err().unwrap().contains("1 MB"));
    s.doc.features[0].name = "bracket [\"name\"]\n雪".into();
    let again = replay(&s, json!({}));
    assert_eq!(again.doc.features[0].name, s.doc.features[0].name);
    s.doc.add_feature(FeatureKind::ScriptRun(fr_core::doc::ScriptRun {
        script_name:"Large input".into(), source:"preserved as data".into(), source_hash:1, inputs:json!({"large":u64::MAX})
    }));
    assert!(script::export_timeline(&s).err().unwrap().contains("runnable script"));
}
#[test]
fn small_inline_mesh_can_still_be_exported() {
    let mut s = Session::default();
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":2, "depth":2, "height":2}));
    let mesh = s.built.bodies[0].mesh.clone();
    let mut imported = Session::default(); imported.doc.add_feature(FeatureKind::Import(mesh)); imported.rebuild();
    let again = replay(&imported, json!({}));
    assert_eq!(again.built.bodies.len(), 1);
    assert!((again.built.bodies[0].mesh.volume()-8.).abs()<1e-6);
}
