//! Freezes the on-disk JSON of every feature kind. A change to how a feature
//! serializes must come with an updated fixture and, when it matters to other
//! readers, a note in docs/FILE_FORMAT.md. Regenerate with
//! `FERRENDER_UPDATE_FIXTURES=1 cargo test -p fr-core --test format_fixture`.

use std::path::PathBuf;

use fr_core::api::execute;
use fr_core::{FeatureKind, Session, io};
use serde_json::{Value as J, json};

fn run(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

fn tiny_stl() -> PathBuf {
    // A fixed file name: the import feature is named after it, and the fixture must not change between runs.
    let dir = std::env::temp_dir().join(format!("ferrender-fixture-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scan.stl");
    let mut bytes = vec![b' '; 80];
    bytes.extend(4u32.to_le_bytes());
    // A tetrahedron: four triangles sharing four corners, closed and outward.
    for t in [[[0.0f32, 0.0, 0.0], [0.0, 10.0, 0.0], [10.0, 0.0, 0.0]], [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, 10.0]], [[0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [0.0, 10.0, 0.0]], [[10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]]] {
        bytes.extend([0u8; 12]);
        for v in t { for c in v { bytes.extend(c.to_le_bytes()); } }
        bytes.extend([0u8; 2]);
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

/// One of everything, built through the public API so ids and values are deterministic.
fn everything() -> Session {
    let mut s = Session::default();
    let last = |s: &Session| s.doc.features.last().unwrap().id;
    run(&mut s, json!({"op": "set_parameter", "name": "width", "expr": "20 mm"}));
    // Sketch with every entity type, a dimension, a position dimension and an angle lock.
    run(&mut s, json!({"op": "create_sketch", "plane": "XY", "name": "Base"}));
    let base = last(&s);
    let items = run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [20, 10]}]}));
    let first_line = items["items"][0]["entities"][0].clone();
    run(&mut s, json!({"op": "add_constraint", "kind": "distance", "refs": [first_line], "value": "$width"}));
    run(&mut s, json!({"op": "add_constraint", "kind": "angle", "refs": [first_line], "value": 0}));
    // Every other entity type, a position dimension, and an arc guide; drawn but never extruded.
    run(&mut s, json!({"op": "create_sketch", "plane": "XY", "name": "Shapes"}));
    let items = run(&mut s, json!({"op": "add_geometry", "items": [
        {"type": "circle", "center": [40, 5], "radius": 3},
        {"type": "arc3", "start": [60, 0], "through": [65, 5], "end": [70, 0]},
        {"type": "line", "from": [70, 0], "to": [60, 0]},
        {"type": "spline", "points": [[80, 0], [84, 4], [88, -4], [92, 0]]},
        {"type": "point", "at": [100, 100]}
    ]}));
    let lone_point = items["items"][4]["points"][0].clone();
    run(&mut s, json!({"op": "point_coordinates", "point": lone_point, "x": "100 mm", "y": "100 mm"}));
    run(&mut s, json!({"op": "extrude", "sketch": base, "profiles": [0], "distance": 5, "operation": "new"}));
    let plate = last(&s);
    run(&mut s, json!({"op": "pattern", "feature": plate, "type": "linear", "axis": "x", "count": 2, "spacing": 30, "axis2": "y", "count2": 2, "spacing2": 15}));
    run(&mut s, json!({"op": "create_plane", "kind": "offset", "base": "XY", "distance": 5}));
    let plane = last(&s);
    run(&mut s, json!({"op": "create_sketch", "plane": {"id": plane}, "name": "On plane"}));
    let on_plane = last(&s);
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "circle", "center": [10, 5], "radius": 2}]}));
    run(&mut s, json!({"op": "extrude", "sketch": on_plane, "distance": 4, "operation": "join"}));
    run(&mut s, json!({"op": "fillet_edges", "body": plate, "edges": "all", "radius": 0.5}));
    run(&mut s, json!({"op": "hole", "body": plate, "at": [5, 5, 5], "thread": "M3", "through": true}));
    run(&mut s, json!({"op": "create_sketch", "plane": "XZ", "name": "Rim"}));
    let rim = last(&s);
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [30, 0], "to": [34, 6]}]}));
    run(&mut s, json!({"op": "revolve", "sketch": rim, "axis": "y", "angle": 360, "operation": "new"}));
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 10, "depth": 10, "height": 10, "position": [-30, -5, 0]}));
    let cube = last(&s);
    run(&mut s, json!({"op": "primitive", "type": "cylinder", "diameter": 6, "height": 20, "position": [-50, 0, 0]}));
    let rod = last(&s);
    run(&mut s, json!({"op": "thread", "body": rod, "face": [-47, 0, 10], "thread": "M6"}));
    run(&mut s, json!({"op": "shell", "body": cube, "open_faces": [[-25, 0, 10]], "thickness": 1}));
    run(&mut s, json!({"op": "text", "text": "FR", "height": "6 mm", "depth": "1 mm", "operation": "new", "plane": "XY", "origin": [0, 30, 0]}));
    run(&mut s, json!({"op": "import_stl", "path": tiny_stl().display().to_string(), "units": "mm"}));
    let scan = last(&s);
    run(&mut s, json!({"op": "transform", "body": scan, "translate": [0, 50, 0], "rotate": [0, 0, 0], "scale": 1}));
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 4, "depth": 4, "height": 4, "position": [-30, -5, 8]}));
    let cap = last(&s);
    run(&mut s, json!({"op": "combine", "target": cube, "tools": [cap], "operation": "join"}));
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 10, "depth": 10, "height": 10, "position": [-30, -5, 20]}));
    let block = last(&s);
    run(&mut s, json!({"op": "split_body", "body": block, "plane": "XZ"}));
    run(&mut s, json!({"op": "remove_body", "body": block}));
    run(&mut s, json!({"op": "create_component", "name": "Bracket", "activate": true}));
    let bracket = last(&s);
    run(&mut s, json!({"op": "primitive", "type": "sphere", "diameter": 5, "position": [0, -20, 0]}));
    run(&mut s, json!({"op": "move_component", "id": bracket, "translate": [0, 0, 20]}));
    run(&mut s, json!({"op": "activate_component", "id": 0}));
    s
}

/// Kernel-derived pick coordinates may differ by a few floating-point ulps on
/// Intel and ARM. Keep schema, integers, strings and array order exact.
fn fixture_difference(a: &J, b: &J, path: &str) -> Option<String> {
    match (a, b) {
        (J::Number(a), J::Number(b)) if a.is_f64() && b.is_f64() => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            if (a - b).abs() <= 32.0 * f64::EPSILON * (1.0 + a.abs().max(b.abs())) { return None; }
        }
        (J::Array(a), J::Array(b)) if a.len() == b.len() => {
            return a.iter().zip(b).enumerate().find_map(|(i, (a, b))| fixture_difference(a, b, &format!("{path}[{i}]")));
        }
        (J::Object(a), J::Object(b)) if a.keys().eq(b.keys()) => {
            return a.iter().find_map(|(key, a)| fixture_difference(a, &b[key], &format!("{path}.{key}")));
        }
        _ if a == b => return None,
        _ => {}
    }
    Some(format!("{path}: fixture {a}, current {b}"))
}

#[test]
fn fixture_comparison_keeps_schema_and_identity_strict() {
    assert!(fixture_difference(&json!({"id":1,"point":[4.999999999999999]}), &json!({"id":1,"point":[5.0]}), "$ ").is_none());
    for changed in [json!({"id":2,"point":[5.0]}), json!({"id":1,"point":[5.001]}), json!({"id":1,"point":[5]}), json!({"id":1,"points":[5.0]}), json!({"id":1,"point":[5.0,0.0]})] {
        assert!(fixture_difference(&json!({"id":1,"point":[5.0]}), &changed, "$ ").is_some(), "accepted schema/identity change: {changed}");
    }
}

#[test]
fn every_feature_kind_serializes_as_the_fixture_says() {
    let s = everything();
    let kinds: Vec<&str> = s.doc.features.iter().map(|f| f.type_name()).collect();
    for expected in ["sketch", "extrude", "pattern", "plane", "fillet", "hole", "revolve", "box", "cylinder", "sphere", "thread", "shell", "text", "import", "transform", "combine", "split", "remove", "component"] {
        assert!(kinds.contains(&expected), "the fixture document has no {expected} feature; kinds: {kinds:?}");
    }
    let errors: Vec<_> = s.built.errors.iter().collect();
    assert!(errors.is_empty(), "the fixture document must build cleanly: {errors:?}");
    assert!(io::needs_container(&s.doc), "the import makes this a container design");
    // The fillet, shell, thread and text learned semantic face tags on their first build, which is format 13.
    assert_eq!(io::design_version(&s.doc), 13);
    let _ = s.doc.features.iter().filter(|f| matches!(f.kind, FeatureKind::Import(_))).count();

    let text = io::to_json(&s.doc);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/all-features.ferr");
    if std::env::var_os("FERRENDER_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let frozen = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no fixture at {}; run with FERRENDER_UPDATE_FIXTURES=1", path.display()));
    if let Some(difference) = fixture_difference(&serde_json::from_str(&frozen).unwrap(), &serde_json::from_str(&text).unwrap(), "$") {
        panic!("the serialized document differs from the fixture: {difference}\nIf the change is intended, regenerate with FERRENDER_UPDATE_FIXTURES=1 and update docs/FILE_FORMAT.md.");
    }
    // The fixture must also round-trip through both encodings.
    let plain = io::from_json(&text).unwrap();
    assert_eq!(plain, s.doc);
    let (bytes, container) = io::encode(&s.doc, &io::Extras::default()).unwrap();
    assert!(container);
    assert_eq!(io::decode(&bytes).unwrap(), s.doc);
}

#[test]
fn early_v04_ordinal_tag_fixture_migrates_without_changing_its_geometry() {
    let text=include_str!("fixtures/all-features-v10.ferr");
    let old=io::from_json(text).unwrap();let features=old.features.len();
    let s=Session::new(old);
    assert!(s.built.errors.is_empty(), "legacy fixture failed: {:?}",s.built.errors);
    assert_eq!(s.doc.features.len(),features);
    for f in &s.doc.features {match &f.kind {
        FeatureKind::Blend(b)=>assert!(b.tags.iter().flatten().all(|t|!t.legacy())),
        FeatureKind::Shell(sh)=>assert!(sh.tags.iter().flatten().all(|t|!t.legacy())),
        FeatureKind::Thread(t)=>assert!(t.tag.as_ref().is_none_or(|t|!t.legacy())),
        _=>{},
    }}
}
