//! Scripts: META, host functions, the sandbox, limits, the samples.

use std::path::PathBuf;

use fr_core::api::execute;
use fr_core::script::{self, Request, SAMPLES};
use fr_core::Session;
use serde_json::{Value as J, json};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferrender-scripts-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_cmd(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

const SPACER: &str = r#"
const META = #{
    name: "Spacer",
    description: "A round spacer.",
    inputs: [
        #{ name: "height", kind: "length", initial: "10 mm" },
        #{ name: "bore", kind: "length", initial: "5 mm" },
        #{ name: "label", kind: "text", initial: "spacer" },
    ],
};
fn run(inputs) {
    log("making " + inputs.label);
    let made = primitive(#{ type: "cylinder", diameter: 20, height: inputs.expr.height });
    let body = made.changed_bodies[0].id;
    hole(#{ body: body, at: [0, 0, inputs.height], diameter: inputs.expr.bore, through: true });
    progress(1.0, "done");
    let bodies = scene().bodies;
    #{ body: body, volume: bodies[bodies.len() - 1].volume }
}
"#;

#[test]
fn meta_is_read_without_running_the_script() {
    let m = script::meta(SPACER).unwrap();
    assert_eq!(m.name, "Spacer");
    assert_eq!(m.inputs.len(), 3);
    assert_eq!(m.inputs[0].kind, "length");
    assert_eq!(m.inputs[0].initial, json!("10 mm"));
    assert!(script::meta("fn run(i) {}").is_err_and(|e| e.contains("META")));
    assert!(script::meta("const META = #{ name: \"x\" };").is_err_and(|e| e.contains("run")));
    assert!(script::meta("const META = #{ name: \"x\" }; fn run(i) { ").is_err_and(|e| e.contains("parse")));
    for (name, source) in SAMPLES {
        let m = script::meta(source).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!m.name.is_empty() && !m.description.is_empty(), "{name} describes itself");
    }
}

#[test]
fn a_script_builds_through_the_commands_and_reports() {
    let mut s = Session::default();
    let mut req = Request::new(SPACER);
    req.inputs = json!({"height": "12 mm", "label": "test"});
    let (tx, rx) = std::sync::mpsc::channel();
    req.events = Some(tx);
    let out = script::run(&mut s, &req).unwrap();
    assert_eq!(out.log, vec!["making test"]);
    assert_eq!(out.features.len(), 2, "a primitive and a hole: {:?}", out.features);
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let v = out.result["volume"].as_f64().unwrap();
    let expected = std::f64::consts::PI * (100.0 - 6.25) * 12.0;
    assert!((v - expected).abs() / expected < 0.01, "{v} vs {expected}");
    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.contains(&script::Event::Progress(1.0, "done".into())));
    // The parameter expression stays live: the hole's diameter is the typed text.
    let hole = s.doc.features.iter().find(|f| f.type_name() == "hole").unwrap();
    assert!(serde_json::to_string(hole).unwrap().contains("5 mm"));
    // Over the API, a failing script leaves the document untouched.
    let before = s.doc.clone();
    let err = execute(&mut s, &json!({"op": "run_script", "source": "const META = #{name: \"bad\"}; fn run(i) { primitive(#{type: \"box\", width: 5, depth: 5, height: 5}); fail(\"stop\"); }"}), None);
    assert!(err.as_ref().is_err_and(|e| e.contains("stop")), "{err:?}");
    assert_eq!(s.doc, before, "the box the failing script added was rolled back");
    let ok = run_cmd(&mut s, json!({"op": "run_script", "source": SPACER, "inputs": {"height": "3 mm"}}));
    assert_eq!(ok["features"].as_array().unwrap().len(), 2);
    assert!(s.can_undo());
    assert!(s.undo(), "the whole script run is one undo step");
    assert_eq!(s.doc, before);
}

#[test]
fn the_sandbox_and_the_limits_hold() {
    let inside = dir("inside");
    let outside = dir("outside");
    let mut s = Session::default();
    let write = |path: &PathBuf| format!("const META = #{{name: \"w\"}}; fn run(i) {{ write_text({:?}, \"hi\"); }}", path.join("note.txt").display().to_string());
    let mut req = Request::new(write(&inside));
    req.sandbox.allowed = vec![inside.clone()];
    script::run(&mut s, &req).unwrap();
    assert_eq!(std::fs::read_to_string(inside.join("note.txt")).unwrap(), "hi");
    let mut req = Request::new(write(&outside));
    req.sandbox.allowed = vec![inside.clone()];
    let err = script::run(&mut s, &req).unwrap_err();
    assert!(err.contains("outside the allowed folders"), "{err}");
    assert!(!outside.join("note.txt").exists());
    // Escaping with .. is caught after canonicalising.
    let sneaky = format!("const META = #{{name: \"w\"}}; fn run(i) {{ write_text({:?}, \"hi\"); }}", inside.join("..").join(outside.file_name().unwrap()).join("note.txt").display().to_string());
    let mut req = Request::new(sneaky);
    req.sandbox.allowed = vec![inside.clone()];
    assert!(script::run(&mut s, &req).is_err());
    // Exports go through the same gate.
    run_cmd(&mut s, json!({"op": "primitive", "type": "box", "width": 5, "depth": 5, "height": 5}));
    let mut req = Request::new(format!("const META = #{{name: \"e\"}}; fn run(i) {{ export_stl(#{{ path: {:?} }}); }}", outside.join("x.stl").display().to_string()));
    req.sandbox.allowed = vec![inside.clone()];
    assert!(script::run(&mut s, &req).unwrap_err().contains("outside"));
    // No eval, no import.
    assert!(script::run(&mut s, &Request::new("const META = #{name: \"e\"}; fn run(i) { eval(\"1+1\") }")).is_err());
    assert!(script::run(&mut s, &Request::new("const META = #{name: \"e\"}; fn run(i) { import \"x\" as x; }")).is_err());
    // An endless loop hits the operation limit.
    let t = std::time::Instant::now();
    let err = script::run(&mut s, &Request::new("const META = #{name: \"loop\"}; fn run(i) { let n = 0; while true { n += 1; } }")).unwrap_err();
    assert!(err.contains("operations") || err.contains("limit") || err.contains("too many"), "{err}");
    assert!(t.elapsed().as_secs() < 30);
    // Cancel from outside.
    let req = Request::new("const META = #{name: \"loop\"}; fn run(i) { let n = 0; while true { n += 1; progress(0.0, \"\"); } }");
    let cancel = req.cancel.clone();
    std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_millis(200)); cancel.store(true, std::sync::atomic::Ordering::Relaxed); });
    let out = script::run(&mut s, &req).unwrap();
    assert!(out.cancelled);
}

#[test]
fn ops_table_matches_the_api_and_the_reference() {
    let source = include_str!("../src/api.rs");
    // Every match arm of the command dispatcher.
    let mut arms = std::collections::BTreeSet::new();
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with('"') && t.contains("=> {") || t.starts_with('"') && t.contains("=> Ok(") {
            for part in t.split("=>").next().unwrap().split('|') {
                let name = part.trim().trim_matches('"');
                if name.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !name.is_empty() { arms.insert(name.to_owned()); }
            }
        }
    }
    // Sketch item types share the dispatcher's shape; only commands count.
    let commands: Vec<&str> = fr_core::api::OPS.to_vec();
    for op in &commands {
        assert!(arms.contains(*op), "OPS lists {op}, which has no match arm");
        assert!(fr_core::api::REFERENCE.contains(&format!("\"op\":\"{op}\"")) || matches!(*op, "import_stl" | "run_script" | "script_meta" | "add_feature" | "get_reference"), "{op} is not in the reference text");
    }
    for arm in arms.iter().filter(|a| !matches!(a.as_str(), "line" | "polyline" | "polygon" | "rect" | "rectangle" | "circle" | "arc" | "tangent_arc" | "spline" | "ngon" | "point" | "sphere" | "box" | "side" | "normal" | "connected")) {
        assert!(commands.contains(&arm.as_str()), "the dispatcher handles {arm}, which OPS does not list");
    }
}

#[test]
fn the_samples_run_headless() {
    let base = dir("samples");
    let sample = |name: &str| SAMPLES.iter().find(|s| s.0 == name).unwrap().1;
    // Variants: a parametric box, three widths, three STLs.
    let mut s = Session::default();
    run_cmd(&mut s, json!({"op": "set_parameter", "name": "width", "expr": "20 mm"}));
    run_cmd(&mut s, json!({"op": "primitive", "type": "box", "width": "$width", "depth": 10, "height": 5}));
    s.save(&base.join("box.ferr")).unwrap();
    let mut req = Request::new(sample("variants.rhai"));
    req.sandbox.allowed = vec![base.clone()];
    req.inputs = json!({"values": "20 mm, 30 mm, 40 mm"});
    let out = script::run(&mut s, &req).unwrap();
    assert_eq!(out.exports.len(), 3, "{:?}", out.log);
    assert!(base.join("box-width-30mm.stl").exists(), "{:?}", std::fs::read_dir(&base).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>());
    assert_eq!(s.doc.params[0].expr, "20 mm", "the parameter is put back");
    // Export a folder: two designs in, STL and STEP out.
    let mut other = Session::default();
    run_cmd(&mut other, json!({"op": "primitive", "type": "sphere", "diameter": 10}));
    other.save(&base.join("ball.ferr")).unwrap();
    let mut s = Session::default();
    let mut req = Request::new(sample("export-folder.rhai"));
    req.sandbox.allowed = vec![base.clone()];
    req.inputs = json!({"folder": base.display().to_string()});
    let out = script::run(&mut s, &req).unwrap();
    assert!(base.join("ball.stl").exists() && base.join("ball.step").exists() && base.join("box.stl").exists(), "{:?}", out.log);
    // CI check: passes on a good design, fails on one with an error, writes a report.
    let mut s = Session::open(&base.join("box.ferr")).unwrap();
    let mut req = Request::new(sample("ci-check.rhai"));
    req.sandbox.allowed = vec![base.clone()];
    req.inputs = json!({"report": base.join("report.csv").display().to_string()});
    let out = script::run(&mut s, &req).unwrap();
    assert_eq!(out.result["bodies"], 1);
    assert!(std::fs::read_to_string(base.join("report.csv")).unwrap().starts_with("id,name,triangles,volume,open_edges\n"));
    // Spur gear: one body, the right outer diameter.
    let mut s = Session::default();
    let mut req = Request::new(sample("spur-gear.rhai"));
    req.inputs = json!({"teeth": 24, "module_size": "2 mm", "thickness": "5 mm", "bore": "6 mm"});
    let out = script::run(&mut s, &req).unwrap();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies.len(), 1);
    let (lo, hi) = s.built.bodies[0].mesh.bbox().unwrap();
    assert!(((hi.x - lo.x) - 52.0).abs() < 0.1, "outer diameter module*(teeth+2) = 52: {}", hi.x - lo.x);
    assert!((hi.z - lo.z - 5.0).abs() < 1e-6);
    assert!(out.log[0].contains("24 teeth"));
    // Bosses at points: a plate, a sketch with three points, three bosses.
    let mut s = Session::default();
    run_cmd(&mut s, json!({"op": "primitive", "type": "box", "width": 60, "depth": 40, "height": 5}));
    let plate = s.doc.features.last().unwrap().id;
    run_cmd(&mut s, json!({"op": "create_sketch", "plane": "XY", "offset": 5}));
    let sketch = s.doc.features.last().unwrap().id;
    run_cmd(&mut s, json!({"op": "add_geometry", "items": [{"type": "point", "at": [10, 10]}, {"type": "point", "at": [30, 20]}, {"type": "point", "at": [50, 30]}]}));
    let before = s.built.bodies[0].mesh.volume();
    let mut req = Request::new(sample("boss-at-points.rhai"));
    req.inputs = json!({"sketch": sketch, "body": plate, "diameter": "6 mm", "height": "4 mm"});
    let out = script::run(&mut s, &req).unwrap();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(out.result, json!(3));
    let added = s.built.bodies[0].solids.iter().map(|l| l.volume()).sum::<f64>() - before;
    let one = std::f64::consts::PI * 9.0 * 4.0;
    assert!((added - 3.0 * one).abs() < 1e-3, "three bosses of {one}: {added}");
}

#[test]
fn the_face_relief_sample_makes_a_printable_solid() {
    let base = dir("relief");
    let (w, h) = (48u32, 32u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (dx, dy) = (x as f64 - 24.0, y as f64 - 16.0);
            let v = (255.0 * (1.0 - (dx * dx / 400.0 + dy * dy / 180.0)).clamp(0.0, 1.0)) as u8;
            rgba[i..i + 3].copy_from_slice(&[v, v, v]);
            rgba[i + 3] = 255;
        }
    }
    let image = base.join("dome.png");
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    }
    std::fs::write(&image, png).unwrap();
    let mut s = Session::default();
    let mut req = Request::new(SAMPLES.iter().find(|s| s.0 == "face-relief.rhai").unwrap().1);
    req.sandbox.allowed = vec![base.clone()];
    req.inputs = json!({"image": image.display().to_string(), "width": "60 mm", "resolution": 96, "triangles": 6000, "stl": base.join("dome.stl").display().to_string()});
    let out = script::run(&mut s, &req).unwrap_or_else(|e| panic!("{e}"));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(base.join("dome.stl").exists(), "{:?}", out.log);
    assert_eq!(s.built.bodies.len(), 1, "relief and plaque are one body");
    let body = s.built.bodies[0].id;
    let m = run_cmd(&mut s, json!({"op": "mesh_measure", "body": body}));
    assert!(m["open_edges"].as_u64().unwrap() < 60, "{m}");
    assert!(m["min"][2].as_f64().unwrap() < -4.9, "plaque under the relief: {}", m["min"]);
    assert_eq!(fr_core::io::design_version(&s.doc), 11);
}

#[test]
fn exporting_the_timeline_makes_a_script_that_rebuilds_it() {
    let mut s = Session::default();
    run_cmd(&mut s, json!({"op": "set_parameter", "name": "w", "expr": "30 mm"}));
    run_cmd(&mut s, json!({"op": "create_sketch", "plane": "XY"}));
    run_cmd(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [30, 10]}]}));
    run_cmd(&mut s, json!({"op": "extrude", "distance": "$w / 3", "operation": "new"}));
    let body = s.doc.features.last().unwrap().id;
    run_cmd(&mut s, json!({"op": "fillet_edges", "body": body, "edges": "all", "radius": 1}));
    let source = script::export_timeline(&s).unwrap();
    assert!(source.contains("add_feature"));
    let m = script::meta(&source).unwrap();
    assert_eq!(m.inputs.len(), 1);
    let mut again = Session::default();
    let mut req = Request::new(source);
    req.inputs = json!({"w": "60 mm"});
    script::run(&mut again, &req).unwrap();
    assert!(again.built.errors.is_empty(), "{:?}", again.built.errors);
    assert_eq!(again.doc.features.len(), s.doc.features.len());
    let (lo, hi) = again.built.bodies[0].mesh.bbox().unwrap();
    assert!((hi.z - lo.z - 20.0).abs() < 1e-6, "rebuilt with w = 60: height {}", hi.z - lo.z);
}
