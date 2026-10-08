//! Files: the native `.ferr` document (JSON) and STL meshes.

use std::path::Path;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use glam::DVec3;

use crate::doc::{Body, Document};
use crate::mesh::Mesh;
use crate::units::Unit;

/// Written into every native file so other tools can recognise it.
pub const FORMAT: &str = "ferrender";
pub const FORMAT_VERSION: u32 = 3;
const MAX_NATIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_STL_BYTES: usize = 128 * 1024 * 1024;

pub fn to_json(doc: &Document) -> String {
    let mut v = serde_json::to_value(doc).expect("documents always serialize");
    let o = v.as_object_mut().unwrap();
    o.insert("format".into(), FORMAT.into());
    // Keep ordinary designs readable by version-1 apps; text needs the new schema.
    let version = if doc.features.iter().any(|f| match &f.kind {
        crate::doc::FeatureKind::Sketch(s) => s.reference.is_some() || !s.arc_guides.is_empty()
            || s.entities.values().any(|e| matches!(e.geom, crate::sketch::Geom::Spline { .. }))
            || s.constraints.values().any(|c| matches!(c.kind, crate::sketch::CKind::PositionX | crate::sketch::CKind::PositionY)),
        _ => false,
    }) { 3 } else if doc.features.iter().any(|f| matches!(f.kind, crate::doc::FeatureKind::Text(_))) { 2 } else { 1 };
    o.insert("version".into(), version.into());
    serde_json::to_string_pretty(&v).unwrap()
}

pub fn from_json(text: &str) -> Result<Document, String> {
    if text.len() > MAX_NATIVE_BYTES { return Err("the Ferrender file exceeds 64 MiB".into()); }
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not a Ferrender file: {e}"))?;
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

/// The same structural and size guarantees apply to save, recovery and open.
pub fn validated_json(doc: &Document) -> Result<String, String> {
    crate::validation::document(doc)?;
    let text = to_json(doc);
    if text.len() > MAX_NATIVE_BYTES { return Err("the Ferrender file exceeds 64 MiB".into()); }
    Ok(text)
}

pub fn save(doc: &Document, path: &Path) -> Result<(), String> {
    let text = validated_json(doc)?;
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
    let result = file.write_all(text.as_bytes()).and_then(|_| file.sync_all()).and_then(|_| std::fs::rename(&tmp, path));
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result.map_err(|e| format!("could not save {}: {e}", path.display()))
}

fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let mut data = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut data).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if data.len() > max { return Err(format!("{} exceeds the {} MiB file limit", path.display(), max / (1024 * 1024))); }
    Ok(data)
}

pub fn load(path: &Path) -> Result<Document, String> {
    let data = read_bounded(path, MAX_NATIVE_BYTES)?;
    from_json(std::str::from_utf8(&data).map_err(|_| "the Ferrender file is not UTF-8")?)
}

/// Binary STL of the bodies, with coordinates written in `unit`.
pub fn stl_bytes<'a>(bodies: impl IntoIterator<Item = &'a Body>, unit: Unit) -> Vec<u8> {
    let tris: Vec<&[DVec3; 3]> = bodies.into_iter().flat_map(|b| &b.mesh.tris).collect();
    let mut out = format!("Ferrender STL, units: {}", unit.name()).into_bytes();
    out.resize(80, b' ');
    out.extend((tris.len() as u32).to_le_bytes());
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
    let bytes = stl_bytes(bodies, unit);
    let n = (bytes.len() - 84) / 50;
    if n == 0 {
        return Err("there are no bodies to export".into());
    }
    std::fs::write(path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(n)
}

/// Reads a binary or ASCII STL; `unit` says what its numbers mean.
pub fn parse_stl(bytes: &[u8], unit: Unit) -> Result<Mesh, String> {
    if bytes.len() > MAX_STL_BYTES { return Err("the STL exceeds 128 MiB".into()); }
    let s = unit.mm();
    let mut m = Mesh::default();
    let count = bytes.get(80..84).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if let Some(n) = count
        && bytes.len() == 84 + n * 50
    {
        for t in bytes[84..].chunks_exact(50) {
            let f = |i: usize| f32::from_le_bytes([t[i], t[i + 1], t[i + 2], t[i + 3]]) as f64 * s;
            let v = |i: usize| DVec3::new(f(i), f(i + 4), f(i + 8));
            m.tris.push([v(12), v(24), v(36)]);
        }
    } else {
        let text = std::str::from_utf8(bytes).map_err(|_| "not an STL file".to_owned())?;
        let mut vs = Vec::new();
        for line in text.lines() {
            let mut w = line.split_whitespace();
            if w.next() == Some("vertex") {
                let c: Vec<f64> = w.map(str::parse).collect::<Result<_, _>>().map_err(|_| "the STL has a malformed vertex")?;
                if c.len() != 3 {
                    return Err("the STL has a malformed vertex".into());
                }
                vs.push(DVec3::new(c[0], c[1], c[2]) * s);
            }
        }
        if vs.len() % 3 != 0 { return Err("the STL has an incomplete triangle".into()); }
        m.tris = vs.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    }
    if m.tris.is_empty() || m.tris.iter().flatten().any(|v| !v.is_finite()) {
        return Err("the STL has no usable triangles".into());
    }
    // Meshes are saved as 32-bit floats; settle on those values now.
    m.map(|v| v.as_vec3().as_dvec3());
    m.validate()?;
    Ok(m)
}

pub fn read_stl(path: &Path, unit: Unit) -> Result<Mesh, String> {
    parse_stl(&read_bounded(path, MAX_STL_BYTES)?, unit)
}
