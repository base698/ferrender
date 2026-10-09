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

/// One sketch segment as a kernel edge, `z` off the plane.
fn seg_edge(s: &Seg, plane: &Plane, z: f64) -> Result<Edge, cadrum::Error> {
    let at = |p: DVec2| c(plane.to_world(p) + plane.normal() * z);
    match *s {
        Seg::Line(a, b) => Edge::line(at(a), at(b)),
        Seg::Arc(a, m, b) => Edge::arc_3pts(at(a), at(m), at(b)),
        Seg::Spline(points) => Edge::bspline(points.map(at).iter(), cadrum::BSplineEnd::NotAKnot),
        Seg::Circle(centre, r) => Edge::circle(r, c(plane.normal())).map(|e| e.translate(at(centre))),
    }
}

/// One closed boundary as kernel edges, `z` off the plane.
fn ring(path: &[Seg], plane: &Plane, z: f64) -> R<Vec<Edge>> {
    path.iter().map(|s| seg_edge(s, plane, z).map_err(|e| format!("the profile has an edge the kernel rejects: {e}"))).collect()
}

mod pipe;
pub use pipe::{sweep, sweep_spans, tag_swept};
mod loft;
pub use loft::{Section, loft, tag_loft};

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
fn fuse_unmerged(parts: Vec<Solid>) -> R<Lumps> {
    let mut it = parts.iter();
    let Some(first) = it.next() else { return Ok(Vec::new()) };
    if parts.len() == 1 {
        return Ok(parts);
    }
    let mut all: Boolean<Solid> = first.into();
    for s in it {
        all = all + s;
    }
    all.build_vec().map_err(|e| format!("the kernel could not join the shapes: {e}"))
}

fn fuse(parts: Vec<Solid>) -> R<Lumps> { tidy(fuse_unmerged(parts)?) }

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

/// The refusal for a profile with points on both sides of the axis, with how far the
/// smaller overhang reaches: usually one point placed just past the axis by hand.
pub fn crosses_axis(lo: f64, hi: f64, axis: DVec2) -> String {
    let past = lo.abs().min(hi) / axis.length().max(1e-12);
    format!("the profile crosses the axis by {} mm; move the points beyond it onto the axis (points snap to the axes when drawn near them)", crate::units::trim_num(past, 3))
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
            return Err(crosses_axis(lo, hi, b - a));
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
pub fn boolean(a: &[Solid], b: &[Solid], op: Bool) -> R<Lumps> { boolean_impl(a,b,op,true) }

fn boolean_impl(a: &[Solid], b: &[Solid], op: Bool, clean: bool) -> R<Lumps> {
    let fail = |e: cadrum::Error| format!("the kernel could not combine the shapes: {e}");
    let (va, vb) = (volume(a), volume(b));
    let out = match op {
        Bool::Union => {
            // Build from borrowed inputs: Solid::clone deep-copies topology and
            // discards the face IDs needed to compose Boolean history.
            let mut inputs=a.iter().chain(b);
            let out=if let Some(first)=inputs.next() {
                let mut expr:Boolean<Solid>=first.into();
                for input in inputs {expr=expr+input;}
                expr.build_vec().map_err(fail)?
            } else {Vec::new()};
            if clean {tidy(out)?} else {out}
        },
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
            if clean { tidy(out)? } else { out }
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

/// A conservative geometric identity for faces without modeling provenance.
/// It is independent of traversal order. Changes to these surfaces invalidate
/// references, rather than allowing an unrelated face to inherit an ordinal.
fn surface_tag(face: &Face, feature: Id) -> Tag {
    let (at, normal) = face.project(face.center());
    let quantize = |v: f64| (v * 1e7).round() as i64;
    let mut signature: Vec<_> = g(face.center()).to_array().into_iter()
        .chain(g(at).to_array()).chain(g(normal).to_array()).chain([face.area()])
        .map(quantize).collect();
    let mut edges: Vec<_> = face.iter_edge().map(|e| {
        let p = e.approximation_segments(FINE);
        let first = p.first().copied().unwrap_or_default();
        let last = p.last().copied().unwrap_or_default();
        let mut ends = [g(first).to_array().map(quantize), g(last).to_array().map(quantize)];
        ends.sort();
        (ends, g(e.project(face.center()).0).to_array().map(quantize))
    }).collect();
    edges.sort();
    for (ends, middle) in edges { signature.extend(ends.into_iter().flatten()); signature.extend(middle); }
    Tag::new(Origin::Surface { feature, signature }, kind_of(face))
}

pub fn fresh_tags(lumps: &[Solid], feature: Id) -> Tags {
    lumps.iter().map(|s| s.iter_face().map(|f| Some(surface_tag(f, feature))).collect()).collect()
}

/// Primitive roles are defined in the constructor's local frame, so changing
/// dimensions, rotation or kernel traversal never exchanges two plane faces.
pub fn primitive_tags(lumps: &[Solid], primitive: &crate::primitives::Primitive, feature: Id) -> Tags {
    use crate::primitives::PrimitiveShape;
    let placement = crate::components::Placement { translate: primitive.position.clone(), rotate: primitive.rotate.clone() }.affine().inverse();
    lumps.iter().map(|s| s.iter_face().map(|f| {
        let (at, normal) = f.project(f.center());
        let p = placement.transform_point3(g(at));
        let n = placement.transform_vector3(g(normal));
        let role = match &primitive.shape {
            PrimitiveShape::Box { width, depth, height } => {
                let dimensions = [width.v, depth.v, height.v];
                let axis = (0..3).max_by(|a,b| n[*a].abs().total_cmp(&n[*b].abs())).unwrap();
                format!("box:{}:{}", ["x","y","z"][axis], if p[axis] > dimensions[axis]/2. { "max" } else { "min" })
            }
            PrimitiveShape::Cylinder { height, .. } => if kind_of(f) == Kind::Plane { format!("cylinder:{}", if p.z > height.v/2. {"end"} else {"start"}) } else {"cylinder:side".into()},
            PrimitiveShape::Cone { height, .. } => if kind_of(f) == Kind::Plane { format!("cone:{}", if p.z > height.v/2. {"end"} else {"start"}) } else {"cone:side".into()},
            PrimitiveShape::Sphere { .. } => "sphere:surface".into(),
            PrimitiveShape::Torus { .. } => "torus:surface".into(),
        };
        Some(Tag::new(Origin::Semantic { feature, role }, kind_of(f)))
    }).collect()).collect()
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
    let mut tags: Tags = lumps.iter().map(|s| s.iter_face().map(|f| {
        let kind = kind_of(f);
        if kind == Kind::Plane && let Some(surface) = f.surface() {
            for (end, (plane, _)) in caps.iter().enumerate() {
                if g(surface.axis_z).dot(plane.normal()).abs() > 1.0 - 1e-8 && (g(surface.origin) - plane.origin).dot(plane.normal()).abs() < 1e-6 {
                    return Some(Tag::new(Origin::Cap { feature, end: end == 1 }, kind));
                }
            }
        }
        let mut source:Vec<_>=probes.iter().filter(|(p,entity)|*entity!=0 && g(f.project(c(*p)).0).distance(*p)<1e-5)
            .map(|(_,entity)|Tag::new(Origin::Swept {feature,entity:*entity},kind)).collect();
        source.sort();source.dedup();
        Some(match source.len() {0=>surface_tag(f,feature),1=>source.pop().unwrap(),_=>Tag::new(Origin::Merged {sources:source},kind)})
    }).collect()).collect();
    for (li,lump) in lumps.iter().enumerate() {
        let mut edges=std::collections::HashMap::new();
        for (fi,f) in lump.iter_face().enumerate() {for e in f.iter_edge(){edges.entry(edge_key(e)).or_insert_with(Vec::new).push(fi);}}
        let before=tags[li].clone();
        for (fi,f) in lump.iter_face().enumerate() {
            if let Some(Tag {origin:Origin::Cap {end,..},..})=&before[fi] {
                let mut entities:Vec<_>=f.iter_edge().flat_map(|e|edges.get(&edge_key(e)).into_iter().flatten()).filter(|&&i|i!=fi)
                    .filter_map(|&i|match &before[i].as_ref()?.origin {Origin::Swept {entity,..}=>Some(*entity),_=>None}).collect();
                entities.sort();entities.dedup();
                tags[li][fi]=Some(if entities.is_empty(){surface_tag(f,feature)}else{Tag::new(Origin::ProfileCap {feature,end:*end,entities},Kind::Plane)});
            }
        }
    }
    tags
}

/// Tags for the result of [`extrude`] with the same arguments.
pub fn tag_extrude(lumps: &[Solid], profiles: &[&Profile], plane: &Plane, z0: f64, z1: f64, feature: Id) -> Tags {
    let (z0, z1) = (z0.min(z1), z0.max(z1));
    let probes: Vec<(DVec3, Id)> = segments(profiles).map(|(seg, id)| (probe(seg, plane, (z0 + z1) / 2.0), id)).collect();
    let on = |z: f64| profiles.first().map_or(plane.origin, |p| plane.to_world(p.centroid())) + plane.normal() * z;
    let at = |z: f64| Plane { origin: plane.origin + plane.normal() * z, ..*plane };
    tag_sweep(lumps, &probes, [(at(z0), on(z0)), (at(z1), on(z1))], feature)
}

/// Font outline order is deterministic for unchanged text. Give its segments
/// explicit content-scoped identities before applying the normal sweep tags;
/// resizing, depth, placement and spacing edits retain those identities while
/// changing the text content invalidates them deliberately.
pub fn tag_text(lumps:&[Solid],profiles:&[Profile],plane:&Plane,z0:f64,z1:f64,text:&str,feature:Id)->Tags {
    let mut named=profiles.to_vec();let mut next=1;
    let mut owners=std::collections::BTreeMap::new();
    let topology:Vec<_>=profiles.iter().map(|p|(p.path.len(),p.hole_paths.iter().map(Vec::len).collect::<Vec<_>>())).collect();
    for (profile,p) in named.iter_mut().enumerate() {
        p.path_ids=(0..p.path.len()).map(|_|{let id=next;next+=1;owners.insert(id,profile);id}).collect();
        p.hole_path_ids=p.hole_paths.iter().map(|path|(0..path.len()).map(|_|{let id=next;next+=1;owners.insert(id,profile);id}).collect()).collect();
    }
    fn scope(tag:&mut Tag,text:&str,topology:&str,owners:&std::collections::BTreeMap<Id,usize>) {
        let role=match &mut tag.origin {
            // Font curves are currently polygonized. A subdivision change must
            // not let a different segment inherit an old side-face identity.
            Origin::Swept {entity,..}=>Some(format!("text:{text:?}:outline:{topology}:{entity}")),
            Origin::ProfileCap {end,entities,..}=>{
                let profiles:std::collections::BTreeSet<_>=entities.iter().filter_map(|e|owners.get(e)).collect();
                Some(format!("text:{text:?}:cap:{end}:{profiles:?}"))
            },
            Origin::Merged {sources}|Origin::Derived {sources,..}=>{for t in sources {scope(t,text,topology,owners);}None},
            _=>None,
        };
        if let Some(role)=role {tag.origin=Origin::Semantic {feature:tag.feature(),role};}
    }
    let mut tags=tag_extrude(lumps,&named.iter().collect::<Vec<_>>(),plane,z0,z1,feature);
    for t in tags.iter_mut().flatten().flatten(){scope(t,text,&format!("{topology:?}"),&owners);}
    tags
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

/// Carries every source relation through an operation. Ambiguous/merged
/// ancestry is represented explicitly; no result inherits the first match.
/// Split regions and newly generated blend faces are named by their adjacent
/// source faces, independent of result order. Unprovable identities fall back
/// to a conservative geometric signature that invalidates on shape changes.
pub fn carry(sources: &[(&[Solid], &Tags)], result: &[Solid], feature: Id) -> Tags {
    struct Old<'a> { id: u64, face: &'a Face, surface: Option<cadrum::Surface>, tag: &'a Tag }
    let old: Vec<Old> = sources.iter().flat_map(|(lumps, tags)| lumps.iter().zip(tags.iter()).flat_map(|(s,t)| s.iter_face().zip(t.iter()).filter_map(|(f,tag)| Some(Old {id:f.id(),face:f,surface:f.surface(),tag:tag.as_ref()?})))).collect();
    let mut faces = Vec::new();
    let mut parents: Vec<Option<Tag>> = Vec::new();
    for lump in result {
        let history: Vec<_> = lump.iter_history().collect();
        for f in lump.iter_face() {
            let surface = f.surface(); let centre = f.project(f.center()).0;
            let overlaps = |o: &&Old<'_>| {
                match (&surface, &o.surface) { (Some(a),Some(b)) if same_surface(a,b) => {}, _ => return false }
                g(o.face.project(centre).0).distance(g(centre)) < 1e-5
                    || g(f.project(o.face.center()).0).distance(g(o.face.center())) < 1e-5
            };
            let history_old: Vec<_> = old.iter().filter(|o| history.iter().any(|h| h[0]==f.id() && h[1]==o.id)).collect();
            let mut ancestors: Vec<_> = old.iter().filter(overlaps).map(|o| o.tag.clone()).collect();
            // Modified surfaces can move (shell offsets). Trust a history relation
            // only when both source and result IDs are spatially unambiguous.
            let mut offset_source=false;
            if ancestors.is_empty() && history_old.len()==1
                && result.iter().flat_map(|s|s.iter_face()).filter(|other|other.id()==f.id()).count()==1
                && kind_of(history_old[0].face)==kind_of(f) {
                let source=history_old[0];
                ancestors.push(source.tag.clone());
                // Boolean trimming of a freeform surface keeps its provenance.
                // Only an actual displacement (such as a shell offset) creates
                // a generated face identity. History alone does not mean moved.
                offset_source=g(source.face.project(centre).0).distance(g(centre))>1e-5;
            }
            ancestors.sort(); ancestors.dedup();
            parents.push(if offset_source {Some(Tag::new(Origin::Derived {feature,sources:ancestors},kind_of(f)))} else {match ancestors.len() { 0=>None, 1=>ancestors.pop(), _=>Some(Tag::new(Origin::Merged {sources:ancestors},kind_of(f))) }});
            faces.push(f);
        }
    }
    let mut by_edge = std::collections::HashMap::new();
    for (i,f) in faces.iter().enumerate() { for e in f.iter_edge() { by_edge.entry(edge_key(e)).or_insert_with(Vec::new).push(i); } }
    let adjacent:Vec<Vec<usize>>=faces.iter().enumerate().map(|(i,f)|f.iter_edge().flat_map(|e|by_edge.get(&edge_key(e)).into_iter().flatten()).copied().filter(|&j|j!=i).collect()).collect();
    let mut named=parents.clone();
    // Corner patches often touch only other generated fillet faces. Establish
    // their source identities after the edge blends, flattening same-operation
    // provenance so a radius edit does not depend on intermediate face order.
    for _ in 0..32 {
        let before=named.clone();let mut changed=false;
        for i in 0..faces.len() {
            if before[i].is_some(){continue;}
            let mut source=Vec::new();
            for &j in &adjacent[i] {if let Some(t)=&before[j] {
                match &t.origin {Origin::Derived {feature:made,sources} if *made==feature=>source.extend(sources.iter().cloned()),_=>source.push(t.clone())}
            }}
            source.sort();source.dedup();
            if source.len()>=2 {named[i]=Some(Tag::new(Origin::Derived {feature,sources:source},kind_of(faces[i])));changed=true;}
        }
        if !changed {break;}
    }
    let boundaries:Vec<Vec<Tag>>=adjacent.iter().enumerate().map(|(i,neighbors)| {
        let mut boundary:Vec<_>=neighbors.iter().filter_map(|&j|named[j].clone()).filter(|t|parents[i].as_ref()!=Some(t)).collect();
        boundary.sort();boundary.dedup();boundary
    }).collect();
    let mut counts = std::collections::HashMap::new();
    for p in parents.iter().flatten() { *counts.entry(p.clone()).or_insert(0usize)+=1; }
    let mut candidates: Vec<Tag> = faces.iter().enumerate().map(|(i,f)| match &parents[i] {
        Some(p) if counts[p]>1 => Tag::new(Origin::Patch {of:Box::new(p.clone()),boundary:boundaries[i].clone(),cycles:Vec::new()},kind_of(f)),
        Some(p)=>p.clone(),
        None=>named[i].clone().unwrap_or_else(||surface_tag(f,feature)),
    }).collect();
    // Equal boundary provenance cannot distinguish symmetric/generated pieces.
    // Keep them distinct by verified geometry; never number them by iteration.
    let mut uses = std::collections::HashMap::new();
    for t in &candidates { *uses.entry(t.clone()).or_insert(0usize)+=1; }
    for (i,t) in candidates.iter_mut().enumerate() {
        if uses[t]>1 {
            if let Origin::Patch {cycles,..}=&mut t.origin {
                if let Some(oriented)=boundary_cycles(i,&faces,&named,&by_edge) {*cycles=oriented;}
            }
        }
    }
    // Cycles distinguish complementary pieces of a ring without ranking their
    // positions or areas. If orientation/connectivity is unavailable or still
    // identical, retain the conservative geometry identity.
    let mut resolved=std::collections::HashMap::new();
    for t in &candidates {*resolved.entry(t.clone()).or_insert(0usize)+=1;}
    for (i,t) in candidates.iter_mut().enumerate() {if resolved[t]>1 {*t=surface_tag(faces[i],feature);}}
    let mut all=candidates.into_iter();
    result.iter().map(|s|s.iter_face().map(|_|Some(all.next().unwrap())).collect()).collect()
}

/// Recover oriented face wires from endpoint connectivity and the local inside
/// of each known boundary. The binding exposes neither wires nor edge-use
/// orientation. Geometry is used only to orient/connect adjacent edges; identity
/// is exclusively the cyclic sequence of source tags, never coordinates.
fn boundary_cycles(index:usize,faces:&[&Face],named:&[Option<Tag>],by_edge:&std::collections::HashMap<(u64,[[i64;3];3]),Vec<usize>>)->Option<Vec<Vec<Tag>>> {
    let face=faces[index];
    let mut edges=Vec::new();
    for edge in face.iter_edge() {
        let mut neighbors:Vec<_>=by_edge.get(&edge_key(edge))?.iter().copied().filter(|&j|j!=index).collect();neighbors.sort();neighbors.dedup();
        if neighbors.is_empty() {continue;} // periodic seam, not a region boundary
        if neighbors.len()!=1 {return None;}
        let tag=named[neighbors[0]].clone()?;
        let points=edge.approximation_segments(FINE);
        let sample=*points.get(points.len()/2)?;
        let (point,tangent)=edge.project(sample);let point=g(point);let tangent=g(tangent).try_normalize()?;
        let normal=g(face.project(c(point)).1);let left=normal.cross(tangent).try_normalize()?;
        let length: f64=points.windows(2).map(|p|g(p[1]).distance(g(p[0]))).sum();
        let epsilon=(length*1e-4).clamp(1e-6,1e-3);
        let plus=point+left*epsilon;let minus=point-left*epsilon;
        let dp=g(face.project(c(plus)).0).distance(plus);let dm=g(face.project(c(minus)).0).distance(minus);
        let (mut start,mut end)=(g(edge.start_point()),g(edge.end_point()));
        if dp<epsilon*0.25 && dm>epsilon*0.75 {} else if dm<epsilon*0.25 && dp>epsilon*0.75 {std::mem::swap(&mut start,&mut end);} else {return None;}
        edges.push((start,end,tag));
    }
    let mut cycles=Vec::new();
    while let Some((start,mut end,tag))=edges.pop() {
        let mut cycle=vec![tag];
        while end.distance(start)>1e-6 {
            let next:Vec<_>=edges.iter().enumerate().filter(|(_,e)|e.0.distance(end)<=1e-6).map(|(i,_)|i).collect();
            if next.len()!=1 {return None;}
            let (_,to,tag)=edges.swap_remove(next[0]);end=to;
            if cycle.last()!=Some(&tag) {cycle.push(tag);}
        }
        if cycle.len()>1 && cycle.first()==cycle.last() {cycle.pop();}
        let canonical=(0..cycle.len()).map(|i|cycle[i..].iter().chain(&cycle[..i]).cloned().collect::<Vec<_>>()).min()?;
        cycles.push(canonical);
    }
    cycles.sort();(!cycles.is_empty()).then_some(cycles)
}

/// The planar cut is a semantic output of Split, qualified by the source
/// boundary so disconnected pieces cannot exchange identities when reordered.
pub fn split_tags(source:&[Solid],source_tags:&Tags,result:&[Solid],plane:Plane,feature:Id)->Tags {
    let mut tags=carry(&[(source,source_tags)],result,feature);
    let old:Vec<_>=source.iter().zip(source_tags).flat_map(|(s,t)|s.iter_face().zip(t).filter_map(|(f,t)|Some((f,t.as_ref()?)))).collect();
    for (li,lump) in result.iter().enumerate(){for (fi,f) in lump.iter_face().enumerate(){
        let (at,n)=f.project(f.center());let at=g(at);let n=g(n);
        if kind_of(f)!=Kind::Plane || n.dot(plane.normal()).abs()<1.-1e-7 || (at-plane.origin).dot(plane.normal()).abs()>1e-5 {continue;}
        let mut boundary=Vec::new();
        for edge in f.iter_edge(){
            let pts=edge.approximation_segments(FINE);let Some(p)=pts.get(pts.len()/2) else {continue};
            for (face,tag) in &old {if g(face.project(*p).0).distance(g(*p))<1e-5 {boundary.push((*tag).clone());}}
        }
        boundary.sort();boundary.dedup();
        tags[li][fi]=Some(Tag::new(Origin::Section {feature,positive:n.dot(plane.normal())>0.,boundary},Kind::Plane));
    }}tags
}

/// An edge's identity within one solid. A fresh prism's top edges share their
/// underlying shape with the bottom ones (moved by a location), so the kernel
/// id alone is ambiguous; the edge's sampled ends and middle settle it.
fn edge_key(e: &Edge) -> (u64, [[i64; 3]; 3]) {
    let pts = e.approximation_segments(FINE);
    let at = |i: usize| pts.get(i).map_or([0; 3], |p| g(*p).to_array().map(|c| (c * 1e6).round() as i64));
    let mut ends = [at(0), at(pts.len().saturating_sub(1))]; ends.sort();
    (e.id(), [ends[0], at(pts.len() / 2), ends[1]])
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

/// The edge nearest a point, for a reference that has none yet: the point moved onto
/// that edge, and the edge's tag. A reported or clicked point lies a little off the true
/// curve (rounded, or on the drawn polyline); the moved point tells the edge apart from
/// another with the same tag, which proximity alone must not do.
pub fn edge_at(lumps: &[Solid], tags: &Tags, point: DVec3) -> Option<(DVec3, Option<EdgeTag>)> {
    edge_table(lumps, tags).into_iter().map(|(_, e, t)| { let on = g(e.project(c(point)).0); (on.distance(point), on, t) })
        .filter(|h| h.0 <= NEAR).min_by(|a, b| a.0.total_cmp(&b.0)).map(|h| (h.1, h.2))
}

/// The tag of the edge nearest a point, for a reference that has none yet.
pub fn edge_tag_at(lumps: &[Solid], tags: &Tags, point: DVec3) -> Option<EdgeTag> {
    edge_at(lumps, tags, point).and_then(|h| h.1)
}

/// The face nearest a point: the point moved onto it, and its tag (see [`edge_at`]).
pub fn face_at(lumps: &[Solid], tags: &Tags, point: DVec3) -> Option<(DVec3, Option<Tag>)> {
    lumps.iter().enumerate().flat_map(|(i, s)| s.iter_face().enumerate().map(move |(fi, f)| (i, fi, f)))
        .map(|(i, fi, f)| { let on = g(f.project(c(point)).0); (on.distance(point), on, tags.get(i).and_then(|t| t.get(fi)).cloned().flatten()) })
        .filter(|h| h.0 <= NEAR).min_by(|a, b| a.0.total_cmp(&b.0)).map(|h| (h.1, h.2))
}

/// The tag of the face nearest a point.
pub fn face_tag_at(lumps: &[Solid], tags: &Tags, point: DVec3) -> Option<Tag> {
    face_at(lumps, tags, point).and_then(|h| h.1)
}

/// A stored edge reference: where it was (and where it would be in the body's current bounds), and its tag.
#[derive(Clone, Debug)]
pub struct EdgePick { pub points: [DVec3; 2], pub tag: Option<EdgeTag> }

#[derive(Clone, Debug)]
pub struct FacePick { pub points: [DVec3; 2], pub tag: Option<Tag> }

/// Select a unique identity. Multiple descendants can only be chosen when
/// the saved/mapped point still lies on exactly one; proximity is not identity.
fn choose_pick<'a,T>(hits: Vec<&'a T>, distance: impl Fn(&T)->f64) -> R<Option<&'a T>> {
    if hits.len()==1 { return Ok(hits.into_iter().next()); }
    let mut located=hits.into_iter().filter(|h|distance(h)<=1e-5);
    let first=located.next();
    if first.is_some() && located.next().is_none() {return Ok(first);}
    Err("the topology reference is ambiguous after the body changed; select the intended face or edge again".into())
}

fn find_edges<'a>(lumps: &'a [Solid], tags: &Tags, picks: &[EdgePick]) -> R<Vec<(usize, &'a Edge, Level, Option<EdgeTag>)>> {
    let table=edge_table(lumps,tags);
    let dist=|e:&Edge,points:&[DVec3]|points.iter().map(|p|g(e.project(c(*p)).0).distance(*p)).fold(f64::MAX,f64::min);
    picks.iter().map(|pick| {
        if let Some(tag)=&pick.tag {
            if tag.legacy() {
                let hits:Vec<_>=table.iter().filter(|t|t.2.as_ref().is_some_and(|new|tag.legacy_compatible(new)) && dist(t.1,&pick.points[..1])<=1e-5).collect();
                if hits.len()==1 {let h=hits[0];return Ok((h.0,h.1,Level::Position,h.2.clone()));}
                return Err("this legacy edge reference is missing or ambiguous; select the intended edge again".into());
            }
            let hits:Vec<_>=table.iter().filter(|t|t.2.as_ref()==Some(tag)).collect();
            if !hits.is_empty() {let h=choose_pick(hits,|t|dist(t.1,&pick.points))?.unwrap();return Ok((h.0,h.1,Level::Tag,h.2.clone()));}
            let hits:Vec<_>=table.iter().filter(|t|t.2.as_ref().is_some_and(|new|new.same_family(tag))).collect();
            if !hits.is_empty() {let h=choose_pick(hits,|t|dist(t.1,&pick.points))?.unwrap();return Ok((h.0,h.1,Level::Origin,h.2.clone()));}
            return Err("an edge it used is no longer there; the body changed shape under it".into());
        }
        // Pre-tag designs may learn a reference from the original pick, but a
        // junction/tie must not silently choose whichever edge is enumerated first.
        let mut hits:Vec<_>=table.iter().map(|t|(dist(t.1,&pick.points),t)).filter(|h|h.0<=NEAR).collect();
        hits.sort_by(|a,b|a.0.total_cmp(&b.0));
        let Some((d,h))=hits.first() else {return Err("an edge it used is no longer there; the body changed shape under it".into())};
        if hits.get(1).is_some_and(|other|(other.0-d).abs()<1e-7) {return Err("the edge pick is ambiguous; select a point away from a vertex".into());}
        Ok((h.0,h.1,Level::Position,h.2.clone()))
    }).collect()
}

fn find_faces<'a>(lumps: &'a [Solid], tags: &Tags, picks: &[FacePick]) -> R<Vec<(usize, &'a Face, Level, Option<Tag>)>> {
    let table:Vec<_>=lumps.iter().enumerate().flat_map(|(i,s)|s.iter_face().enumerate().map(move |(fi,f)|(i,f,fi))).map(|(i,f,fi)|(i,f,tags.get(i).and_then(|t|t.get(fi)).and_then(|t|t.as_ref()))).collect();
    let dist=|f:&Face,points:&[DVec3]|points.iter().map(|p|g(f.project(c(*p)).0).distance(*p)).fold(f64::MAX,f64::min);
    picks.iter().map(|pick| {
        if let Some(tag)=&pick.tag {
            if tag.legacy() {
                let hits:Vec<_>=table.iter().filter(|t|t.2.is_some_and(|new|tag.legacy_compatible(new)) && dist(t.1,&pick.points[..1])<=1e-5).collect();
                if hits.len()==1 {let h=hits[0];return Ok((h.0,h.1,Level::Position,h.2.cloned()));}
                return Err("this legacy face reference is missing or ambiguous; select the intended face again".into());
            }
            let hits:Vec<_>=table.iter().filter(|t|t.2==Some(tag)).collect();
            if !hits.is_empty() {let h=choose_pick(hits,|t|dist(t.1,&pick.points))?.unwrap();return Ok((h.0,h.1,Level::Tag,h.2.cloned()));}
            let hits:Vec<_>=table.iter().filter(|t|t.2.is_some_and(|new|new.descends_from(tag))).collect();
            if !hits.is_empty() {let h=choose_pick(hits,|t|dist(t.1,&pick.points))?.unwrap();return Ok((h.0,h.1,Level::Origin,h.2.cloned()));}
            return Err("a face it used is no longer there; the body changed shape under it".into());
        }
        let mut hits:Vec<_>=table.iter().map(|t|(dist(t.1,&pick.points),t)).filter(|h|h.0<=NEAR).collect();
        hits.sort_by(|a,b|a.0.total_cmp(&b.0));
        let Some((d,h))=hits.first() else {return Err("a face it used is no longer there; the body changed shape under it".into())};
        if hits.get(1).is_some_and(|other|(other.0-d).abs()<1e-7) {return Err("the face pick is ambiguous; select a point inside the face".into());}
        Ok((h.0,h.1,Level::Position,h.2.cloned()))
    }).collect()
}

/// Shared reference resolution for exact operations, text and construction
/// planes. Keeping one resolver prevents a UI feature from quietly falling back
/// to a different nearest face when strict provenance resolution failed.
pub fn resolve_face(lumps:&[Solid],tags:&Tags,pick:&FacePick)->R<(FaceInfo,Level)> {
    let picked=find_faces(lumps,tags,std::slice::from_ref(pick))?;
    let (_,f,level,tag)=&picked[0]; let (at,normal)=f.project(f.center());
    Ok((FaceInfo {at:g(at),normal:g(normal),area:f.area(),kind:match kind_of(f) {Kind::Plane=>"plane",Kind::Cylinder=>"cylinder",Kind::Cone=>"cone",Kind::Sphere=>"sphere",Kind::Torus=>"torus",Kind::Freeform=>"freeform"},tag:tag.clone()},*level))
}

/// What resolving picks produced: the new lumps and tags, the weakest level any pick needed,
/// and the tag of what each pick found (for a reference to learn).
pub struct Blended<T> {
    pub lumps: Lumps,
    pub tags: Tags,
    pub level: Level,
    pub picked: Vec<Option<T>>,
    /// Each pick's point moved onto the edge or face it found, one per pick.
    pub points: Vec<DVec3>,
}

/// The pick's point moved onto what it found: whichever of its two points
/// (as picked, and mapped into the body's current bounds) lands nearest.
fn settled(points: &[DVec3; 2], onto: impl Fn(DVec3) -> DVec3) -> DVec3 {
    points.iter().map(|p| (onto(*p), *p)).min_by(|a, b| a.0.distance(a.1).total_cmp(&b.0.distance(b.1))).map_or(points[0], |h| h.0)
}

/// A boolean whose result carries the tags of both inputs.
pub fn boolean_tagged(a: (&[Solid], &Tags), b: (&[Solid], &Tags), op: Bool, feature: Id) -> R<(Lumps, Tags)> {
    let out = boolean_impl(a.0,b.0,op,false)?;
    let tags = carry(&[a,b],&out,feature);
    // clean() only reports history against its immediate input. Compose tags
    // before cleaning so boolean provenance is not discarded by unification.
    let mut cleaned=Vec::with_capacity(out.len());
    let mut cleaned_tags=Vec::with_capacity(out.len());
    for (original,original_tags) in out.into_iter().zip(tags) {
        // Keep the independent original as the safe fallback. A BRep copy has
        // identical face traversal but fresh IDs, so associate its tags before
        // cleaning and compose history against that immediate input.
        let working=original.clone();
        let before=original.volume();
        match working.clean() {
            Ok(result) if result.volume().is_finite() && (result.volume()-before).abs()<=1e-7*before.abs().max(1.) => {
                let input_tags=vec![original_tags];
                let mut result_tags=carry(&[(std::slice::from_ref(&working),&input_tags)],std::slice::from_ref(&result),feature);
                cleaned.push(result);cleaned_tags.push(result_tags.remove(0));
            }
            _ => {cleaned.push(original);cleaned_tags.push(original_tags);}
        }
    }
    Ok((cleaned,cleaned_tags))
}

/// Rounds (or, with `chamfer`, bevels) the picked edges by `size`. Returns the
/// result, its tags, and the weakest level any pick needed.
pub fn blend(lumps: &[Solid], tags: &Tags, picks: &[EdgePick], size: f64, chamfer: bool, feature: Id) -> R<Blended<EdgeTag>> {
    let what = if chamfer { "chamfer" } else { "fillet" };
    if size <= 0.0 {
        return Err(format!("the {what} size must be greater than zero"));
    }
    let asked = picks;
    let picks = find_edges(lumps, tags, picks)?;
    let points = asked.iter().zip(&picks).map(|(pick, found)| settled(&pick.points, |p| g(found.1.project(c(p)).0))).collect();
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
    let mut out_tags = carry(&[(lumps, tags)], &out, feature);
    // Unselected lumps are exact deep copies, whose kernel history is empty.
    // Their traversal is unchanged, so preserve their existing identities.
    for i in 0..out.len() {if !picks.iter().any(|p|p.0==i) {out_tags[i]=tags[i].clone();}}
    Ok(Blended { lumps: out, tags: out_tags, level, picked: picks.into_iter().map(|p| p.3).collect(), points })
}

/// A point for a message, in millimetres.
fn point_text(p: DVec3) -> String {
    format!("({}, {}, {}) mm", crate::units::trim_num(p.x, 2), crate::units::trim_num(p.y, 2), crate::units::trim_num(p.z, 2))
}

/// Whether a face picked to be opened is still in the result, unchanged: same surface,
/// same extent. A face that became an opening's rim keeps its surface but not its area.
fn face_kept(made: &Solid, face: &Face) -> bool {
    let (at, normal) = face.project(face.center());
    let (at, normal) = (g(at), g(normal));
    made.iter_face().any(|f| {
        if kind_of(f) != kind_of(face) { return false; }
        let (on, n) = f.project(c(at));
        g(on).distance(at) < 1e-6 && g(n).dot(normal) > 1.0 - 1e-7 && (f.area() - face.area()).abs() <= 1e-6 * face.area().abs().max(1.0)
    })
}

/// Opens flat faces of a shell the kernel could not: hollows the body into a sealed
/// shell with an inner void, then cuts the cap over each face away with a prism of the
/// face's outline, a little deeper than the wall so the void is reached.
fn open_through_cap(lump: &Solid, thickness: f64, faces: &[&Face]) -> R<Solid> {
    if let Some(f) = faces.iter().find(|f| kind_of(f) != Kind::Plane) {
        let (at, _) = f.project(f.center());
        return Err(format!("the face at {} meets a fillet or a tangent face and is not flat, so it cannot be opened; shell before filleting the edges around it, or choose a flat face", point_text(g(at))));
    }
    let mut made = lump.shell(-thickness, std::iter::empty()).map_err(|_| "the body cannot be hollowed to that wall thickness".to_owned())?;
    for face in faces {
        let (at, normal) = face.project(face.center());
        let (at, normal) = (g(at), g(normal).normalize_or_zero());
        if normal == DVec3::ZERO { return Err("the face to open has no normal".into()); }
        let reach = thickness + 0.02;
        let prism = Solid::extrude(face.iter_edge(), c(-normal * reach)).map_err(|e| format!("the kernel could not build the opening: {e}"))?.translate(c(normal * 0.01));
        let cut = boolean(std::slice::from_ref(&made), std::slice::from_ref(&prism), Bool::Subtract)?;
        made = cut.into_iter().max_by(|a, b| a.volume().total_cmp(&b.volume())).ok_or("the opening left nothing of the body")?;
        let _ = at;
    }
    Ok(made)
}

/// Hollows the body to a wall of `thickness`, open at the picked faces.
pub fn shell(lumps: &[Solid], tags: &Tags, picks: &[FacePick], thickness: f64, feature: Id) -> R<Blended<Tag>> {
    if thickness <= 0.0 {
        return Err("the wall thickness must be greater than zero".into());
    }
    if picks.is_empty() {
        return Err("choose at least one face to leave open".into());
    }
    let asked = picks;
    let picks = find_faces(lumps, tags, picks)?;
    let points = asked.iter().zip(&picks).map(|(pick, found)| settled(&pick.points, |p| g(found.1.project(c(p)).0))).collect();
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
        let mut made = lump.shell(-thickness, faces.iter().copied()).map_err(|_| "the body cannot be hollowed to that wall thickness".to_owned())?;
        // A shell that did not carve anything out is the kernel failing quietly.
        if !(made.volume() > 0.0) || made.volume() > lump.volume() * (1.0 - 1e-9) {
            return Err("the wall is too thick to leave a cavity".into());
        }
        // The kernel also fails quietly when an open face meets a fillet or another tangent
        // face: it keeps the face and seals an offset of it underneath. Open it ourselves then.
        if faces.iter().any(|f| face_kept(&made, f)) {
            made = open_through_cap(lump, thickness, &faces)?;
            if let Some(f) = faces.iter().find(|f| face_kept(&made, f)) {
                let (at, _) = f.project(f.center());
                return Err(format!("the face at {} could not be opened; shell before filleting the edges around it, or choose another face", point_text(g(at))));
            }
        }
        out.push(made);
    }
    let mut out_tags = carry(&[(lumps, tags)], &out, feature);
    for i in 0..out.len() {if !picks.iter().any(|p|p.0==i) {out_tags[i]=tags[i].clone();}}
    // Cadrum's thick-solid bridge does not expose every offset Generated face.
    // Recover the operation's exact normal-offset relation, not a nearest face:
    // the inner face must lie one wall thickness inward, with opposite normals,
    // and have exactly one qualifying source. This survives wall/size edits.
    let sources:Vec<_>=lumps.iter().zip(tags).flat_map(|(s,t)|s.iter_face().zip(t).filter_map(|(f,t)|Some((f,t.as_ref()?)))).collect();
    for (li,lump) in out.iter().enumerate() {for (fi,face) in lump.iter_face().enumerate() {
        if !matches!(out_tags[li][fi].as_ref().map(|t|&t.origin),Some(Origin::Surface {..})) {continue;}
        let (point,normal)=face.project(face.center());let point=g(point);let normal=g(normal);
        let mut found:Vec<_>=sources.iter().filter(|(old,_)| {
            if kind_of(old)!=kind_of(face) {return false;}
            let (on,outward)=old.project(c(point));let on=g(on);let outward=g(outward);
            normal.dot(outward) < -1.+1e-7 && (on-point-outward*thickness).length()<1e-5
        }).map(|(_,tag)|(*tag).clone()).collect();
        found.sort();found.dedup();
        if found.len()==1 {out_tags[li][fi]=Some(Tag::new(Origin::Derived {feature,sources:found},kind_of(face)));}
    }}
    Ok(Blended { lumps: out, tags: out_tags, level, picked: picks.into_iter().map(|p| p.3).collect(), points })
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

/// Each drill location has explicit barrel/head/tip roles. Point coordinates
/// scope repeated holes without relying on their order in the feature's list;
/// changing diameter/depth keeps the role, removing/replacing a point does not.
pub fn drill_tags(lumps:&[Solid],feature:Id,at:DVec3,dir:DVec3,drill:&Drill)->Tags {
    lumps.iter().map(|s|s.iter_face().map(|f| {
        let z=(g(f.center())-at).dot(dir);
        let role=match f.surface().map(|s|s.kind) {
            Some(SurfaceKind::Cylinder {radius})=>if (radius-drill.diameter/2.).abs()<1e-6 {"barrel"} else {"counterbore"},
            Some(SurfaceKind::Cone {..})=>if z>=drill.depth-1e-6 {"tip"} else {"countersink"},
            Some(SurfaceKind::Plane)=>if z>=drill.depth-1e-6 {"bottom"} else if z<0. {"entry"} else {"shoulder"},
            _=>return Some(surface_tag(f,feature)),
        };
        Some(Tag::new(Origin::Semantic {feature,role:format!("drill:{:?}:{:?}:{role}",at.to_array(),dir.to_array())},kind_of(f)))
    }).collect()).collect()
}

/// Named portions of a cylinder used internally by a thread feature.
pub fn cylinder_tags(lumps:&[Solid],feature:Id,from:DVec3,to:DVec3,scope:&str)->Tags {
    let axis=(to-from).normalize();let length=from.distance(to);
    lumps.iter().map(|s|s.iter_face().map(|f| {
        let role=if kind_of(f)==Kind::Plane {if (g(f.center())-from).dot(axis)>length/2. {"end"} else {"start"}} else {"barrel"};
        Some(Tag::new(Origin::Semantic {feature,role:format!("{scope}:{role}")},kind_of(f)))
    }).collect()).collect()
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
            // The polyline is an approximation; a curved edge passes some way from it.
            // Put the point on the edge itself, so that a reference made from it can be
            // told from another edge with the same tag by where it is.
            let mid = g(e.project(c(mid)).0);
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
