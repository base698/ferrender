//! Triangle meshes and the sweeps that make them from sketch profiles.
//!
//! A mesh is indexed: shared vertex positions and index triples. Meshes
//! that come from files or from mesh operations are *welded*, which means
//! the indices are the topology and no distance test is needed to find
//! neighbours. Meshes swept here or triangulated by the kernel are not,
//! and are welded by distance when their topology is wanted. A bounding
//! volume hierarchy and vertex adjacency are built on first use and kept
//! until the mesh changes, so picking and face spreading stay fast on
//! meshes of millions of triangles.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, OnceLock};

use base64::Engine;
use glam::{DVec2, DVec3, Vec3};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::profile::{Profile, signed_area};
use crate::sketch::{CIRCLE_SEGS, Plane};

/// More triangles than this is refused everywhere a mesh is made or read.
pub const MAX_TRIANGLES: usize = 16_000_000;
/// Inline JSON (the pre-container form) keeps the one-million limit the older readers enforce.
pub const MAX_INLINE_TRIANGLES: usize = 1_000_000;

/// A small, fast hasher for integer keys (the FxHash recipe); vertex welding and edge tables hash tens of millions of keys.
#[derive(Default, Clone, Copy)]
pub struct Fx(u64);

impl Hasher for Fx {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(b));
        }
    }
    fn write_u64(&mut self, i: u64) { self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95); }
    fn write_u32(&mut self, i: u32) { self.write_u64(i as u64); }
    fn write_usize(&mut self, i: usize) { self.write_u64(i as u64); }
}

pub type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    positions: Vec<DVec3>,
    indices: Vec<[u32; 3]>,
    /// For meshes made from exact solids, the kernel face each triangle came from.
    /// Empty otherwise; dropped by anything that re-cuts the triangles.
    pub face_ids: Vec<u64>,
    /// Vertices are shared wherever triangles meet, so the indices are the topology.
    welded: bool,
    bvh: OnceLock<Arc<Bvh>>,
    adjacency: OnceLock<Arc<Adjacency>>,
}

impl PartialEq for Mesh {
    /// Two meshes are equal when they hold the same triangles in the same order, however they are indexed.
    fn eq(&self, other: &Mesh) -> bool {
        self.indices.len() == other.indices.len() && self.face_ids == other.face_ids && self.tris().eq(other.tris())
    }
}

// On disk a mesh is either the legacy inline form, base64 of little-endian
// f32 triangle triples, or a container blob (see `io`). The blob store is a
// thread-local that `io` fills around a save or load, so the document model
// does not need to know where its meshes live.
thread_local! {
    pub(crate) static BLOBS: std::cell::RefCell<BlobStore> = std::cell::RefCell::new(BlobStore::default());
}

#[derive(Default)]
pub(crate) struct BlobStore {
    /// Set by a save that detaches meshes: each serialized mesh is pushed here and a marker takes its place in the JSON.
    pub detaching: bool,
    pub out: Vec<Vec<u8>>,
    /// Set by a load: marker name to decoded mesh.
    pub incoming: HashMap<String, Mesh>,
}

impl Serialize for Mesh {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let detached = BLOBS.with(|b| {
            let mut b = b.borrow_mut();
            if !b.detaching { return None; }
            b.out.push(crate::meshfile::encode_blob(self));
            Some(b.out.len() - 1)
        });
        if let Some(k) = detached {
            let mut m = serde_json::Map::new();
            m.insert("blob".into(), serde_json::Value::String(format!("#{k}")));
            m.insert("triangles".into(), serde_json::Value::from(self.len()));
            return m.serialize(s);
        }
        let mut bytes = Vec::with_capacity(self.indices.len() * 36);
        for v in self.tris().flatten() {
            for c in v.to_array() {
                bytes.extend((c as f32).to_le_bytes());
            }
        }
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

impl<'de> Deserialize<'de> for Mesh {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Mesh, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored { Inline(String), Blob { blob: String } }
        match Stored::deserialize(d)? {
            Stored::Blob { blob } => BLOBS.with(|b| b.borrow_mut().incoming.remove(&blob)).ok_or_else(|| serde::de::Error::custom(format!("the mesh blob {blob} is missing"))),
            Stored::Inline(text) => {
                if text.len() > MAX_INLINE_TRIANGLES * 48 { return Err(serde::de::Error::custom("the inline mesh exceeds one million triangles")); }
                let bytes = base64::engine::general_purpose::STANDARD.decode(text).map_err(serde::de::Error::custom)?;
                if bytes.len() % 36 != 0 { return Err(serde::de::Error::custom("the mesh has an incomplete triangle")); }
                let f: Vec<f64> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64).collect();
                let tris: Vec<[DVec3; 3]> = f.chunks_exact(9).map(|t| [DVec3::new(t[0], t[1], t[2]), DVec3::new(t[3], t[4], t[5]), DVec3::new(t[6], t[7], t[8])]).collect();
                let mesh = Mesh::welded_from_tris(&tris);
                mesh.validate().map_err(serde::de::Error::custom)?;
                Ok(mesh)
            }
        }
    }
}

/// A corner's f32 coordinates as one key, which is how imported vertices are matched.
fn key(p: DVec3) -> u128 {
    // Adding +0.0 turns a negative zero positive, so the two zeros share a vertex.
    let [x, y, z] = p.as_vec3().to_array().map(|c| (c + 0.0).to_bits());
    ((x as u128) << 64) | ((y as u128) << 32) | z as u128
}

impl Mesh {
    // ----- construction and access -----

    /// An unwelded mesh from triangles: every corner is its own vertex.
    pub fn from_tris(tris: Vec<[DVec3; 3]>) -> Mesh {
        let mut m = Mesh::default();
        m.extend(tris);
        m
    }

    /// A welded mesh from triangles, matching corners that are the same point at 32-bit precision.
    pub fn welded_from_tris(tris: &[[DVec3; 3]]) -> Mesh {
        let mut map: FxMap<u128, u32> = FxMap::default();
        map.reserve(tris.len() * 3 / 2);
        let mut positions = Vec::with_capacity(tris.len() * 3 / 2);
        let mut indices = Vec::with_capacity(tris.len());
        for t in tris {
            let mut tri = [0u32; 3];
            for (k, p) in t.iter().enumerate() {
                let p = p.as_vec3().as_dvec3();
                let next = positions.len() as u32;
                tri[k] = *map.entry(key(p)).or_insert_with(|| { positions.push(p); next });
            }
            indices.push(tri);
        }
        Mesh { positions, indices, face_ids: Vec::new(), welded: true, ..Default::default() }
    }

    /// A mesh from shared vertices and index triples; `welded` says the vertices are already merged.
    pub fn from_indexed(positions: Vec<DVec3>, indices: Vec<[u32; 3]>, welded: bool) -> Result<Mesh, String> {
        let m = Mesh { positions, indices, face_ids: Vec::new(), welded, ..Default::default() };
        m.validate()?;
        Ok(m)
    }

    /// Welds an unwelded mesh by exact 32-bit coordinates and drops the triangles that collapse.
    pub fn weld_exact(&mut self) {
        if self.welded { return; }
        let tris: Vec<[DVec3; 3]> = self.tris().collect();
        let mut m = Mesh::welded_from_tris(&tris);
        m.retain_tris(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
        *self = m;
    }

    pub fn is_welded(&self) -> bool { self.welded }
    pub fn positions(&self) -> &[DVec3] { &self.positions }
    pub fn indices(&self) -> &[[u32; 3]] { &self.indices }
    pub fn len(&self) -> usize { self.indices.len() }
    pub fn is_empty(&self) -> bool { self.indices.is_empty() }
    pub fn vertex_count(&self) -> usize { self.positions.len() }

    pub fn tri(&self, i: usize) -> [DVec3; 3] {
        let t = self.indices[i];
        [self.positions[t[0] as usize], self.positions[t[1] as usize], self.positions[t[2] as usize]]
    }

    pub fn tris(&self) -> impl Iterator<Item = [DVec3; 3]> + '_ + ExactSizeIterator {
        self.indices.iter().map(move |t| [self.positions[t[0] as usize], self.positions[t[1] as usize], self.positions[t[2] as usize]])
    }

    fn touched(&mut self) {
        self.bvh = OnceLock::new();
        self.adjacency = OnceLock::new();
    }

    /// Adds one triangle as three new vertices.
    pub fn push(&mut self, t: [DVec3; 3]) {
        let n = self.positions.len() as u32;
        self.positions.extend(t);
        self.indices.push([n, n + 1, n + 2]);
        self.welded = false;
        self.touched();
    }

    pub fn extend(&mut self, tris: impl IntoIterator<Item = [DVec3; 3]>) {
        for t in tris {
            let n = self.positions.len() as u32;
            self.positions.extend(t);
            self.indices.push([n, n + 1, n + 2]);
        }
        self.welded = false;
        self.touched();
    }

    /// Appends another mesh's triangles, keeping its vertices separate.
    pub fn append(&mut self, other: &Mesh) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.indices.extend(other.indices.iter().map(|t| t.map(|i| i + base)));
        self.welded = false;
        self.touched();
    }

    /// The first `n` triangles as their own mesh.
    pub fn head(&self, n: usize) -> Mesh {
        if n >= self.len() { return self.clone(); }
        let mut m = Mesh { positions: Vec::new(), indices: Vec::new(), face_ids: self.face_ids.iter().take(n).copied().collect(), welded: self.welded, ..Default::default() };
        if self.welded {
            // Keep the shared vertices; compact them to those still used.
            let mut remap: Vec<u32> = vec![u32::MAX; self.positions.len()];
            for t in &self.indices[..n] {
                let mut tri = [0u32; 3];
                for (k, i) in t.iter().enumerate() {
                    if remap[*i as usize] == u32::MAX {
                        remap[*i as usize] = m.positions.len() as u32;
                        m.positions.push(self.positions[*i as usize]);
                    }
                    tri[k] = remap[*i as usize];
                }
                m.indices.push(tri);
            }
        } else {
            m.extend(self.tris().take(n));
            m.welded = false;
        }
        m
    }

    pub fn retain_tris(&mut self, mut keep: impl FnMut(&[u32; 3]) -> bool) {
        let faces = self.face_ids.len() == self.indices.len();
        let mut kept_ids = Vec::new();
        let mut i = 0;
        self.indices.retain(|t| {
            let k = keep(t);
            if k && faces { kept_ids.push(self.face_ids[i]); }
            i += 1;
            k
        });
        if faces { self.face_ids = kept_ids; } else { self.face_ids.clear(); }
        self.touched();
    }

    /// Drops vertices no triangle uses and renumbers.
    pub fn compact(&mut self) {
        let mut remap: Vec<u32> = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::with_capacity(self.positions.len());
        for t in &mut self.indices {
            for i in t.iter_mut() {
                if remap[*i as usize] == u32::MAX {
                    remap[*i as usize] = positions.len() as u32;
                    positions.push(self.positions[*i as usize]);
                }
                *i = remap[*i as usize];
            }
        }
        self.positions = positions;
        self.touched();
    }

    /// Rounds every coordinate to what a 32-bit float holds, which is how meshes are stored.
    pub fn snap(&mut self) {
        for p in &mut self.positions { *p = p.as_vec3().as_dvec3(); }
        self.touched();
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.indices.len() > MAX_TRIANGLES { return Err(format!("the mesh exceeds {} million triangles", MAX_TRIANGLES / 1_000_000)); }
        if self.positions.len() > u32::MAX as usize - 3 { return Err("the mesh has too many vertices".into()); }
        if self.indices.iter().flatten().any(|i| *i as usize >= self.positions.len()) { return Err("the mesh refers to a vertex it does not have".into()); }
        if !self.face_ids.is_empty() && self.face_ids.len() != self.indices.len() { return Err("the mesh's face ids do not match its triangles".into()); }
        if self.positions.iter().any(|p| !p.is_finite() || !p.as_vec3().is_finite()) {
            return Err("mesh coordinates must be finite and representable as 32-bit floats".into());
        }
        Ok(())
    }

    // ----- measures -----

    pub fn bbox(&self) -> Option<(DVec3, DVec3)> {
        if self.indices.is_empty() { return None; }
        let used: Box<dyn Iterator<Item = DVec3> + '_> = if self.welded { Box::new(self.positions.iter().copied()) } else { Box::new(self.tris().flatten()) };
        let mut it = used;
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// Enclosed volume; negative when the mesh is inside out.
    pub fn volume(&self) -> f64 {
        let o = self.positions.first().copied().unwrap_or(DVec3::ZERO);
        self.tris().map(|t| (t[0] - o).dot((t[1] - o).cross(t[2] - o))).sum::<f64>() / 6.0
    }

    pub fn area(&self) -> f64 {
        self.tris().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).length()).sum::<f64>() / 2.0
    }

    pub fn normal(&self, i: usize) -> DVec3 {
        let t = self.tri(i);
        (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero()
    }

    pub fn flip(&mut self) {
        for t in &mut self.indices {
            t.swap(1, 2);
        }
        self.touched();
    }

    pub fn map(&mut self, f: impl Fn(DVec3) -> DVec3) {
        for v in &mut self.positions {
            *v = f(*v);
        }
        self.touched();
    }

    /// Moves vertices individually; `f` gets the vertex index and its position.
    pub fn map_indexed(&mut self, mut f: impl FnMut(usize, DVec3) -> DVec3) {
        for (i, v) in self.positions.iter_mut().enumerate() {
            *v = f(i, *v);
        }
        self.touched();
    }

    pub fn set_position(&mut self, i: usize, p: DVec3) {
        self.positions[i] = p;
        self.touched();
    }

    // ----- acceleration structures -----

    pub fn bvh(&self) -> Arc<Bvh> {
        self.bvh.get_or_init(|| Arc::new(Bvh::build(self))).clone()
    }

    pub fn adjacency(&self) -> Arc<Adjacency> {
        self.adjacency.get_or_init(|| Arc::new(Adjacency::build(self))).clone()
    }

    /// Nearest triangle hit by a ray: distance along it and the triangle's index.
    pub fn ray(&self, origin: DVec3, dir: DVec3) -> Option<(f64, usize)> {
        if self.indices.is_empty() { return None; }
        if self.indices.len() < 64 {
            let mut best: Option<(f64, usize)> = None;
            for i in 0..self.indices.len() {
                if let Some(d) = ray_tri(origin, dir, &self.tri(i)) && best.is_none_or(|b| d < b.0) {
                    best = Some((d, i));
                }
            }
            return best;
        }
        self.bvh().ray(self, origin, dir)
    }

    /// The triangle nearest to a point.
    pub fn nearest_tri(&self, p: DVec3) -> Option<usize> {
        if self.indices.is_empty() { return None; }
        if self.indices.len() < 64 {
            return (0..self.indices.len()).min_by(|a, b| dist_tri(p, &self.tri(*a)).total_cmp(&dist_tri(p, &self.tri(*b))));
        }
        self.bvh().nearest(self, p).map(|n| n.1)
    }

    /// Shared vertices and index triangles, merging vertices closer than `eps`.
    /// A welded mesh is returned as it is.
    pub(crate) fn weld(&self, eps: f64) -> (Vec<DVec3>, Vec<[usize; 3]>) {
        if self.welded {
            return (self.positions.clone(), self.indices.iter().map(|t| t.map(|i| i as usize)).collect());
        }
        let cell = |p: DVec3| ((p.x / eps).floor() as i64, (p.y / eps).floor() as i64, (p.z / eps).floor() as i64);
        let mut grid: FxMap<(i64, i64, i64), Vec<usize>> = FxMap::default();
        let mut verts: Vec<DVec3> = Vec::new();
        let mut index = |p: DVec3| {
            let c = cell(p);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(&i) = grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz)).and_then(|l| l.iter().find(|i| verts[**i].distance(p) < eps)) {
                            return i;
                        }
                    }
                }
            }
            verts.push(p);
            grid.entry(c).or_default().push(verts.len() - 1);
            verts.len() - 1
        };
        let tris = self.tris().map(|t| [index(t[0]), index(t[1]), index(t[2])]).collect();
        (verts, tris)
    }

    /// Directed edges whose opposite is missing.
    fn unmatched(tris: &[[usize; 3]]) -> Vec<(usize, usize, usize)> {
        let flat = |t: &[usize; 3]| t[0] == t[1] || t[1] == t[2] || t[0] == t[2];
        let mut count: FxMap<(usize, usize), i32> = FxMap::default();
        for t in tris.iter().filter(|t| !flat(t)) {
            for i in 0..3 {
                *count.entry((t[i], t[(i + 1) % 3])).or_insert(0) += 1;
            }
        }
        let mut out = Vec::new();
        for (ti, t) in tris.iter().enumerate().filter(|(_, t)| !flat(t)) {
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                if count.get(&(b, a)).copied().unwrap_or(0) < count[&(a, b)] {
                    out.push((ti, a, b));
                }
            }
        }
        out
    }

    /// Triangle edges with no matching neighbour; zero for a watertight mesh.
    pub fn open_edges(&self) -> usize {
        if self.welded { return self.inspect().open_edges; }
        Self::unmatched(&self.weld(1e-5).1).len()
    }

    /// Closes the T-junctions that booleans leave: where a vertex of one
    /// triangle sits partway along another's edge, that edge is split there.
    pub fn stitch(&mut self) {
        let (verts, mut tris) = self.weld(1e-5);
        tris.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
        for _ in 0..6 {
            let open = Self::unmatched(&tris);
            if open.is_empty() {
                break;
            }
            let mut loose: Vec<usize> = open.iter().flat_map(|e| [e.1, e.2]).collect();
            loose.sort();
            loose.dedup();
            // For each triangle, the vertices found along one of its open edges.
            let mut splits: HashMap<usize, (usize, usize, Vec<(f64, usize)>)> = HashMap::new();
            for (ti, a, b) in open {
                if splits.contains_key(&ti) {
                    continue;
                }
                let (pa, d) = (verts[a], verts[b] - verts[a]);
                let len2 = d.length_squared();
                let on: Vec<(f64, usize)> = loose
                    .iter()
                    .filter(|v| **v != a && **v != b)
                    .filter_map(|v| {
                        let t = (verts[*v] - pa).dot(d) / len2;
                        (t > 1e-9 && t < 1.0 - 1e-9 && (pa + d * t).distance(verts[*v]) < 2e-5).then_some((t, *v))
                    })
                    .collect();
                if !on.is_empty() {
                    splits.insert(ti, (a, b, on));
                }
            }
            if splits.is_empty() {
                break;
            }
            let mut next = Vec::with_capacity(tris.len() + splits.len() * 2);
            for (ti, t) in tris.iter().enumerate() {
                match splits.remove(&ti) {
                    None => next.push(*t),
                    Some((a, b, mut on)) => {
                        on.sort_by(|x, y| x.0.total_cmp(&y.0));
                        let c = *t.iter().find(|v| **v != a && **v != b).unwrap();
                        let chain: Vec<usize> = std::iter::once(a).chain(on.iter().map(|o| o.1)).chain([b]).collect();
                        next.extend(chain.windows(2).map(|w| [w[0], w[1], c]));
                    }
                }
            }
            tris = next;
        }
        self.positions = verts;
        self.indices = tris.iter().map(|t| t.map(|i| i as u32)).collect();
        self.welded = true;
        self.face_ids.clear();
        self.drop_slivers();
    }

    /// The triangles of the face containing triangle `tri`: the patch of
    /// surface the viewport outlines, spreading across edges that turn gently.
    /// On a large mesh with no kernel faces the spread stops at `FACE_BUDGET`
    /// triangles, since a scan has no faces worth outlining whole.
    pub fn face(&self, tri: usize) -> Vec<usize> {
        // Kernel face ids are not unique (the two ends of an extrude share one), so they
        // only narrow the spread: it stays on one kernel face, and within it turns freely.
        let ids = (self.face_ids.len() == self.indices.len()).then_some(&self.face_ids);
        let adj = self.adjacency();
        let budget = if ids.is_none() && self.len() > FACE_BUDGET { FACE_BUDGET } else { usize::MAX };
        let mut seen = vec![false; self.indices.len()];
        let (mut out, mut queue) = (Vec::new(), vec![tri]);
        seen[tri] = true;
        let n_tri = self.normal(tri);
        while let Some(t) = queue.pop() {
            out.push(t);
            if out.len() >= budget { break; }
            let nt = self.normal(t);
            for o in adj.neighbours(t) {
                if !seen[o] && ids.map_or(self.normal(o).dot(nt) > 0.94, |ids| ids[o] == ids[tri]) {
                    seen[o] = true;
                    queue.push(o);
                }
            }
        }
        let _ = n_tri;
        out.sort();
        out
    }

    /// For meshes made from exact solids, which face each triangle belongs to:
    /// one number per connected kernel face. Renderers outline where it changes.
    pub fn groups(&self) -> Option<Vec<u32>> {
        if self.face_ids.len() != self.indices.len() || self.indices.is_empty() {
            return None;
        }
        let adj = self.adjacency();
        let mut group = vec![u32::MAX; self.indices.len()];
        let mut next = 0;
        for start in 0..self.indices.len() {
            if group[start] != u32::MAX {
                continue;
            }
            let mut queue = vec![start];
            group[start] = next;
            while let Some(t) = queue.pop() {
                for o in adj.neighbours(t) {
                    if group[o] == u32::MAX && self.face_ids[o] == self.face_ids[t] {
                        group[o] = next;
                        queue.push(o);
                    }
                }
            }
            next += 1;
        }
        Some(group)
    }

    /// A point on a face and its normal, if the face is flat.
    pub fn face_plane(&self, tris: &[usize]) -> Option<(DVec3, DVec3)> {
        let n = self.normal(*tris.first()?);
        tris.iter().all(|t| self.normal(*t).dot(n) > 0.99999).then_some((self.tri(tris[0])[0], n))
    }

    pub fn face_area(&self, tris: &[usize]) -> f64 {
        tris.iter().map(|t| { let t = self.tri(*t); (t[1] - t[0]).cross(t[2] - t[0]).length() }).sum::<f64>() / 2.0
    }

    /// The closed outlines of a face, outer boundary and holes alike.
    pub fn face_loops(&self, tris: &[usize]) -> Vec<Vec<DVec3>> {
        let adj = self.adjacency();
        let directed: Vec<(u32, u32)> = tris.iter().flat_map(|t| { let v = adj.verts(*t); (0..3).map(move |i| (v[i], v[(i + 1) % 3])) }).filter(|e| e.0 != e.1).collect();
        let all: std::collections::HashSet<(u32, u32), BuildHasherDefault<Fx>> = directed.iter().copied().collect();
        let mut next: std::collections::BTreeMap<u32, u32> = directed.iter().filter(|e| !all.contains(&(e.1, e.0))).copied().collect();
        let mut loops = Vec::new();
        while let Some(&start) = next.keys().next() {
            let (mut at, mut ring) = (start, Vec::new());
            while let Some(to) = next.remove(&at) {
                ring.push(adj.position(at));
                at = to;
            }
            // Drop the points that only divide a straight side.
            let n = ring.len();
            let kept: Vec<DVec3> = (0..n).filter(|i| (ring[*i] - ring[(i + n - 1) % n]).normalize_or_zero().cross((ring[(i + 1) % n] - ring[*i]).normalize_or_zero()).length() > 1e-7).map(|i| ring[i]).collect();
            if at == start && kept.len() >= 3 {
                loops.push(kept);
            }
        }
        loops
    }

    fn drop_slivers(&mut self) {
        self.face_ids.clear();
        let positions = std::mem::take(&mut self.positions);
        self.indices.retain(|t| {
            let (a, b, c) = (positions[t[0] as usize], positions[t[1] as usize], positions[t[2] as usize]);
            (b - a).cross(c - a).length_squared() > 1e-20
        });
        self.positions = positions;
        self.touched();
    }

    // ----- repair and inspection -----

    /// What a mesh looks like topologically, without changing it.
    pub fn inspect(&self) -> Report {
        let mut copy = self.clone();
        copy.repair_inner(false)
    }

    /// Removes degenerate and duplicate triangles, makes neighbouring
    /// triangles face the same way and turns closed shells outward.
    /// Returns what was found and done.
    pub fn repair(&mut self) -> Report {
        let r = self.repair_inner(true);
        self.touched();
        r
    }

    fn repair_inner(&mut self, fix: bool) -> Report {
        if !self.welded { self.weld_exact(); }
        let mut report = Report { vertices: self.positions.len(), ..Default::default() };
        // Degenerate triangles: repeated vertices or no area.
        let before = self.indices.len();
        let positions = std::mem::take(&mut self.positions);
        self.indices.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2] && {
            let (a, b, c) = (positions[t[0] as usize], positions[t[1] as usize], positions[t[2] as usize]);
            (b - a).cross(c - a).length_squared() > 0.0
        });
        self.positions = positions;
        report.degenerate_removed = before - self.indices.len();
        // Duplicates: the same three vertices, either way round.
        let mut keyed: Vec<(u128, u32)> = self.indices.iter().enumerate().map(|(i, t)| { let mut s = *t; s.sort(); (((s[0] as u128) << 64) | ((s[1] as u128) << 32) | s[2] as u128, i as u32) }).collect();
        keyed.sort_unstable();
        let mut doomed = vec![false; self.indices.len()];
        for w in keyed.windows(2) {
            if w[0].0 == w[1].0 { doomed[w[1].1 as usize] = true; }
        }
        report.duplicates_removed = doomed.iter().filter(|d| **d).count();
        if report.duplicates_removed > 0 {
            let mut i = 0;
            self.indices.retain(|_| { let k = !doomed[i]; i += 1; k });
        }
        drop(keyed);
        // The edge table: every directed edge, sorted by its undirected key.
        let n = self.indices.len();
        let mut edges: Vec<(u64, u32)> = Vec::with_capacity(n * 3);
        for (i, t) in self.indices.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let (lo, hi, forward) = if a < b { (a, b, 0) } else { (b, a, 1) };
                edges.push((((lo as u64) << 32) | hi as u64, (i as u32) << 1 | forward));
            }
        }
        edges.sort_unstable();
        // Pairs of triangles that share a manifold edge, and whether they disagree about its direction.
        let mut pairs: Vec<(u32, u32, bool)> = Vec::new();
        let mut i = 0;
        while i < edges.len() {
            let mut j = i + 1;
            while j < edges.len() && edges[j].0 == edges[i].0 { j += 1; }
            match j - i {
                1 => report.open_edges += 1,
                2 => pairs.push((edges[i].1 >> 1, edges[i + 1].1 >> 1, (edges[i].1 & 1) == (edges[i + 1].1 & 1))),
                _ => report.non_manifold_edges += 1,
            }
            i = j;
        }
        drop(edges);
        // Adjacency through the pairs, as CSR.
        let mut count = vec![0u32; n + 1];
        for p in &pairs { count[p.0 as usize + 1] += 1; count[p.1 as usize + 1] += 1; }
        for k in 1..=n { count[k] += count[k - 1]; }
        let mut fill = count.clone();
        let mut list: Vec<(u32, bool)> = vec![(0, false); pairs.len() * 2];
        for p in &pairs {
            list[fill[p.0 as usize] as usize] = (p.1, p.2); fill[p.0 as usize] += 1;
            list[fill[p.1 as usize] as usize] = (p.0, p.2); fill[p.1 as usize] += 1;
        }
        drop(pairs);
        // Walk each component, flipping triangles that disagree with the one they were reached from.
        let mut state = vec![0u8; n]; // 0 unseen, 1 keep, 2 flip
        let mut component = vec![u32::MAX; n];
        let mut stack = Vec::new();
        for start in 0..n {
            if state[start] != 0 { continue; }
            let c = report.components as u32;
            report.components += 1;
            state[start] = 1;
            component[start] = c;
            stack.push(start);
            while let Some(t) = stack.pop() {
                let flipped = state[t] == 2;
                for &(o, disagree) in &list[count[t] as usize..count[t + 1] as usize] {
                    if state[o as usize] == 0 {
                        state[o as usize] = if flipped != disagree { 2 } else { 1 };
                        component[o as usize] = c;
                        stack.push(o as usize);
                    }
                }
            }
        }
        report.flipped = state.iter().filter(|s| **s == 2).count();
        if fix {
            for (t, s) in self.indices.iter_mut().zip(&state) {
                if *s == 2 { t.swap(1, 2); }
            }
        }
        // Closed components turned inside out are turned outward.
        if report.open_edges == 0 && report.non_manifold_edges == 0 {
            let mut vol = vec![0.0f64; report.components];
            let o = self.positions.first().copied().unwrap_or(DVec3::ZERO);
            for (i, t) in self.indices.iter().enumerate() {
                let (a, b, c) = (self.positions[t[0] as usize] - o, self.positions[t[1] as usize] - o, self.positions[t[2] as usize] - o);
                let v = a.dot(b.cross(c));
                vol[component[i] as usize] += if state[i] == 2 && !fix { -v } else { v };
            }
            let inside_out: Vec<bool> = vol.iter().map(|v| *v < 0.0).collect();
            let turned = self.indices.iter().enumerate().filter(|(i, _)| inside_out[component[*i] as usize]).count();
            if turned > 0 {
                report.flipped += turned;
                if fix {
                    for (i, t) in self.indices.iter_mut().enumerate() {
                        if inside_out[component[i] as usize] { t.swap(1, 2); }
                    }
                }
            }
        }
        report.triangles = self.indices.len();
        report.watertight = report.open_edges == 0 && report.non_manifold_edges == 0 && report.triangles > 0;
        if fix && (report.degenerate_removed > 0 || report.duplicates_removed > 0) {
            self.compact();
            report.vertices = self.positions.len();
        }
        report
    }

    /// Welds vertices that fall in the same cell of a grid `cell` wide and
    /// drops the triangles that collapse: a fast, coarse simplification used
    /// for level of detail. Quality is low where the grid cuts across detail.
    pub fn clustered(&self, cell: f64) -> Mesh {
        let mut map: FxMap<(i32, i32, i32), u32> = FxMap::default();
        let mut sums: Vec<(DVec3, u32)> = Vec::new();
        let mut remap: Vec<u32> = Vec::with_capacity(self.positions.len());
        let cell = cell.max(1e-9);
        for p in &self.positions {
            let k = ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32, (p.z / cell).floor() as i32);
            let next = sums.len() as u32;
            let i = *map.entry(k).or_insert_with(|| { sums.push((DVec3::ZERO, 0)); next });
            sums[i as usize].0 += *p;
            sums[i as usize].1 += 1;
            remap.push(i);
        }
        let positions: Vec<DVec3> = sums.iter().map(|(s, n)| *s / *n as f64).collect();
        let mut indices: Vec<[u32; 3]> = self.indices.iter().map(|t| t.map(|i| remap[i as usize])).filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]).collect();
        indices.retain(|t| {
            let (a, b, c) = (positions[t[0] as usize], positions[t[1] as usize], positions[t[2] as usize]);
            (b - a).cross(c - a).length_squared() > 0.0
        });
        let mut m = Mesh { positions, indices, face_ids: Vec::new(), welded: true, ..Default::default() };
        m.compact();
        m
    }
}

impl Mesh {
    /// A coarse copy of roughly `target` triangles, by vertex clustering.
    pub fn clustered_to(&self, target: usize) -> Mesh {
        // A surface's clusters are about one per cell it crosses, two triangles each.
        let cell = (2.0 * self.area() / target.max(1) as f64).sqrt();
        self.clustered(cell)
    }
}

/// Face spreading on a kernel-less mesh stops here, so a click on a scan stays responsive.
pub const FACE_BUDGET: usize = 20_000;

/// What import and repair find out about a mesh.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Report {
    pub triangles: usize,
    pub vertices: usize,
    pub degenerate_removed: usize,
    pub duplicates_removed: usize,
    /// Triangles turned round to agree with their neighbours or to face outward.
    pub flipped: usize,
    pub open_edges: usize,
    pub non_manifold_edges: usize,
    pub components: usize,
    pub watertight: bool,
}

impl Report {
    /// One line for a toast or a log.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} triangles, {} vertices, {} shell{}", self.triangles, self.vertices, self.components, if self.components == 1 { "" } else { "s" })];
        if self.watertight { parts.push("watertight".into()); } else {
            if self.open_edges > 0 { parts.push(format!("{} open edges", self.open_edges)); }
            if self.non_manifold_edges > 0 { parts.push(format!("{} non-manifold edges", self.non_manifold_edges)); }
        }
        let fixed = self.degenerate_removed + self.duplicates_removed;
        if fixed > 0 { parts.push(format!("{fixed} bad triangles removed")); }
        if self.flipped > 0 { parts.push(format!("{} triangles turned round", self.flipped)); }
        parts.join(", ")
    }
}

// ----- geometry helpers -----

/// Möller–Trumbore: distance along the ray to the triangle, if it is hit.
pub fn ray_tri(origin: DVec3, dir: DVec3, t: &[DVec3; 3]) -> Option<f64> {
    let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let s = origin - t[0];
    let u = s.dot(p) / det;
    let q = s.cross(e1);
    let v = dir.dot(q) / det;
    if u < 0.0 || v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(e2.dot(q) / det)
}

/// Distance from a point to a triangle.
pub fn dist_tri(p: DVec3, t: &[DVec3; 3]) -> f64 {
    // Closest point on the triangle: inside its plane footprint, or on an edge.
    let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
    let q = p - n * (p - t[0]).dot(n);
    let inside = n != DVec3::ZERO && (0..3).all(|i| (t[(i + 1) % 3] - t[i]).cross(q - t[i]).dot(n) >= 0.0);
    if inside {
        return q.distance(p);
    }
    (0..3)
        .map(|i| {
            let (a, d) = (t[i], t[(i + 1) % 3] - t[i]);
            (a + d * ((p - a).dot(d) / d.length_squared().max(1e-18)).clamp(0.0, 1.0)).distance(p)
        })
        .fold(f64::MAX, f64::min)
}

// ----- bounding volume hierarchy -----

#[derive(Debug, Clone, Copy)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    /// A leaf's first triangle in `order`, or an inner node's right child (the left child follows the node).
    start: u32,
    /// Triangles in a leaf; zero for an inner node.
    count: u32,
}

/// A tree of triangle bounds, for ray and nearest-point queries.
#[derive(Debug)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn build(m: &Mesh) -> Bvh {
        let n = m.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let centroids: Vec<Vec3> = m.tris().map(|t| ((t[0] + t[1] + t[2]) / 3.0).as_vec3()).collect();
        let bounds: Vec<(Vec3, Vec3)> = m.tris().map(|t| {
            let (lo, hi) = (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
            // f32 bounds are padded so no triangle leaks out through rounding.
            (lo.as_vec3() - Vec3::splat(1e-5) - lo.as_vec3().abs() * 1e-6, hi.as_vec3() + Vec3::splat(1e-5) + hi.as_vec3().abs() * 1e-6)
        }).collect();
        let mut nodes = Vec::with_capacity(2 * n / LEAF + 1);
        Self::split(&mut nodes, &mut order, 0, n, &centroids, &bounds);
        Bvh { nodes, order }
    }

    fn split(nodes: &mut Vec<Node>, order: &mut [u32], from: usize, to: usize, centroids: &[Vec3], bounds: &[(Vec3, Vec3)]) {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let (mut clo, mut chi) = (lo, hi);
        for &i in &order[from..to] {
            lo = lo.min(bounds[i as usize].0);
            hi = hi.max(bounds[i as usize].1);
            clo = clo.min(centroids[i as usize]);
            chi = chi.max(centroids[i as usize]);
        }
        let me = nodes.len();
        nodes.push(Node { lo, hi, start: from as u32, count: (to - from) as u32 });
        if to - from <= LEAF {
            return;
        }
        let ext = chi - clo;
        let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
        if ext[axis] <= 0.0 {
            return;
        }
        let mid = (to - from) / 2;
        order[from..to].select_nth_unstable_by(mid, |a, b| centroids[*a as usize][axis].total_cmp(&centroids[*b as usize][axis]));
        Self::split(nodes, order, from, from + mid, centroids, bounds);
        nodes[me].count = 0;
        nodes[me].start = nodes.len() as u32;
        Self::split(nodes, order, from + mid, to, centroids, bounds);
    }

    pub fn node_count(&self) -> usize { self.nodes.len() }

    fn slab(n: &Node, o: Vec3, inv: Vec3) -> Option<f32> {
        let t0 = (n.lo - o) * inv;
        let t1 = (n.hi - o) * inv;
        let tmin = t0.min(t1).max_element().max(0.0);
        let tmax = t0.max(t1).min_element();
        (tmax >= tmin).then_some(tmin)
    }

    pub fn ray(&self, m: &Mesh, origin: DVec3, dir: DVec3) -> Option<(f64, usize)> {
        if self.nodes.is_empty() { return None; }
        let o = origin.as_vec3();
        let inv = Vec3::new(1.0 / dir.x as f32, 1.0 / dir.y as f32, 1.0 / dir.z as f32);
        let mut best: Option<(f64, usize)> = None;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni];
            let Some(tmin) = Self::slab(n, o, inv) else { continue };
            if best.is_some_and(|b| tmin as f64 > b.0 + 1e-4) { continue; }
            if n.count > 0 {
                for &i in &self.order[n.start as usize..(n.start + n.count) as usize] {
                    if let Some(d) = ray_tri(origin, dir, &m.tri(i as usize)) && d >= 0.0 && best.is_none_or(|b| d < b.0) {
                        best = Some((d, i as usize));
                    }
                }
            } else {
                stack.push(ni + 1);
                stack.push(n.start as usize);
            }
        }
        best
    }

    fn box_dist(n: &Node, p: Vec3) -> f32 {
        (p.max(n.lo).min(n.hi) - p).length()
    }

    /// The nearest triangle to a point: its distance and index.
    pub fn nearest(&self, m: &Mesh, p: DVec3) -> Option<(f64, usize)> {
        if self.nodes.is_empty() { return None; }
        let pf = p.as_vec3();
        let mut best: Option<(f64, usize)> = None;
        let mut stack = vec![(0usize, 0.0f32)];
        while let Some((ni, d)) = stack.pop() {
            if best.is_some_and(|b| d as f64 > b.0 + 1e-6) { continue; }
            let n = &self.nodes[ni];
            if n.count > 0 {
                for &i in &self.order[n.start as usize..(n.start + n.count) as usize] {
                    let dt = dist_tri(p, &m.tri(i as usize));
                    if best.is_none_or(|b| dt < b.0) { best = Some((dt, i as usize)); }
                }
            } else {
                let (l, r) = (ni + 1, n.start as usize);
                let (dl, dr) = (Self::box_dist(&self.nodes[l], pf), Self::box_dist(&self.nodes[r], pf));
                // Nearer child on top of the stack.
                if dl < dr { stack.push((r, dr)); stack.push((l, dl)); } else { stack.push((l, dl)); stack.push((r, dr)); }
            }
        }
        best
    }

    /// Triangles whose bounds overlap a box.
    pub fn in_box(&self, lo: DVec3, hi: DVec3) -> Vec<usize> {
        let (lo, hi) = (lo.as_vec3(), hi.as_vec3());
        let mut out = Vec::new();
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni];
            if n.lo.cmpgt(hi).any() || n.hi.cmplt(lo).any() { continue; }
            if n.count > 0 {
                out.extend(self.order[n.start as usize..(n.start + n.count) as usize].iter().map(|i| *i as usize));
            } else {
                stack.push(ni + 1);
                stack.push(n.start as usize);
            }
        }
        out
    }

    /// Triangles within `r` of a point (by bounds, so a few more than exactly).
    pub fn near(&self, p: DVec3, r: f64) -> Vec<usize> {
        self.in_box(p - DVec3::splat(r), p + DVec3::splat(r))
    }
}

// ----- adjacency -----

/// Which triangles meet at each vertex, for walking a mesh. For an unwelded
/// mesh the vertices are first merged by distance.
#[derive(Debug)]
pub struct Adjacency {
    /// The merged vertex positions.
    verts: Vec<DVec3>,
    /// Each triangle's merged vertices.
    tris: Vec<[u32; 3]>,
    offsets: Vec<u32>,
    list: Vec<u32>,
}

impl Adjacency {
    pub fn build(m: &Mesh) -> Adjacency {
        let (verts, tris): (Vec<DVec3>, Vec<[u32; 3]>) = if m.welded {
            (m.positions.clone(), m.indices.clone())
        } else {
            let (v, t) = m.weld(1e-5);
            (v, t.iter().map(|t| t.map(|i| i as u32)).collect())
        };
        let mut offsets = vec![0u32; verts.len() + 1];
        for t in &tris {
            for i in t { offsets[*i as usize + 1] += 1; }
        }
        for i in 1..offsets.len() { offsets[i] += offsets[i - 1]; }
        let mut fill = offsets.clone();
        let mut list = vec![0u32; tris.len() * 3];
        for (ti, t) in tris.iter().enumerate() {
            for i in t {
                list[fill[*i as usize] as usize] = ti as u32;
                fill[*i as usize] += 1;
            }
        }
        Adjacency { verts, tris, offsets, list }
    }

    pub fn vertex_count(&self) -> usize { self.verts.len() }
    pub fn position(&self, v: u32) -> DVec3 { self.verts[v as usize] }
    pub fn verts(&self, tri: usize) -> [u32; 3] { self.tris[tri] }

    /// The triangles that use a vertex.
    pub fn tris_of(&self, v: u32) -> &[u32] {
        &self.list[self.offsets[v as usize] as usize..self.offsets[v as usize + 1] as usize]
    }

    /// The triangles sharing an edge with `tri`.
    pub fn neighbours(&self, tri: usize) -> impl Iterator<Item = usize> + '_ {
        let t = self.tris[tri];
        (0..3).flat_map(move |k| {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            self.tris_of(a).iter().copied().filter(move |o| *o as usize != tri && self.tris[*o as usize].contains(&b)).map(|o| o as usize)
        })
    }

    /// The vertices joined to `v` by an edge.
    pub fn ring(&self, v: u32) -> Vec<u32> {
        let mut out: Vec<u32> = self.tris_of(v).iter().flat_map(|t| self.tris[*t as usize]).filter(|o| *o != v).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Whether a vertex lies on an open edge.
    pub fn on_boundary(&self, v: u32) -> bool {
        // A vertex is interior when every edge out of it is used by two of its triangles.
        let tris = self.tris_of(v);
        for t in tris {
            let tri = self.tris[*t as usize];
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                if a != v && b != v { continue; }
                let other = if a == v { b } else { a };
                let uses = tris.iter().filter(|o| self.tris[**o as usize].contains(&other)).count();
                if uses < 2 { return true; }
            }
        }
        false
    }
}

/// The boundary segments and cap triangles of a set of profiles. Segments
/// shared by two selected profiles cancel, so neighbours merge into one solid.
fn outline(profiles: &[&Profile]) -> Result<(Vec<(DVec2, DVec2)>, Vec<[DVec2; 3]>), String> {
    let key = |p: DVec2| ((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64);
    let mut segs: HashMap<_, (DVec2, DVec2)> = HashMap::new();
    let mut caps = Vec::new();
    for p in profiles {
        let mut flat = Vec::new();
        let mut starts = Vec::new();
        for (i, ring) in std::iter::once(&p.outer).chain(&p.holes).enumerate() {
            if i > 0 {
                starts.push(flat.len() / 2);
            }
            for j in 0..ring.len() {
                let (a, b) = (ring[j], ring[(j + 1) % ring.len()]);
                flat.extend([a.x, a.y]);
                if segs.remove(&(key(b), key(a))).is_none() {
                    segs.insert((key(a), key(b)), (a, b));
                }
            }
        }
        let idx = earcutr::earcut(&flat, &starts, 2).map_err(|e| format!("could not triangulate the profile: {e:?}"))?;
        for t in idx.chunks_exact(3) {
            let v = |i: usize| DVec2::new(flat[i * 2], flat[i * 2 + 1]);
            let mut tri = [v(t[0]), v(t[1]), v(t[2])];
            if signed_area(&tri) < 0.0 {
                tri.swap(1, 2);
            }
            caps.push(tri);
        }
    }
    if caps.is_empty() {
        return Err("the profile has no area".into());
    }
    let mut segs: Vec<_> = segs.into_iter().collect();
    segs.sort_by_key(|s| s.0);
    Ok((segs.into_iter().map(|s| s.1).collect(), caps))
}

/// A profile's area as triangles, for drawing it filled.
pub fn triangulate(p: &Profile) -> Vec<[DVec2; 3]> {
    outline(&[p]).map(|o| o.1).unwrap_or_default()
}

fn quad(m: &mut Mesh, a: DVec3, b: DVec3, c: DVec3, d: DVec3) {
    m.push([a, b, c]);
    m.push([a, c, d]);
}

/// Sweeps profiles along the plane normal from offset `z0` to `z1`.
pub fn extrude(profiles: &[&Profile], plane: &Plane, z0: f64, z1: f64) -> Result<Mesh, String> {
    let (z0, z1) = (z0.min(z1), z0.max(z1));
    if z1 - z0 < 1e-6 {
        return Err("the extrude distance is zero".into());
    }
    let (segs, caps) = outline(profiles)?;
    let n = plane.normal();
    let at = |p: DVec2, z: f64| plane.to_world(p) + n * z;
    let mut m = Mesh::default();
    for (p, q) in segs {
        quad(&mut m, at(p, z0), at(q, z0), at(q, z1), at(p, z1));
    }
    for t in caps {
        m.push([at(t[0], z1), at(t[1], z1), at(t[2], z1)]);
        m.push([at(t[0], z0), at(t[2], z0), at(t[1], z0)]);
    }
    Ok(m)
}

/// Like [`extrude`], with the walls leaning outward by `taper` degrees as
/// they leave the sketch plane (inward when negative).
pub fn extrude_tapered(profiles: &[&Profile], plane: &Plane, z0: f64, z1: f64, taper: f64) -> Result<Mesh, String> {
    let (z0, z1) = (z0.min(z1), z0.max(z1));
    if z1 - z0 < 1e-6 {
        return Err("the extrude distance is zero".into());
    }
    if taper.abs() >= 80.0 {
        return Err("the taper angle must be under 80 degrees".into());
    }
    let n = plane.normal();
    let mut m = Mesh::default();
    for p in profiles {
        let grown = |z: f64| -> Profile {
            let by = z.abs() * taper.to_radians().tan();
            Profile { outer: crate::profile::offset_path(&p.outer, by, true), holes: p.holes.iter().map(|h| crate::profile::offset_path(h, by, true)).collect(), edges: Vec::new(), depth: 0, path: Vec::new(), hole_paths: Vec::new(), path_ids: Vec::new(), hole_path_ids: Vec::new() }
        };
        let (lo, hi) = (grown(z0), grown(z1));
        if signed_area(&lo.outer) <= 1e-9 || signed_area(&hi.outer) <= 1e-9 || lo.area() <= 1e-9 || hi.area() <= 1e-9 {
            return Err("the taper closes the profile up before the end".into());
        }
        let at = |q: DVec2, z: f64| plane.to_world(q) + n * z;
        let walls = |m: &mut Mesh, lower: &Profile, upper: &Profile, from: f64, to: f64| {
            for (a, b) in std::iter::once((&lower.outer, &upper.outer)).chain(lower.holes.iter().zip(&upper.holes)) {
                for i in 0..a.len() {
                    let j = (i + 1) % a.len();
                    quad(m, at(a[i], from), at(a[j], from), at(b[j], to), at(b[i], to));
                }
            }
        };
        if z0 < 0.0 && z1 > 0.0 {
            // Offset is proportional to |z|, so its slope changes at the sketch
            // plane. Keep that original ring: joining equal-size end rings
            // directly would erase the taper of a symmetric extrusion.
            walls(&mut m, &lo, p, z0, 0.0);
            walls(&mut m, p, &hi, 0.0, z1);
        } else {
            walls(&mut m, &lo, &hi, z0, z1);
        }
        for t in triangulate(&hi) {
            m.push([at(t[0], z1), at(t[1], z1), at(t[2], z1)]);
        }
        for t in triangulate(&lo) {
            m.push([at(t[0], z0), at(t[2], z0), at(t[1], z0)]);
        }
    }
    m.drop_slivers();
    Ok(m)
}

/// Turns profiles around the line through `a` and `b` (sketch coordinates) by `degrees`.
pub fn revolve(profiles: &[&Profile], plane: &Plane, a: DVec2, b: DVec2, degrees: f64) -> Result<Mesh, String> {
    if a.distance(b) < 1e-9 {
        return Err("the axis has no length".into());
    }
    let degrees = degrees.clamp(-360.0, 360.0);
    if degrees.abs() < 1e-6 {
        return Err("the revolve angle is zero".into());
    }
    let (segs, caps) = outline(profiles)?;
    let side = |p: DVec2| (b - a).perp_dot(p - a);
    let (lo, hi) = segs.iter().map(|s| side(s.0)).fold((0.0f64, 0.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if lo < -1e-6 && hi > 1e-6 {
        return Err("the profile crosses the axis".into());
    }
    let full = degrees.abs() > 360.0 - 1e-6;
    let steps = ((degrees.abs() / 360.0 * CIRCLE_SEGS as f64).ceil() as usize).max(3);
    let (o, u) = (plane.to_world(a), (plane.to_world(b) - plane.to_world(a)).normalize());
    let at = |p: DVec2, k: usize| {
        let k = if full { k % steps } else { k };
        let (v, ang) = (plane.to_world(p) - o, degrees.to_radians() * k as f64 / steps as f64);
        // Rodrigues' rotation about the axis.
        o + v * ang.cos() + u.cross(v) * ang.sin() + u * u.dot(v) * (1.0 - ang.cos())
    };
    let mut m = Mesh::default();
    for k in 0..steps {
        for (p, q) in &segs {
            quad(&mut m, at(*p, k), at(*q, k), at(*q, k + 1), at(*p, k + 1));
        }
    }
    if !full {
        for t in caps {
            m.push([at(t[0], steps), at(t[1], steps), at(t[2], steps)]);
            m.push([at(t[0], 0), at(t[2], 0), at(t[1], 0)]);
        }
    }
    m.drop_slivers();
    if m.volume() < 0.0 {
        m.flip();
    }
    Ok(m)
}
