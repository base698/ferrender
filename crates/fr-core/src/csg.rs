//! Solid mesh Boolean operations through the Manifold kernel.
//!
//! Indexed topology stays intact across the solver boundary. The native Rust
//! port uses Manifold's symbolic perturbation algorithm on clean operands and
//! exact rational arrangements for self-intersecting operands. There is no BSP
//! or ray-parity fallback that can silently return a partially cut surface.

use std::time::{Duration, Instant};

use glam::DVec3;
use manifold_rust::{
    cancel::CancelToken,
    manifold::Manifold,
    types::{BooleanEngine, Error, MeshGL64, OpType},
};

use crate::mesh::{MAX_TRIANGLES, Mesh};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bool { Union, Subtract, Intersect }

/// Boolean working sets are larger than imported meshes. Larger meshes remain
/// importable, but must be decimated before combining.
pub const MAX_TRIS: usize = 8_000_000;
const MAX_CROSS_PAIRS: usize = 2_000_000;
const MAX_SELF_PAIRS: usize = 2_000_000;
const MAX_PAIR_EXAMINATIONS: usize = 256_000_000;
const TIME_LIMIT: Duration = Duration::from_secs(60);
const BUDGET_ERROR: &str = "the mesh intersection exceeds the Boolean work budget; decimate the meshes or use a smaller tool before combining";

fn check_time(start: Instant, limit: Duration) -> Result<(), String> {
    if start.elapsed() >= limit { Err("the mesh Boolean reached its time limit; decimate the meshes or use a smaller tool before combining".into()) }
    else { Ok(()) }
}

/// Bound overlapping triangle-box pairs before the solver allocates its
/// intersection graph. A shared vertex is not an interior self-intersection.
/// The cap applies to actual triangle bounds, not merely overlapping BVH leaves.
fn check_pairs(a: &Mesh, b: &Mesh, same: bool, max: usize, max_examined: usize, start: Instant, limit: Duration) -> Result<usize, String> {
    let bvh = b.bvh();
    let mut count = 0usize;
    let mut examined = 0usize;
    for (i, t) in a.tris().enumerate() {
        if i % 1024 == 0 { check_time(start, limit)?; }
        let (lo, hi) = (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
        for j in bvh.in_box(lo, hi) {
            // High-valence fans can visit many pairs that are later skipped.
            // Count/poll those too, so a single triangle cannot hide unbounded
            // work between the outer-loop deadline checks.
            examined += 1;
            if examined > max_examined { return Err(BUDGET_ERROR.into()); }
            if examined % 4096 == 0 { check_time(start, limit)?; }
            if same && (j <= i || a.indices()[i].iter().any(|v| b.indices()[j].contains(v))) { continue; }
            let u = b.tri(j);
            if u[0].min(u[1]).min(u[2]).cmpgt(hi).any() || u[0].max(u[1]).max(u[2]).cmplt(lo).any() { continue; }
            count += 1;
            if count > max { return Err(BUDGET_ERROR.into()); }
        }
    }
    Ok(count)
}

fn input(m: &Mesh) -> Result<Manifold, String> {
    let gl = MeshGL64 {
        num_prop: 3,
        vert_properties: m.positions().iter().flat_map(|p| p.to_array()).collect(),
        tri_verts: m.indices().iter().flatten().map(|i| *i as u64).collect(),
        ..Default::default()
    };
    let solid = Manifold::from_mesh_gl64(&gl);
    if solid.status() != Error::NoError {
        return Err(format!("mesh booleans require closed, manifold inputs; Manifold rejected an operand ({:?}); use Mesh Repair before combining", solid.status()));
    }
    // Collapse only redundant coplanar/collinear tessellation at the kernel's
    // existing tolerance. This is topology cleanup, not mesh decimation: a
    // densely subdivided planar face should enter the Boolean as that face.
    Ok(solid.simplify(0.0))
}

fn output(solid: &Manifold) -> Result<Mesh, String> {
    if solid.num_tri() > MAX_TRIANGLES { return Err(BUDGET_ERROR.into()); }
    // A face-only contact can be represented as a closed zero-volume sheet by
    // the kernel. Ferrender Booleans operate on solids, so it is empty.
    if solid.is_empty() || solid.volume() == 0.0 { return Ok(Mesh::default()); }
    let gl = solid.get_mesh_gl64(-1);
    // Even without user properties, export may duplicate vertices at source
    // face/run boundaries. Its merge vectors carry the authoritative topology.
    let mut remap: Vec<u32> = (0..gl.num_vert() as u32).collect();
    for (&from, &to) in gl.merge_from_vert.iter().zip(&gl.merge_to_vert) {
        if from as usize >= remap.len() || to as usize >= remap.len() { return Err("the mesh Boolean returned invalid vertex links".into()); }
        remap[from as usize] = to as u32;
    }
    let positions = gl.vert_properties.chunks_exact(gl.num_prop as usize).map(|p| DVec3::new(p[0], p[1], p[2])).collect();
    let indices = gl.tri_verts.chunks_exact(3).map(|t| [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]]).collect();
    let mut out = Mesh::from_indexed(positions, indices, true)?;
    // The container and STL formats use f32 coordinates. Check the actual
    // representable result now; a watertight f64 result alone is insufficient.
    out.snap();
    // STL has no indices. Vertices identical at file precision must also be
    // safe to merge, otherwise a nominally closed indexed result exports as
    // non-manifold edges (the real relief/plaque regression exercises this).
    out.weld_rounded_positions();
    out.compact();
    let report = out.inspect();
    if !report.watertight || report.flipped != 0 {
        return Err(format!("the mesh boolean could not store a closed, manifold result at file precision ({} open edges, {} non-manifold edges, {} collapsed triangles); move the model closer to the origin or increase its smallest details before combining", report.open_edges, report.non_manifold_edges, report.degenerate_removed));
    }
    // Exact coordinate welding can leave zero-area triangles (for example,
    // two adjacent cut points round to one). Inspection already judged the
    // surface after excluding those and duplicate faces. Apply precisely that
    // cleanup only if it stays closed and needs no winding changes; never fill
    // holes, move vertices, or flip a shell to force Boolean acceptance.
    if report.degenerate_removed != 0 || report.duplicates_removed != 0 { out.repair(); }
    // Check solid volume as well as connectivity. The allowance is bounded
    // by the actual f32 coordinate quantum times surface area, not a user-
    // invisible modeling tolerance or a percentage of the desired feature.
    let (lo, hi) = out.bbox().ok_or("the mesh Boolean lost its solid bounds")?;
    let quantum = lo.abs().max(hi.abs()).max_element() * f32::EPSILON as f64;
    let volume = solid.volume().abs();
    let rounding_bound = 2.0 * solid.surface_area() * quantum + volume * 1e-10;
    if (out.volume() - volume).abs() > rounding_bound {
        return Err("the mesh Boolean cannot preserve the solid's volume at file precision; move it closer to the origin or increase its smallest details before combining".into());
    }
    out.validate()?;
    Ok(out)
}

pub fn boolean(a: &Mesh, b: &Mesh, op: Bool) -> Result<Mesh, String> {
    boolean_with_limit(a, b, op, TIME_LIMIT)
}

fn boolean_with_limit(a: &Mesh, b: &Mesh, op: Bool, limit: Duration) -> Result<Mesh, String> {
    let start = Instant::now();
    a.validate()?; b.validate()?;
    if a.len().saturating_add(b.len()) > MAX_TRIS { return Err(format!("mesh booleans support at most {MAX_TRIS} input triangles in total; decimate the meshes before combining")); }
    if a.is_empty() || b.is_empty() { return Err("a boolean needs two meshes with triangles".into()); }
    let (mut a, mut b) = (a.clone(), b.clone());
    a.weld_exact(); b.weld_exact();
    if !a.repair().watertight || !b.repair().watertight {
        return Err("mesh booleans require closed, manifold inputs; use Mesh Repair to close holes and remove non-manifold edges before combining".into());
    }
    check_time(start, limit)?;
    check_pairs(&a, &a, true, MAX_SELF_PAIRS, MAX_PAIR_EXAMINATIONS, start, limit)?;
    check_pairs(&b, &b, true, MAX_SELF_PAIRS, MAX_PAIR_EXAMINATIONS, start, limit)?;
    let pairs = check_pairs(&a, &b, false, MAX_CROSS_PAIRS, MAX_PAIR_EXAMINATIONS, start, limit)?;
    if a.len().saturating_add(b.len()).saturating_add(pairs.saturating_mul(4)) > MAX_TRIANGLES { return Err(BUDGET_ERROR.into()); }
    let token = CancelToken::new();
    let (done, completed) = std::sync::mpsc::channel::<()>();
    let cancel = token.clone();
    let remaining = limit.saturating_sub(start.elapsed());
    let timer = std::thread::Builder::new().name("mesh-boolean-deadline".into()).spawn(move || {
        if completed.recv_timeout(remaining) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) { cancel.cancel(); }
    }).map_err(|e| format!("could not start the mesh Boolean deadline: {e}"))?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (a, b) = (input(&a)?, input(&b)?);
        check_time(start, limit)?;
        let op = match op { Bool::Union => OpType::Add, Bool::Subtract => OpType::Subtract, Bool::Intersect => OpType::Intersect };
        let result = a.boolean_with_engine_and_token(&b, op, BooleanEngine::Auto, Some(&token));
        check_time(start, limit)?;
        if result.status() != Error::NoError { return Err(format!("the mesh Boolean could not complete ({:?}); repair or decimate the inputs before combining", result.status())); }
        match output(&result) {
            Ok(mesh) => Ok(mesh),
            // Near-coincident tessellations can survive the fast symbolic
            // path as microscopic sliver shells. Recompute from the original
            // operands using exact rational arrangements, never patch a seam
            // or relax the acceptance test on an already damaged result.
            Err(error) if error.contains("file precision") => {
                let exact = a.boolean_with_engine_and_token(&b, op, BooleanEngine::Robust, Some(&token));
                check_time(start, limit)?;
                if exact.status() != Error::NoError { return Err(format!("the exact mesh Boolean could not complete ({:?}); repair or decimate the inputs before combining",exact.status())); }
                output(&exact)
            }
            Err(error) => Err(error),
        }
    })).unwrap_or_else(|_| Err("the mesh Boolean solver failed; the original bodies have been preserved".into()));
    let result = result.and_then(|mesh| { check_time(start, limit)?; Ok(mesh) });
    let _ = done.send(());
    let _ = timer.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_deadline_refuses_without_running_solver() {
        let solid = Manifold::cube(manifold_rust::linalg::Vec3::splat(1.0), true);
        let mesh = output(&solid).unwrap();
        assert!(boolean_with_limit(&mesh, &mesh, Bool::Union, Duration::ZERO).unwrap_err().contains("time limit"));
    }
    #[test]
    fn skipped_shared_vertex_candidates_still_consume_work_budget() {
        let mesh = Mesh::from_indexed(vec![DVec3::ZERO,DVec3::X,DVec3::Y],vec![[0,1,2];16],true).unwrap();
        assert!(check_pairs(&mesh, &mesh, true, usize::MAX, 32, Instant::now(), TIME_LIMIT).unwrap_err().contains("work budget"));
    }
    #[test]
    fn intersection_candidates_are_bounded_before_solver_allocation() {
        let m = Mesh::from_tris(vec![[DVec3::ZERO, DVec3::X, DVec3::Y]; 16]);
        assert!(check_pairs(&m, &m, false, 100, MAX_PAIR_EXAMINATIONS, Instant::now(), TIME_LIMIT).unwrap_err().contains("work budget"));
    }
}
