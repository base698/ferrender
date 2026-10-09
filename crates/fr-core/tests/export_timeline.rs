//! Exported generators must survive a fresh session and preserve document meaning.
use std::path::PathBuf;

use fr_core::{Session, FeatureKind, api::execute, script::{self, Export, Request}};
use serde_json::{json, Value};

fn cmd(s: &mut Session, c: Value) -> Value { execute(s, &c, None).unwrap_or_else(|e| panic!("{c}: {e}")) }

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferrender-export-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Writes the export beside its sidecars and runs it from there.
fn replay_export(export: &Export, folder: &PathBuf, inputs: Value) -> Session {
    script::meta(&export.source).unwrap();
    for (name, bytes) in &export.files { std::fs::write(folder.join(name), bytes).unwrap(); }
    std::fs::write(folder.join("design.rhai"), &export.source).unwrap();
    let mut req = Request::new(export.source.clone());
    req.inputs = inputs;
    req.script_dir = Some(folder.clone());
    req.sandbox.allowed = vec![folder.clone()];
    let mut target = Session::default();
    script::run(&mut target, &req).unwrap_or_else(|e| panic!("{e}\n{}", export.source));
    target
}

fn replay(s: &Session, inputs: Value) -> Session {
    let export = script::export_timeline(s).unwrap();
    assert!(export.files.is_empty(), "no sidecars expected: {:?}", export.files.iter().map(|f| &f.0).collect::<Vec<_>>());
    replay_export(&export, &dir("inline"), inputs)
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
fn the_timeline_marker_is_restored_and_stale_hidden_ids_are_dropped() {
    let mut s = Session::default();
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":10, "depth":6, "height":7}));
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":2, "depth":2, "height":2, "position":[20,0,0]}));
    let first = s.doc.features[0].id;
    // Marker after the first feature: the whole timeline is exported and the marker put back.
    cmd(&mut s, json!({"op":"rollback", "to": first}));
    assert_eq!(s.built.bodies.len(), 1);
    let export = script::export_timeline(&s).unwrap();
    assert!(export.notes.iter().any(|n| n.contains("marker")), "{:?}", export.notes);
    let again = replay_export(&export, &dir("marker"), json!({}));
    assert_eq!(again.doc.features.len(), 2);
    assert_eq!(again.doc.rollback, Some(1));
    assert_eq!(again.built.bodies.len(), 1);
    // Marker at the start.
    cmd(&mut s, json!({"op":"rollback", "to": "start"}));
    let again = replay_export(&script::export_timeline(&s).unwrap(), &dir("marker-start"), json!({}));
    assert_eq!(again.doc.rollback, Some(0));
    assert!(again.built.bodies.is_empty());
    // A hidden id that no longer names anything is dropped with a note; the rest survive.
    cmd(&mut s, json!({"op":"rollback", "to": "end"}));
    s.doc.hidden_bodies.push(999);
    let export = script::export_timeline(&s).unwrap();
    assert!(export.notes.iter().any(|n| n.contains("hidden")), "{:?}", export.notes);
    let again = replay_export(&export, &dir("stale-hidden"), json!({}));
    assert!(again.doc.hidden_bodies.is_empty());
    assert_eq!(again.built.bodies.len(), 2);
    // Read-only designs still have nothing to export.
    s.read_only = true;
    assert!(script::export_timeline(&s).unwrap_err().contains("read-only"));
}

#[test]
fn a_failed_feature_is_exported_suppressed_with_a_note() {
    let mut s = Session::default();
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":10, "depth":6, "height":3}));
    let body = s.built.bodies[0].id;
    // A combine with a tool that does not exist fails to build.
    s.doc.add_feature(FeatureKind::Combine(fr_core::doc::Combine { target: body, tools: vec![999], op: fr_core::doc::Op::Join, keep_tools: false }));
    cmd(&mut s, json!({"op":"primitive", "type":"box", "width":2, "depth":2, "height":2, "position":[30,0,0]}));
    s.rebuild();
    assert_eq!(s.built.errors.len(), 1, "{:?}", s.built.errors);
    let failed = s.doc.features[1].id;
    let export = script::export_timeline(&s).unwrap();
    assert!(export.source.contains("failed in the exported design"), "{}", export.source);
    assert!(export.notes.iter().any(|n| n.contains("suppressed")), "{:?}", export.notes);
    let again = replay_export(&export, &dir("failed"), json!({}));
    assert!(again.built.errors.is_empty(), "{:?}", again.built.errors);
    assert_eq!(again.doc.features.len(), 3);
    assert!(again.doc.feature(failed).unwrap().suppressed);
    assert_eq!(again.built.bodies.len(), 2);
}

#[test]
fn a_large_imported_mesh_becomes_a_sidecar_stl_the_script_reads_back() {
    // A sphere of a few thousand triangles: well above the inline limit.
    let mut source = Session::default();
    cmd(&mut source, json!({"op":"primitive", "type":"sphere", "diameter":20}));
    let mesh = source.built.bodies[0].mesh.clone();
    assert!(mesh.len() * 48 > script::SIDECAR_BYTES, "{} triangles", mesh.len());
    let volume = mesh.volume();
    let mut s = Session::default();
    s.doc.add_feature(FeatureKind::Import(mesh));
    s.doc.features[0].name = "Scan".into();
    s.rebuild();
    let scan = s.doc.features[0].id;
    cmd(&mut s, json!({"op":"mesh_decimate", "body": scan, "target": 400}));
    let export = script::export_timeline_named(&s, "ball").unwrap();
    assert_eq!(export.files.len(), 1);
    assert_eq!(export.files[0].0, format!("ball-{scan}.stl"));
    assert!(export.source.contains("mesh_path: join(script_dir(), \"ball-"), "{}", export.source);
    assert!(export.source.len() < 20_000, "the mesh must not be inline: {} bytes", export.source.len());
    let again = replay_export(&export, &dir("sidecar"), json!({}));
    assert!(again.built.errors.is_empty(), "{:?}", again.built.errors);
    assert_eq!(again.doc.features.len(), 2, "the import keeps its id so the decimate still finds it");
    assert_eq!(again.doc.features[0].id, s.doc.features[0].id);
    assert_eq!(again.doc.features[0].name, "Scan");
    let FeatureKind::Import(m) = &again.doc.features[0].kind else { panic!() };
    assert!((m.volume() - volume).abs() < 1e-6 * volume, "{} vs {volume}", m.volume());
    assert_eq!(again.built.bodies.len(), 1);
    assert!(again.built.bodies[0].mesh.len() <= 400);
}

#[test]
fn a_large_relief_image_becomes_a_sidecar_png_the_script_reads_back() {
    // Noise does not compress: a 160 x 160 image is well over the inline limit.
    let folder = dir("relief-source");
    let (w, h) = (300u32, 300u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    let mut x: u32 = 12345;
    for _ in 0..w * h {
        x ^= x << 13; x ^= x >> 17; x ^= x << 5;
        rgba.extend([(x & 0xff) as u8, ((x >> 8) & 0xff) as u8, ((x >> 16) & 0xff) as u8, 255]);
    }
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    }
    let image = folder.join("noise.png");
    std::fs::write(&image, &png).unwrap();
    let mut s = Session::default();
    cmd(&mut s, json!({"op":"mesh_from_image", "path": image.display().to_string(), "width": 40, "depth": 2, "base": 1, "resolution": 40}));
    let volume = s.built.bodies[0].mesh.volume();
    let export = script::export_timeline_named(&s, "plaque").unwrap();
    assert_eq!(export.files.len(), 1, "{:?}", export.notes);
    assert!(export.files[0].0.ends_with(".png"));
    assert!(export.source.contains("image_path: join(script_dir()"), "{}", export.source);
    let again = replay_export(&export, &dir("relief-replay"), json!({}));
    assert!(again.built.errors.is_empty(), "{:?}", again.built.errors);
    assert_eq!(again.doc.features, s.doc.features, "the relief, image and placement are the same");
    assert!((again.built.bodies[0].mesh.volume() - volume).abs() < 1e-9);
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
