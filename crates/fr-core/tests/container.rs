//! The 0.4 file container: plain JSON stays plain, designs with payloads become
//! ZIP containers, legacy files are backed up when converted, and hostile
//! archives are refused before anything is decompressed.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use fr_core::api::execute;
use fr_core::reference::ReferenceImage;
use fr_core::{Session, io};
use serde_json::{Value as J, json};

fn run(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferrender-container-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&vec![shade; (w * h * 4) as usize]).unwrap();
    data
}

/// A box, plus a sketch with a reference image when `image` is set.
fn design(image: bool) -> Session {
    let mut s = Session::default();
    run(&mut s, json!({"op": "create_sketch", "plane": "XY"}));
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [20, 10]}]}));
    run(&mut s, json!({"op": "extrude", "distance": 5}));
    if image {
        let sketch = s.doc.sketches().next().unwrap().0.id;
        s.edit(|d| {
            d.sketch_mut(sketch).unwrap().reference = Some(ReferenceImage::from_bytes("photo.png", &png(8, 4, 90), 40.0)?);
            Ok(())
        }).unwrap();
    }
    s
}

fn entries(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    let archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    names.sort();
    names
}

fn entry(path: &Path, name: &str) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = Vec::new();
    archive.by_name(name).unwrap().read_to_end(&mut out).unwrap();
    out
}

#[test]
fn plain_designs_stay_plain_json_and_reopen() {
    let d = dir("plain");
    let path = d.join("box.ferr");
    let mut s = design(false);
    let saved = s.save(&path).unwrap();
    assert!(!saved.container && saved.backup.is_none() && !s.container);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"{"));
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("\"version\": 1"), "a box is still a version-1 design");
    let again = Session::open(&path).unwrap();
    assert_eq!(again.doc, s.doc);
    assert!(!again.container);
}

#[test]
fn a_reference_image_makes_a_container_that_round_trips() {
    let d = dir("image");
    let path = d.join("traced.ferr");
    let mut s = design(true);
    let saved = s.save(&path).unwrap();
    assert!(saved.container && saved.backup.is_none() && s.container);
    assert!(std::fs::read(&path).unwrap().starts_with(b"PK"));
    // Containers also carry the geometry cache of their bodies.
    assert_eq!(entries(&path), ["cache/2.brep", "cache/index.json", "design.json", "images/1.png", "manifest.json", "thumbnail.png"]);

    let design = String::from_utf8(entry(&path, "design.json")).unwrap();
    assert!(design.contains("\"blob\": \"images/1.png\""));
    assert!(!design.contains("iVBOR"), "no base64 PNG stays inside the JSON");
    assert!(entry(&path, "images/1.png").starts_with(b"\x89PNG"));
    assert!(entry(&path, "thumbnail.png").starts_with(b"\x89PNG"));
    let manifest: J = serde_json::from_slice(&entry(&path, "manifest.json")).unwrap();
    assert_eq!(manifest["format"], "ferrender-container");
    assert_eq!(manifest["min_reader"], io::CONTAINER_VERSION);
    assert_eq!(manifest["design_version"], 3, "the JSON inside keeps the version a plain file would have");
    assert!(manifest["saved"].as_str().unwrap().ends_with('Z'));

    let again = Session::open(&path).unwrap();
    assert!(again.container);
    assert_eq!(again.doc, s.doc);
    let image = again.doc.sketches().next().unwrap().1.reference.as_ref().unwrap();
    assert_eq!((image.pixel_width, image.pixel_height), (8, 4));
    assert_eq!(image.pixels().unwrap().rgba[0], 90);
    assert!(io::thumbnail(&path).is_some_and(|t| t.starts_with(b"\x89PNG")));
    assert!(io::manifest(&path).is_some());
    assert!(io::thumbnail(&d.join("missing.ferr")).is_none());
}

#[test]
fn an_imported_mesh_is_stored_as_a_binary_blob() {
    let d = dir("mesh");
    let stl = d.join("tri.stl");
    let mut bytes = vec![b' '; 80];
    bytes.extend(4u32.to_le_bytes());
    // A tetrahedron: four triangles sharing four corners, closed and outward.
    for t in [[[0.0f32, 0.0, 0.0], [0.0, 10.0, 0.0], [10.0, 0.0, 0.0]], [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, 10.0]], [[0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [0.0, 10.0, 0.0]], [[10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]]] {
        bytes.extend([0u8; 12]);
        for v in t { for c in v { bytes.extend(c.to_le_bytes()); } }
        bytes.extend([0u8; 2]);
    }
    std::fs::write(&stl, bytes).unwrap();
    let mut s = Session::default();
    run(&mut s, json!({"op": "import_stl", "path": stl.display().to_string(), "units": "mm"}));
    let path = d.join("scan.ferr");
    assert!(s.save(&path).unwrap().container);
    assert_eq!(entries(&path), ["cache/1.mesh", "cache/index.json", "design.json", "manifest.json", "meshes/1.mesh", "thumbnail.png"]);
    // Four triangles sharing four corners: a 32-byte header, four f32 positions, four u32 index triples.
    assert_eq!(entry(&path, "meshes/1.mesh").len(), 32 + 4 * 12 + 4 * 12);
    let again = Session::open(&path).unwrap();
    assert_eq!(again.doc, s.doc);
    assert_eq!(again.built.bodies.len(), 1);
    assert_eq!(again.built.bodies[0].mesh.len(), 4);
    assert!(again.built.bodies[0].mesh.is_welded() && again.built.bodies[0].mesh.open_edges() == 0);
}

#[test]
fn a_legacy_json_file_opens_and_is_backed_up_once_when_it_becomes_a_container() {
    let d = dir("legacy");
    let path = d.join("old.ferr");
    let s = design(true);
    // What 0.3 wrote: everything inline, including the image.
    std::fs::write(&path, io::to_json(&s.doc)).unwrap();
    assert!(!io::is_container(&path));
    let mut opened = Session::open(&path).unwrap();
    assert!(!opened.container);
    assert_eq!(opened.doc, s.doc);

    let saved = opened.save(&path).unwrap();
    let backup = io::backup_path(&path);
    assert_eq!(saved.backup.as_deref(), Some(backup.as_path()));
    assert_eq!(backup.file_name().unwrap(), "old (0.3 backup).ferr");
    assert!(io::is_container(&path) && !io::is_container(&backup));
    assert_eq!(Session::open(&backup).unwrap().doc, s.doc, "the backup is the untouched original");
    let before = std::fs::read(&backup).unwrap();

    // A second save neither replaces the backup nor makes another.
    let saved = opened.save(&path).unwrap();
    assert!(saved.backup.is_none());
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 2);

    // A plain design saved over a legacy plain file needs no backup.
    let plain = d.join("plain.ferr");
    std::fs::write(&plain, io::to_json(&design(false).doc)).unwrap();
    assert!(design(false).save(&plain).unwrap().backup.is_none());
}

fn container_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn hostile_containers_are_refused_before_decompression() {
    let d = dir("hostile");
    let path = d.join("good.ferr");
    design(true).save(&path).unwrap();
    let manifest = entry(&path, "manifest.json");
    let json = entry(&path, "design.json");
    let image = entry(&path, "images/1.png");

    let refuse = |entries: &[(&str, &[u8])], needle: &str| {
        let err = io::decode(&container_with(entries)).unwrap_err();
        assert!(err.contains(needle), "{err:?} should mention {needle:?}");
    };
    refuse(&[("manifest.json", &manifest), ("design.json", &json), ("images/1.png", &image), ("../escape.png", b"x")], "unexpected entry");
    refuse(&[("manifest.json", &manifest), ("design.json", &json), ("images/1.png", &image), ("notes.txt", b"x")], "unexpected entry");
    refuse(&[("manifest.json", &manifest), ("design.json", &json), ("images/1.png", &image), ("images/one.png", b"x")], "unexpected entry");
    refuse(&[("design.json", &json), ("images/1.png", &image)], "no manifest.json");
    refuse(&[("manifest.json", &manifest), ("images/1.png", &image)], "no design.json");
    refuse(&[("manifest.json", &manifest), ("design.json", &json)], "no images/1.png");
    let newer = serde_json::to_vec(&json!({"format": "ferrender-container", "min_reader": io::FORMAT_VERSION + 1})).unwrap();
    refuse(&[("manifest.json", &newer), ("design.json", &json), ("images/1.png", &image)], "newer version");
    let foreign = serde_json::to_vec(&json!({"format": "something-else", "min_reader": 1})).unwrap();
    refuse(&[("manifest.json", &foreign), ("design.json", &json), ("images/1.png", &image)], "not a Ferrender container");
    refuse(&[("manifest.json", &manifest), ("design.json", &json), ("images/1.png", b"not a png")], "invalid Ferrender file");

    // An entry that declares more than its limit is refused without being read.
    let big = vec![0u8; 21 * 1024 * 1024];
    refuse(&[("manifest.json", &manifest), ("design.json", &json), ("images/1.png", &big)], "more than the 20 MiB limit");

    // Truncated and non-ZIP bytes starting with PK.
    let bytes = std::fs::read(&path).unwrap();
    assert!(io::decode(&bytes[..bytes.len() / 2]).is_err());
    assert!(io::decode(b"PK\x03\x04 nonsense").is_err());

    // A plain file that merely claims a future version is still refused.
    let mut v: J = serde_json::from_str(&io::to_json(&design(false).doc)).unwrap();
    v["version"] = json!(io::FORMAT_VERSION + 1);
    assert!(io::from_json(&v.to_string()).unwrap_err().contains("newer version"));
}

#[test]
fn save_reports_the_container_over_the_api() {
    let d = dir("api");
    let path = d.join("api.ferr");
    let mut s = design(true);
    let out = run(&mut s, json!({"op": "save", "path": path.display().to_string()}));
    assert_eq!(out["container"], true);
    assert!(out.get("backup").is_none());
    let info = run(&mut s, json!({"op": "get_scene_info"}));
    assert_eq!(info["container"], true);
    let mut fresh = Session::default();
    let info = run(&mut fresh, json!({"op": "open", "path": path.display().to_string()}));
    assert_eq!(info["container"], true);
    assert_eq!(fresh.doc, s.doc);
}
