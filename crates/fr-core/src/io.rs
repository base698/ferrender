//! Files: the native `.ferr` document and STL meshes.
//!
//! A design is JSON. When it carries binary payloads (reference images,
//! imported meshes) it is saved as a ZIP container instead: `design.json`
//! with the payloads replaced by `{"blob": "images/3.png"}` markers, one entry
//! per payload in its native encoding, a `manifest.json` and a `thumbnail.png`.
//! Readers tell the two apart by the first bytes (`{` or `PK`). Plain designs
//! stay plain JSON so they diff, round-trip over MCP and open in older apps.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::doc::{Body, Document, FeatureKind};
use crate::units::Unit;

/// Written into every native file so other tools can recognise it.
pub const FORMAT: &str = "ferrender";
/// The newest version this build reads. 9 is the ZIP container; the JSON inside
/// a container keeps its own, lower version, computed as for a plain file.
pub const FORMAT_VERSION: u32 = 12;
pub const CONTAINER_VERSION: u32 = 9;
pub const CONTAINER_FORMAT: &str = "ferrender-container";

const MAX_NATIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CONTAINER_BYTES: usize = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_IMAGE_BLOB: usize = 20 * 1024 * 1024;
/// A mesh blob at the triangle limit, every vertex unshared (see `meshfile`).
const MAX_MESH_BLOB: usize = crate::meshfile::MAX_BLOB_BYTES;
const MAX_THUMBNAIL_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
/// Thumbnails are rendered in software; a scene this large is skipped rather than slow down every save.
pub const THUMBNAIL_MAX_TRIANGLES: usize = 2_000_000;
pub const THUMBNAIL_SIZE: usize = 256;
/// Mesh blobs above this are deflated at the fastest level: a 5 million triangle save must not wait on the compressor.
const FAST_DEFLATE_ABOVE: usize = 8 * 1024 * 1024;

pub use crate::meshfile::{parse_stl, read_mesh_file as import_mesh, read_stl};

/// What a save adds beyond the document.
#[derive(Default, Clone)]
pub struct Extras {
    /// A rendered preview, kept only when the file is a container.
    pub thumbnail_png: Option<Vec<u8>>,
    /// The writing application, for the manifest; `None` names fr-core.
    pub app: Option<String>,
    /// The built geometry, which makes the file a container (see `cache`).
    pub cache: Option<crate::cache::Cache>,
}

/// What opening a file yields: the design when this build can read it, the
/// geometry cache when the file has one, and whether the design is from a
/// newer Ferrender (in which case only the cache can be shown).
pub struct Opened {
    pub doc: Option<Document>,
    pub cache: Option<crate::cache::Cache>,
    pub container: bool,
    /// The design's version is beyond this reader; `doc` is `None`.
    pub newer: bool,
    /// What the manifest says wrote the file.
    pub app: Option<String>,
}

const MAX_CACHE_INDEX_BYTES: usize = 8 * 1024 * 1024;
const MAX_CACHE_BLOB: usize = crate::cache::MAX_CACHE_BYTES;

/// What a save did.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub path: PathBuf,
    /// The file was written as a ZIP container rather than plain JSON.
    pub container: bool,
    /// How many bodies the geometry cache holds (0 when none was written).
    pub cached_bodies: usize,
    /// Where the previous plain-JSON file was copied when this save converted it to a container.
    pub backup: Option<PathBuf>,
}

/// The lowest format version that can read this design, which is what a plain file is stamped with.
pub fn design_version(doc: &Document) -> u32 {
    if doc.features.iter().any(|f| matches!(&f.kind, FeatureKind::ScriptRun(_)) || f.made_by.is_some()) { 12 }
    else if doc.features.iter().any(|f| matches!(&f.kind, FeatureKind::MeshOp(_) | FeatureKind::Relief(_))) { 11 }
    else if doc.features.iter().any(|f| has_tags(&f.kind)) { 10 }
    else if doc.features.iter().any(|f| matches!(&f.kind, FeatureKind::Remove(_) | FeatureKind::Split(_))) { 8 }
    else if doc.features.iter().any(|f| matches!(&f.kind, FeatureKind::Primitive(_))) { 7 }
    else if doc.features.iter().any(|f| matches!(&f.kind, FeatureKind::Pattern(crate::doc::Pattern { kind: crate::doc::PatternKind::Linear { second: Some(_), .. }, .. }))) { 6 }
    else if doc.active_component != 0 || doc.features.iter().any(|f| f.owner != 0 || matches!(&f.kind, FeatureKind::Plane(_) | FeatureKind::Component(_)) || matches!(&f.kind, FeatureKind::Sketch(s) if s.on.is_some())) { 5 }
    else if doc.features.iter().any(|f| match &f.kind {
        FeatureKind::Sketch(s) => s.constraints.values().any(|c| c.kind == crate::sketch::CKind::Angle && c.refs.len() == 1),
        _ => false,
    }) { 4 }
    else if doc.features.iter().any(|f| match &f.kind {
        FeatureKind::Sketch(s) => s.reference.is_some() || !s.arc_guides.is_empty()
            || s.entities.values().any(|e| matches!(e.geom, crate::sketch::Geom::Spline { .. }))
            || s.constraints.values().any(|c| matches!(c.kind, crate::sketch::CKind::PositionX | crate::sketch::CKind::PositionY)),
        _ => false,
    }) { 3 }
    else if doc.features.iter().any(|f| matches!(f.kind, FeatureKind::Text(_))) { 2 }
    else { 1 }
}

/// Whether a feature stores face or edge tags (format 10).
fn has_tags(kind: &FeatureKind) -> bool {
    use crate::planes::{PlaneKind, PlaneRef};
    let tagged_ref = |r: &PlaneRef| matches!(r, PlaneRef::Face { tag: Some(_), .. });
    match kind {
        FeatureKind::Blend(b) => !b.tags.is_empty(),
        FeatureKind::Shell(s) => !s.tags.is_empty(),
        FeatureKind::Thread(t) => t.tag.is_some(),
        FeatureKind::Text(t) => t.tag.is_some(),
        FeatureKind::Split(s) => tagged_ref(&s.plane),
        FeatureKind::Plane(p) => match &p.kind {
            PlaneKind::Offset { base, .. } => tagged_ref(base),
            PlaneKind::Midplane { a, b, .. } => tagged_ref(a) || tagged_ref(b),
            PlaneKind::ThreePoint { .. } => false,
        },
        _ => false,
    }
}

/// The document as the JSON object a plain file holds, payloads inline.
fn json_value(doc: &Document) -> Value {
    let mut v = serde_json::to_value(doc).expect("documents always serialize");
    let o = v.as_object_mut().unwrap();
    o.insert("format".into(), FORMAT.into());
    o.insert("version".into(), design_version(doc).into());
    v
}

/// The document's JSON with every mesh taken out: markers `{"blob": "#k"}` stand in, and the
/// k-th encoded mesh is in the returned list. Meshes never pass through base64 this way.
fn json_value_detached(doc: &Document) -> (Value, Vec<Vec<u8>>) {
    crate::mesh::BLOBS.with(|b| { let mut b = b.borrow_mut(); b.detaching = true; b.out.clear(); });
    let v = json_value(doc);
    let out = crate::mesh::BLOBS.with(|b| { let mut b = b.borrow_mut(); b.detaching = false; std::mem::take(&mut b.out) });
    (v, out)
}

/// The plain-JSON form, payloads inline, as every version before 9 wrote it.
pub fn to_json(doc: &Document) -> String {
    serde_json::to_string_pretty(&json_value(doc)).unwrap()
}

/// Whether a save of this document produces a container.
pub fn needs_container(doc: &Document) -> bool {
    doc.features.iter().any(|f| match &f.kind {
        FeatureKind::Sketch(s) => s.reference.is_some(),
        FeatureKind::Import(_) | FeatureKind::Relief(_) => true,
        _ => false,
    })
}

struct Blob {
    name: String,
    bytes: Vec<u8>,
    /// Already compressed (PNG), so the ZIP stores it as is.
    stored: bool,
}

/// Moves the binary payloads out of the JSON into blobs, leaving markers behind.
/// `meshes` are the encoded meshes `json_value_detached` took out, in marker order.
fn detach_blobs(v: &mut Value, mut meshes: Vec<Vec<u8>>) -> Result<Vec<Blob>, String> {
    let mut blobs = Vec::new();
    let Some(features) = v["features"].as_array_mut() else { return Ok(blobs) };
    for f in features {
        let Some(id) = f["id"].as_u64() else { continue };
        if let Some(png) = f["kind"]["sketch"]["reference"]["png"].as_str() {
            let bytes = B64.decode(png).map_err(|_| "a reference image is not valid base64")?;
            let name = format!("images/{id}.png");
            f["kind"]["sketch"]["reference"]["png"] = json!({"blob": name});
            blobs.push(Blob { name, bytes, stored: true });
        }
        if let Some(png) = f["kind"]["relief"]["image"]["png"].as_str() {
            let bytes = B64.decode(png).map_err(|_| "a relief image is not valid base64")?;
            let name = format!("images/{id}.png");
            f["kind"]["relief"]["image"]["png"] = json!({"blob": name});
            blobs.push(Blob { name, bytes, stored: true });
        }
        if let Some(k) = f["kind"]["import"]["blob"].as_str().and_then(|m| m.strip_prefix('#')).and_then(|k| k.parse::<usize>().ok()) {
            let bytes = std::mem::take(meshes.get_mut(k).ok_or("a mesh marker points past the detached meshes")?);
            let name = format!("meshes/{id}.mesh");
            f["kind"]["import"]["blob"] = json!(name);
            blobs.push(Blob { name, bytes, stored: false });
        }
    }
    Ok(blobs)
}

/// Reads the blobs `detach_blobs` left markers for: images go back inline, meshes are decoded into
/// the thread-local store the document reader takes them from. `read` is given the entry name and its byte limit.
fn attach_blobs(v: &mut Value, mut read: impl FnMut(&str, usize) -> Result<Vec<u8>, String>) -> Result<(), String> {
    let Some(features) = v["features"].as_array_mut() else { return Ok(()) };
    for f in features {
        if let Some(name) = f["kind"]["sketch"]["reference"]["png"]["blob"].as_str().map(str::to_owned) {
            let bytes = read(&name, MAX_IMAGE_BLOB)?;
            f["kind"]["sketch"]["reference"]["png"] = json!(B64.encode(bytes));
        }
        if let Some(name) = f["kind"]["relief"]["image"]["png"]["blob"].as_str().map(str::to_owned) {
            let bytes = read(&name, MAX_IMAGE_BLOB)?;
            f["kind"]["relief"]["image"]["png"] = json!(B64.encode(bytes));
        }
        if let Some(name) = f["kind"]["import"]["blob"].as_str().map(str::to_owned) {
            let bytes = read(&name, MAX_MESH_BLOB)?;
            let mesh = if name.ends_with(".tris") { crate::meshfile::decode_tris_blob(&bytes) } else { crate::meshfile::decode_blob(&bytes) }.map_err(|e| format!("{name}: {e}"))?;
            crate::mesh::BLOBS.with(|b| b.borrow_mut().incoming.insert(name, mesh));
        }
    }
    Ok(())
}

/// Validates and encodes the document: plain JSON, or a container when it has payloads.
/// Returns the bytes and whether they are a container.
pub fn encode(doc: &Document, extras: &Extras) -> Result<(Vec<u8>, bool), String> {
    crate::validation::document(doc)?;
    let (mut v, meshes) = json_value_detached(doc);
    let blobs = detach_blobs(&mut v, meshes)?;
    if blobs.is_empty() && extras.cache.is_none() {
        let text = serde_json::to_string_pretty(&v).unwrap();
        if text.len() > MAX_NATIVE_BYTES { return Err("the Ferrender file exceeds 64 MiB".into()); }
        return Ok((text.into_bytes(), false));
    }
    let design = serde_json::to_string_pretty(&v).unwrap();
    if design.len() > MAX_NATIVE_BYTES { return Err("the Ferrender design exceeds 64 MiB".into()); }
    let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).large_file(true);
    let fast = deflate.compression_level(Some(1));
    let store = SimpleFileOptions::default().compression_method(CompressionMethod::Stored).large_file(true);
    let cache_entries = extras.cache.as_ref().map(|c| c.entries()).unwrap_or_default();
    let mut entries: Vec<&str> = vec!["manifest.json", "design.json"];
    entries.extend(blobs.iter().map(|b| b.name.as_str()));
    entries.extend(cache_entries.iter().map(|e| e.0.as_str()));
    let thumbnail = extras.thumbnail_png.as_ref().filter(|t| !t.is_empty() && t.len() <= MAX_THUMBNAIL_BYTES);
    if thumbnail.is_some() { entries.push("thumbnail.png"); }
    let manifest = json!({
        "format": CONTAINER_FORMAT,
        "version": CONTAINER_VERSION,
        "min_reader": CONTAINER_VERSION,
        "design_version": design_version(doc),
        "app": extras.app.clone().unwrap_or_else(|| format!("fr-core {}", env!("CARGO_PKG_VERSION"))),
        "kernel": {"cadrum": "0.8.20"},
        "saved": iso_now(),
        "entries": entries,
        "cached_bodies": extras.cache.as_ref().map(|c| c.body_count()),
    });
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let put = |zip: &mut ZipWriter<Cursor<Vec<u8>>>, name: &str, bytes: &[u8], options: SimpleFileOptions| -> Result<(), String> {
        zip.start_file(name, options).and_then(|_| zip.write_all(bytes).map_err(Into::into)).map_err(|e| format!("could not write {name}: {e}"))
    };
    put(&mut zip, "manifest.json", serde_json::to_string_pretty(&manifest).unwrap().as_bytes(), deflate)?;
    put(&mut zip, "design.json", design.as_bytes(), deflate)?;
    for b in &blobs {
        put(&mut zip, &b.name, &b.bytes, if b.stored { store } else if b.bytes.len() > FAST_DEFLATE_ABOVE { fast } else { deflate })?;
    }
    for (name, bytes) in &cache_entries {
        put(&mut zip, name, bytes, if bytes.len() > FAST_DEFLATE_ABOVE { fast } else { deflate })?;
    }
    if let Some(t) = thumbnail {
        put(&mut zip, "thumbnail.png", t, store)?;
    }
    let bytes = zip.finish().map_err(|e| format!("could not finish the container: {e}"))?.into_inner();
    if bytes.len() > MAX_CONTAINER_BYTES { return Err("the Ferrender file exceeds 2 GiB".into()); }
    Ok((bytes, true))
}

fn from_value(v: Value) -> Result<Document, String> {
    if v["format"] != FORMAT {
        return Err("not a Ferrender file".into());
    }
    if v["version"].as_u64().unwrap_or(0) > FORMAT_VERSION as u64 {
        return Err("this file was written by a newer version of Ferrender".into());
    }
    let doc: Document = serde_json::from_value(v).map_err(|e| format!("could not read the file: {e}"))?;
    crate::validation::document(&doc).map_err(|e| format!("invalid Ferrender file: {e}"))?;
    Ok(doc)
}

/// Reads the plain-JSON form.
pub fn from_json(text: &str) -> Result<Document, String> {
    if text.len() > MAX_NATIVE_BYTES { return Err("the Ferrender file exceeds 64 MiB".into()); }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a Ferrender file: {e}"))?;
    from_value(v)
}

/// Entry names a container may hold. Anything else is refused before it is read.
fn entry_allowed(name: &str) -> bool {
    if name.contains('\\') || name.starts_with('/') || name.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return false;
    }
    let numbered = |dir: &str, ext: &str| name.strip_prefix(dir).and_then(|r| r.strip_suffix(ext)).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    matches!(name, "manifest.json" | "design.json" | "thumbnail.png" | "cache/index.json") || numbered("images/", ".png") || numbered("meshes/", ".mesh") || numbered("meshes/", ".tris") || numbered("cache/", ".brep") || numbered("cache/", ".mesh")
}

/// Reads one entry, refusing before decompression if it declares more than `max` bytes.
fn read_entry(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str, max: usize) -> Result<Vec<u8>, String> {
    let entry = archive.by_name(name).map_err(|_| format!("the container has no {name}"))?;
    if entry.size() > max as u64 {
        return Err(format!("{name} declares {} bytes, more than the {} MiB limit", entry.size(), max / (1024 * 1024)));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.take(max as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("could not read {name}: {e}"))?;
    if bytes.len() > max { return Err(format!("{name} is larger than it declares")); }
    Ok(bytes)
}

fn open_container(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, String> {
    let archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a Ferrender container: {e}"))?;
    if archive.len() > MAX_ENTRIES { return Err("the container has too many entries".into()); }
    if let Some(bad) = archive.file_names().find(|n| !entry_allowed(n)) {
        return Err(format!("the container has an unexpected entry: {bad}"));
    }
    Ok(archive)
}

/// Reads a container's manifest, checking that this build can read it.
fn read_manifest(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<Value, String> {
    let manifest: Value = serde_json::from_slice(&read_entry(archive, "manifest.json", MAX_MANIFEST_BYTES)?).map_err(|e| format!("the container manifest is damaged: {e}"))?;
    if manifest["format"] != CONTAINER_FORMAT {
        return Err("not a Ferrender container".into());
    }
    if manifest["min_reader"].as_u64().unwrap_or(0) > FORMAT_VERSION as u64 {
        return Err("this file was written by a newer version of Ferrender".into());
    }
    Ok(manifest)
}

/// Reads a container's geometry cache, if it has one that is well formed. A damaged cache is
/// simply absent: the design rebuilds.
fn read_cache(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Option<crate::cache::Cache> {
    if archive.by_name("cache/index.json").is_err() { return None; }
    let index: crate::cache::Index = serde_json::from_slice(&read_entry(archive, "cache/index.json", MAX_CACHE_INDEX_BYTES).ok()?).ok()?;
    let mut blobs = std::collections::HashMap::new();
    for e in &index.bodies {
        for name in e.brep.iter().chain(e.mesh.iter()) {
            if !entry_allowed(name) || !name.starts_with("cache/") { return None; }
            blobs.insert(name.clone(), read_entry(archive, name, MAX_CACHE_BLOB).ok()?);
        }
    }
    Some(crate::cache::Cache { index, blobs })
}

/// Opens a file fully: the design, its cache, and whether it is from a newer version.
pub fn open(path: &Path) -> Result<Opened, String> {
    let bytes = read_bounded(path, MAX_CONTAINER_BYTES)?;
    if !bytes.starts_with(b"PK") {
        let doc = from_json(std::str::from_utf8(&bytes).map_err(|_| "the Ferrender file is not UTF-8")?)?;
        return Ok(Opened { doc: Some(doc), cache: None, container: false, newer: false, app: None });
    }
    if bytes.len() > MAX_CONTAINER_BYTES { return Err("the Ferrender file exceeds 2 GiB".into()); }
    let mut archive = open_container(&bytes)?;
    let manifest = read_manifest(&mut archive)?;
    let app = manifest["app"].as_str().map(str::to_owned);
    let cache = read_cache(&mut archive);
    let design = read_entry(&mut archive, "design.json", MAX_NATIVE_BYTES)?;
    let mut v: Value = serde_json::from_slice(&design).map_err(|e| format!("the container's design is damaged: {e}"))?;
    if v["version"].as_u64().unwrap_or(0) > FORMAT_VERSION as u64 {
        if cache.is_some() {
            return Ok(Opened { doc: None, cache, container: true, newer: true, app });
        }
        return Err("this file was written by a newer version of Ferrender".into());
    }
    crate::mesh::BLOBS.with(|b| b.borrow_mut().incoming.clear());
    let result = attach_blobs(&mut v, |name, max| read_entry(&mut archive, name, max)).and_then(|_| from_value(v));
    crate::mesh::BLOBS.with(|b| b.borrow_mut().incoming.clear());
    Ok(Opened { doc: Some(result?), cache, container: true, newer: false, app })
}

/// Decodes either form from its bytes.
pub fn decode(bytes: &[u8]) -> Result<Document, String> {
    if !bytes.starts_with(b"PK") {
        return from_json(std::str::from_utf8(bytes).map_err(|_| "the Ferrender file is not UTF-8")?);
    }
    if bytes.len() > MAX_CONTAINER_BYTES { return Err("the Ferrender file exceeds 2 GiB".into()); }
    let mut archive = open_container(bytes)?;
    read_manifest(&mut archive)?;
    let design = read_entry(&mut archive, "design.json", MAX_NATIVE_BYTES)?;
    let mut v: Value = serde_json::from_slice(&design).map_err(|e| format!("the container's design is damaged: {e}"))?;
    crate::mesh::BLOBS.with(|b| b.borrow_mut().incoming.clear());
    let result = attach_blobs(&mut v, |name, max| read_entry(&mut archive, name, max)).and_then(|_| from_value(v));
    crate::mesh::BLOBS.with(|b| b.borrow_mut().incoming.clear());
    result
}

/// The same structural and size guarantees apply to save, recovery and open.
pub fn validated_json(doc: &Document) -> Result<String, String> {
    crate::validation::document(doc)?;
    let text = to_json(doc);
    if text.len() > MAX_NATIVE_BYTES { return Err("the Ferrender file exceeds 64 MiB".into()); }
    Ok(text)
}

/// Whether the file at `path` is a container (as opposed to plain JSON or absent).
pub fn is_container(path: &Path) -> bool {
    let mut head = [0u8; 2];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"PK"
}

/// Where the plain-JSON original goes when a save converts it to a container.
pub fn backup_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("design");
    path.with_file_name(format!("{stem} (0.3 backup).ferr"))
}

pub fn save(doc: &Document, path: &Path) -> Result<Saved, String> {
    save_with(doc, path, &Extras::default())
}

/// Writes the document atomically. A plain file that becomes a container is first
/// copied to [`backup_path`], once, so the pre-0.4 original is never lost.
pub fn save_with(doc: &Document, path: &Path, extras: &Extras) -> Result<Saved, String> {
    let (bytes, container) = encode(doc, extras)?;
    let mut backup = None;
    if container && path.is_file() && !is_container(path) {
        let to = backup_path(path);
        if !to.exists() {
            std::fs::copy(path, &to).map_err(|e| format!("could not keep a backup of {} at {}: {e}", path.display(), to.display()))?;
            backup = Some(to);
        }
    }
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    // Never follow a predictable .ferr.tmp symlink or truncate another writer's file.
    let (tmp, mut file) = loop {
        let tmp = path.with_extension(format!("ferr.{}.{}.tmp", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("could not save {}: {e}", path.display())),
        }
    };
    let result = file.write_all(&bytes).and_then(|_| file.sync_all()).and_then(|_| std::fs::rename(&tmp, path));
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result.map_err(|e| format!("could not save {}: {e}", path.display()))?;
    Ok(Saved { path: path.to_owned(), container, backup, cached_bodies: extras.cache.as_ref().map_or(0, |c| c.body_count()) })
}

fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let mut data = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut data).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if data.len() > max { return Err(format!("{} exceeds the {} MiB file limit", path.display(), max / (1024 * 1024))); }
    Ok(data)
}

pub fn load(path: &Path) -> Result<Document, String> {
    decode(&read_bounded(path, MAX_CONTAINER_BYTES)?)
}

/// A container's embedded preview, if the file is a container that has one.
pub fn thumbnail(path: &Path) -> Option<Vec<u8>> {
    let bytes = read_bounded(path, MAX_CONTAINER_BYTES).ok()?;
    if !bytes.starts_with(b"PK") { return None; }
    let mut archive = open_container(&bytes).ok()?;
    read_entry(&mut archive, "thumbnail.png", MAX_THUMBNAIL_BYTES).ok()
}

/// A container's manifest, if the file is a container.
pub fn manifest(path: &Path) -> Option<Value> {
    let bytes = read_bounded(path, MAX_CONTAINER_BYTES).ok()?;
    if !bytes.starts_with(b"PK") { return None; }
    read_manifest(&mut open_container(&bytes).ok()?).ok()
}

/// The current time as RFC 3339 UTC, without pulling in a date crate.
fn iso_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Binary STL of the bodies, with coordinates written in `unit`.
pub fn stl_bytes<'a>(bodies: impl IntoIterator<Item = &'a Body>, unit: Unit) -> Vec<u8> {
    let bodies: Vec<&Body> = bodies.into_iter().collect();
    let count: usize = bodies.iter().map(|b| b.mesh.len()).sum();
    let mut out = format!("Ferrender STL, units: {}", unit.name()).into_bytes();
    out.resize(80, b' ');
    out.reserve(count * 50 + 4);
    out.extend((count as u32).to_le_bytes());
    for t in bodies.iter().flat_map(|b| b.mesh.tris()) {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        for v in [n, t[0] / unit.mm(), t[1] / unit.mm(), t[2] / unit.mm()] {
            for c in v.to_array() {
                out.extend((c as f32).to_le_bytes());
            }
        }
        out.extend([0, 0]);
    }
    out
}

pub fn write_stl<'a>(bodies: impl IntoIterator<Item = &'a Body>, unit: Unit, path: &Path) -> Result<usize, String> {
    let bytes = stl_bytes(bodies, unit);
    let n = (bytes.len() - 84) / 50;
    if n == 0 {
        return Err("there are no bodies to export".into());
    }
    std::fs::write(path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(n)
}
