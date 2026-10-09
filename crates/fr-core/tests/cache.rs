//! The geometry cache: a saved design opens from its cached bodies when they
//! match, falls back to a rebuild when anything disagrees, and a newer file
//! shows its cache read-only.

use std::path::PathBuf;
use std::time::Instant;

use fr_core::api::execute;
use fr_core::doc::CachePolicy;
use fr_core::{Session, io};
use serde_json::{Value as J, json};

fn run(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferrender-cache-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A block with a grid of holes and every edge filleted: the kind of design a rebuild spends time on.
fn heavy() -> Session {
    let mut s = Session::default();
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 60, "depth": 40, "height": 10}));
    let body = s.doc.features.last().unwrap().id;
    for x in [10, 30, 50] {
        for y in [10, 30] {
            run(&mut s, json!({"op": "hole", "body": body, "at": [x, y, 10], "thread": "M4", "through": true}));
        }
    }
    run(&mut s, json!({"op": "fillet_edges", "body": body, "edges": "all", "radius": 0.8}));
    run(&mut s, json!({"op": "create_plane", "kind": "offset", "base": "XY", "distance": 10}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    s
}

/// Body ids, exact volumes rounded past the BRep text's precision, and triangle counts.
fn volumes(s: &Session) -> Vec<(u32, f64, usize)> {
    s.built.bodies.iter().map(|b| (b.id, (b.solids.iter().map(|l| l.volume()).sum::<f64>() * 1e6).round() / 1e6, b.mesh.len())).collect()
}

fn entries(path: &PathBuf) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    names.sort();
    names
}

#[test]
fn a_cached_design_opens_without_a_rebuild_and_matches_it() {
    let mut s = heavy();
    let path = dir().join("heavy.ferr");
    s.cache_policy = CachePolicy::Always;
    let saved = s.save(&path).unwrap();
    assert!(saved.container && saved.cached_bodies == 1, "{saved:?}");
    let names = entries(&path);
    assert!(names.contains(&"cache/index.json".to_owned()) && names.iter().any(|n| n.ends_with(".brep")), "{names:?}");
    let manifest = io::manifest(&path).unwrap();
    assert_eq!(manifest["cached_bodies"], 1);

    let t = Instant::now();
    let cached = Session::open(&path).unwrap();
    let from_cache_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(cached.from_cache, "the matching cache is used");
    assert!(!cached.read_only);
    assert_eq!(cached.doc, s.doc);
    assert_eq!(volumes(&cached), volumes(&s), "cached bodies are the rebuilt bodies");
    assert_eq!(cached.built.planes.len(), 1, "planes come back too");
    assert_eq!(cached.built.bodies[0].tags, s.built.bodies[0].tags, "face tags survive the cache");
    assert_eq!(cached.built.resolutions, s.built.resolutions);
    let t = Instant::now();
    let rebuilt = Session::new(cached.doc.clone());
    let rebuild_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("open from cache {from_cache_ms:.0} ms, rebuild {rebuild_ms:.0} ms");
    assert!(from_cache_ms < 2000.0, "cache load must be quick: {from_cache_ms:.0} ms");
    assert_eq!(volumes(&rebuilt), volumes(&cached));

    // The first edit rebuilds and the cache flag drops.
    let mut cached = cached;
    run(&mut cached, json!({"op": "set_parameter", "name": "k", "expr": "1 mm"}));
    assert!(!cached.from_cache);
    assert_eq!(volumes(&cached), volumes(&s));
    // A fillet in the cached body still resolves by tag after the reload: its tags came back with it.
    let info = run(&mut cached, json!({"op": "get_scene_info"}));
    assert!(info["features"].as_array().unwrap().iter().any(|f| f["type"] == "fillet" && f["resolved"] == "tag"), "{info}");
}

#[test]
fn a_cache_that_disagrees_is_discarded_and_the_design_rebuilds() {
    let mut s = heavy();
    s.cache_policy = CachePolicy::Always;
    let good = dir().join("good.ferr");
    s.save(&good).unwrap();
    let bytes = std::fs::read(&good).unwrap();
    let rewrite = |edit: &dyn Fn(&str, Vec<u8>) -> Vec<u8>, name: &str| -> PathBuf {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut data).unwrap();
            let fname = f.name().to_owned();
            out.start_file(fname.clone(), zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut out, &edit(&fname, data)).unwrap();
        }
        let path = dir().join(name);
        std::fs::write(&path, out.finish().unwrap().into_inner()).unwrap();
        path
    };
    // A design edited by hand (a feature renamed) no longer matches the cache's CRC.
    let edited = rewrite(&|name, data| if name == "design.json" { let mut v: J = serde_json::from_slice(&data).unwrap(); v["features"][0]["name"] = json!("Renamed"); serde_json::to_vec_pretty(&v).unwrap() } else { data }, "edited.ferr");
    let opened = Session::open(&edited).unwrap();
    assert!(!opened.from_cache, "a changed design rebuilds");
    assert_eq!(opened.built.bodies.len(), 1);
    // A tampered shape fails the volume check.
    let tampered = rewrite(&|name, mut data| { if name.ends_with(".brep") { let k = data.len() / 2; data[k] ^= 0x55; } data }, "tampered.ferr");
    let opened = Session::open(&tampered).unwrap();
    assert!(!opened.from_cache || volumes(&opened) == volumes(&s), "a tampered cache is discarded or still agrees");
    // Another kernel's cache is not used.
    let other = rewrite(&|name, data| if name == "cache/index.json" { String::from_utf8(data).unwrap().replace("cadrum 0.8.20", "cadrum 9.9.9").into_bytes() } else { data }, "kernel.ferr");
    let opened = Session::open(&other).unwrap();
    assert!(!opened.from_cache);
    assert_eq!(volumes(&opened), volumes(&s));
}

#[test]
fn plain_designs_stay_plain_unless_slow_or_forced() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "primitive", "type": "box", "width": 10, "depth": 10, "height": 10}));
    let path = dir().join("quick.ferr");
    let saved = s.save(&path).unwrap();
    assert!(!saved.container && saved.cached_bodies == 0, "a quick design is still a plain JSON file");
    let forced = run(&mut s, json!({"op": "save", "path": dir().join("forced.ferr").display().to_string(), "cache": true}));
    assert_eq!(forced["container"], true);
    assert_eq!(forced["cached_bodies"], 1);
    let scene = run(&mut Session::open(&dir().join("forced.ferr")).unwrap(), json!({"op": "get_scene_info"}));
    assert_eq!(scene["from_cache"], true);
}

#[test]
fn a_newer_file_shows_its_cache_read_only() {
    let mut s = heavy();
    s.cache_policy = CachePolicy::Always;
    let path = dir().join("future.ferr");
    s.save(&path).unwrap();
    // Rewrite the design's version beyond this reader, leaving the cache as it is.
    let bytes = std::fs::read(&path).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).unwrap();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut data).unwrap();
        let name = f.name().to_owned();
        if name == "design.json" {
            let mut v: J = serde_json::from_slice(&data).unwrap();
            v["version"] = json!(io::FORMAT_VERSION + 1);
            data = serde_json::to_vec_pretty(&v).unwrap();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut out, &data).unwrap();
    }
    let future = dir().join("future-rewritten.ferr");
    std::fs::write(&future, out.finish().unwrap().into_inner()).unwrap();
    let mut view = Session::open(&future).unwrap();
    assert!(view.read_only && view.from_cache);
    assert_eq!(volumes(&view), volumes(&s), "the newer file's geometry is shown");
    let refused = execute(&mut view, &json!({"op": "primitive", "type": "box", "width": 1, "depth": 1, "height": 1}), None);
    assert!(refused.is_err_and(|e| e.contains("read-only")));
    assert!(view.save(&dir().join("nope.ferr")).is_err());
    let scene = run(&mut view, json!({"op": "get_scene_info"}));
    assert_eq!(scene["read_only"], true);
    // Export still works from the cached bodies.
    let stl = dir().join("future.stl");
    run(&mut view, json!({"op": "export_stl", "path": stl.display().to_string()}));
    assert!(std::fs::metadata(&stl).unwrap().len() > 84);
}
