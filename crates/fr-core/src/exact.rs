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
use crate::sketch::{Id, Plane};
use crate::tag::{EdgeTag, Kind, Level, Origin, Tag};

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
    Ok(lumps.into_iter().map(|s| {
        // UnifySameDomain can both corrupt a trimmed torus and change the
        // topology passed to it. Keep an independent original for the fallback.
        let before=s.volume();
        let cleaned=s.clone().clean();
        match cleaned {
            Ok(cleaned) if cleaned.volume().is_finite() && (cleaned.volume()-before).abs()<=1e-7*before.abs().max(1.0) => cleaned,
            _ => s,
        }
    }).collect())
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
    if !out.is_empty() {
        // Some broken periodic trims have plausible exact mass properties but
        // triangulate to a missing or inverted surface. Refuse those results,
        // rather than displaying/exporting a body different from its exact one.
        // Deflection times surface area is a conservative volume error bound;
        // it also avoids false failures on tiny or very thin curved bodies.
        let (mesh,_)=tessellate(&out)?;
        let mesh_volume=mesh.volume();
        let tolerance=2.0*FINE.deflection_linear*out.iter().map(Solid::area).sum::<f64>()+slack;
        if !mesh_volume.is_finite() || mesh_volume < -slack || (mesh_volume-v).abs()>tolerance {
            return Err("the kernel could not make a reliable closed surface for that operation; try simplifying the intersecting faces".into());
        }
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
    let mut tris = Vec::with_capacity(m.indices.len() / 3);
    let mut face_ids = Vec::with_capacity(m.indices.len() / 3);
    for (i, t) in m.indices.chunks_exact(3).enumerate() {
        let tri = [g(m.vertices[t[0]]), g(m.vertices[t[1]]), g(m.vertices[t[2]])];
        // Blend surfaces can collapse a triangle to nothing at a pole.
        if (tri[1] - tri[0]).cross(tri[2] - tri[0]).length_squared() > 1e-20 {
            tris.push(tri);
            face_ids.push(m.face_ids[i]);
        }
    }
    let mut mesh = Mesh::from_tris(tris);
    mesh.face_ids = face_ids;
    let edges = lumps.iter().flat_map(real_edges).map(|e| e.approximation_segments(FINE).into_iter().map(g).collect::<Vec<_>>()).filter(|e| e.len() >= 2).collect();
    Ok((mesh, edges))
}

/// A flat face's true edge curves for a frozen face sketch. The binding exposes
/// edge samples and tangents rather than curve kinds, so only promote a sampled
/// edge to a circle when both its samples and tangents agree at kernel precision.
/// Sampling each kernel edge separately keeps deliberate polygon facets straight.
pub fn face_edges(lumps: &[Solid], id: u64, plane: Plane, at: DVec3) -> Option<Vec<Seg>> {
    let matches=|face: &&Face| {
        let Some(surface)=face.surface() else {return false};
        matches!(surface.kind,SurfaceKind::Plane)
            && g(surface.axis_z).dot(plane.normal()).abs()>1.0-1e-8
            && (g(surface.origin)-plane.origin).dot(plane.normal()).abs()<1e-6
            && g(face.project(c(at)).0).distance(at)<1e-6
    };
    // A kernel history ID alone is not a unique spatial face. Also check its
    // supporting plane and that the picked point lies within the trimmed face.
    let face = lumps.iter().flat_map(|s|s.iter_face()).filter(matches).min_by_key(|f|f.id()!=id)?;
    let mut out=Vec::new();
    for edge in face.iter_edge() {
        let points:Vec<_>=edge.approximation_segments(FINE).into_iter().map(|p|plane.to_local(g(p))).collect();
        if points.len()<2 {continue;}
        let (a,b)=(points[0],*points.last()?);
        let tolerance=Edge::precision_distance().max(1e-9);
        let closed=a.distance(b)<=tolerance;
        let middle=points[points.len()/2];
        let fit=if closed && points.len()>=5 { crate::sketch::arc3_center(a,points[points.len()/3],points[2*points.len()/3]) }
            else { crate::sketch::arc3_center(a,middle,b) };
        if let Ok(centre)=fit {
            let radius=a.distance(centre);
            let circular=points.iter().all(|p|(p.distance(centre)-radius).abs()<=tolerance)
                && [a,middle,b].iter().all(|p| {
                    let (_,tangent)=edge.project(c(plane.to_world(*p)));
                    let t=g(tangent).normalize_or_zero();
                    t.length_squared()>0.5 && t.dot(plane.to_world(*p)-plane.to_world(centre)).abs()<=tolerance
                });
            if circular {
                out.push(if closed {Seg::Circle(centre,radius)} else {Seg::Arc(a,middle,b)});
                continue;
            }
        }
        // Non-circular curves still use their display approximation. This is
        // intentionally not a best-fit arc, which could change the face's shape.
        out.extend(points.windows(2).filter(|p|p[0].distance(p[1])>tolerance).map(|p|Seg::Line(p[0],p[1])));
    }
    (!out.is_empty()).then_some(out)
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

// ----- face and edge tags -----

/// Face tags per lump, in the kernel's face order. Kept beside a body's lumps;
/// the order survives clones and rigid moves, which is why tags are positional
/// rather than keyed by the kernel's process-local face ids.
pub type Tags = Vec<Vec<Option<Tag>>>;

pub fn kind_of(face: &Face) -> Kind {
    match face.surface().map(|s| s.kind) {
        Some(SurfaceKind::Plane) => Kind::Plane,
        Some(SurfaceKind::Cylinder { .. }) => Kind::Cylinder,
        Some(SurfaceKind::Cone { .. }) => Kind::Cone,
        Some(SurfaceKind::Sphere { .. }) => Kind::Sphere,
        Some(SurfaceKind::Torus { .. }) => Kind::Torus,
        None => Kind::Freeform,
    }
}

/// No tags: a body read from a file or made before tagging existed.
pub fn no_tags(lumps: &[Solid]) -> Tags {
    lumps.iter().map(|s| vec![None; s.iter_face().count()]).collect()
}

/// Every face named `Made` by `feature`, in kernel order: primitives, text, drills.
pub fn fresh_tags(lumps: &[Solid], feature: Id) -> Tags {
    let mut n = 0;
    lumps.iter().map(|s| s.iter_face().map(|f| { let t = Tag::new(Origin::Made { feature, n }, kind_of(f)); n += 1; Some(t) }).collect()).collect()
}

/// A point on a segment away from its ends, lifted off the sketch plane by `lift`.
fn probe(seg: &Seg, plane: &Plane, lift: f64) -> DVec3 {
    let p = match *seg {
        Seg::Line(a, b) => (a + b) / 2.0,
        Seg::Arc(_, m, _) => m,
        Seg::Circle(centre, r) => centre + DVec2::X * r,
        Seg::Spline(points) => points[1],
    };
    plane.to_world(p) + plane.normal() * lift
}

fn segments<'a>(profiles: &'a [&Profile]) -> impl Iterator<Item = (&'a Seg, Id)> + 'a {
    profiles.iter().flat_map(|p| {
        let outer = p.path.iter().zip(p.path_ids.iter().copied().chain(std::iter::repeat(0)));
        let holes = p.hole_paths.iter().zip(p.hole_path_ids.iter().map(|v| v.as_slice()).chain(std::iter::repeat(&[][..]))).flat_map(|(path, ids)| path.iter().zip(ids.iter().copied().chain(std::iter::repeat(0))));
        outer.chain(holes)
    })
}

/// Names each face of a sweep: `Swept` from the sketch entity whose segment it
/// contains, `Cap` where a plane matches an end, `Made` for anything else.
/// `probes` give, per sketch segment, a point that lies only on that segment's face;
/// `caps` give for each end a plane and a point on it.
fn tag_sweep(lumps: &[Solid], probes: &[(DVec3, Id)], caps: [(Plane, DVec3); 2], feature: Id) -> Tags {
    let mut n = 0;
    lumps.iter().map(|s| s.iter_face().map(|f| {
        let kind = kind_of(f);
        if kind == Kind::Plane && let Some(surface) = f.surface() {
            for (end, (plane, on)) in caps.iter().enumerate() {
                if g(surface.axis_z).dot(plane.normal()).abs() > 1.0 - 1e-8 && (g(surface.origin) - plane.origin).dot(plane.normal()).abs() < 1e-6 && g(f.project(c(*on)).0).distance(*on) < 1e-5 {
                    return Some(Tag::new(Origin::Cap { feature, end: end == 1 }, kind));
                }
            }
        }
        for (p, entity) in probes {
            if *entity != 0 && g(f.project(c(*p)).0).distance(*p) < 1e-5 {
                return Some(Tag::new(Origin::Swept { feature, entity: *entity }, kind));
            }
        }
        let t = Tag::new(Origin::Made { feature, n }, kind);
        n += 1;
        Some(t)
    }).collect()).collect()
}

/// Tags for the result of [`extrude`] with the same arguments.
pub fn tag_extrude(lumps: &[Solid], profiles: &[&Profile], plane: &Plane, z0: f64, z1: f64, feature: Id) -> Tags {
    let (z0, z1) = (z0.min(z1), z0.max(z1));
    let probes: Vec<(DVec3, Id)> = segments(profiles).map(|(seg, id)| (probe(seg, plane, (z0 + z1) / 2.0), id)).collect();
    let on = |z: f64| profiles.first().map_or(plane.origin, |p| plane.to_world(p.centroid())) + plane.normal() * z;
    let at = |z: f64| Plane { origin: plane.origin + plane.normal() * z, ..*plane };
    tag_sweep(lumps, &probes, [(at(z0), on(z0)), (at(z1), on(z1))], feature)
}

/// Tags for the result of [`revolve`] with the same arguments.
pub fn tag_revolve(lumps: &[Solid], profiles: &[&Profile], plane: &Plane, a: DVec2, b: DVec2, degrees: f64, feature: Id) -> Tags {
    let degrees = degrees.clamp(-360.0, 360.0);
    let (origin, axis) = (plane.to_world(a), (plane.to_world(b) - plane.to_world(a)).normalize_or_zero());
    let turn = |p: DVec3, deg: f64| origin + glam::DQuat::from_axis_angle(axis, deg.to_radians()) * (p - origin);
    let probes: Vec<(DVec3, Id)> = segments(profiles).map(|(seg, id)| (turn(probe(seg, plane, 0.0), degrees / 2.0), id)).collect();
    let centroid = profiles.first().map_or(plane.origin, |p| plane.to_world(p.centroid()));
    let end_plane = plane.transformed(glam::DAffine3::from_translation(origin) * glam::DAffine3::from_quat(glam::DQuat::from_axis_angle(axis, degrees.to_radians())) * glam::DAffine3::from_translation(-origin));
    tag_sweep(lumps, &probes, [(*plane, centroid), (end_plane, turn(centroid, degrees))], feature)
}

/// Whether two faces lie on the same elementary surface.
fn same_surface(a: &cadrum::Surface, b: &cadrum::Surface) -> bool {
    let (oa, ob, za, zb) = (g(a.origin), g(b.origin), g(a.axis_z).normalize_or_zero(), g(b.axis_z).normalize_or_zero());
    let parallel = za.dot(zb).abs() > 1.0 - 1e-9;
    let on_axis = (ob - oa).cross(za).length() < 1e-6;
    let close = |x: f64, y: f64| (x - y).abs() < 1e-6;
    match (a.kind, b.kind) {
        (SurfaceKind::Plane, SurfaceKind::Plane) => parallel && (ob - oa).dot(za).abs() < 1e-6,
        (SurfaceKind::Cylinder { radius: ra }, SurfaceKind::Cylinder { radius: rb }) => parallel && on_axis && close(ra, rb),
        (SurfaceKind::Sphere { radius: ra }, SurfaceKind::Sphere { radius: rb }) => oa.distance(ob) < 1e-6 && close(ra, rb),
        (SurfaceKind::Cone { .. }, SurfaceKind::Cone { .. }) | (SurfaceKind::Torus { .. }, SurfaceKind::Torus { .. }) => parallel && on_axis && oa.distance(ob) < 1e-6 && format!("{:?}", a.kind) == format!("{:?}", b.kind),
        _ => false,
    }
}

/// Carries tags from the inputs of an operation to its result. Each result
/// face takes the tag of the input face the kernel's history names when that
/// is known, else of the input face on the same surface that contains it;
/// several results from one input become its numbered `Split` pieces; faces
/// with no ancestor are `Made` by `feature`.
pub fn carry(sources: &[(&[Solid], &Tags)], result: &[Solid], feature: Id) -> Tags {
    struct Old<'a> { id: u64, face: &'a Face, surface: Option<cadrum::Surface>, tag: &'a Tag }
    let old: Vec<Old> = sources.iter().flat_map(|(lumps, tags)| lumps.iter().zip(tags.iter()).flat_map(|(s, t)| s.iter_face().zip(t.iter()).filter_map(|(f, tag)| Some(Old { id: f.id(), face: f, surface: f.surface(), tag: tag.as_ref()? })))).collect();
    // First pass: which old face (by index into `old`) each new face descends from.
    let mut parent: Vec<Vec<Option<usize>>> = Vec::new();
    for lump in result {
        let history: Vec<[u64; 2]> = lump.iter_history().collect();
        parent.push(lump.iter_face().map(|f| {
            let id = f.id();
            // A history pair settles it only when the old id names one face: a fresh prism's two caps can share one.
            if let Some(k) = history.iter().filter(|h| h[0] == id).find_map(|h| { let mut hits = old.iter().enumerate().filter(|(_, o)| o.id == h[1]); let k = hits.next()?.0; hits.next().is_none().then_some(k) }) {
                return Some(k);
            }
            let kind = kind_of(f);
            let surface = f.surface();
            let (centre, _) = f.project(f.center());
            old.iter().position(|o| {
                kind_of(o.face) == kind
                    && match (&surface, &o.surface) { (Some(a), Some(b)) => same_surface(a, b), (None, None) => true, _ => false }
                    && g(o.face.project(centre).0).distance(g(centre)) < 1e-5
            })
        }).collect());
    }
    // Second pass: number the pieces of inputs that split, and the faces nobody made.
    let mut uses = std::collections::HashMap::<usize, u32>::new();
    for k in parent.iter().flatten().flatten() { *uses.entry(*k).or_insert(0) += 1; }
    let mut seen = std::collections::HashMap::<usize, u32>::new();
    let mut made = 0;
    result.iter().zip(parent).map(|(lump, parents)| lump.iter_face().zip(parents).map(|(f, k)| {
        Some(match k {
            Some(k) if uses[&k] > 1 => { let n = seen.entry(k).or_insert(0); let t = old[k].tag.split(*n); *n += 1; t }
            Some(k) => old[k].tag.clone(),
            None => { let t = Tag::new(Origin::Made { feature, n: made }, kind_of(f)); made += 1; t }
        })
    }).collect()).collect()
}

/// An edge's identity within one solid. A fresh prism's top edges share their
/// underlying shape with the bottom ones (moved by a location), so the kernel
/// id alone is ambiguous; the edge's sampled ends and middle settle it.
fn edge_key(e: &Edge) -> (u64, [[i64; 3]; 3]) {
    let pts = e.approximation_segments(FINE);
    let at = |i: usize| pts.get(i).map_or([0; 3], |p| g(*p).to_array().map(|c| (c * 1e6).round() as i64));
    (e.id(), [at(0), at(pts.len() / 2), at(pts.len().saturating_sub(1))])
}

/// Every edge of the lumps with the tags of the two faces it separates.
fn edge_table<'a>(lumps: &'a [Solid], tags: &Tags) -> Vec<(usize, &'a Edge, Option<EdgeTag>)> {
    let mut out = Vec::new();
    for (i, s) in lumps.iter().enumerate() {
        let mut faces: std::collections::HashMap<(u64, [[i64; 3]; 3]), Vec<(usize, Option<Tag>)>> = std::collections::HashMap::new();
        for (fi, f) in s.iter_face().enumerate() {
            for e in f.iter_edge() {
                let list = faces.entry(edge_key(e)).or_default();
                if !list.iter().any(|(k, _)| *k == fi) {
                    list.push((fi, tags.get(i).and_then(|t| t.get(fi)).cloned().flatten()));
                }
            }
        }
        for e in real_edges(s) {
            let tag = match faces.get(&edge_key(e)).map(Vec::as_slice) {
                Some([(_, Some(a)), (_, Some(b)), ..]) => Some(EdgeTag::new(a.clone(), b.clone())),
                _ => None,
            };
            out.push((i, e, tag));
        }
    }
    out
}

/// The tag of the edge nearest a point, for a reference that has none yet.
pub fn edge_tag_at(lumps: &[Solid], tags: &Tags, point: DVec3) -> Option<EdgeTag> {
    edge_table(lumps, tags).into_iter().map(|(_, e, t)| (g(e.project(c(point)).0).distance(point), t)).filter(|h| h.0 <= NEAR).min_by(|a, b| a.0.total_cmp(&b.0)).and_then(|h| h.1)
}

/// The tag of the face nearest a point.
pub fn face_tag_at(lumps: &[Solid], tags: &Tags, point: DVec3) -> Option<Tag> {
    lumps.iter().enumerate().flat_map(|(i, s)| s.iter_face().enumerate().map(move |(fi, f)| (i, fi, f)))
        .map(|(i, fi, f)| (g(f.project(c(point)).0).distance(point), tags.get(i).and_then(|t| t.get(fi)).cloned().flatten()))
        .filter(|h| h.0 <= NEAR).min_by(|a, b| a.0.total_cmp(&b.0)).and_then(|h| h.1)
}

/// A stored edge reference: where it was (and where it would be in the body's current bounds), and its tag.
#[derive(Clone, Debug)]
pub struct EdgePick { pub points: [DVec3; 2], pub tag: Option<EdgeTag> }

#[derive(Clone, Debug)]
pub struct FacePick { pub points: [DVec3; 2], pub tag: Option<Tag> }

/// The lump and edge each pick names, and how it was found.
fn find_edges<'a>(lumps: &'a [Solid], tags: &Tags, picks: &[EdgePick]) -> R<Vec<(usize, &'a Edge, Level, Option<EdgeTag>)>> {
    let table = edge_table(lumps, tags);
    let dist = |e: &Edge, tries: &[DVec3; 2]| tries.iter().map(|p| g(e.project(c(*p)).0).distance(*p)).fold(f64::MAX, f64::min);
    picks.iter().map(|pick| {
        if let Some(tag) = &pick.tag {
            // Pattern copies share tags; the nearest of equals is the one meant.
            if let Some(hit) = table.iter().filter(|t| t.2.as_ref() == Some(tag)).min_by(|a, b| dist(a.1, &pick.points).total_cmp(&dist(b.1, &pick.points))) {
                return Ok((hit.0, hit.1, Level::Tag, hit.2.clone()));
            }
            if let Some(hit) = table.iter().filter(|t| t.2.as_ref().is_some_and(|t| t.same_family(tag))).min_by(|a, b| dist(a.1, &pick.points).total_cmp(&dist(b.1, &pick.points))) {
                return Ok((hit.0, hit.1, Level::Origin, hit.2.clone()));
            }
            // The body knows its faces and none of them is this edge's: it is gone. Guessing by position would
            // put the blend on whatever edge now lies where this one was.
            if table.iter().any(|t| t.2.is_some()) {
                return Err("an edge it used is no longer there; the body changed shape under it".to_owned());
            }
        }
        table.iter().map(|t| (dist(t.1, &pick.points), t)).min_by(|a, b| a.0.total_cmp(&b.0)).filter(|h| h.0 <= NEAR).map(|h| (h.1.0, h.1.1, Level::Position, h.1.2.clone()))
            .ok_or_else(|| "an edge it used is no longer there; the body changed shape under it".to_owned())
    }).collect()
}

fn find_faces<'a>(lumps: &'a [Solid], tags: &Tags, picks: &[FacePick]) -> R<Vec<(usize, &'a Face, Level, Option<Tag>)>> {
    let table: Vec<(usize, &Face, Option<&Tag>)> = lumps.iter().enumerate().flat_map(|(i, s)| s.iter_face().enumerate().map(move |(fi, f)| (i, f, fi))).map(|(i, f, fi)| (i, f, tags.get(i).and_then(|t| t.get(fi)).and_then(|t| t.as_ref()))).collect();
    let dist = |f: &Face, tries: &[DVec3; 2]| tries.iter().map(|p| g(f.project(c(*p)).0).distance(*p)).fold(f64::MAX, f64::min);
    picks.iter().map(|pick| {
        if let Some(tag) = &pick.tag {
            if let Some(hit) = table.iter().filter(|t| t.2 == Some(tag)).min_by(|a, b| dist(a.1, &pick.points).total_cmp(&dist(b.1, &pick.points))) {
                return Ok((hit.0, hit.1, Level::Tag, hit.2.cloned()));
            }
            if let Some(hit) = table.iter().filter(|t| t.2.is_some_and(|t| t.family() == tag.family())).min_by(|a, b| dist(a.1, &pick.points).total_cmp(&dist(b.1, &pick.points))) {
                return Ok((hit.0, hit.1, Level::Origin, hit.2.cloned()));
            }
            if table.iter().any(|t| t.2.is_some()) {
                return Err("a face it used is no longer there; the body changed shape under it".to_owned());
            }
        }
        table.iter().map(|t| (dist(t.1, &pick.points), t)).min_by(|a, b| a.0.total_cmp(&b.0)).filter(|h| h.0 <= NEAR).map(|h| (h.1.0, h.1.1, Level::Position, h.1.2.cloned()))
            .ok_or_else(|| "a face it used is no longer there; the body changed shape under it".to_owned())
    }).collect()
}

/// What resolving picks produced: the new lumps and tags, the weakest level any pick needed,
/// and the tag of what each pick found (for a reference to learn).
pub struct Blended<T> {
    pub lumps: Lumps,
    pub tags: Tags,
    pub level: Level,
    pub picked: Vec<Option<T>>,
}

/// A boolean whose result carries the tags of both inputs.
pub fn boolean_tagged(a: (&[Solid], &Tags), b: (&[Solid], &Tags), op: Bool, feature: Id) -> R<(Lumps, Tags)> {
    let out = boolean(a.0, b.0, op)?;
    let tags = carry(&[a, b], &out, feature);
    Ok((out, tags))
}

/// Rounds (or, with `chamfer`, bevels) the picked edges by `size`. Returns the
/// result, its tags, and the weakest level any pick needed.
pub fn blend(lumps: &[Solid], tags: &Tags, picks: &[EdgePick], size: f64, chamfer: bool, feature: Id) -> R<Blended<EdgeTag>> {
    let what = if chamfer { "chamfer" } else { "fillet" };
    if size <= 0.0 {
        return Err(format!("the {what} size must be greater than zero"));
    }
    let picks = find_edges(lumps, tags, picks)?;
    let level = crate::tag::weakest(picks.iter().map(|p| p.2)).unwrap_or(Level::Tag);
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
    let out_tags = carry(&[(lumps, tags)], &out, feature);
    Ok(Blended { lumps: out, tags: out_tags, level, picked: picks.into_iter().map(|p| p.3).collect() })
}

/// Hollows the body to a wall of `thickness`, open at the picked faces.
pub fn shell(lumps: &[Solid], tags: &Tags, picks: &[FacePick], thickness: f64, feature: Id) -> R<Blended<Tag>> {
    if thickness <= 0.0 {
        return Err("the wall thickness must be greater than zero".into());
    }
    if picks.is_empty() {
        return Err("choose at least one face to leave open".into());
    }
    let picks = find_faces(lumps, tags, picks)?;
    let level = crate::tag::weakest(picks.iter().map(|p| p.2)).unwrap_or(Level::Tag);
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
    let out_tags = carry(&[(lumps, tags)], &out, feature);
    Ok(Blended { lumps: out, tags: out_tags, level, picked: picks.into_iter().map(|p| p.3).collect() })
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
    barrel_tagged(lumps, &no_tags(lumps), &FacePick { points: *tries, tag: None }).map(|b| b.0)
}

/// [`barrel`] for a stored reference, with how the face was found.
pub fn barrel_tagged(lumps: &[Solid], tags: &Tags, pick: &FacePick) -> R<(Barrel, Level, Option<Tag>)> {
    let (_, face, level, found) = find_faces(lumps, tags, std::slice::from_ref(pick))?.remove(0);
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
    let (on, normal) = face.project(c(pick.points[0]));
    let (on, normal) = (g(on), g(normal));
    let outward = on - origin - axis * (on - origin).dot(axis);
    Ok((Barrel { start: origin + axis * lo, axis, length: hi - lo, radius, internal: normal.dot(outward) < 0.0 }, level, found))
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
    pub tag: Option<EdgeTag>,
}

pub struct FaceInfo {
    /// A point on the face, for naming it in commands.
    pub at: DVec3,
    pub normal: DVec3,
    pub area: f64,
    pub kind: &'static str,
    pub tag: Option<Tag>,
}

pub fn edges(lumps: &[Solid]) -> Vec<EdgeInfo> {
    edges_tagged(lumps, &no_tags(lumps))
}

pub fn edges_tagged(lumps: &[Solid], tags: &Tags) -> Vec<EdgeInfo> {
    edge_table(lumps, tags)
        .into_iter()
        .filter_map(|(_, e, tag)| {
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
            Some(EdgeInfo { mid, length, straight: pts.len() == 2, tag })
        })
        .collect()
}

pub fn faces(lumps: &[Solid]) -> Vec<FaceInfo> {
    faces_tagged(lumps, &no_tags(lumps))
}

pub fn faces_tagged(lumps: &[Solid], tags: &Tags) -> Vec<FaceInfo> {
    lumps
        .iter()
        .enumerate()
        .flat_map(|(i, s)| s.iter_face().enumerate().map(move |(fi, f)| (f, tags.get(i).and_then(|t| t.get(fi)).cloned().flatten())))
        .map(|(f, tag)| {
            let (at, normal) = f.project(f.center());
            let kind = match f.surface().map(|s| s.kind) {
                Some(SurfaceKind::Plane) => "plane",
                Some(SurfaceKind::Cylinder { .. }) => "cylinder",
                Some(SurfaceKind::Cone { .. }) => "cone",
                Some(SurfaceKind::Sphere { .. }) => "sphere",
                Some(SurfaceKind::Torus { .. }) => "torus",
                None => "freeform",
            };
            FaceInfo { at: g(at), normal: g(normal), area: f.area(), kind, tag }
        })
        .collect()
}
