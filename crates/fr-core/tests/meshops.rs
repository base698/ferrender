//! Mesh operations as timeline features, driven through the API.

use fr_core::api::execute;
use fr_core::{FeatureKind, Session};
use glam::DVec3;
use serde_json::{Value as J, json};

fn run(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

fn last(s: &Session) -> u32 {
    s.doc.features.last().unwrap().id
}

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ferrender-meshops-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A sphere STL of about `n` triangles, radius 20, centred at the origin.
fn sphere_stl(n: usize) -> std::path::PathBuf {
    let rows = ((n / 2) as f64).sqrt() as usize;
    let at = |i: usize, j: usize| {
        if i == 0 { return DVec3::new(0.0, 0.0, 20.0); }
        if i == rows { return DVec3::new(0.0, 0.0, -20.0); }
        let (u, v) = (i as f64 / rows as f64 * std::f64::consts::PI, j as f64 / rows as f64 * std::f64::consts::TAU);
        DVec3::new(20.0 * u.sin() * v.cos(), 20.0 * u.sin() * v.sin(), 20.0 * u.cos())
    };
    let mut bytes = vec![b' '; 80];
    bytes.extend([0u8; 4]);
    let mut count = 0u32;
    for i in 0..rows {
        for j in 0..rows {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, (j + 1) % rows), at(i, (j + 1) % rows));
            for t in [[a, b, c], [a, c, d]] {
                bytes.extend([0u8; 12]);
                for v in t { for x in v.to_array() { bytes.extend((x as f32).to_le_bytes()); } }
                bytes.extend([0u8; 2]);
                count += 1;
            }
        }
    }
    bytes[80..84].copy_from_slice(&count.to_le_bytes());
    // Tests run in parallel: each gets its own file, written whole before it is named.
    static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let k = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir().join(format!("sphere-{n}-{k}.stl"));
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    path
}

fn sphere_session(n: usize) -> (Session, u32) {
    let mut s = Session::default();
    run(&mut s, json!({"op": "import_mesh", "path": sphere_stl(n).display().to_string()}));
    let body = last(&s);
    (s, body)
}

fn measure(s: &mut Session, body: u32) -> J {
    run(s, json!({"op": "mesh_measure", "body": body}))
}

const SPHERE_VOLUME: f64 = 4.0 / 3.0 * std::f64::consts::PI * 20.0 * 20.0 * 20.0;

#[test]
fn decimate_keeps_the_shape_and_stays_watertight() {
    let (mut s, body) = sphere_session(20_000);
    let before = measure(&mut s, body);
    assert_eq!(before["watertight"], true);
    run(&mut s, json!({"op": "mesh_decimate", "body": body, "target": 2000}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let after = measure(&mut s, body);
    let n = after["triangles"].as_u64().unwrap();
    assert!((1800..=2400).contains(&n), "about 2000 triangles, got {n}");
    assert_eq!(after["watertight"], true, "{after}");
    let v = after["volume"].as_f64().unwrap();
    assert!((v - SPHERE_VOLUME).abs() / SPHERE_VOLUME < 0.03, "volume within 3 %: {v} vs {SPHERE_VOLUME}");
    // The fast method too.
    run(&mut s, json!({"op": "mesh_decimate", "body": body, "target": 500, "method": "cluster"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(measure(&mut s, body)["triangles"].as_u64().unwrap() < 1200);
    assert_eq!(s.doc.features.last().unwrap().type_name(), "mesh_decimate");
}

#[test]
fn smooth_and_subdivide_keep_a_sphere_a_sphere() {
    let (mut s, body) = sphere_session(5_000);
    run(&mut s, json!({"op": "mesh_subdivide", "body": body, "levels": 1, "scheme": "loop"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true);
    assert!(m["triangles"].as_u64().unwrap() >= 4 * 4_000);
    let v = m["volume"].as_f64().unwrap();
    assert!((v - SPHERE_VOLUME).abs() / SPHERE_VOLUME < 0.03, "{v}");
    run(&mut s, json!({"op": "mesh_smooth", "body": body, "iterations": 20, "strength": 0.5}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    let v = m["volume"].as_f64().unwrap();
    assert!((v - SPHERE_VOLUME).abs() / SPHERE_VOLUME < 0.05, "Taubin smoothing does not shrink: {v}");
    // A region keeps the rest still: smooth only the top cap.
    let before: Vec<DVec3> = s.built.body(body).unwrap().mesh.positions().to_vec();
    run(&mut s, json!({"op": "mesh_smooth", "body": body, "iterations": 5, "strength": 0.5, "region": {"sphere": {"centre": [0, 0, 20], "radius": 8}}}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let after = s.built.body(body).unwrap().mesh.positions().to_vec();
    assert_eq!(before.len(), after.len());
    let moved_low = before.iter().zip(&after).filter(|(a, b)| a.z < 0.0 && a.distance(**b) > 1e-9).count();
    assert_eq!(moved_low, 0, "vertices outside the region do not move");
}

#[test]
fn cut_keeps_a_side_caps_it_and_can_make_two_bodies() {
    let (mut s, body) = sphere_session(5_000);
    run(&mut s, json!({"op": "mesh_cut", "body": body, "plane": "XY", "keep": "negative", "cap": true}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true, "{m}");
    let v = m["volume"].as_f64().unwrap();
    assert!((v - SPHERE_VOLUME / 2.0).abs() / SPHERE_VOLUME < 0.02, "half a sphere: {v}");
    assert!(m["max"][2].as_f64().unwrap() < 1e-6, "nothing above the plane: {}", m["max"]);
    assert_eq!(s.built.bodies.len(), 1);
    // Both sides as two bodies.
    let (mut s, body) = sphere_session(5_000);
    run(&mut s, json!({"op": "mesh_cut", "body": body, "plane": "XZ", "keep": "both", "cap": true}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies.len(), 2, "both halves are bodies");
    let total: f64 = s.built.bodies.iter().map(|b| b.mesh.volume()).sum();
    assert!((total - SPHERE_VOLUME).abs() / SPHERE_VOLUME < 0.02, "{total}");
    assert!(s.built.bodies.iter().all(|b| b.mesh.open_edges() == 0));
}

#[test]
fn mirror_welds_the_halves() {
    let (mut s, body) = sphere_session(5_000);
    run(&mut s, json!({"op": "mesh_cut", "body": body, "plane": "XY", "keep": "negative", "cap": false}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(measure(&mut s, body)["open_edges"].as_u64().unwrap() > 0, "an uncapped half is open");
    run(&mut s, json!({"op": "mesh_mirror", "body": body, "plane": "XY", "weld": true}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true, "{m}");
    let v = m["volume"].as_f64().unwrap();
    assert!((v - SPHERE_VOLUME).abs() / SPHERE_VOLUME < 0.02, "{v}");
}

#[test]
fn offset_thickens_an_open_surface_and_hollows_a_closed_one() {
    let (mut s, body) = sphere_session(5_000);
    run(&mut s, json!({"op": "mesh_cut", "body": body, "plane": "XY", "keep": "positive", "cap": false}));
    // A dome, open at the bottom: thicken it 2 mm downward into a closed shell with walls.
    run(&mut s, json!({"op": "mesh_offset", "body": body, "distance": 2, "direction": [0, 0, -1]}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true, "{m}");
    let v = m["volume"].as_f64().unwrap();
    // Thickened straight down, the volume is the dome's footprint times the thickness.
    let footprint = std::f64::consts::PI * 20.0 * 20.0;
    assert!((v - footprint * 2.0).abs() / (footprint * 2.0) < 0.05, "shell volume about footprint x thickness: {v} vs {}", footprint * 2.0);
    assert!(m["min"][2].as_f64().unwrap() < -1.9 && m["min"][2].as_f64().unwrap() > -2.1, "{}", m["min"]);
    // Hollowing a closed sphere to a 2 mm wall.
    let (mut s, body) = sphere_session(5_000);
    run(&mut s, json!({"op": "mesh_offset", "body": body, "distance": 2}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true, "{m}");
    let v = m["volume"].as_f64().unwrap();
    let expected = SPHERE_VOLUME - 4.0 / 3.0 * std::f64::consts::PI * 18.0f64.powi(3);
    assert!((v - expected).abs() / expected < 0.05, "a 2 mm shell: {v} vs {expected}");
    assert_eq!(m["components"], 2);
    let thick = run(&mut s, json!({"op": "mesh_measure", "body": body, "at": [0, 0, 20]}));
    let t = thick["thickness_at"].as_f64().unwrap();
    assert!((t - 2.0).abs() < 0.2, "wall thickness under the top: {t}");
}

#[test]
fn a_region_can_be_extruded() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 40, "depth": 40, "height": 10}));
    let body = last(&s);
    // Remesh the top so a region has something to grab, then pull up a 10 mm square in the middle.
    run(&mut s, json!({"op": "mesh_subdivide", "body": body, "levels": 3, "scheme": "midpoint"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(!s.built.body(body).unwrap().is_exact(), "a mesh operation turns the body into a mesh");
    let before = measure(&mut s, body)["volume"].as_f64().unwrap();
    run(&mut s, json!({"op": "mesh_extrude_region", "body": body, "region": {"box": {"lo": [15, 15, 9.9], "hi": [25, 25, 10.1]}}, "distance": 5}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true, "{m}");
    let v = m["volume"].as_f64().unwrap();
    assert!((v - before - 10.0 * 10.0 * 5.0).abs() < 1.0, "a 10 x 10 x 5 boss was added: {v} - {before}");
    assert!((m["max"][2].as_f64().unwrap() - 15.0).abs() < 1e-6);
}

#[test]
fn a_relief_from_an_image_is_a_printable_slab() {
    // A gradient: dark on the left, bright on the right, with a bright square in the middle.
    let (w, h) = (64u32, 32u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let mut v = (x * 255 / (w - 1)) as u8;
            if (24..40).contains(&x) && (8..24).contains(&y) { v = 255; }
            rgba[i..i + 3].copy_from_slice(&[v, v, v]);
            rgba[i + 3] = 255;
        }
    }
    let path = dir().join("gradient.png");
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    }
    std::fs::write(&path, png).unwrap();
    let mut s = Session::default();
    run(&mut s, json!({"op": "mesh_from_image", "path": path.display().to_string(), "width": 64, "depth": 4, "base": 2, "resolution": 64, "blur": 0}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let body = last(&s);
    assert_eq!(s.doc.features.last().unwrap().type_name(), "relief");
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true, "{m}");
    assert_eq!(m["min"][2].as_f64().unwrap(), -2.0, "the slab is 2 mm");
    assert!((m["max"][2].as_f64().unwrap() - 4.0).abs() < 1e-6, "the brightest pixel is 4 mm high");
    assert!((m["max"][0].as_f64().unwrap() - 64.0).abs() < 1e-6 && (m["max"][1].as_f64().unwrap() - 32.0).abs() < 1e-6, "{}", m["max"]);
    // The surface rises from left to right.
    let b = s.built.body(body).unwrap();
    let z_at = |x: f64| b.mesh.ray(DVec3::new(x, 4.0, 50.0), -DVec3::Z).map(|(d, _)| 50.0 - d).unwrap();
    assert!(z_at(2.0) < 1.0 && z_at(62.0) > 3.5, "left {} right {}", z_at(2.0), z_at(62.0));
    // It saves as a container with the image as a blob, and reopens identically.
    let file = dir().join("relief.ferr");
    assert!(s.save(&file).unwrap().container);
    let again = Session::open(&file).unwrap();
    assert_eq!(again.doc, s.doc);
    assert!(matches!(again.doc.features.last().unwrap().kind, FeatureKind::Relief(_)));
    assert_eq!(fr_core::io::design_version(&s.doc), 11);
    // The inverted, blurred, thinner variant is a lithophane.
    run(&mut s, json!({"op": "mesh_from_image", "path": path.display().to_string(), "width": 64, "depth": 2.5, "base": 0.6, "resolution": 128, "invert": true, "blur": 1, "origin": [0, 40, 0]}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies.len(), 2);
}

#[test]
fn repair_fills_small_holes() {
    let (mut s, body) = sphere_session(3_000);
    // Knock a hole in it: cut away the top cap without capping.
    run(&mut s, json!({"op": "mesh_cut", "body": body, "plane": {"origin": [0, 0, 19], "normal": [0, 0, 1]}, "keep": "negative", "cap": false}));
    let open = measure(&mut s, body)["open_edges"].as_u64().unwrap();
    assert!(open > 0 && open < 200, "a small opening: {open}");
    run(&mut s, json!({"op": "mesh_repair", "body": body, "fill_holes": 400}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true, "{m}");
}

#[test]
fn booleans_between_meshes_cull_the_far_triangles() {
    use fr_core::csg::Bool;
    use fr_core::meshops::boolean;
    let (s, body) = sphere_session(20_000);
    let big = s.built.body(body).unwrap().mesh.clone();
    // A small sphere (radius 6) sitting at the big one's top pole.
    let (t, tool_id) = sphere_session(2_000);
    let mut tool = t.built.body(tool_id).unwrap().mesh.clone();
    tool.map(|p| p * 0.3 + DVec3::new(0.0, 0.0, 20.0));
    let small = 4.0 / 3.0 * std::f64::consts::PI * 6.0f64.powi(3);
    let t0 = std::time::Instant::now();
    let cut = boolean(&big, &tool, Bool::Subtract).unwrap();
    let took = t0.elapsed().as_secs_f64();
    assert_eq!(cut.open_edges(), 0, "the cut is watertight");
    let v = cut.volume();
    assert!(v < SPHERE_VOLUME - small * 0.35 && v > SPHERE_VOLUME - small * 0.65, "about half the small sphere is removed: {v} of {SPHERE_VOLUME}");
    let joined = boolean(&big, &tool, Bool::Union).unwrap();
    assert_eq!(joined.open_edges(), 0);
    assert!(joined.volume() > SPHERE_VOLUME + small * 0.35 && joined.volume() < SPHERE_VOLUME + small * 0.65, "{}", joined.volume());
    let common = boolean(&big, &tool, Bool::Intersect).unwrap();
    assert_eq!(common.open_edges(), 0);
    assert!(common.volume() > small * 0.35 && common.volume() < small * 0.65, "{}", common.volume());
    // Disjoint meshes: a union is both, a cut is the first, an intersection is nothing.
    let mut far = tool.clone();
    far.map(|p| p + DVec3::new(100.0, 0.0, 0.0));
    let both = boolean(&big, &far, Bool::Union).unwrap();
    assert!((both.volume() - SPHERE_VOLUME - small).abs() / SPHERE_VOLUME < 0.02);
    assert!((boolean(&big, &far, Bool::Subtract).unwrap().volume() - SPHERE_VOLUME).abs() / SPHERE_VOLUME < 0.01);
    assert!(boolean(&big, &far, Bool::Intersect).unwrap().is_empty());
    println!("20 k-triangle sphere cut by a 2 k tool: {took:.3} s");
    // And through the document: combine as before.
    let mut s = s;
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 10, "depth": 10, "height": 10, "position": [15, 0, 0]}));
    let box_id = last(&s);
    run(&mut s, json!({"op": "combine", "target": body, "tools": [box_id], "operation": "cut"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert_eq!(m["watertight"], true, "{m}");
    assert!(m["volume"].as_f64().unwrap() < SPHERE_VOLUME - 100.0);
}

#[test]
fn sculpt_strokes_are_features() {
    let (mut s, body) = sphere_session(5_000);
    run(&mut s, json!({"op": "mesh_sculpt", "body": body, "brush": "pull", "at": [0, 0, 20], "radius": 8, "strength": 3}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let m = measure(&mut s, body);
    assert!((m["max"][2].as_f64().unwrap() - 23.0).abs() < 0.2, "the pole is pulled up 3 mm: {}", m["max"]);
    assert_eq!(m["watertight"], true);
    // Flattening around (20,0,0) pulls the nearby surface onto the tangent plane x = 20.
    let spread = |s: &Session| { let b = s.built.body(body).unwrap(); let near: Vec<f64> = b.mesh.positions().iter().filter(|p| p.distance(DVec3::new(20.0, 0.0, 0.0)) < 4.0).map(|p| (p.x - 20.0).abs()).collect(); near.iter().sum::<f64>() / near.len() as f64 };
    let before = spread(&s);
    run(&mut s, json!({"op": "mesh_sculpt", "body": body, "brush": "flatten", "at": [20, 0, 0], "radius": 6, "strength": 1}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(spread(&s) < before * 0.7, "the side is flatter: {} -> {}", before, spread(&s));
    run(&mut s, json!({"op": "mesh_sculpt", "body": body, "brush": "smooth", "at": [0, 20, 0], "radius": 6, "strength": 0.5}));
    run(&mut s, json!({"op": "mesh_sculpt", "body": body, "brush": "inflate", "at": [0, 0, -20], "radius": 6, "strength": 1}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let kinds: Vec<&str> = s.doc.features.iter().map(|f| f.type_name()).collect();
    assert_eq!(kinds, ["import", "mesh_sculpt", "mesh_sculpt", "mesh_sculpt", "mesh_sculpt"]);
    let m = measure(&mut s, body);
    assert!(m["min"][2].as_f64().unwrap() < -20.5, "inflating the bottom pushes it out: {}", m["min"]);
    let bad = execute(&mut s, &json!({"op": "mesh_sculpt", "body": body, "brush": "pull", "at": [0, 0, 20], "radius": 0.001, "strength": 1}), None);
    assert!(bad.is_err_and(|e| e.contains("no vertex lies within the brush")), "a brush smaller than the mesh's triangles is refused");
}
