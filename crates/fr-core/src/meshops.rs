//! Operations on mesh bodies: repair, decimation, smoothing, subdivision,
//! cutting, mirroring, offsetting, regions and relief from an image. Each
//! takes a welded mesh and returns a welded mesh; the features in `doc`
//! wrap them so every edit is a step in the timeline.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::mesh::{FxMap, Mesh};
use crate::sketch::Plane;

type R<T> = Result<T, String>;

/// Which side of a cutting plane to keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    /// The side the plane's normal points away from.
    #[default]
    Negative,
    Positive,
    Both,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecimateMethod {
    /// Quadric edge collapse: slow but faithful.
    #[default]
    Quadric,
    /// Vertex clustering on a grid: fast and coarse.
    Cluster,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    #[default]
    Loop,
    Midpoint,
}

fn welded(m: &Mesh) -> Mesh {
    let mut m = m.clone();
    m.face_ids.clear();
    if !m.is_welded() { m.weld_exact(); }
    m
}

fn finish(mut m: Mesh) -> R<Mesh> {
    m.snap();
    m.compact();
    m.validate()?;
    if m.is_empty() { return Err("the operation left no triangles".into()); }
    Ok(m)
}

/// Area-weighted vertex normals.
pub fn vertex_normals(m: &Mesh) -> Vec<DVec3> {
    let mut n = vec![DVec3::ZERO; m.vertex_count()];
    for (t, tri) in m.indices().iter().zip(m.tris()) {
        let a = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
        for i in t { n[*i as usize] += a; }
    }
    n.iter().map(|v| v.normalize_or_zero()).collect()
}

/// The open boundary of a mesh as closed loops of vertex indices.
pub fn boundary_loops(m: &Mesh) -> Vec<Vec<u32>> {
    let mut count: FxMap<(u32, u32), u32> = FxMap::default();
    for t in m.indices() {
        for k in 0..3 { *count.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1; }
    }
    // A boundary directed edge has no opposite; following them in reverse keeps the loop's orientation with the surface.
    let mut next: FxMap<u32, u32> = FxMap::default();
    for (&(a, b), _) in &count {
        if !count.contains_key(&(b, a)) { next.insert(b, a); }
    }
    let mut loops = Vec::new();
    let mut keys: Vec<u32> = next.keys().copied().collect();
    keys.sort_unstable();
    for start in keys {
        if !next.contains_key(&start) { continue; }
        let (mut at, mut ring) = (start, Vec::new());
        while let Some(to) = next.remove(&at) {
            ring.push(at);
            at = to;
            if ring.len() > m.vertex_count() { break; }
        }
        if at == start && ring.len() >= 3 { loops.push(ring); }
    }
    loops
}

/// Fills boundary loops of at most `max_edges` edges by fanning from the loop's centroid
/// (small holes; a large open side is left alone). Returns how many holes were closed.
pub fn fill_holes(m: &mut Mesh, max_edges: usize) -> usize {
    if max_edges < 3 { return 0; }
    let loops = boundary_loops(m);
    let mut positions: Vec<DVec3> = m.positions().to_vec();
    let mut indices: Vec<[u32; 3]> = m.indices().to_vec();
    let mut filled = 0;
    for ring in loops.into_iter().filter(|l| l.len() <= max_edges) {
        if ring.len() == 3 {
            indices.push([ring[0], ring[1], ring[2]]);
        } else {
            let centre = ring.iter().map(|v| positions[*v as usize]).sum::<DVec3>() / ring.len() as f64;
            let c = positions.len() as u32;
            positions.push(centre);
            for k in 0..ring.len() {
                indices.push([ring[k], ring[(k + 1) % ring.len()], c]);
            }
        }
        filled += 1;
    }
    if filled > 0 {
        *m = Mesh::from_indexed(positions, indices, true).unwrap_or_else(|_| m.clone());
    }
    filled
}

/// Repairs and optionally closes small holes.
pub fn repair(m: &Mesh, fill_holes_up_to: usize) -> R<(Mesh, crate::mesh::Report, usize)> {
    let mut out = welded(m);
    let mut report = out.repair();
    let filled = fill_holes(&mut out, fill_holes_up_to);
    if filled > 0 { report = out.repair(); }
    Ok((finish(out)?, report, filled))
}

// ----- decimation -----

/// A symmetric 4x4 quadric as its ten upper-triangle terms.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: DVec3, d: f64) -> Quadric {
        let (a, b, c) = (n.x, n.y, n.z);
        Quadric([a * a, a * b, a * c, a * d, b * b, b * c, b * d, c * c, c * d, d * d])
    }
    fn add(&mut self, o: &Quadric) { for k in 0..10 { self.0[k] += o.0[k]; } }
    fn scaled(&self, s: f64) -> Quadric { let mut q = *self; for k in 0..10 { q.0[k] *= s; } q }
    fn eval(&self, p: DVec3) -> f64 {
        let q = &self.0;
        q[0] * p.x * p.x + 2.0 * q[1] * p.x * p.y + 2.0 * q[2] * p.x * p.z + 2.0 * q[3] * p.x + q[4] * p.y * p.y + 2.0 * q[5] * p.y * p.z + 2.0 * q[6] * p.y + q[7] * p.z * p.z + 2.0 * q[8] * p.z + q[9]
    }
    /// The point minimising the quadric, when the 3x3 part is well conditioned.
    fn optimum(&self) -> Option<DVec3> {
        let q = &self.0;
        let m = glam::DMat3::from_cols(DVec3::new(q[0], q[1], q[2]), DVec3::new(q[1], q[4], q[5]), DVec3::new(q[2], q[5], q[7]));
        let det = m.determinant();
        if det.abs() < 1e-12 { return None; }
        let p = m.inverse() * -DVec3::new(q[3], q[6], q[8]);
        p.is_finite().then_some(p)
    }
}

#[derive(PartialEq)]
struct Cost(f64);
impl Eq for Cost {}
impl PartialOrd for Cost { fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) } }
impl Ord for Cost { fn cmp(&self, o: &Self) -> std::cmp::Ordering { self.0.total_cmp(&o.0) } }

/// Quadric edge-collapse decimation to about `target` triangles. Boundary
/// edges are weighted so open meshes keep their outline, and collapses that
/// would turn a triangle over are refused.
pub fn decimate(m: &Mesh, target: usize, preserve_boundary: bool) -> R<Mesh> {
    let m = welded(m);
    let n = m.vertex_count();
    let mut pos: Vec<DVec3> = m.positions().to_vec();
    let mut tris: Vec<[u32; 3]> = m.indices().to_vec();
    if tris.len() <= target.max(4) { return finish(m); }
    let mut alive = vec![true; tris.len()];
    let mut quadrics = vec![Quadric::default(); n];
    let mut faces_of: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut edge_uses: FxMap<(u32, u32), u8> = FxMap::default();
    for (ti, t) in tris.iter().enumerate() {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let nrm = (b - a).cross(c - a);
        let area = nrm.length();
        if area > 0.0 {
            let nn = nrm / area;
            let q = Quadric::plane(nn, -nn.dot(a)).scaled(area);
            for i in t { quadrics[*i as usize].add(&q); }
        }
        for i in t { faces_of[*i as usize].push(ti as u32); }
        for k in 0..3 {
            let (x, y) = (t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3]));
            *edge_uses.entry((x, y)).or_insert(0) += 1;
        }
    }
    if preserve_boundary {
        // A heavy plane through each boundary edge, perpendicular to its face, pins the outline.
        for (ti, t) in tris.iter().enumerate() {
            let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            let fnrm = (b - a).cross(c - a).normalize_or_zero();
            for k in 0..3 {
                let (i, j) = (t[k], t[(k + 1) % 3]);
                if edge_uses.get(&(i.min(j), i.max(j))).copied().unwrap_or(0) == 1 {
                    let (p, q) = (pos[i as usize], pos[j as usize]);
                    let wn = (q - p).cross(fnrm).normalize_or_zero();
                    let weight = (q - p).length_squared() * 100.0;
                    let qd = Quadric::plane(wn, -wn.dot(p)).scaled(weight);
                    quadrics[i as usize].add(&qd);
                    quadrics[j as usize].add(&qd);
                }
            }
            let _ = ti;
        }
    }
    let mut version = vec![0u32; n];
    let mut live = tris.len();
    let best = |q: &Quadric, a: DVec3, b: DVec3| -> (f64, DVec3) {
        let mut cands = vec![a, b, (a + b) / 2.0];
        if let Some(o) = q.optimum() && o.distance(a) < 2.0 * a.distance(b) + 1e-9 { cands.push(o); }
        cands.into_iter().map(|p| (q.eval(p), p)).min_by(|x, y| x.0.total_cmp(&y.0)).unwrap()
    };
    let mut heap: BinaryHeap<Reverse<(Cost, u32, u32, u32, u32)>> = BinaryHeap::new();
    for &(a, b) in edge_uses.keys() {
        let mut q = quadrics[a as usize]; q.add(&quadrics[b as usize]);
        let (c, _) = best(&q, pos[a as usize], pos[b as usize]);
        heap.push(Reverse((Cost(c), a, b, 0, 0)));
    }
    drop(edge_uses);
    let mut dead = vec![false; n];
    while live > target {
        let Some(Reverse((_, a, b, va, vb))) = heap.pop() else { break };
        let (a, b) = (a as usize, b as usize);
        if dead[a] || dead[b] || version[a] != va || version[b] != vb { continue; }
        let mut q = quadrics[a]; q.add(&quadrics[b]);
        let (_, p) = best(&q, pos[a], pos[b]);
        // Refuse a collapse that turns any surviving face over or squashes it.
        let shared: Vec<u32> = faces_of[a].iter().filter(|f| faces_of[b].contains(f)).copied().collect();
        let mut ok = true;
        for &fi in faces_of[a].iter().chain(faces_of[b].iter()) {
            if !alive[fi as usize] || shared.contains(&fi) { continue; }
            let t = tris[fi as usize];
            let before = [pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]];
            let after = t.map(|i| if i as usize == a || i as usize == b { p } else { pos[i as usize] });
            let n0 = (before[1] - before[0]).cross(before[2] - before[0]);
            let n1 = (after[1] - after[0]).cross(after[2] - after[0]);
            if n1.length_squared() < 1e-24 || n0.dot(n1) <= 0.0 { ok = false; break; }
        }
        if !ok { continue; }
        // Collapse b into a.
        pos[a] = p;
        quadrics[a] = q;
        for &fi in &shared {
            if alive[fi as usize] { alive[fi as usize] = false; live -= 1; }
        }
        let moved = std::mem::take(&mut faces_of[b]);
        for fi in moved {
            if !alive[fi as usize] { continue; }
            for i in &mut tris[fi as usize] { if *i as usize == b { *i = a as u32; } }
            if !faces_of[a].contains(&fi) { faces_of[a].push(fi); }
        }
        faces_of[a].retain(|f| alive[*f as usize]);
        dead[b] = true;
        version[a] += 1;
        // New costs for the edges around a.
        let mut ring: Vec<u32> = faces_of[a].iter().flat_map(|f| tris[*f as usize]).filter(|v| *v as usize != a).collect();
        ring.sort_unstable();
        ring.dedup();
        for o in ring {
            if dead[o as usize] { continue; }
            let mut q = quadrics[a]; q.add(&quadrics[o as usize]);
            let (c, _) = best(&q, pos[a], pos[o as usize]);
            heap.push(Reverse((Cost(c), a as u32, o, version[a], version[o as usize])));
        }
    }
    let indices: Vec<[u32; 3]> = tris.iter().zip(&alive).filter(|(_, l)| **l).map(|(t, _)| *t).filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]).collect();
    let out = Mesh::from_indexed(pos, indices, true)?;
    finish(out)
}

/// Decimation by the chosen method.
pub fn decimate_by(m: &Mesh, target: usize, method: DecimateMethod, preserve_boundary: bool) -> R<Mesh> {
    match method {
        DecimateMethod::Quadric => decimate(m, target, preserve_boundary),
        DecimateMethod::Cluster => finish(welded(m).clustered_to(target)),
    }
}

// ----- smoothing and subdivision -----

/// Taubin smoothing: a shrinking pass of `strength` followed by an inflating
/// pass, repeated, so the surface loses noise without losing volume. `mask`
/// limits the moved vertices.
pub fn smooth(m: &Mesh, iterations: u32, strength: f64, mask: Option<&[bool]>) -> R<Mesh> {
    let mut out = welded(m);
    let adj = out.adjacency();
    let lambda = strength.clamp(0.0, 1.0);
    let mu = -(lambda + 0.03).min(1.0);
    let moving: Vec<bool> = match mask {
        Some(mask) => (0..out.vertex_count() as u32).map(|v| adj.tris_of(v).iter().any(|t| mask.get(*t as usize).copied().unwrap_or(false))).collect(),
        None => vec![true; out.vertex_count()],
    };
    let boundary: Vec<bool> = (0..out.vertex_count() as u32).map(|v| adj.on_boundary(v)).collect();
    let rings: Vec<Vec<u32>> = (0..out.vertex_count() as u32).map(|v| adj.ring(v)).collect();
    let mut p: Vec<DVec3> = out.positions().to_vec();
    for _ in 0..iterations.min(500) {
        for factor in [lambda, mu] {
            let before = p.clone();
            for v in 0..p.len() {
                if !moving[v] || boundary[v] || rings[v].is_empty() { continue; }
                let mean = rings[v].iter().map(|o| before[*o as usize]).sum::<DVec3>() / rings[v].len() as f64;
                p[v] = before[v] + (mean - before[v]) * factor;
            }
        }
    }
    out.map_indexed(|i, _| p[i]);
    finish(out)
}

/// Loop or midpoint subdivision of the whole mesh, `levels` times.
pub fn subdivide(m: &Mesh, levels: u32, scheme: Scheme) -> R<Mesh> {
    let mut out = welded(m);
    for _ in 0..levels.min(6) {
        if out.len() * 4 > crate::mesh::MAX_TRIANGLES { return Err("subdividing further would exceed the triangle limit".into()); }
        let mut pos: Vec<DVec3> = out.positions().to_vec();
        let old_count = pos.len();
        let tris = out.indices();
        // Edge -> (new vertex index, opposite vertices).
        let mut edges: FxMap<(u32, u32), (u32, Vec<u32>)> = FxMap::default();
        for t in tris {
            for k in 0..3 {
                let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                let key = (a.min(b), a.max(b));
                let e = edges.entry(key).or_insert_with(|| { pos.push(DVec3::ZERO); (pos.len() as u32 - 1, Vec::new()) });
                e.1.push(c);
            }
        }
        let adj = out.adjacency();
        let mut rings: Vec<Vec<u32>> = Vec::new();
        let mut boundary_vertex = vec![false; old_count];
        if scheme == Scheme::Loop {
            rings = (0..old_count as u32).map(|v| adj.ring(v)).collect();
            for (&(a, b), (_, opp)) in &edges {
                if opp.len() == 1 { boundary_vertex[a as usize] = true; boundary_vertex[b as usize] = true; }
            }
        }
        // Edge points.
        for (&(a, b), (v, opp)) in &edges {
            let (pa, pb) = (out.positions()[a as usize], out.positions()[b as usize]);
            pos[*v as usize] = match (scheme, opp.as_slice()) {
                (Scheme::Loop, [c, d]) => (pa + pb) * 0.375 + (out.positions()[*c as usize] + out.positions()[*d as usize]) * 0.125,
                _ => (pa + pb) * 0.5,
            };
        }
        // Old points move under Loop.
        if scheme == Scheme::Loop {
            for v in 0..old_count {
                let p = out.positions()[v];
                let ring = &rings[v];
                if ring.is_empty() { continue; }
                if boundary_vertex[v] {
                    let ends: Vec<DVec3> = ring.iter().filter(|o| boundary_vertex[**o as usize]).map(|o| out.positions()[*o as usize]).collect();
                    if ends.len() == 2 { pos[v] = p * 0.75 + (ends[0] + ends[1]) * 0.125; }
                } else {
                    let n = ring.len() as f64;
                    let beta = (0.625 - (0.375 + 0.25 * (std::f64::consts::TAU / n).cos()).powi(2)) / n;
                    let sum: DVec3 = ring.iter().map(|o| out.positions()[*o as usize]).sum();
                    pos[v] = p * (1.0 - n * beta) + sum * beta;
                }
            }
        }
        let mut next = Vec::with_capacity(tris.len() * 4);
        for t in tris {
            let e = |a: u32, b: u32| edges[&(a.min(b), a.max(b))].0;
            let (ab, bc, ca) = (e(t[0], t[1]), e(t[1], t[2]), e(t[2], t[0]));
            next.push([t[0], ab, ca]);
            next.push([t[1], bc, ab]);
            next.push([t[2], ca, bc]);
            next.push([ab, bc, ca]);
        }
        out = Mesh::from_indexed(pos, next, true)?;
    }
    finish(out)
}

// ----- cutting, mirroring, offsetting -----

/// Boundary loops that lie in a plane, as 2D rings in the plane's coordinates, triangulated.
fn cap_loops(m: &Mesh, plane: &Plane, flip: bool) -> Vec<[DVec3; 3]> {
    let n = plane.normal();
    let on = |p: DVec3| (p - plane.origin).dot(n).abs() < 1e-6;
    let loops: Vec<Vec<DVec3>> = boundary_loops(m).into_iter().map(|l| l.iter().map(|v| m.positions()[*v as usize]).collect::<Vec<_>>()).filter(|l| l.iter().all(|p| on(*p))).collect();
    let mut loops = loops;
    let mut rings: Vec<Vec<DVec2>> = loops.iter().map(|l| l.iter().map(|p| plane.to_local(*p)).collect()).collect();
    let area = |r: &[DVec2]| crate::profile::signed_area(r);
    // Loops run one way round for one side of the plane and the other way for the other; the
    // triangles' final facing is settled by the repair that follows, so only the nesting matters here.
    if rings.iter().map(|r| area(r)).sum::<f64>() < 0.0 {
        for r in &mut rings { r.reverse(); }
        for l in &mut loops { l.reverse(); }
    }
    let outers: Vec<usize> = (0..rings.len()).filter(|i| area(&rings[*i]) > 0.0).collect();
    let mut out = Vec::new();
    for &o in &outers {
        let holes: Vec<usize> = (0..rings.len()).filter(|i| area(&rings[*i]) < 0.0 && crate::profile::inside(&rings[o], rings[*i][0])).collect();
        let mut flat: Vec<f64> = rings[o].iter().flat_map(|p| [p.x, p.y]).collect();
        // The cap's corners are the rim's own points, bit for bit, so they weld to the walls.
        let mut points: Vec<DVec3> = loops[o].clone();
        let mut starts = Vec::new();
        for h in &holes {
            starts.push(flat.len() / 2);
            flat.extend(rings[*h].iter().flat_map(|p| [p.x, p.y]));
            points.extend(loops[*h].iter().copied());
        }
        let Ok(idx) = earcutr::earcut(&flat, &starts, 2) else { continue };
        for t in idx.chunks_exact(3) {
            let mut tri = [points[t[0]], points[t[1]], points[t[2]]];
            if flip { tri.swap(1, 2); }
            out.push(tri);
        }
    }
    out
}

/// Keeps one side of a plane (or both, as two meshes), splitting the
/// triangles it crosses and, with `cap`, closing the cut with flat faces.
pub fn cut(m: &Mesh, plane: Plane, keep: Keep, cap: bool) -> R<Vec<Mesh>> {
    let m = welded(m);
    let n = plane.normal();
    let side = |positive: bool| -> R<Mesh> {
        let sign = if positive { 1.0 } else { -1.0 };
        let dist = |p: DVec3| (p - plane.origin).dot(n) * sign;
        let mut tris: Vec<[DVec3; 3]> = Vec::with_capacity(m.len());
        for t in m.tris() {
            let d = t.map(dist);
            if d.iter().all(|x| *x >= -1e-9) { tris.push(t); continue; }
            if d.iter().all(|x| *x <= 1e-9) { continue; }
            // Clip the triangle against the half-space.
            let mut poly: Vec<DVec3> = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let (da, db) = (d[k], d[(k + 1) % 3]);
                if da >= 0.0 { poly.push(a); }
                if (da > 0.0) != (db > 0.0) && (da - db).abs() > 1e-15 {
                    let s = da / (da - db);
                    let p = a + (b - a) * s;
                    poly.push(p - n * (p - plane.origin).dot(n));
                }
            }
            for k in 1..poly.len().saturating_sub(1) {
                tris.push([poly[0], poly[k], poly[k + 1]]);
            }
        }
        let mut out = Mesh::welded_from_tris(&tris);
        if cap {
            // The cap faces away from the kept side: its outward normal is -n for the positive side.
            let caps = cap_loops(&out, &plane, !positive);
            let mut all: Vec<[DVec3; 3]> = out.tris().collect();
            all.extend(caps);
            out = Mesh::welded_from_tris(&all);
        }
        out.repair();
        finish(out)
    };
    Ok(match keep {
        Keep::Negative => vec![side(false)?],
        Keep::Positive => vec![side(true)?],
        Keep::Both => vec![side(false)?, side(true)?],
    })
}

/// Adds the mirror image across a plane; with `weld`, vertices on the plane join their images.
pub fn mirror(m: &Mesh, plane: Plane, weld: bool) -> R<Mesh> {
    let m = welded(m);
    let n = plane.normal();
    let mut copy = m.clone();
    copy.map(|p| p - n * 2.0 * (p - plane.origin).dot(n));
    copy.flip();
    let mut all: Vec<[DVec3; 3]> = m.tris().collect();
    all.extend(copy.tris());
    if weld {
        // Points within a hair of the plane are put on it, so both halves share them exactly.
        for t in &mut all {
            for p in t.iter_mut() {
                let d = (*p - plane.origin).dot(n);
                if d.abs() < 1e-5 { *p -= n * d; }
            }
        }
    }
    let mut out = Mesh::welded_from_tris(&all);
    out.repair();
    finish(out)
}

/// Thickens an open mesh into a closed solid, or hollows a closed one. The
/// offset copy moves along the vertex normals, or along `direction` when given
/// (down for a relief). A closed mesh gets its offset copy inside; an open one
/// gets walls along its boundary.
pub fn offset(m: &Mesh, distance: f64, direction: Option<DVec3>) -> R<Mesh> {
    if !(distance.abs() > 1e-9) { return Err("the offset distance must not be zero".into()); }
    let m = welded(m);
    let report = m.inspect();
    let normals = vertex_normals(&m);
    let closed = report.open_edges == 0 && report.non_manifold_edges == 0;
    let dir = direction.and_then(|d| d.try_normalize());
    let mut copy = m.clone();
    // A closed shell is hollowed inward; an open sheet is thickened behind its surface, or the way it was asked.
    copy.map_indexed(|i, p| match (closed, dir) {
        (true, _) => p - normals[i] * distance.abs(),
        (false, Some(d)) => p + d * distance,
        (false, None) => p - normals[i] * distance,
    });
    copy.flip();
    let mut all: Vec<[DVec3; 3]> = m.tris().collect();
    all.extend(copy.tris());
    if !closed {
        let base = m.vertex_count();
        let joined_positions: Vec<DVec3> = m.positions().iter().copied().chain(copy.positions().iter().copied()).collect();
        for ring in boundary_loops(&m) {
            for k in 0..ring.len() {
                let (a, b) = (ring[k] as usize, ring[(k + 1) % ring.len()] as usize);
                let (pa, pb, qa, qb) = (joined_positions[a], joined_positions[b], joined_positions[base + a], joined_positions[base + b]);
                // The boundary runs with the surface on its left; walls face outward.
                all.push([pa, qa, qb]);
                all.push([pa, qb, pb]);
            }
        }
    }
    let mut out = Mesh::welded_from_tris(&all);
    out.repair();
    finish(out)
}

// ----- regions -----

/// A set of triangles, described rather than listed, so it survives re-meshing upstream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionSpec {
    Sphere { centre: DVec3, radius: f64 },
    Box { lo: DVec3, hi: DVec3 },
    /// Triangles whose centre is on the positive side of the plane.
    Side { plane: Plane },
    /// Triangles whose normal is within `degrees` of a direction.
    Normal { direction: DVec3, degrees: f64 },
    /// The connected shell containing the triangle nearest a point.
    Connected { seed: DVec3 },
}

pub fn region(m: &Mesh, spec: &RegionSpec) -> Vec<bool> {
    match spec {
        RegionSpec::Sphere { centre, radius } => m.tris().map(|t| ((t[0] + t[1] + t[2]) / 3.0).distance(*centre) <= *radius).collect(),
        RegionSpec::Box { lo, hi } => m.tris().map(|t| { let c = (t[0] + t[1] + t[2]) / 3.0; c.cmpge(*lo).all() && c.cmple(*hi).all() }).collect(),
        RegionSpec::Side { plane } => { let n = plane.normal(); m.tris().map(|t| ((t[0] + t[1] + t[2]) / 3.0 - plane.origin).dot(n) > 0.0).collect() }
        RegionSpec::Normal { direction, degrees } => {
            let d = direction.normalize_or_zero();
            let cos = degrees.to_radians().cos();
            (0..m.len()).map(|i| m.normal(i).dot(d) >= cos).collect()
        }
        RegionSpec::Connected { seed } => {
            let mut mask = vec![false; m.len()];
            let Some(start) = m.nearest_tri(*seed) else { return mask };
            let adj = m.adjacency();
            let mut stack = vec![start];
            mask[start] = true;
            while let Some(t) = stack.pop() {
                for o in adj.neighbours(t) {
                    if !mask[o] { mask[o] = true; stack.push(o); }
                }
            }
            mask
        }
    }
}

/// Moves a region of triangles by `distance` along their mean normal (or
/// `direction`), with walls along the region's boundary.
pub fn extrude_region(m: &Mesh, mask: &[bool], distance: f64, direction: Option<DVec3>) -> R<Mesh> {
    let m = welded(m);
    let count = mask.iter().filter(|x| **x).count();
    if count == 0 { return Err("the region holds no triangles".into()); }
    if count == m.len() { return Err("the region is the whole mesh; use a transform to move it".into()); }
    let dir = match direction.and_then(|d| d.try_normalize()) {
        Some(d) => d,
        None => (0..m.len()).filter(|i| mask[*i]).map(|i| { let t = m.tri(i); (t[1] - t[0]).cross(t[2] - t[0]) }).sum::<DVec3>().try_normalize().ok_or("the region has no direction")?,
    };
    let shift = dir * distance;
    // Vertices used by region triangles and by others are the boundary: they are duplicated.
    let mut in_region = vec![false; m.vertex_count()];
    let mut outside = vec![false; m.vertex_count()];
    for (i, t) in m.indices().iter().enumerate() {
        for v in t { if mask[i] { in_region[*v as usize] = true; } else { outside[*v as usize] = true; } }
    }
    let mut positions: Vec<DVec3> = m.positions().to_vec();
    let mut copy: FxMap<u32, u32> = FxMap::default();
    let mut indices: Vec<[u32; 3]> = Vec::with_capacity(m.len() + 2 * count);
    for (i, t) in m.indices().iter().enumerate() {
        if !mask[i] { indices.push(*t); continue; }
        indices.push(t.map(|v| {
            if outside[v as usize] {
                *copy.entry(v).or_insert_with(|| { positions.push(m.positions()[v as usize] + shift); positions.len() as u32 - 1 })
            } else {
                v
            }
        }));
    }
    for v in 0..m.vertex_count() {
        if in_region[v] && !outside[v] { positions[v] += shift; }
    }
    // Walls along region boundary edges: edges with exactly one region triangle.
    let mut edge_count: FxMap<(u32, u32), (u8, u8)> = FxMap::default();
    for (i, t) in m.indices().iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let e = edge_count.entry((a.min(b), a.max(b))).or_insert((0, 0));
            if mask[i] { e.0 += 1; } else { e.1 += 1; }
        }
    }
    for (i, t) in m.indices().iter().enumerate() {
        if !mask[i] { continue; }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let (r, o) = edge_count[&(a.min(b), a.max(b))];
            if r == 1 && o >= 1 {
                let (ca, cb) = (copy[&a], copy[&b]);
                // The region triangle runs a->b; the wall closes the gap between the old edge and the moved one.
                indices.push([a, b, cb]);
                indices.push([a, cb, ca]);
            }
        }
    }
    let mut out = Mesh::from_indexed(positions, indices, true)?;
    out.repair();
    finish(out)
}

// ----- relief from an image -----

pub struct ReliefParams {
    /// Size along the image's width, in millimetres.
    pub width: f64,
    /// Height of the brightest (or darkest, with `invert`) pixel above the base plane.
    pub depth: f64,
    /// Thickness of the slab under the relief; 0 leaves an open surface.
    pub base: f64,
    /// Cells along the longer side.
    pub resolution: u32,
    pub invert: bool,
    /// Box-blur radius in cells, applied three times.
    pub blur: u32,
    pub gamma: f64,
}

/// A height field from an image's luminance: a grid of `resolution` cells
/// along the longer side on the XY plane, Z up, lower-left at the origin.
pub fn from_image(pixels: &crate::reference::Pixels, p: &ReliefParams) -> R<Mesh> {
    if !(p.width > 0.0) || !(p.depth >= 0.0) || !(p.base >= 0.0) || !(p.gamma > 0.0) { return Err("the relief needs a positive width and non-negative depth, base and gamma".into()); }
    let res = p.resolution.clamp(2, 1200) as usize;
    let (iw, ih) = (pixels.width as usize, pixels.height as usize);
    let (cols, rows) = if iw >= ih { (res, (res as f64 * ih as f64 / iw as f64).round().max(2.0) as usize) } else { ((res as f64 * iw as f64 / ih as f64).round().max(2.0) as usize, res) };
    // Average the pixels under each cell.
    let mut height = vec![0.0f64; (cols + 1) * (rows + 1)];
    for r in 0..=rows {
        for c in 0..=cols {
            let (x0, x1) = ((c as f64 / cols as f64 * (iw - 1) as f64).floor() as usize, ((c as f64 + 1.0) / cols as f64 * (iw - 1) as f64).ceil().min(iw as f64 - 1.0) as usize);
            // Image rows run top to bottom; the relief's rows run bottom to top.
            let yy = (rows - r) as f64 / rows as f64 * (ih - 1) as f64;
            let (y0, y1) = (yy.floor() as usize, (yy + ih as f64 / rows as f64).ceil().min(ih as f64 - 1.0) as usize);
            let (mut sum, mut n) = (0.0, 0.0);
            for y in y0..=y1.max(y0) {
                for x in x0..=x1.max(x0) {
                    let i = (y * iw + x) * 4;
                    let (rr, gg, bb) = (pixels.rgba[i] as f64, pixels.rgba[i + 1] as f64, pixels.rgba[i + 2] as f64);
                    sum += (0.2126 * rr + 0.7152 * gg + 0.0722 * bb) / 255.0;
                    n += 1.0;
                }
            }
            let mut v = if n > 0.0 { sum / n } else { 0.0 };
            if p.invert { v = 1.0 - v; }
            height[r * (cols + 1) + c] = v.powf(p.gamma);
        }
    }
    for _ in 0..3 {
        if p.blur == 0 { break; }
        let b = p.blur as isize;
        let mut next = height.clone();
        for r in 0..=rows as isize {
            for c in 0..=cols as isize {
                let (mut sum, mut n) = (0.0, 0.0);
                for dr in -b..=b {
                    for dc in -b..=b {
                        let (rr, cc) = (r + dr, c + dc);
                        if rr < 0 || cc < 0 || rr > rows as isize || cc > cols as isize { continue; }
                        sum += height[rr as usize * (cols + 1) + cc as usize];
                        n += 1.0;
                    }
                }
                next[r as usize * (cols + 1) + c as usize] = sum / n;
            }
        }
        height = next;
    }
    let cell = p.width / cols as f64;
    let at = |r: usize, c: usize| DVec3::new(c as f64 * cell, r as f64 * cell, height[r * (cols + 1) + c] * p.depth);
    let mut tris: Vec<[DVec3; 3]> = Vec::with_capacity(cols * rows * 2 + if p.base > 0.0 { 2 + 4 * (cols + rows) } else { 0 });
    for r in 0..rows {
        for c in 0..cols {
            let (a, b, cc, d) = (at(r, c), at(r, c + 1), at(r + 1, c + 1), at(r + 1, c));
            tris.push([a, b, cc]);
            tris.push([a, cc, d]);
        }
    }
    if p.base > 0.0 {
        let z = -p.base;
        let down = |q: DVec3| DVec3::new(q.x, q.y, z);
        // The underside is the same grid, facing down, so it shares every rim vertex with the walls.
        for r in 0..rows {
            for c in 0..cols {
                let (a, b, cc, d) = (down(at(r, c)), down(at(r, c + 1)), down(at(r + 1, c + 1)), down(at(r + 1, c)));
                tris.push([a, cc, b]);
                tris.push([a, d, cc]);
            }
        }
        // Walls run around the rim in the order that leaves their normals pointing outward.
        for c in 0..cols {
            let (a, b) = (at(0, c), at(0, c + 1));
            tris.push([a, down(a), down(b)]);
            tris.push([a, down(b), b]);
            let (a, b) = (at(rows, c + 1), at(rows, c));
            tris.push([a, down(a), down(b)]);
            tris.push([a, down(b), b]);
        }
        for r in 0..rows {
            let (a, b) = (at(r + 1, 0), at(r, 0));
            tris.push([a, down(a), down(b)]);
            tris.push([a, down(b), b]);
            let (a, b) = (at(r, cols), at(r + 1, cols));
            tris.push([a, down(a), down(b)]);
            tris.push([a, down(b), b]);
        }
    }
    let mut out = Mesh::welded_from_tris(&tris);
    out.repair();
    finish(out)
}

// ----- booleans -----

/// Union, difference or intersection of two meshes. Triangles that cannot
/// touch the other mesh (their bounds clear every triangle of it) are kept
/// or dropped whole; only the triangles near the other surface are split,
/// by a BSP of the other mesh's near triangles, and each piece is classified
/// by ray parity against the whole other mesh. So a small tool cut from a
/// scan of millions of triangles costs about the size of the tool.
pub fn boolean(a: &Mesh, b: &Mesh, op: crate::csg::Bool) -> R<Mesh> {
    use crate::csg::Bool;
    let (a, b) = (welded(a), welded(b));
    if a.is_empty() || b.is_empty() { return Err("a boolean needs two meshes with triangles".into()); }
    let (bvh_a, bvh_b) = (a.bvh(), b.bvh());
    let pad = DVec3::splat(1e-6);
    // Near triangles: those whose bounds overlap some triangle bound of the other mesh.
    let near = |m: &Mesh, other_bvh: &crate::mesh::Bvh| -> Vec<bool> {
        m.tris().map(|t| {
            let (lo, hi) = (t[0].min(t[1]).min(t[2]) - pad, t[0].max(t[1]).max(t[2]) + pad);
            !other_bvh.in_box(lo, hi).is_empty()
        }).collect()
    };
    let (near_a, near_b) = (near(&a, &bvh_b), near(&b, &bvh_a));
    let pick = |m: &Mesh, mask: &[bool], want: bool| -> Mesh { let tris: Vec<[DVec3; 3]> = m.tris().zip(mask).filter(|(_, n)| **n == want).map(|(t, _)| t).collect(); Mesh::from_tris(tris) };
    let (a_near, b_near) = (pick(&a, &near_a, true), pick(&b, &near_b, true));
    let _ = pick;
    if a_near.len() + b_near.len() > crate::csg::MAX_TRIS {
        return Err(format!("the meshes meet across {} triangles; booleans handle up to {} near the other surface, so decimate first", a_near.len() + b_near.len(), crate::csg::MAX_TRIS));
    }
    // Pieces of each near set that lie wholly inside or outside the other mesh.
    let classify = |pieces: Vec<[DVec3; 3]>, other: &Mesh, other_bvh: &crate::mesh::Bvh| -> (Vec<[DVec3; 3]>, Vec<[DVec3; 3]>) {
        let (mut inside, mut outside) = (Vec::new(), Vec::new());
        for t in pieces {
            let c = (t[0] + t[1] + t[2]) / 3.0;
            // A piece hugging the other surface (a sliver along the seam) is judged by which side of the
            // nearest triangle it lies on; anything farther off by ray parity. Coplanar pieces count as outside.
            let is_inside = match other_bvh.nearest(other, c) {
                Some((d, ti)) if d < 1e-4 => {
                    let tri = other.tri(ti);
                    (c - tri[0]).dot(other.normal(ti)) < -1e-9
                }
                _ => other_bvh.contains(other, c),
            };
            if is_inside { inside.push(t) } else { outside.push(t) }
        }
        (inside, outside)
    };
    let (a_in, a_out) = if b_near.is_empty() { (Vec::new(), a_near.tris().collect()) } else { classify(crate::csg::fragments(&b_near, &a_near), &b, &bvh_b) };
    let (b_in, b_out) = if a_near.is_empty() { (Vec::new(), b_near.tris().collect()) } else { classify(crate::csg::fragments(&a_near, &b_near), &a, &bvh_a) };
    let flip = |mut tris: Vec<[DVec3; 3]>| { for t in &mut tris { t.swap(1, 2); } tris };
    // Far triangles never cross the other surface, so each connected patch of them is wholly inside
    // or outside it: one parity test per patch decides, however many triangles it has.
    let far_split = |m: &Mesh, near: &[bool], other: &Mesh, other_bvh: &crate::mesh::Bvh| -> (Vec<[DVec3; 3]>, Vec<[DVec3; 3]>) {
        let adj = m.adjacency();
        let mut seen = vec![false; m.len()];
        let (mut inside, mut outside) = (Vec::new(), Vec::new());
        for start in 0..m.len() {
            if near[start] || seen[start] { continue; }
            let mut patch = vec![start];
            seen[start] = true;
            let mut k = 0;
            while k < patch.len() {
                for o in adj.neighbours(patch[k]) {
                    if !near[o] && !seen[o] { seen[o] = true; patch.push(o); }
                }
                k += 1;
            }
            let t = m.tri(start);
            let is_inside = other_bvh.contains(other, (t[0] + t[1] + t[2]) / 3.0);
            let into = if is_inside { &mut inside } else { &mut outside };
            into.extend(patch.iter().map(|i| m.tri(*i)));
        }
        (inside, outside)
    };
    let (a_far_in, a_far_out) = far_split(&a, &near_a, &b, &bvh_b);
    let (b_far_in, b_far_out) = far_split(&b, &near_b, &a, &bvh_a);
    let mut all: Vec<[DVec3; 3]> = Vec::new();
    match op {
        Bool::Union => { all.extend(a_far_out); all.extend(b_far_out); all.extend(a_out); all.extend(b_out); }
        Bool::Subtract => { all.extend(a_far_out); all.extend(a_out); all.extend(flip(b_far_in)); all.extend(flip(b_in)); }
        Bool::Intersect => { all.extend(a_far_in); all.extend(b_far_in); all.extend(a_in); all.extend(b_in); }
    }
    if all.is_empty() { return Ok(Mesh::default()); }
    let mut out = Mesh::from_tris(all);
    out.stitch();
    out.weld_exact();
    out.repair();
    // Splitting along the seam can drop a sliver whose corners all land on the cut; those leave
    // three-edge holes, closed here before the result is judged.
    if fill_holes(&mut out, 8) > 0 { out.repair(); }
    finish(out)
}

// ----- sculpting -----

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Brush {
    /// Pushes the surface in along its normal.
    Push,
    /// Pulls it out.
    #[default]
    Pull,
    /// Moves vertices toward the average of their neighbours.
    Smooth,
    /// Moves vertices toward the plane of the brush's centre.
    Flatten,
    /// Moves vertices along their own normals (a bulge).
    Inflate,
}

/// One brush stroke at a point: vertices within `radius` move by up to `strength`
/// (a length), with a smooth falloff to the rim.
pub fn sculpt(m: &Mesh, brush: Brush, at: DVec3, radius: f64, strength: f64) -> R<Mesh> {
    if !(radius > 0.0) { return Err("the brush radius must be greater than zero".into()); }
    let mut out = welded(m);
    let Some(centre_tri) = out.nearest_tri(at) else { return Err("the brush is not on the mesh".into()) };
    let normals = vertex_normals(&out);
    let tri = out.tri(centre_tri);
    let centre = {
        // Drop the point onto the surface.
        let n = out.normal(centre_tri);
        at - n * (at - tri[0]).dot(n)
    };
    let centre_normal = out.normal(centre_tri);
    let adj = out.adjacency();
    let positions = out.positions().to_vec();
    let falloff = |p: DVec3| { let d = p.distance(centre) / radius; if d >= 1.0 { 0.0 } else { let x = 1.0 - d * d; x * x } };
    let mut moved = 0;
    out.map_indexed(|i, p| {
        let w = falloff(p);
        if w <= 0.0 { return p; }
        moved += 1;
        match brush {
            Brush::Push => p - centre_normal * (strength * w),
            Brush::Pull => p + centre_normal * (strength * w),
            Brush::Inflate => p + normals[i] * (strength * w),
            Brush::Flatten => { let d = (p - centre).dot(centre_normal); p - centre_normal * d * (w * strength.clamp(0.0, 1.0)) }
            Brush::Smooth => {
                let ring = adj.ring(i as u32);
                if ring.is_empty() { return p; }
                let mean = ring.iter().map(|o| positions[*o as usize]).sum::<DVec3>() / ring.len() as f64;
                p + (mean - p) * (w * strength.clamp(0.0, 1.0))
            }
        }
    });
    if moved == 0 { return Err("no vertex lies within the brush; enlarge the radius or subdivide the mesh".into()); }
    finish(out)
}

// ----- measure -----

#[derive(Clone, Debug, Serialize)]
pub struct Measure {
    pub triangles: usize,
    pub vertices: usize,
    pub components: usize,
    pub open_edges: usize,
    pub non_manifold_edges: usize,
    pub watertight: bool,
    pub volume: f64,
    pub area: f64,
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// Wall thickness under a point: the distance from the surface there to the surface behind it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickness_at: Option<f64>,
}

pub fn measure(m: &Mesh, at: Option<DVec3>) -> Measure {
    let report = m.inspect();
    let (lo, hi) = m.bbox().unwrap_or_default();
    let thickness_at = at.and_then(|p| {
        let t = m.nearest_tri(p)?;
        let n = m.normal(t);
        let tri = m.tri(t);
        let on = (tri[0] + tri[1] + tri[2]) / 3.0;
        // Step inside and look for the far wall.
        m.ray(on - n * 1e-6, -n).map(|(d, _)| d + 1e-6)
    });
    Measure { triangles: m.len(), vertices: m.vertex_count(), components: report.components, open_edges: report.open_edges, non_manifold_edges: report.non_manifold_edges, watertight: report.watertight, volume: m.volume(), area: m.area(), min: lo.to_array(), max: hi.to_array(), thickness_at }
}

pub fn mask_count(mask: &[bool]) -> usize {
    mask.iter().filter(|x| **x).count()
}
