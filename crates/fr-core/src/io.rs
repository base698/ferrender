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
use crate::mesh::Mesh;
use crate::units::Unit;
use glam::DVec3;

/// Written into every native file so other tools can recognise it.
pub const FORMAT: &str = "ferrender";
/// The newest version this build reads. 9 is the ZIP container; the JSON inside
/// a container keeps its own, lower version, computed as for a plain file.
pub const FORMAT_VERSION: u32 = 14;
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
    /// Why a wanted cache was not captured, reported back in [`Saved`].
    pub cache_skipped: Option<String>,
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
    /// Why no geometry cache was written although the session wanted one: the design
    /// rebuilds when opened. `None` when a cache was written or none was wanted.
    pub cache_skipped: Option<String>,
}

/// The lowest format version that can read this design, which is what a plain file is stamped with.
pub fn design_version(doc: &Document) -> u32 {
    if doc.features.iter().any(|f| matches!(&f.kind, FeatureKind::Loft(_))) { 14 }
    else if doc.features.iter().any(|f| has_tags(&f.kind) || f.script_key.is_some() || matches!(&f.kind, FeatureKind::Sweep(_))) { 13 }
    else if doc.features.iter().any(|f| matches!(&f.kind, FeatureKind::ScriptRun(_)) || f.made_by.is_some()) { 12 }
    else if doc.features.iter().any(|f| matches!(&f.kind, FeatureKind::MeshOp(_) | FeatureKind::Relief(_))) { 11 }
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

/// Whether a feature stores face or edge tags (semantic tag schema, format 13).
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
    // Count references as well as ZIP entries: a small design can otherwise
    // inline the same large image thousands of times during attachment.
    let mut expanded = 0usize;
    let mut read = |name: &str, max: usize| -> Result<Vec<u8>, String> {
        let bytes = read(name, max)?;
        expanded = expanded.checked_add(bytes.len()).ok_or("the container payloads are too large")?;
        if expanded > MAX_CONTAINER_BYTES { return Err("the expanded container payloads exceed 2 GiB".into()); }
        Ok(bytes)
    };
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
    encode_using(doc, extras, crate::cache_auth::Store::local().as_ref())
}

fn encode_using(doc: &Document, extras: &Extras, trust: Option<&crate::cache_auth::Store>) -> Result<(Vec<u8>, bool), String> {
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
    let mut bytes = zip.finish().map_err(|e| format!("could not finish the container: {e}"))?.into_inner();
    // Never turn an untrusted imported preview into a signed cache by merely
    // re-encoding it. Supported sessions rebuild before they can save one.
    if extras.cache.as_ref().is_some_and(|c| c.matches(doc)) {
        if let Some(trust) = trust { trust.seal(&mut bytes); }
    }
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
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a Ferrender container: {e}"))?;
    if archive.len() > MAX_ENTRIES { return Err("the container has too many entries".into()); }
    if let Some(bad) = archive.file_names().find(|n| !entry_allowed(n)) {
        return Err(format!("the container has an unexpected entry: {bad}"));
    }
    let mut expanded = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| format!("could not inspect the container: {e}"))?;
        expanded = expanded.checked_add(entry.size()).ok_or("the container declares too much data")?;
        if expanded > MAX_CONTAINER_BYTES as u64 {
            return Err("the expanded container exceeds the 2 GiB limit".into());
        }
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
fn read_cache(archive: &mut ZipArchive<Cursor<&[u8]>>, trusted: bool) -> Option<crate::cache::Cache> {
    if archive.by_name("cache/index.json").is_err() { return None; }
    let index: crate::cache::Index = serde_json::from_slice(&read_entry(archive, "cache/index.json", MAX_CACHE_INDEX_BYTES).ok()?).ok()?;
    let mut blobs = std::collections::HashMap::new();
    let mut total = 0usize;
    let mut ids = std::collections::HashSet::new();
    for e in &index.bodies {
        if !ids.insert(e.id) || e.brep.is_some() == e.mesh.is_some() { return None; }
        for name in e.brep.iter().chain(e.mesh.iter()) {
            if !entry_allowed(name) || !name.starts_with("cache/") { return None; }
            if blobs.contains_key(name) { return None; }
            let bytes = read_entry(archive, name, MAX_CACHE_BLOB.checked_sub(total)?).ok()?;
            total = total.checked_add(bytes.len())?;
            blobs.insert(name.clone(), bytes);
        }
    }
    Some(crate::cache::Cache { index, blobs, trusted })
}

/// Opens a file fully: the design, its cache, and whether it is from a newer version.
pub fn open(path: &Path) -> Result<Opened, String> {
    let bytes = read_bounded(path, MAX_CONTAINER_BYTES)?;
    open_bytes_using(&bytes, crate::cache_auth::Store::local().as_ref())
}

fn open_bytes_using(bytes: &[u8], trust: Option<&crate::cache_auth::Store>) -> Result<Opened, String> {
    if !bytes.starts_with(b"PK") {
        let doc = from_json(std::str::from_utf8(&bytes).map_err(|_| "the Ferrender file is not UTF-8")?)?;
        return Ok(Opened { doc: Some(doc), cache: None, container: false, newer: false, app: None });
    }
    if bytes.len() > MAX_CONTAINER_BYTES { return Err("the Ferrender file exceeds 2 GiB".into()); }
    let mut archive = open_container(&bytes)?;
    let manifest = read_manifest(&mut archive)?;
    let app = manifest["app"].as_str().map(str::to_owned);
    let authenticated = trust.is_some_and(|store| store.verify(bytes, archive.comment()));
    let design = read_entry(&mut archive, "design.json", MAX_NATIVE_BYTES)?;
    let mut v: Value = serde_json::from_slice(&design).map_err(|e| format!("the container's design is damaged: {e}"))?;
    if v["version"].as_u64().unwrap_or(0) > FORMAT_VERSION as u64 {
        // Its timeline cannot be validated by this reader. Even a local MAC
        // does not make a future-version preview authoritative here.
        let cache = read_cache(&mut archive, false);
        if cache.is_some() {
            return Ok(Opened { doc: None, cache, container: true, newer: true, app });
        }
        return Err("this file was written by a newer version of Ferrender".into());
    }
    // Discard foreign caches before parsing any cached BRep or mesh. Only the
    // supported authoritative design and its referenced payloads are loaded.
    let cache = authenticated.then(|| read_cache(&mut archive, true)).flatten();
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

/// Keeps the plain-JSON file at `path` as [`backup_path`] when a container is
/// about to replace it, once; returns where the backup went. Scripts that
/// write a design through a staged file apply the same rule when they commit.
pub fn keep_backup(path: &Path, new_is_container: bool) -> Result<Option<PathBuf>, String> {
    if !(new_is_container && path.is_file() && !is_container(path)) { return Ok(None); }
    let to = backup_path(path);
    // A predictable backup filename must never follow a symlink (including
    // a dangling one), or a save could overwrite an unrelated file.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&to) {
        Ok(mut destination) => {
            let result = std::fs::File::open(path)
                .and_then(|mut source| std::io::copy(&mut source, &mut destination))
                .and_then(|_| destination.sync_all());
            if let Err(e) = result {
                let _ = std::fs::remove_file(&to);
                return Err(format!("could not keep a backup of {} at {}: {e}", path.display(), to.display()));
            }
            Ok(Some(to))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !std::fs::symlink_metadata(&to).is_ok_and(|m| m.file_type().is_file()) {
                return Err(format!("the backup path {} already exists and is not a regular file", to.display()));
            }
            Ok(None)
        }
        Err(e) => Err(format!("could not create the backup {}: {e}", to.display())),
    }
}

/// Writes the document atomically. A plain file that becomes a container is first
/// copied to [`backup_path`], once, so the pre-0.4 original is never lost.
pub fn save_with(doc: &Document, path: &Path, extras: &Extras) -> Result<Saved, String> {
    let (bytes, container) = encode(doc, extras)?;
    let backup = keep_backup(path, container)?;
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
    Ok(Saved { path: path.to_owned(), container, backup, cached_bodies: extras.cache.as_ref().map_or(0, |c| c.body_count()), cache_skipped: extras.cache_skipped.clone() })
}

fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("could not inspect {}: {e}", path.display()))?;
    if !metadata.is_file() { return Err(format!("{} is not a regular file", path.display())); }
    let mut file = std::fs::File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let mut head = [0; 2];
    let n = file.read(&mut head).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let max = if n == 2 && &head == b"PK" { max } else { max.min(MAX_NATIVE_BYTES) };
    if metadata.len() > max as u64 { return Err(format!("{} exceeds the {} MiB file limit", path.display(), max / (1024 * 1024))); }
    let mut data = head[..n].to_vec();
    file.take(max as u64 + 1 - n as u64).read_to_end(&mut data).map_err(|e| format!("could not read {}: {e}", path.display()))?;
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
    stl_bytes_of(count, bodies.iter().flat_map(|b| b.mesh.tris()), unit)
}

/// One mesh as binary STL, as an exported script's sidecar file.
pub fn mesh_stl_bytes(mesh: &Mesh, unit: Unit) -> Vec<u8> {
    stl_bytes_of(mesh.len(), mesh.tris(), unit)
}

fn stl_bytes_of(count: usize, tris: impl Iterator<Item = [DVec3; 3]>, unit: Unit) -> Vec<u8> {
    let mut out = format!("Ferrender STL, units: {}", unit.name()).into_bytes();
    out.resize(80, b' ');
    out.reserve(count * 50 + 4);
    out.extend((count as u32).to_le_bytes());
    for t in tris {
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
    write_stl_with_provenance(bodies, unit, path, false)
}

/// Exported preview geometry keeps its unverified status in the STL header.
pub fn write_stl_with_provenance<'a>(bodies: impl IntoIterator<Item = &'a Body>, unit: Unit, path: &Path, unverified: bool) -> Result<usize, String> {
    let mut bytes = stl_bytes(bodies, unit);
    if unverified {
        let label = format!("Ferrender UNVERIFIED cached preview; units: {}", unit.name());
        bytes[..80].fill(b' ');
        bytes[..label.len()].copy_from_slice(label.as_bytes());
    }
    let n = (bytes.len() - 84) / 50;
    if n == 0 {
        return Err("there are no bodies to export".into());
    }
    std::fs::write(path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(n)
}

/// STEP supports comments, so exported future previews retain their provenance
/// without inventing geometry attributes or changing the kernel's entities.
pub fn step_with_provenance<'a>(solids: impl IntoIterator<Item = &'a cadrum::Solid>, unverified: bool) -> Result<Vec<u8>, String> {
    let mut bytes = crate::exact::step(solids)?;
    if unverified {
        let at = bytes.iter().position(|b| *b == b'\n').map_or(0, |n| n + 1);
        bytes.splice(at..at, b"/* Ferrender UNVERIFIED cached preview: newer timeline was not rebuilt. */\n".iter().copied());
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod cache_trust_tests {
    use super::*;
    use crate::{cache::Cache, cache_auth::Store, doc::Session};
    use std::os::unix::fs::DirBuilderExt;

    fn store(name: &str) -> (PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!("ferrender-file-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let store = Store::in_directory(dir.join("private"));
        (dir, store)
    }
    fn box_session(width: f64) -> Session {
        let mut s = Session::default();
        crate::api::execute(&mut s, &json!({"op":"primitive", "type":"box", "width":width, "depth":10, "height":10}), None).unwrap();
        s
    }
    fn extras(s: &Session) -> Extras { Extras { cache: Some(Cache::capture(&s.doc, &s.built).unwrap()), ..Default::default() } }
    fn rewrite(bytes: &[u8], edit: impl Fn(&str, Vec<u8>) -> Vec<u8>) -> Vec<u8> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        out.set_raw_comment(archive.comment().into()); // replay the original MAC, as an attacker could
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let mut payload = Vec::new();
            entry.read_to_end(&mut payload).unwrap();
            out.start_file(entry.name(), SimpleFileOptions::default()).unwrap();
            out.write_all(&edit(entry.name(), payload)).unwrap();
        }
        out.finish().unwrap().into_inner()
    }
    fn volume(s: &Session) -> f64 { s.built.bodies.iter().flat_map(|b| &b.solids).map(cadrum::Solid::volume).sum() }

    #[test]
    fn cache_auth_foreign_and_unsigned_files_rebuild_but_local_copy_stays_fast() {
        let (a, local) = store("origin");
        let (b, foreign) = store("destination");
        let original = box_session(10.0);
        let (bytes, _) = encode_using(&original.doc, &extras(&original), Some(&local)).unwrap();
        let opened = open_bytes_using(&bytes, Some(&local)).unwrap();
        let cached = Session::with_cache(opened.doc.unwrap(), opened.cache.as_ref());
        assert!(cached.from_cache);
        assert_eq!(cached.geometry_trust(), "local_authenticated_cache");
        assert!((volume(&cached) - 1000.0).abs() < 1e-6);
        for trust in [None, Some(&foreign)] {
            let opened = open_bytes_using(&bytes, trust).unwrap();
            assert!(opened.cache.is_none(), "foreign cache is discarded before BRep parsing");
            let rebuilt = Session::with_cache(opened.doc.unwrap(), opened.cache.as_ref());
            assert!(!rebuilt.from_cache);
            assert!((volume(&rebuilt) - 1000.0).abs() < 1e-6);
        }
        let (unsigned, _) = encode_using(&original.doc, &extras(&original), None).unwrap();
        assert!(open_bytes_using(&unsigned, Some(&local)).unwrap().cache.is_none());
        assert_eq!(decode(&unsigned).unwrap(), original.doc, "legacy unsigned files remain editable");
        std::fs::remove_dir_all(a).unwrap();
        std::fs::remove_dir_all(b).unwrap();
    }

    #[test]
    fn cache_auth_rejects_coordinated_geometry_index_forgery_before_adoption() {
        let (dir, local) = store("forged");
        let original = box_session(10.0);
        let (bytes, _) = encode_using(&original.doc, &extras(&original), Some(&local)).unwrap();
        let mut wrong = extras(&box_session(20.0)).cache.unwrap();
        wrong.index.design_crc = crate::cache::design_crc(&original.doc);
        let wrong_entries: std::collections::HashMap<_, _> = wrong.entries().into_iter().collect();
        let forged = rewrite(&bytes, |name, data| wrong_entries.get(name).cloned().unwrap_or(data));
        // Both the fake shape and its fake index agree: the previous CRC +
        // volume/bounds checks alone would have accepted the wrong 2000mm³ box.
        let mut archive = open_container(&forged).unwrap();
        let untrusted = read_cache(&mut archive, false).unwrap();
        let restored = untrusted.restore().unwrap();
        let forged_volume: f64 = restored.bodies.iter().flat_map(|b| &b.solids).map(cadrum::Solid::volume).sum();
        assert!((forged_volume - 2000.0).abs() < 1e-6);
        assert!(!untrusted.matches(&original.doc));
        let opened = open_bytes_using(&forged, Some(&local)).unwrap();
        assert!(opened.cache.is_none());
        let rebuilt = Session::with_cache(opened.doc.unwrap(), Some(&untrusted));
        assert!(!rebuilt.from_cache);
        assert!((volume(&rebuilt) - 1000.0).abs() < 1e-6);
        // Re-encoding a parsed untrusted cache is not a signing oracle.
        let bad_extras = Extras { cache: Some(untrusted), ..Default::default() };
        let (resaved, _) = encode_using(&original.doc, &bad_extras, Some(&local)).unwrap();
        assert!(open_bytes_using(&resaved, Some(&local)).unwrap().cache.is_none());
        // A genuinely rebuilt document can produce a new trusted local cache.
        let (fresh, _) = encode_using(&rebuilt.doc, &extras(&rebuilt), Some(&local)).unwrap();
        assert!(open_bytes_using(&fresh, Some(&local)).unwrap().cache.is_some());
        let broken = rewrite(&bytes, |name, data| if name.ends_with(".brep") { b"not even a BRep".to_vec() } else { data });
        assert!(open_bytes_using(&broken, Some(&local)).unwrap().cache.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_auth_binds_design_images_and_mesh_payloads() {
        use crate::{mesh::Mesh, reference::ReferenceImage, sketch::{Plane, Sketch}};
        use glam::DVec3;
        let (dir, local) = store("payloads");
        let mut s = box_session(10.0);
        let mesh = Mesh::from_indexed(vec![DVec3::ZERO, DVec3::X, DVec3::Y], vec![[0,1,2]], true).unwrap();
        s.doc.add_feature(FeatureKind::Import(mesh.clone()));
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.write_header().unwrap().write_image_data(&[255,255,255,255]).unwrap();
        }
        let mut sketch = Sketch::new(Plane::XY);
        sketch.reference = Some(ReferenceImage::from_bytes("image.png", &png, 10.0).unwrap());
        s.doc.add_feature(FeatureKind::Sketch(sketch));
        s.rebuild();
        let (bytes, _) = encode_using(&s.doc, &extras(&s), Some(&local)).unwrap();
        assert!(open_bytes_using(&bytes, Some(&local)).unwrap().cache.is_some());
        for target in ["design.json", "meshes/2.mesh", "images/3.png"] {
            let edited = rewrite(&bytes, |name, mut data| {
                if name != target { return data; }
                if name == "design.json" {
                    let mut v: Value = serde_json::from_slice(&data).unwrap();
                    v["features"][0]["name"] = json!("changed design");
                    serde_json::to_vec(&v).unwrap()
                } else if name.ends_with(".mesh") {
                    let mut changed = mesh.clone(); changed.map(|p| p + DVec3::Z);
                    crate::meshfile::encode_blob(&changed)
                } else {
                    // Trailing PNG bytes are tolerated by image decoding but
                    // must still invalidate the file's authentication tag.
                    data.extend_from_slice(b"changed image"); data
                }
            });
            let opened = open_bytes_using(&edited, Some(&local)).unwrap();
            assert!(opened.cache.is_none(), "{target} must be bound to the MAC");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_auth_future_preview_is_explicitly_unverified_even_with_a_local_mac() {
        let (dir, local) = store("future");
        let s = box_session(10.0);
        let (bytes, _) = encode_using(&s.doc, &extras(&s), Some(&local)).unwrap();
        let future = rewrite(&bytes, |name, data| if name == "design.json" {
            let mut v: Value = serde_json::from_slice(&data).unwrap();
            v["version"] = json!(FORMAT_VERSION + 1);
            serde_json::to_vec(&v).unwrap()
        } else { data });
        let archive = open_container(&future).unwrap();
        let comment_len = archive.comment().len();
        drop(archive);
        let mut authenticated_future = future[..future.len()-comment_len].to_vec();
        let end = authenticated_future.len(); authenticated_future[end-2..].fill(0);
        local.seal(&mut authenticated_future);
        for bytes in [&future, &authenticated_future] {
            let opened = open_bytes_using(bytes, Some(&local)).unwrap();
            assert!(opened.newer && opened.doc.is_none());
            assert!(!opened.cache.unwrap().trusted, "a newer timeline is never verified by this reader");
            let path = dir.join("future.ferr");
            std::fs::write(&path, bytes).unwrap();
            let mut session = Session::open(&path).unwrap();
            assert!(session.read_only);
            assert_eq!(session.geometry_trust(), "unverified_preview");
            let scene = crate::api::execute(&mut session, &json!({"op":"get_scene_info"}), None).unwrap();
            assert_eq!(scene["geometry_trust"], "unverified_preview");
            let replies = crate::api::execute(&mut session, &json!({"op":"batch", "commands":[{"op":"get_object_info", "id":1}]}), None).unwrap();
            assert_eq!(replies[0]["geometry_trust"], "unverified_preview");
            assert!(replies[0]["geometry_warning"].as_str().unwrap().contains("Unverified"));
            let stl = dir.join("preview.stl");
            let exported = crate::api::execute(&mut session, &json!({"op":"export_stl", "path":stl}), None).unwrap();
            assert_eq!(exported["geometry_trust"], "unverified_preview");
            assert!(String::from_utf8_lossy(&std::fs::read(stl).unwrap()[..80]).contains("UNVERIFIED"));
            let step = dir.join("preview.step");
            crate::api::execute(&mut session, &json!({"op":"export_step", "path":step}), None).unwrap();
            let step_bytes = std::fs::read(step).unwrap();
            assert!(String::from_utf8_lossy(&step_bytes).contains("UNVERIFIED cached preview"));
            let exported_solids = cadrum::Solid::read_step(&mut step_bytes.as_slice()).unwrap();
            assert!((exported_solids.iter().map(cadrum::Solid::volume).sum::<f64>() - 1000.0).abs() < 1e-6, "provenance comments retain a valid STEP file");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
