//! Triangle meshes and the sweeps that make them from sketch profiles.

use std::collections::HashMap;

use base64::Engine;
use glam::{DVec2, DVec3};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::profile::{Profile, signed_area};
use crate::sketch::{CIRCLE_SEGS, Plane};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub tris: Vec<[DVec3; 3]>,
    /// For meshes made from exact solids, the kernel face each triangle came from.
    /// Empty otherwise; dropped by anything that re-cuts the triangles.
    pub face_ids: Vec<u64>,
}

// Stored as base64 of little-endian f32 triples; imported meshes are big.
impl Serialize for Mesh {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut bytes = Vec::with_capacity(self.tris.len() * 36);
        for v in self.tris.iter().flatten() {
            for c in v.to_array() {
                bytes.extend((c as f32).to_le_bytes());
            }
        }
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

impl<'de> Deserialize<'de> for Mesh {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Mesh, D::Error> {
        let text = String::deserialize(d)?;
        if text.len() > 48_000_000 { return Err(serde::de::Error::custom("the imported mesh exceeds one million triangles")); }
        let bytes = base64::engine::general_purpose::STANDARD.decode(text).map_err(serde::de::Error::custom)?;
        if bytes.len() % 36 != 0 { return Err(serde::de::Error::custom("the mesh has an incomplete triangle")); }
        let f: Vec<f64> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64).collect();
        let mesh = Mesh { face_ids: Vec::new(), tris: f.chunks_exact(9).map(|t| [DVec3::new(t[0], t[1], t[2]), DVec3::new(t[3], t[4], t[5]), DVec3::new(t[6], t[7], t[8])]).collect() };
        mesh.validate().map_err(serde::de::Error::custom)?;
        Ok(mesh)
    }
}

impl Mesh {
    pub fn validate(&self) -> Result<(), String> {
        if self.tris.len() > 1_000_000 { return Err("the mesh exceeds one million triangles".into()); }
        if self.tris.iter().flatten().any(|p| !p.is_finite() || !p.as_vec3().is_finite()) {
            return Err("mesh coordinates must be finite and representable as 32-bit floats".into());
        }
        Ok(())
    }

    pub fn bbox(&self) -> Option<(DVec3, DVec3)> {
        let mut it = self.tris.iter().flatten();
        let first = *it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
    }

    /// Enclosed volume; negative when the mesh is inside out.
    pub fn volume(&self) -> f64 {
        self.tris.iter().map(|t| t[0].dot(t[1].cross(t[2]))).sum::<f64>() / 6.0
    }

    pub fn area(&self) -> f64 {
        self.tris.iter().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).length()).sum::<f64>() / 2.0
    }

    pub fn normal(&self, i: usize) -> DVec3 {
        let t = &self.tris[i];
        (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero()
    }

    pub fn flip(&mut self) {
        for t in &mut self.tris {
            t.swap(1, 2);
        }
    }

    pub fn map(&mut self, f: impl Fn(DVec3) -> DVec3) {
        for v in self.tris.iter_mut().flatten() {
            *v = f(*v);
        }
    }

    /// Nearest triangle hit by a ray: distance along it and the triangle's index.
    pub fn ray(&self, origin: DVec3, dir: DVec3) -> Option<(f64, usize)> {
        let mut best: Option<(f64, usize)> = None;
        for (i, t) in self.tris.iter().enumerate() {
            let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-12 {
                continue;
            }
            let s = origin - t[0];
            let u = s.dot(p) / det;
            let q = s.cross(e1);
            let v = dir.dot(q) / det;
            if u < 0.0 || v < 0.0 || u + v > 1.0 {
                continue;
            }
            let d = e2.dot(q) / det;
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, i));
            }
        }
        best
    }

    /// Shared vertices and index triangles, merging vertices closer than `eps`.
    fn weld(&self, eps: f64) -> (Vec<DVec3>, Vec<[usize; 3]>) {
        let cell = |p: DVec3| ((p.x / eps).floor() as i64, (p.y / eps).floor() as i64, (p.z / eps).floor() as i64);
        let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
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
        let tris = self.tris.iter().map(|t| [index(t[0]), index(t[1]), index(t[2])]).collect();
        (verts, tris)
    }

    /// Directed edges whose opposite is missing.
    fn unmatched(tris: &[[usize; 3]]) -> Vec<(usize, usize, usize)> {
        let flat = |t: &[usize; 3]| t[0] == t[1] || t[1] == t[2] || t[0] == t[2];
        let mut count: HashMap<(usize, usize), i32> = HashMap::new();
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
        self.tris = tris.iter().map(|t| [verts[t[0]], verts[t[1]], verts[t[2]]]).collect();
        self.face_ids.clear();
        self.drop_slivers();
    }

    /// The triangles of the face containing triangle `tri`: the patch of
    /// surface the viewport outlines, spreading across edges that turn gently.
    pub fn face(&self, tri: usize) -> Vec<usize> {
        // Kernel face ids are not unique (the two ends of an extrude share one), so they
        // only narrow the spread: it stays on one kernel face, and within it turns freely.
        let ids = (self.face_ids.len() == self.tris.len()).then_some(&self.face_ids);
        let (_, idx) = self.weld(1e-5);
        let idx = &idx;
        let edges = |t: usize| (0..3).map(move |i| (idx[t][i].min(idx[t][(i + 1) % 3]), idx[t][i].max(idx[t][(i + 1) % 3]))).filter(|e| e.0 != e.1);
        let mut by_edge: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for t in 0..idx.len() {
            for e in edges(t) {
                by_edge.entry(e).or_default().push(t);
            }
        }
        let normals: Vec<DVec3> = (0..idx.len()).map(|i| self.normal(i)).collect();
        let mut seen = vec![false; idx.len()];
        let (mut out, mut queue) = (Vec::new(), vec![tri]);
        seen[tri] = true;
        while let Some(t) = queue.pop() {
            out.push(t);
            for e in edges(t) {
                for &o in &by_edge[&e] {
                    if !seen[o] && ids.map_or(normals[o].dot(normals[t]) > 0.94, |ids| ids[o] == ids[tri]) {
                        seen[o] = true;
                        queue.push(o);
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// For meshes made from exact solids, which face each triangle belongs to:
    /// one number per connected kernel face. Renderers outline where it changes.
    pub fn groups(&self) -> Option<Vec<u32>> {
        if self.face_ids.len() != self.tris.len() || self.tris.is_empty() {
            return None;
        }
        let (_, idx) = self.weld(1e-5);
        let mut by_edge: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (t, tri) in idx.iter().enumerate() {
            for i in 0..3 {
                let (a, b) = (tri[i].min(tri[(i + 1) % 3]), tri[i].max(tri[(i + 1) % 3]));
                if a != b {
                    by_edge.entry((a, b)).or_default().push(t);
                }
            }
        }
        let mut group = vec![u32::MAX; idx.len()];
        let mut next = 0;
        for start in 0..idx.len() {
            if group[start] != u32::MAX {
                continue;
            }
            let mut queue = vec![start];
            group[start] = next;
            while let Some(t) = queue.pop() {
                for i in 0..3 {
                    let (a, b) = (idx[t][i].min(idx[t][(i + 1) % 3]), idx[t][i].max(idx[t][(i + 1) % 3]));
                    for &o in by_edge.get(&(a, b)).into_iter().flatten() {
                        if group[o] == u32::MAX && self.face_ids[o] == self.face_ids[t] {
                            group[o] = next;
                            queue.push(o);
                        }
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
        tris.iter().all(|t| self.normal(*t).dot(n) > 0.99999).then_some((self.tris[tris[0]][0], n))
    }

    pub fn face_area(&self, tris: &[usize]) -> f64 {
        tris.iter().map(|t| (self.tris[*t][1] - self.tris[*t][0]).cross(self.tris[*t][2] - self.tris[*t][0]).length()).sum::<f64>() / 2.0
    }

    /// The closed outlines of a face, outer boundary and holes alike.
    pub fn face_loops(&self, tris: &[usize]) -> Vec<Vec<DVec3>> {
        let (verts, idx) = self.weld(1e-5);
        let directed: Vec<(usize, usize)> = tris.iter().flat_map(|t| (0..3).map(|i| (idx[*t][i], idx[*t][(i + 1) % 3]))).filter(|e| e.0 != e.1).collect();
        let all: std::collections::HashSet<(usize, usize)> = directed.iter().copied().collect();
        let mut next: HashMap<usize, usize> = directed.iter().filter(|e| !all.contains(&(e.1, e.0))).copied().collect();
        let mut loops = Vec::new();
        while let Some(&start) = next.keys().min() {
            let (mut at, mut ring) = (start, Vec::new());
            while let Some(to) = next.remove(&at) {
                ring.push(verts[at]);
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

    /// The triangle nearest to a point.
    pub fn nearest_tri(&self, p: DVec3) -> Option<usize> {
        let dist = |t: &[DVec3; 3]| {
            // Closest point on the triangle: inside its plane footprint, or on an edge.
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            let q = p - n * (p - t[0]).dot(n);
            let inside = (0..3).all(|i| (t[(i + 1) % 3] - t[i]).cross(q - t[i]).dot(n) >= 0.0);
            if inside {
                return q.distance(p);
            }
            (0..3)
                .map(|i| {
                    let (a, d) = (t[i], t[(i + 1) % 3] - t[i]);
                    (a + d * ((p - a).dot(d) / d.length_squared().max(1e-18)).clamp(0.0, 1.0)).distance(p)
                })
                .fold(f64::MAX, f64::min)
        };
        (0..self.tris.len()).min_by(|a, b| dist(&self.tris[*a]).total_cmp(&dist(&self.tris[*b])))
    }

    fn drop_slivers(&mut self) {
        self.face_ids.clear();
        self.tris.retain(|t| (t[1] - t[0]).cross(t[2] - t[0]).length_squared() > 1e-20);
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
    m.tris.push([a, b, c]);
    m.tris.push([a, c, d]);
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
        m.tris.push([at(t[0], z1), at(t[1], z1), at(t[2], z1)]);
        m.tris.push([at(t[0], z0), at(t[2], z0), at(t[1], z0)]);
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
            Profile { outer: crate::profile::offset_path(&p.outer, by, true), holes: p.holes.iter().map(|h| crate::profile::offset_path(h, by, true)).collect(), edges: Vec::new(), depth: 0, path: Vec::new(), hole_paths: Vec::new() }
        };
        let (lo, hi) = (grown(z0), grown(z1));
        if signed_area(&lo.outer) <= 1e-9 || signed_area(&hi.outer) <= 1e-9 || lo.area() <= 1e-9 || hi.area() <= 1e-9 {
            return Err("the taper closes the profile up before the end".into());
        }
        let at = |q: DVec2, z: f64| plane.to_world(q) + n * z;
        for (a, b) in std::iter::once((&lo.outer, &hi.outer)).chain(lo.holes.iter().zip(&hi.holes)) {
            for i in 0..a.len() {
                let j = (i + 1) % a.len();
                quad(&mut m, at(a[i], z0), at(a[j], z0), at(b[j], z1), at(b[i], z1));
            }
        }
        for t in triangulate(&hi) {
            m.tris.push([at(t[0], z1), at(t[1], z1), at(t[2], z1)]);
        }
        for t in triangulate(&lo) {
            m.tris.push([at(t[0], z0), at(t[2], z0), at(t[1], z0)]);
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
            m.tris.push([at(t[0], steps), at(t[1], steps), at(t[2], steps)]);
            m.tris.push([at(t[0], 0), at(t[2], 0), at(t[1], 0)]);
        }
    }
    m.drop_slivers();
    if m.volume() < 0.0 {
        m.flip();
    }
    Ok(m)
}
