//! Exact solids: OpenCascade, reached through the `cadrum` binding. Bodies
//! built from sketches are kept as true surfaces (planes, cylinders, fillet
//! blends) and only turned into triangles for display and STL.
//!
//! OpenCascade can hand back a wrong solid without reporting an error, so
//! every operation here checks its result against what the operation must
//! do to the volume or the bounds.

use cadrum::{Boolean, Edge, Face, Solid, SurfaceKind, Tessellation};
use glam::{DVec2, DVec3};

use crate::csg::Bool;
use crate::mesh::Mesh;
use crate::profile::{Profile, Seg};
use crate::sketch::Plane;

/// A body's exact shape: one solid, or several when a cut has split it.
pub type Lumps = Vec<Solid>;

type R<T> = Result<T, String>;

fn c(v: DVec3) -> cadrum::DVec3 {
    cadrum::DVec3::from_array(v.to_array())
}

fn g(v: cadrum::DVec3) -> DVec3 {
    DVec3::from_array(v.to_array())
}

const FINE: Tessellation = Tessellation { deflection_linear: 0.02, deflection_angular: 0.15, relative_linear: false };

/// How close a point has to be to an edge or face to name it.
const NEAR: f64 = 1.0;

fn volume(lumps: &[Solid]) -> f64 {
    lumps.iter().map(Solid::volume).sum()
}

pub fn bounds(lumps: &[Solid]) -> Option<(DVec3, DVec3)> {
    lumps.iter().map(|s| s.bounding_box()).map(|b| (g(b[0]), g(b[1]))).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
}

/// One closed boundary as kernel edges, `z` off the plane.
fn ring(path: &[Seg], plane: &Plane, z: f64) -> R<Vec<Edge>> {
    let at = |p: DVec2| c(plane.to_world(p) + plane.normal() * z);
    path.iter()
        .map(|s| {
            match *s {
                Seg::Line(a, b) => Edge::line(at(a), at(b)),
                Seg::Arc(a, m, b) => Edge::arc_3pts(at(a), at(m), at(b)),
                Seg::Spline(points) => Edge::bspline(points.map(at).iter(), cadrum::BSplineEnd::NotAKnot),
                Seg::Circle(centre, r) => Edge::circle(r, c(plane.normal())).map(|e| e.translate(at(centre))),
            }
            .map_err(|e| format!("the profile has an edge the kernel rejects: {e}"))
        })
        .collect()
}

/// A profile's outer boundary followed by its holes.
fn outline(p: &Profile, plane: &Plane, z: f64) -> R<Vec<Edge>> {
    if p.path.is_empty() {
        return Err("the profile has no exact outline".into());
    }
    let mut edges = ring(&p.path, plane, z)?;
    for hole in &p.hole_paths {
        edges.extend(ring(hole, plane, z)?);
    }
    Ok(edges)
}

/// Joins solids that may or may not touch into as few lumps as they make.
fn fuse(parts: Vec<Solid>) -> R<Lumps> {
    let mut it = parts.iter();
    let Some(first) = it.next() else { return Ok(Vec::new()) };
    if parts.len() == 1 {
        return Ok(parts);
    }
    let mut all: Boolean<Solid> = first.into();
    for s in it {
        all = all + s;
    }
    tidy(all.build_vec().map_err(|e| format!("the kernel could not join the shapes: {e}"))?)
}

/// Merges faces that lie on the same surface, as a user expects after a join.
fn tidy(lumps: Vec<Solid>) -> R<Lumps> {
    Ok(lumps.into_iter().map(|s| s.clean().unwrap_or(s)).collect())
}

/// Sweeps profiles along the plane normal from offset `z0` to `z1`.
pub fn extrude(profiles: &[&Profile], plane: &Plane, z0: f64, z1: f64) -> R<Lumps> {
    let (z0, z1) = (z0.min(z1), z0.max(z1));
    if z1 - z0 < 1e-6 {
        return Err("the extrude distance is zero".into());
    }
    let parts: Vec<Solid> = profiles
        .iter()
        .map(|p| Solid::extrude(&outline(p, plane, z0)?, c(plane.normal() * (z1 - z0))).map_err(|e| format!("the kernel could not extrude the profile: {e}")))
        .collect::<R<_>>()?;
    fuse(parts)
}

/// Turns profiles around the line through `a` and `b` (sketch coordinates) by `degrees`.
pub fn revolve(profiles: &[&Profile], plane: &Plane, a: DVec2, b: DVec2, degrees: f64) -> R<Lumps> {
    if a.distance(b) < 1e-9 {
        return Err("the axis has no length".into());
    }
    let degrees = degrees.clamp(-360.0, 360.0);
    if degrees.abs() < 1e-6 {
        return Err("the revolve angle is zero".into());
    }
    let side = |p: DVec2| (b - a).perp_dot(p - a);
    for p in profiles {
        let (lo, hi) = p.outer.iter().map(|q| side(*q)).fold((0.0f64, 0.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
        if lo < -1e-6 && hi > 1e-6 {
            return Err("the profile crosses the axis".into());
        }
    }
    let (origin, dir) = (plane.to_world(a), plane.to_world(b) - plane.to_world(a));
    let parts: Vec<Solid> = profiles
        .iter()
        .map(|p| Solid::revolve(&outline(p, plane, 0.0)?, c(origin), c(dir), degrees.to_radians()).map_err(|e| format!("the kernel could not revolve the profile: {e}")))
        .collect::<R<_>>()?;
    fuse(parts)
}

/// `a` joined with, cut by or intersected with `b`. An empty result means nothing is left.
pub fn boolean(a: &[Solid], b: &[Solid], op: Bool) -> R<Lumps> {
    let fail = |e: cadrum::Error| format!("the kernel could not combine the shapes: {e}");
    let (va, vb) = (volume(a), volume(b));
    let out = match op {
        Bool::Union => fuse(a.iter().chain(b).cloned().collect())?,
        Bool::Subtract | Bool::Intersect => {
            let mut out = Vec::new();
            for lump in a {
                let mut expr: Boolean<Solid> = lump.into();
                if op == Bool::Subtract {
                    for tool in b {
                        expr = expr - tool;
                    }
                    out.extend(leftover(expr.build_vec()).map_err(fail)?);
                } else {
                    for tool in b {
                        out.extend(leftover((expr.clone() * tool).build_vec()).map_err(fail)?);
                    }
                }
            }
            tidy(out)?
        }
    };
    let v = volume(&out);
    let slack = 1e-6 * (va + vb).max(1.0);
    let sane = match op {
        Bool::Union => v >= va.max(vb) - slack && v <= va + vb + slack,
        Bool::Subtract => v <= va + slack && v >= va - vb - slack,
        Bool::Intersect => v <= va.min(vb) + slack,
    };
    if !sane || !v.is_finite() || v < -slack {
        return Err("the kernel returned a shape with the wrong volume for that operation".into());
    }
    Ok(out)
}

/// A boolean that leaves nothing reports "not one solid"; that is a valid empty result.
fn leftover(r: Result<Vec<Solid>, cadrum::Error>) -> Result<Vec<Solid>, cadrum::Error> {
    match r {
        Err(cadrum::Error::NotOne(0)) => Ok(Vec::new()),
        other => other,
    }
}

/// Triangles for display and STL, the face each came from, and the edges as polylines.
pub fn tessellate(lumps: &[Solid]) -> R<(Mesh, Vec<Vec<DVec3>>)> {
    if lumps.is_empty() {
        return Ok((Mesh::default(), Vec::new()));
    }
    let m = Solid::mesh(lumps.iter(), FINE).map_err(|e| format!("the kernel could not triangulate the body: {e}"))?;
    let mut mesh = Mesh::default();
    for (i, t) in m.indices.chunks_exact(3).enumerate() {
        let tri = [g(m.vertices[t[0]]), g(m.vertices[t[1]]), g(m.vertices[t[2]])];
        // Blend surfaces can collapse a triangle to nothing at a pole.
        if (tri[1] - tri[0]).cross(tri[2] - tri[0]).length_squared() > 1e-20 {
            mesh.tris.push(tri);
            mesh.face_ids.push(m.face_ids[i]);
        }
    }
    let edges = lumps.iter().flat_map(real_edges).map(|e| e.approximation_segments(FINE).into_iter().map(g).collect::<Vec<_>>()).filter(|e| e.len() >= 2).collect();
    Ok((mesh, edges))
}

/// The edges of a solid that separate two faces. A cylinder or sphere also
/// carries a seam, where its one face meets itself; that is not an edge anyone can see.
fn real_edges(s: &Solid) -> impl Iterator<Item = &Edge> {
    let mut uses: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for f in s.iter_face() {
        for e in f.iter_edge() {
            *uses.entry(e.id()).or_insert(0) += 1;
        }
    }
    s.iter_edge().filter(move |e| uses.get(&e.id()).copied().unwrap_or(0) >= 2)
}

/// Where a point picked on a body probably is now. Each pick is tried where
/// it was, and where it would be had it kept its place within the body's
/// bounds, so edges survive both a body that grows and features that stay put.
pub fn candidates(lumps: &[Solid], points: &[DVec3], frame: Option<[DVec3; 2]>) -> Vec<[DVec3; 2]> {
    let now = bounds(lumps);
    points
        .iter()
        .map(|p| match (frame, now) {
            (Some([lo, hi]), Some((nlo, nhi))) => {
                let size = (hi - lo).max(DVec3::splat(1e-9));
                [*p, nlo + (*p - lo) / size * (nhi - nlo)]
            }
            _ => [*p, *p],
        })
        .collect()
}

/// The lump and edge nearest each point.
fn find_edges<'a>(lumps: &'a [Solid], points: &[[DVec3; 2]]) -> R<Vec<(usize, &'a Edge)>> {
    points
        .iter()
        .map(|tries| {
            lumps
                .iter()
                .enumerate()
                .flat_map(|(i, s)| real_edges(s).map(move |e| (i, e)))
                .map(|(i, e)| (tries.iter().map(|p| g(e.project(c(*p)).0).distance(*p)).fold(f64::MAX, f64::min), i, e))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .filter(|hit| hit.0 <= NEAR)
                .map(|hit| (hit.1, hit.2))
                .ok_or_else(|| "an edge it used is no longer there; the body changed shape under it".to_owned())
        })
        .collect()
}

fn find_faces<'a>(lumps: &'a [Solid], points: &[[DVec3; 2]]) -> R<Vec<(usize, &'a Face)>> {
    points
        .iter()
        .map(|tries| {
            lumps
                .iter()
                .enumerate()
                .flat_map(|(i, s)| s.iter_face().map(move |f| (i, f)))
                .map(|(i, f)| (tries.iter().map(|p| g(f.project(c(*p)).0).distance(*p)).fold(f64::MAX, f64::min), i, f))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .filter(|hit| hit.0 <= NEAR)
                .map(|hit| (hit.1, hit.2))
                .ok_or_else(|| "a face it used is no longer there; the body changed shape under it".to_owned())
        })
        .collect()
}

/// Rounds (or, with `chamfer`, bevels) the edges nearest `points` by `size`.
pub fn blend(lumps: &[Solid], points: &[[DVec3; 2]], size: f64, chamfer: bool) -> R<Lumps> {
    let what = if chamfer { "chamfer" } else { "fillet" };
    if size <= 0.0 {
        return Err(format!("the {what} size must be greater than zero"));
    }
    let picks = find_edges(lumps, points)?;
    let mut out = Vec::new();
    for (i, lump) in lumps.iter().enumerate() {
        let mut edges: Vec<&Edge> = picks.iter().filter(|p| p.0 == i).map(|p| p.1).collect();
        edges.sort_by_key(|e| e.id());
        edges.dedup_by_key(|e| e.id());
        if edges.is_empty() {
            out.push(lump.clone());
            continue;
        }
        let made = if chamfer { lump.chamfer_edges(size, edges) } else { lump.fillet_edges(size, edges) }.map_err(|_| format!("that {what} is too big for these edges, or they cannot be blended together"))?;
        // A blend only ever takes off or fills in corners, so it can never reach outside the
        // body. Measured on the triangles, because the kernel's own bounds are loose on curves.
        let tight = |s: &Solid| tessellate(std::slice::from_ref(s)).ok().and_then(|t| t.0.bbox());
        let grew = match (tight(lump), tight(&made)) {
            (Some((lo, hi)), Some((nlo, nhi))) => (lo - nlo).max_element() > 0.05 || (nhi - hi).max_element() > 0.05,
            _ => true,
        };
        if grew || !(made.volume() > 0.0) || (made.volume() - lump.volume()).abs() < 1e-12 {
            return Err(format!("that {what} is too big for these edges"));
        }
        out.push(made);
    }
    Ok(out)
}

/// Hollows the body to a wall of `thickness`, open at the faces nearest `points`.
pub fn shell(lumps: &[Solid], points: &[[DVec3; 2]], thickness: f64) -> R<Lumps> {
    if thickness <= 0.0 {
        return Err("the wall thickness must be greater than zero".into());
    }
    if points.is_empty() {
        return Err("choose at least one face to leave open".into());
    }
    let picks = find_faces(lumps, points)?;
    let mut out = Vec::new();
    for (i, lump) in lumps.iter().enumerate() {
        let mut faces: Vec<&Face> = picks.iter().filter(|p| p.0 == i).map(|p| p.1).collect();
        faces.sort_by_key(|f| f.id());
        faces.dedup_by_key(|f| f.id());
        if faces.is_empty() {
            out.push(lump.clone());
            continue;
        }
        let made = lump.shell(-thickness, faces).map_err(|_| "the body cannot be hollowed to that wall thickness".to_owned())?;
        // A shell that did not carve anything out is the kernel failing quietly.
        if !(made.volume() > 0.0) || made.volume() > lump.volume() * (1.0 - 1e-9) {
            return Err("the wall is too thick to leave a cavity".into());
        }
        out.push(made);
    }
    Ok(out)
}

/// What a drill and its countersink or counterbore take out, in millimetres and degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drill {
    pub diameter: f64,
    pub depth: f64,
    /// The included angle of a pointed bottom; `None` leaves it flat.
    pub tip_angle: Option<f64>,
    pub head: Option<DrillHead>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrillHead {
    Counterbore { diameter: f64, depth: f64 },
    /// `angle` is the included angle of the cone.
    Countersink { diameter: f64, angle: f64 },
}

/// The space a hole takes: entering at `at`, going along `dir`. It starts a
/// little outside the surface so that a curved or uneven face is cut clean through.
pub fn drill(at: DVec3, dir: DVec3, d: &Drill) -> R<Lumps> {
    let dir = dir.try_normalize().ok_or("the hole has no direction")?;
    let r = d.diameter / 2.0;
    if !(r > 0.0) {
        return Err("the hole diameter must be greater than zero".into());
    }
    if !(d.depth > 0.0) {
        return Err("the hole depth must be greater than zero".into());
    }
    let cylinder = |radius: f64, from: f64, to: f64| Solid::cylinder(radius, c(dir * (to - from))).translate(c(at + dir * from));
    let head_r = match d.head {
        Some(DrillHead::Counterbore { diameter, .. } | DrillHead::Countersink { diameter, .. }) => diameter / 2.0,
        None => r,
    };
    if d.head.is_some() && head_r <= r + 1e-6 {
        return Err("the counterbore or countersink must be wider than the hole".into());
    }
    let lead = head_r;
    let mut parts = vec![cylinder(r, -lead, d.depth)];
    if let Some(angle) = d.tip_angle {
        if !(angle > 1.0 && angle < 179.0) {
            return Err("the drill point angle must be between 1 and 179 degrees".into());
        }
        parts.push(Solid::cone(r, 0.0, c(dir * (r / (angle.to_radians() / 2.0).tan()))).translate(c(at + dir * d.depth)));
    }
    match d.head {
        Some(DrillHead::Counterbore { depth, .. }) => {
            if !(depth > 0.0) {
                return Err("the counterbore depth must be greater than zero".into());
            }
            parts.push(cylinder(head_r, -lead, depth));
        }
        Some(DrillHead::Countersink { angle, .. }) => {
            if !(angle > 1.0 && angle < 179.0) {
                return Err("the countersink angle must be between 1 and 179 degrees".into());
            }
            parts.push(cylinder(head_r, -lead, 0.0));
            parts.push(Solid::cone(head_r, r, c(dir * ((head_r - r) / (angle.to_radians() / 2.0).tan()))).translate(c(at)));
        }
        None => {}
    }
    let tool = fuse(parts)?;
    if tool.len() != 1 || !(volume(&tool) > 0.0) {
        return Err("the kernel could not build the hole's shape".into());
    }
    Ok(tool)
}

/// A plain cylinder from `from` to `to`.
pub fn cylinder(from: DVec3, to: DVec3, radius: f64) -> R<Lumps> {
    if from.distance(to) < 1e-9 || !(radius > 0.0) {
        return Err("the cylinder has no size".into());
    }
    Ok(vec![Solid::cylinder(radius, c(to - from)).translate(c(from))])
}

/// A cylindrical face, as a thread needs to know it.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Barrel {
    /// A point on the axis where the face starts, and the direction it runs.
    pub start: DVec3,
    pub axis: DVec3,
    pub length: f64,
    pub radius: f64,
    /// The inside of a hole, rather than the outside of a rod.
    pub internal: bool,
}

/// The cylindrical face nearest the point, with any other pieces of the same cylinder.
pub fn barrel(lumps: &[Solid], tries: &[DVec3; 2]) -> R<Barrel> {
    let (_, face) = find_faces(lumps, std::slice::from_ref(tries))?[0];
    let (Some(surface), Some(radius)) = (face.surface(), face.surface().and_then(|s| if let SurfaceKind::Cylinder { radius } = s.kind { Some(radius) } else { None })) else {
        return Err("a thread goes on a cylindrical face; that face is not one".into());
    };
    let (origin, axis) = (g(surface.origin), g(surface.axis_z).normalize());
    let same = |f: &Face| {
        f.surface().is_some_and(|s| {
            let on_axis = (g(s.origin) - origin).cross(axis).length() < 1e-6;
            matches!(s.kind, SurfaceKind::Cylinder { radius: r } if (r - radius).abs() < 1e-6) && g(s.axis_z).cross(axis).length() < 1e-6 && on_axis
        })
    };
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for f in lumps.iter().flat_map(|s| s.iter_face()).filter(|f| same(f)) {
        for p in f.iter_edge().flat_map(|e| e.approximation_segments(FINE)) {
            let along = (g(p) - origin).dot(axis);
            (lo, hi) = (lo.min(along), hi.max(along));
        }
    }
    if hi - lo < 1e-6 {
        return Err("that cylindrical face has no length".into());
    }
    // The kernel's normals point out of the material, so on a hole they point at the axis.
    let (on, normal) = face.project(c(tries[0]));
    let (on, normal) = (g(on), g(normal));
    let outward = on - origin - axis * (on - origin).dot(axis);
    Ok(Barrel { start: origin + axis * lo, axis, length: hi - lo, radius, internal: normal.dot(outward) < 0.0 })
}

/// The shrinking conical chamfer immediately beyond a rod's cylindrical end.
/// It must share the axis and the end ring; a wider shoulder or screw head is not a lead-in.
pub fn end_chamfer(lumps: &[Solid], at: DVec3, outward: DVec3, radius: f64) -> Option<f64> {
    lumps.iter().flat_map(|s| s.iter_face()).filter_map(|face| {
        let surface = face.surface()?;
        if !matches!(surface.kind, SurfaceKind::Cone { .. })
            || g(surface.axis_z).cross(outward).length() > 1e-6
            || (g(surface.origin) - at).cross(outward).length() > 1e-6 { return None; }
        let pts: Vec<_> = face.iter_edge().flat_map(|e| e.approximation_segments(FINE)).map(g).collect();
        let along = |p: DVec3| (p - at).dot(outward);
        let radial = |p: DVec3| (p - at - outward * along(p)).length();
        let lo = pts.iter().map(|p| along(*p)).fold(f64::INFINITY, f64::min);
        let hi = pts.iter().map(|p| along(*p)).fold(f64::NEG_INFINITY, f64::max);
        if lo.abs() > 1e-6 || hi <= 1e-6 || pts.iter().any(|p| radial(*p) > radius + 1e-6) { return None; }
        let joins = pts.iter().any(|p| along(*p).abs() < 1e-6 && (radial(*p) - radius).abs() < 1e-6);
        let narrows = pts.iter().filter(|p| (along(**p) - hi).abs() < 1e-6).all(|p| radial(*p) < radius - 1e-6);
        let (on, normal) = face.project(face.center());
        let on = g(on);
        let radial_out = on - at - outward * along(on);
        (joins && narrows && g(normal).dot(radial_out) > 0.0).then_some(hi)
    }).max_by(f64::total_cmp)
}

/// Where a copy or move puts a body.
#[derive(Clone, Copy, Debug)]
pub enum Place {
    Shift(DVec3),
    /// About an axis through `origin`, in radians.
    Turn { origin: DVec3, axis: DVec3, angle: f64 },
    /// About `centre`.
    Scale { centre: DVec3, factor: f64 },
    Mirror { origin: DVec3, normal: DVec3 },
}

impl Place {
    pub fn point(&self, p: DVec3) -> DVec3 {
        match *self {
            Place::Shift(v) => p + v,
            Place::Turn { origin, axis, angle } => origin + glam::DQuat::from_axis_angle(axis.normalize(), angle) * (p - origin),
            Place::Scale { centre, factor } => centre + (p - centre) * factor,
            Place::Mirror { origin, normal } => {
                let n = normal.normalize();
                p - n * 2.0 * (p - origin).dot(n)
            }
        }
    }

    /// Whether the move turns a mesh inside out.
    pub fn flips(&self) -> bool {
        matches!(self, Place::Mirror { .. }) || matches!(self, Place::Scale { factor, .. } if *factor < 0.0)
    }
}

pub fn place(lumps: Lumps, how: &Place) -> Lumps {
    lumps
        .into_iter()
        .map(|s| match *how {
            Place::Shift(v) => s.translate(c(v)),
            Place::Turn { origin, axis, angle } => s.rotate(c(origin), c(axis), angle),
            Place::Scale { centre, factor } => s.scale(c(centre), factor),
            Place::Mirror { origin, normal } => s.mirror(c(origin), c(normal)),
        })
        .collect()
}

/// A STEP file of the solids, in millimetres.
pub fn step<'a>(solids: impl IntoIterator<Item = &'a Solid>) -> R<Vec<u8>> {
    let mut out = Vec::new();
    Solid::write_step(solids, &mut out).map_err(|e| format!("the kernel could not write STEP: {e}"))?;
    Ok(out)
}

pub struct EdgeInfo {
    /// A point on the edge, for naming it in commands.
    pub mid: DVec3,
    pub length: f64,
    pub straight: bool,
}

pub struct FaceInfo {
    /// A point on the face, for naming it in commands.
    pub at: DVec3,
    pub normal: DVec3,
    pub area: f64,
    pub kind: &'static str,
}

pub fn edges(lumps: &[Solid]) -> Vec<EdgeInfo> {
    lumps
        .iter()
        .flat_map(real_edges)
        .filter_map(|e| {
            let pts: Vec<DVec3> = e.approximation_segments(FINE).into_iter().map(g).collect();
            let length: f64 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
            // Halfway along, by length, so that it lies on the edge itself.
            let mut left = length / 2.0;
            let mut mid = *pts.first()?;
            for w in pts.windows(2) {
                let d = w[0].distance(w[1]);
                if left <= d {
                    mid = w[0].lerp(w[1], if d > 0.0 { left / d } else { 0.0 });
                    break;
                }
                left -= d;
            }
            Some(EdgeInfo { mid, length, straight: pts.len() == 2 })
        })
        .collect()
}

pub fn faces(lumps: &[Solid]) -> Vec<FaceInfo> {
    lumps
        .iter()
        .flat_map(|s| s.iter_face())
        .map(|f| {
            let (at, normal) = f.project(f.center());
            let kind = match f.surface().map(|s| s.kind) {
                Some(SurfaceKind::Plane) => "plane",
                Some(SurfaceKind::Cylinder { .. }) => "cylinder",
                Some(SurfaceKind::Cone { .. }) => "cone",
                Some(SurfaceKind::Sphere { .. }) => "sphere",
                Some(SurfaceKind::Torus { .. }) => "torus",
                None => "freeform",
            };
            FaceInfo { at: g(at), normal: g(normal), area: f.area(), kind }
        })
        .collect()
}
