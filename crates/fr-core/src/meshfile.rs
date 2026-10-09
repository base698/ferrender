//! Mesh files: STL, OBJ and 3MF in, and the binary blob a container keeps a
//! mesh in. Imports stream from disk and weld vertices as they go, so a
//! file of millions of triangles never exists in memory as a triangle soup.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use glam::{DAffine3, DMat3, DVec3};

use crate::mesh::{FxMap, MAX_TRIANGLES, Mesh, Report};
use crate::units::Unit;

/// Files larger than this are refused before they are read: 16 million binary STL triangles.
pub const MAX_MESH_FILE_BYTES: u64 = 16_000_000 * 50 + 84;
/// What one mesh blob may declare in a container: 16 million triangles with every vertex unshared.
pub const MAX_BLOB_BYTES: usize = 32 + MAX_TRIANGLES * (12 + 36);

const MAGIC: &[u8; 8] = b"FRMESH01";

/// Collects triangles corner by corner, sharing vertices that are the same point at 32-bit precision.
pub struct Welder {
    map: FxMap<u128, u32>,
    positions: Vec<DVec3>,
    indices: Vec<[u32; 3]>,
    scale: f64,
}

impl Welder {
    pub fn new(scale: f64, expect_tris: usize) -> Welder {
        let mut map = FxMap::default();
        map.reserve(expect_tris / 2 + 16);
        Welder { map, positions: Vec::with_capacity(expect_tris / 2 + 16), indices: Vec::with_capacity(expect_tris), scale }
    }

    /// `raw` are the file's own 32-bit coordinates, before the unit scale.
    #[inline]
    fn corner(&mut self, raw: [f32; 3]) -> u32 {
        // Adding +0.0 turns a negative zero positive, so the two zeros share a vertex.
        let raw = raw.map(|c| c + 0.0);
        let key = ((raw[0].to_bits() as u128) << 64) | ((raw[1].to_bits() as u128) << 32) | raw[2].to_bits() as u128;
        let next = self.positions.len() as u32;
        let positions = &mut self.positions;
        let scale = self.scale;
        *self.map.entry(key).or_insert_with(|| {
            positions.push((DVec3::new(raw[0] as f64, raw[1] as f64, raw[2] as f64) * scale).as_vec3().as_dvec3());
            next
        })
    }

    pub fn triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Result<(), String> {
        if self.indices.len() >= MAX_TRIANGLES { return Err(format!("the mesh exceeds {} million triangles", MAX_TRIANGLES / 1_000_000)); }
        if !a.iter().chain(&b).chain(&c).all(|v| v.is_finite()) { return Err("the mesh has a vertex that is not a number".into()); }
        let t = [self.corner(a), self.corner(b), self.corner(c)];
        self.indices.push(t);
        Ok(())
    }

    pub fn len(&self) -> usize { self.indices.len() }
    pub fn is_empty(&self) -> bool { self.indices.is_empty() }

    pub fn finish(self) -> Result<Mesh, String> {
        if self.indices.is_empty() { return Err("the file has no usable triangles".into()); }
        Mesh::from_indexed(self.positions, self.indices, true)
    }
}

fn open(path: &Path) -> Result<(std::fs::File, u64), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let len = file.metadata().map_err(|e| format!("could not read {}: {e}", path.display()))?.len();
    if len > MAX_MESH_FILE_BYTES { return Err(format!("{} is larger than the {} MB import limit", path.display(), MAX_MESH_FILE_BYTES / 1_000_000)); }
    Ok((file, len))
}

/// Reads a mesh file by its extension (`.stl`, `.obj`, `.3mf`), repairs it and
/// reports what was found. `unit` says what an STL's or OBJ's numbers mean; a
/// 3MF carries its own unit.
pub fn read_mesh_file(path: &Path, unit: Unit) -> Result<(Mesh, Report), String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let mut mesh = match ext.as_str() {
        "obj" => read_obj(path, unit)?,
        "3mf" => read_3mf(path)?,
        _ => read_stl(path, unit)?,
    };
    let report = mesh.repair();
    mesh.validate()?;
    Ok((mesh, report))
}

/// Reads a binary or ASCII STL, streaming; `unit` says what its numbers mean.
pub fn read_stl(path: &Path, unit: Unit) -> Result<Mesh, String> {
    let (file, len) = open(path)?;
    let mut reader = BufReader::with_capacity(4 << 20, file);
    let mut header = [0u8; 84];
    let got = read_up_to(&mut reader, &mut header).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if got == 84 {
        let n = u32::from_le_bytes([header[80], header[81], header[82], header[83]]) as u64;
        if len == 84 + n * 50 && !header.starts_with(b"solid ") || (len == 84 + n * 50 && n > 0 && looks_binary(&mut reader)) {
            return stream_binary_stl(&mut reader, n as usize, unit.mm()).map_err(|e| format!("{}: {e}", path.display()));
        }
    }
    // ASCII: start again from the top.
    let file = std::fs::File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let reader = BufReader::with_capacity(1 << 20, file);
    ascii_stl(reader, unit.mm(), len as usize / 140).map_err(|e| format!("{}: {e}", path.display()))
}

/// A file starting with "solid " can still be binary; a binary body is not text.
fn looks_binary(reader: &mut BufReader<std::fs::File>) -> bool {
    reader.fill_buf().map(|b| b.iter().take(200).any(|c| *c == 0 || (*c > 127))).unwrap_or(false)
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        let n = r.read(&mut buf[got..])?;
        if n == 0 { break; }
        got += n;
    }
    Ok(got)
}

fn stream_binary_stl(reader: &mut impl Read, n: usize, scale: f64) -> Result<Mesh, String> {
    if n > MAX_TRIANGLES { return Err(format!("the STL has {n} triangles, more than the {} million limit", MAX_TRIANGLES / 1_000_000)); }
    let mut welder = Welder::new(scale, n);
    const CHUNK: usize = 65_536;
    let mut buf = vec![0u8; CHUNK * 50];
    let mut left = n;
    while left > 0 {
        let take = left.min(CHUNK);
        let bytes = &mut buf[..take * 50];
        let got = read_up_to(reader, bytes).map_err(|e| format!("could not read the STL: {e}"))?;
        if got != take * 50 { return Err("the STL ends before its declared triangle count".into()); }
        for rec in bytes.chunks_exact(50) {
            let f = |i: usize| f32::from_le_bytes([rec[i], rec[i + 1], rec[i + 2], rec[i + 3]]);
            let v = |i: usize| [f(i), f(i + 4), f(i + 8)];
            welder.triangle(v(12), v(24), v(36))?;
        }
        left -= take;
    }
    welder.finish()
}

fn ascii_stl(reader: impl BufRead, scale: f64, expect: usize) -> Result<Mesh, String> {
    let mut welder = Welder::new(scale, expect);
    let mut corners: Vec<[f32; 3]> = Vec::with_capacity(3);
    let mut any = false;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("could not read the STL: {e}"))?;
        let mut w = line.split_whitespace();
        match w.next() {
            Some("vertex") => {
                any = true;
                let c: Vec<f32> = w.map(str::parse).collect::<Result<_, _>>().map_err(|_| "the STL has a malformed vertex")?;
                if c.len() != 3 { return Err("the STL has a malformed vertex".into()); }
                corners.push([c[0], c[1], c[2]]);
                if corners.len() == 3 {
                    welder.triangle(corners[0], corners[1], corners[2])?;
                    corners.clear();
                }
            }
            Some(_) | None => {}
        }
    }
    if !any { return Err("not an STL file".into()); }
    if !corners.is_empty() { return Err("the STL has an incomplete triangle".into()); }
    welder.finish()
}

/// Parses STL bytes already in memory (tests and small files).
pub fn parse_stl(bytes: &[u8], unit: Unit) -> Result<Mesh, String> {
    if bytes.len() as u64 > MAX_MESH_FILE_BYTES { return Err("the STL exceeds the import limit".into()); }
    let count = bytes.get(80..84).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let mesh = if let Some(n) = count && bytes.len() == 84 + n * 50 && n > 0 {
        stream_binary_stl(&mut &bytes[84..], n, unit.mm())?
    } else {
        std::str::from_utf8(bytes).map_err(|_| "not an STL file".to_owned())?;
        ascii_stl(BufReader::new(bytes), unit.mm(), bytes.len() / 140)?
    };
    mesh.validate()?;
    Ok(mesh)
}

/// Reads a Wavefront OBJ: vertices and faces, other records ignored, polygons fanned.
pub fn read_obj(path: &Path, unit: Unit) -> Result<Mesh, String> {
    let (file, len) = open(path)?;
    let reader = BufReader::with_capacity(1 << 20, file);
    let scale = unit.mm();
    let mut raw: Vec<[f32; 3]> = Vec::with_capacity(len as usize / 40);
    let mut welder = Welder::new(scale, len as usize / 30);
    let index = |s: &str, n: usize| -> Result<usize, String> {
        let i: i64 = s.split('/').next().unwrap_or("").parse().map_err(|_| format!("the OBJ has a malformed face index: {s}"))?;
        let i = if i < 0 { n as i64 + i } else { i - 1 };
        if i < 0 || i as usize >= n { return Err("the OBJ refers to a vertex it does not have".into()); }
        Ok(i as usize)
    };
    for line in reader.lines() {
        let line = line.map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let mut w = line.split_whitespace();
        match w.next() {
            Some("v") => {
                let c: Vec<f32> = w.take(3).map(str::parse).collect::<Result<_, _>>().map_err(|_| "the OBJ has a malformed vertex")?;
                if c.len() != 3 { return Err("the OBJ has a malformed vertex".into()); }
                raw.push([c[0], c[1], c[2]]);
            }
            Some("f") => {
                let ids: Vec<usize> = w.map(|s| index(s, raw.len())).collect::<Result<_, _>>()?;
                if ids.len() < 3 { return Err("the OBJ has a face with fewer than three corners".into()); }
                for k in 1..ids.len() - 1 {
                    welder.triangle(raw[ids[0]], raw[ids[k]], raw[ids[k + 1]])?;
                }
            }
            _ => {}
        }
    }
    if welder.is_empty() { return Err(format!("{} has no faces", path.display())); }
    welder.finish()
}

// ----- 3MF -----

/// A tag's name and attributes, from a minimal XML scan that is enough for 3MF model files.
struct Tag<'a> { name: &'a str, attrs: Vec<(&'a str, &'a str)>, closing: bool }

fn tags(xml: &str) -> impl Iterator<Item = Tag<'_>> {
    let mut rest = xml;
    std::iter::from_fn(move || {
        loop {
            let start = rest.find('<')?;
            let after = &rest[start + 1..];
            // Skip comments, declarations and processing instructions.
            if after.starts_with("!--") {
                let end = after.find("-->")?;
                rest = &after[end + 3..];
                continue;
            }
            if after.starts_with('?') || after.starts_with('!') {
                let end = after.find('>')?;
                rest = &after[end + 1..];
                continue;
            }
            let end = after.find('>')?;
            let body = &after[..end];
            rest = &after[end + 1..];
            let closing = body.starts_with('/');
            let body = body.trim_start_matches('/').trim_end_matches('/').trim();
            let (name, attr_text) = body.split_once(|c: char| c.is_whitespace()).unwrap_or((body, ""));
            let name = name.rsplit(':').next().unwrap_or(name);
            let mut attrs = Vec::new();
            let mut a = attr_text;
            while let Some(eq) = a.find('=') {
                let key = a[..eq].trim();
                let value = a[eq + 1..].trim_start();
                let Some(quote) = value.chars().next().filter(|q| *q == '"' || *q == '\'') else { break };
                let value = &value[1..];
                let Some(close) = value.find(quote) else { break };
                attrs.push((key.rsplit(':').next().unwrap_or(key), &value[..close]));
                a = &value[close + 1..];
            }
            return Some(Tag { name, attrs, closing });
        }
    })
}

fn attr<'a>(t: &Tag<'a>, key: &str) -> Option<&'a str> {
    t.attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn transform_of(text: Option<&str>) -> Result<DAffine3, String> {
    let Some(text) = text else { return Ok(DAffine3::IDENTITY) };
    let v: Vec<f64> = text.split_whitespace().map(str::parse).collect::<Result<_, _>>().map_err(|_| "the 3MF has a malformed transform")?;
    if v.len() != 12 { return Err("the 3MF has a malformed transform".into()); }
    // 3MF uses row vectors (p' = p M + t), so the rows of its 3x3 are the columns of ours.
    let m = DMat3::from_cols(DVec3::new(v[0], v[1], v[2]), DVec3::new(v[3], v[4], v[5]), DVec3::new(v[6], v[7], v[8]));
    Ok(DAffine3::from_mat3_translation(m, DVec3::new(v[9], v[10], v[11])))
}

#[derive(Default)]
struct Object3mf {
    positions: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
    components: Vec<(String, DAffine3)>,
}

/// Reads a 3MF's meshes, placed as its build says, in millimetres.
pub fn read_3mf(path: &Path) -> Result<Mesh, String> {
    let (file, _) = open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{} is not a 3MF archive: {e}", path.display()))?;
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let model = names.iter().find(|n| n.eq_ignore_ascii_case("3D/3dmodel.model")).or_else(|| names.iter().find(|n| n.to_ascii_lowercase().ends_with(".model"))).ok_or("the 3MF has no model file")?;
    let mut xml = String::new();
    archive.by_name(model).map_err(|e| format!("could not read the 3MF model: {e}"))?.take(MAX_MESH_FILE_BYTES).read_to_string(&mut xml).map_err(|e| format!("could not read the 3MF model: {e}"))?;
    let mut scale = 1.0;
    let mut objects: FxMap<String, Object3mf> = FxMap::default();
    let mut current: Option<(String, Object3mf)> = None;
    let mut build: Vec<(String, DAffine3)> = Vec::new();
    for t in tags(&xml) {
        match (t.name, t.closing) {
            ("model", false) => {
                scale = match attr(&t, "unit").unwrap_or("millimeter") {
                    "micron" => 0.001, "millimeter" => 1.0, "centimeter" => 10.0, "inch" => 25.4, "foot" => 304.8, "meter" => 1000.0,
                    other => return Err(format!("the 3MF uses an unknown unit: {other}")),
                };
            }
            ("object", false) => { current = Some((attr(&t, "id").unwrap_or("").to_owned(), Object3mf::default())); }
            ("object", true) => { if let Some((id, o)) = current.take() { objects.insert(id, o); } }
            ("vertex", false) => {
                let Some((_, o)) = current.as_mut() else { continue };
                let c = ["x", "y", "z"].map(|k| attr(&t, k).and_then(|v| v.parse::<f64>().ok()));
                let [Some(x), Some(y), Some(z)] = c else { return Err("the 3MF has a malformed vertex".into()) };
                o.positions.push(DVec3::new(x, y, z) * scale);
            }
            ("triangle", false) => {
                let Some((_, o)) = current.as_mut() else { continue };
                let c = ["v1", "v2", "v3"].map(|k| attr(&t, k).and_then(|v| v.parse::<u32>().ok()));
                let [Some(a), Some(b), Some(cc)] = c else { return Err("the 3MF has a malformed triangle".into()) };
                if [a, b, cc].iter().any(|i| *i as usize >= o.positions.len()) { return Err("the 3MF refers to a vertex it does not have".into()); }
                o.tris.push([a, b, cc]);
            }
            ("component", false) => {
                let Some((_, o)) = current.as_mut() else { continue };
                o.components.push((attr(&t, "objectid").unwrap_or("").to_owned(), transform_of(attr(&t, "transform"))?));
            }
            ("item", false) => { build.push((attr(&t, "objectid").unwrap_or("").to_owned(), transform_of(attr(&t, "transform"))?)); }
            _ => {}
        }
    }
    if build.is_empty() { build = objects.keys().map(|k| (k.clone(), DAffine3::IDENTITY)).collect(); }
    let mut welder = Welder::new(1.0, objects.values().map(|o| o.tris.len()).sum());
    fn place(objects: &FxMap<String, Object3mf>, id: &str, at: DAffine3, depth: usize, welder: &mut Welder) -> Result<(), String> {
        if depth > 16 { return Err("the 3MF nests components too deeply".into()); }
        let o = objects.get(id).ok_or_else(|| format!("the 3MF build refers to a missing object {id}"))?;
        let scaled = |p: DVec3| at.transform_point3(p).as_vec3().to_array();
        for t in &o.tris {
            welder.triangle(scaled(o.positions[t[0] as usize]), scaled(o.positions[t[1] as usize]), scaled(o.positions[t[2] as usize]))?;
        }
        for (cid, ct) in &o.components {
            place(objects, cid, at * *ct, depth + 1, welder)?;
        }
        Ok(())
    }
    for (id, at) in &build {
        place(&objects, id, *at, 0, &mut welder)?;
    }
    // The transforms are applied with the unit scale already in the positions, so the translation needs it too.
    let _ = scale;
    if welder.is_empty() { return Err(format!("{} has no triangles", path.display())); }
    welder.finish()
}

// ----- container blob -----

/// The binary form of a mesh inside a container: a 32-byte header, f32 positions, u32 indices.
pub fn encode_blob(m: &Mesh) -> Vec<u8> {
    let positions = m.positions();
    let indices = m.indices();
    let mut out = Vec::with_capacity(32 + positions.len() * 12 + indices.len() * 12);
    out.extend_from_slice(MAGIC);
    out.extend((positions.len() as u32).to_le_bytes());
    out.extend((indices.len() as u32).to_le_bytes());
    out.extend((m.is_welded() as u32).to_le_bytes());
    out.extend([0u8; 4]); // crc, filled below
    out.extend([0u8; 8]);
    for p in positions {
        for c in p.as_vec3().to_array() { out.extend(c.to_le_bytes()); }
    }
    for t in indices {
        for i in t { out.extend(i.to_le_bytes()); }
    }
    let crc = crc32fast::hash(&out[32..]);
    out[20..24].copy_from_slice(&crc.to_le_bytes());
    out
}

pub fn decode_blob(bytes: &[u8]) -> Result<Mesh, String> {
    if bytes.len() < 32 || &bytes[..8] != MAGIC { return Err("the mesh blob is not in Ferrender's format".into()); }
    let u = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let (nv, nt, flags, crc) = (u(8) as usize, u(12) as usize, u(16), u(20));
    if nt > MAX_TRIANGLES { return Err(format!("the mesh blob exceeds {} million triangles", MAX_TRIANGLES / 1_000_000)); }
    if nv > nt * 3 { return Err("the mesh blob has more vertices than its triangles can use".into()); }
    if bytes.len() != 32 + nv * 12 + nt * 12 { return Err("the mesh blob is not the size its header says".into()); }
    if crc32fast::hash(&bytes[32..]) != crc { return Err("the mesh blob is corrupt (checksum mismatch)".into()); }
    let f = |i: usize| f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as f64;
    let positions: Vec<DVec3> = (0..nv).map(|k| { let i = 32 + k * 12; DVec3::new(f(i), f(i + 4), f(i + 8)) }).collect();
    let base = 32 + nv * 12;
    let indices: Vec<[u32; 3]> = (0..nt).map(|k| { let i = base + k * 12; [u(i), u(i + 4), u(i + 8)] }).collect();
    Mesh::from_indexed(positions, indices, flags & 1 == 1)
}

/// The pre-release blob form: raw little-endian f32 triangle triples.
pub fn decode_tris_blob(bytes: &[u8]) -> Result<Mesh, String> {
    if bytes.len() % 36 != 0 { return Err("the mesh blob has an incomplete triangle".into()); }
    let f: Vec<f64> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64).collect();
    let tris: Vec<[DVec3; 3]> = f.chunks_exact(9).map(|t| [DVec3::new(t[0], t[1], t[2]), DVec3::new(t[3], t[4], t[5]), DVec3::new(t[6], t[7], t[8])]).collect();
    let m = Mesh::welded_from_tris(&tris);
    m.validate()?;
    Ok(m)
}
