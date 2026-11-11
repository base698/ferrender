//! Pipes and embossing: a round section carried along paths that may leave their
//! sketch's plane, and shapes pressed onto a curved body.
//!
//! A path can be laid onto a body: each point of it is carried along the
//! sketch's normal until it meets the body's surface, which turns a flat curve
//! into one that follows the surface (a seam on a ball, a groove round a can).
//!
//! The section is a circle, so how it turns about the path does not matter; it
//! only has to stay square to the path. A path drawn in a sketch never runs
//! along the sketch's normal and a path laid onto a body never runs along the
//! direction it was carried in, so either direction serves as the sweep's "up".

use cadrum::{BSplineEnd, ProfileOrient};

use super::pipe::{KINK, legs};
use super::*;
use crate::profile::Chain;

/// One run of a pipe's path: the pieces of a sketch joined end to end.
pub struct PathPart<'a> {
    pub chain: &'a Chain,
    /// The sketch's plane, in the frame the pipe is built in.
    pub plane: Plane,
    /// Lay the run onto this body, carried along `toward` (a unit vector) until it meets it.
    pub onto: Option<Onto<'a>>,
}

#[derive(Clone, Copy)]
pub struct Onto<'a> {
    pub solids: &'a [Solid],
    pub mesh: &'a Mesh,
    pub toward: DVec3,
}

/// A smooth stretch of path, swept in one go.
struct Spine {
    edges: Vec<Edge>,
    /// The direction the section's frame keeps as "up". A curve laid onto a body can
    /// climb along any direction, so it has none: its tube is built from `laid` instead.
    up: Option<DVec3>,
    /// The points a laid curve passes through, in order (a loop's last does not repeat its first),
    /// and the body's outward normal at each.
    laid: Vec<DVec3>,
    normals: Vec<DVec3>,
    start: DVec3,
    end: DVec3,
    /// Unit tangents where it starts and ends, pointing along it.
    t0: DVec3,
    t1: DVec3,
    closed: bool,
    len: f64,
    /// The tightest bend's radius, where it is known.
    bend: f64,
}

/// Where two ends count as the same point.
const MEET: f64 = 1e-4;

/// The nearest point of the body's surface to `p`, its outward normal, and the face it is on.
fn nearest(solids: &[Solid], p: DVec3) -> Option<(DVec3, DVec3, &Face)> {
    solids.iter().flat_map(|s| s.iter_face()).map(|f| { let (q, n) = f.project(c(p)); (g(q), g(n), f) })
        .min_by(|a, b| a.0.distance(p).total_cmp(&b.0.distance(p)))
}

/// The nearest point of the body's surface to `p` and the outward normal there.
pub fn surface_at(solids: &[Solid], p: DVec3) -> Option<(DVec3, DVec3)> {
    nearest(solids, p).map(|(q, n, _)| (q, n))
}

/// Where a line through `p` along `toward` first meets the body, coming from behind `p`.
/// The display mesh finds the face; the exact surface then fixes the point on it.
pub fn land(onto: &Onto, p: DVec3) -> Option<(DVec3, DVec3)> {
    let (lo, hi) = onto.mesh.bbox()?;
    let reach = (lo - p).length().max((hi - p).length()) + 1.0;
    let start = p - onto.toward * reach;
    let (t, tri) = onto.mesh.ray(start, onto.toward)?;
    let mut at = start + onto.toward * t;
    let [a, b, cc] = onto.mesh.tri(tri);
    let mut normal = (b - a).cross(cc - a).normalize_or_zero();
    let Some((_, _, face)) = nearest(onto.solids, at) else { return Some((at, normal)) };
    for _ in 0..6 {
        let (q, n) = face.project(c(at));
        let (q, n) = (g(q), g(n));
        if n.length() < 0.5 { break; }
        normal = n;
        let along = onto.toward.dot(n);
        if along.abs() < 1e-6 { break; }
        let step = (q - at).dot(n) / along;
        at += onto.toward * step;
        if step.abs() < 1e-9 { break; }
    }
    Some((at, normal))
}

/// Points along `pts` no more than `step` apart, keeping the original corners.
fn densify(pts: &[DVec3], step: f64) -> Vec<DVec3> {
    let mut out = vec![pts[0]];
    for w in pts.windows(2) {
        let n = ((w[1] - w[0]).length() / step).ceil().max(1.0) as usize;
        out.extend((1..=n).map(|k| w[0].lerp(w[1], k as f64 / n as f64)));
    }
    out
}

/// The radius of the circle through three points of a sketch arc.
fn arc_radius(a: DVec2, m: DVec2, b: DVec2) -> f64 {
    let (ab, am, mb) = (a.distance(b), a.distance(m), m.distance(b));
    let twice_area = (m - a).perp_dot(b - a).abs();
    if twice_area < 1e-12 { f64::INFINITY } else { ab * am * mb / (2.0 * twice_area) }
}

fn spines(part: &PathPart) -> R<Vec<Spine>> {
    let legs = legs(part.chain, &part.plane)?;
    if legs.is_empty() { return Err("the path is empty".into()); }
    let m = legs.len();
    let corner = |i: usize| legs[i].t1.angle_between(legs[(i + 1) % m].t0) > KINK;
    // Smooth runs, each starting just after a corner (or at the start of an open path).
    let corners: Vec<usize> = (0..m).filter(|i| (part.chain.closed || *i + 1 < m) && corner(*i)).collect();
    let mut runs: Vec<Vec<usize>> = Vec::new();
    if part.chain.closed && corners.is_empty() {
        runs.push((0..m).collect());
    } else {
        let first = if part.chain.closed { (corners[0] + 1) % m } else { 0 };
        let mut run = Vec::new();
        for k in 0..m {
            let i = (first + k) % m;
            run.push(i);
            if corners.contains(&i) || k + 1 == m { runs.push(std::mem::take(&mut run)); }
        }
    }
    let whole_loop = part.chain.closed && corners.is_empty();
    let mut out = Vec::new();
    for run in runs {
        let bend = run.iter().map(|i| match legs[*i].seg {
            Seg::Arc(a, m, b) => arc_radius(a, m, b),
            Seg::Circle(_, r) => r,
            _ => f64::INFINITY,
        }).fold(f64::INFINITY, f64::min);
        let len: f64 = run.iter().map(|i| legs[*i].len).sum();
        match part.onto {
            None => {
                let (first, last) = (&legs[run[0]], &legs[*run.last().unwrap()]);
                out.push(Spine {
                    edges: run.iter().map(|i| legs[*i].edge.clone()).collect(),
                    up: Some(part.plane.normal()), laid: Vec::new(), normals: Vec::new(), start: first.pts[0], end: *last.pts.last().unwrap(),
                    t0: first.t0, t1: last.t1, closed: whole_loop, len, bend,
                });
            }
            Some(onto) => {
                // The run as close-set points, each laid onto the body, then one smooth curve through them.
                // Enough points to follow the surface closely; more only make the kernel's surface heavier.
                let step = (len / 64.0).clamp(1e-3, 5.0);
                let mut pts: Vec<DVec3> = Vec::new();
                for i in &run {
                    // Close-set points between the drawn curve's own, put back onto the curve:
                    // chords between them cut inside a bend and would make the curve wiggle.
                    let leg = &legs[*i];
                    let dense: Vec<DVec3> = densify(&leg.pts, step).into_iter().map(|p| g(leg.edge.project(c(p)).0)).collect();
                    let skip = usize::from(!pts.is_empty());
                    pts.extend(dense.into_iter().skip(skip));
                }
                if whole_loop && pts.len() > 1 && pts[0].distance(*pts.last().unwrap()) < MEET { pts.pop(); }
                // A loop's seam is where its curve starts. Start it away from where the drawn
                // curve does, which is often on a seam of the body as well (a circle's +X side
                // over a sphere's), and two seams in one place trouble the kernel's Booleans.
                if whole_loop { let k = pts.len() * 3 / 8; pts.rotate_left(k); }
                let mut laid = Vec::with_capacity(pts.len());
                for p in &pts {
                    let (at, normal) = land(&onto, *p).ok_or("part of the path misses the body it is laid onto; move it over the body or choose the other side")?;
                    if normal.dot(onto.toward) > -0.05 {
                        return Err("part of the path lands where the body's surface runs along the direction it is laid in; move it in from the body's edge".into());
                    }
                    laid.push((at, normal));
                }
                // Drop points that landed on top of each other.
                laid.dedup_by(|a, b| a.0.distance(b.0) < 1e-6);
                let (laid, normals): (Vec<DVec3>, Vec<DVec3>) = laid.into_iter().unzip();
                if laid.len() < 3 { return Err("the path laid onto the body is too short".into()); }
                let edge = Edge::bspline(laid.iter().map(|p| c(*p)).collect::<Vec<_>>().iter(), if whole_loop { BSplineEnd::Periodic } else { BSplineEnd::NotAKnot })
                    .map_err(|e| format!("the kernel could not follow the path laid onto the body: {e}"))?;
                let len: f64 = edge.approximation_segments(FINE).windows(2).map(|w| g(w[1]).distance(g(w[0]))).sum();
                let (start, end) = (laid[0], if whole_loop { laid[0] } else { *laid.last().unwrap() });
                let (t0, t1) = (g(edge.start_tangent()).normalize_or_zero(), g(edge.end_tangent()).normalize_or_zero());
                // The tightest bend, from the circles through neighbouring points.
                let m = laid.len();
                let triples = if whole_loop { m } else { m.saturating_sub(2) };
                let bend = (0..triples).map(|i| {
                    let (a, b, cc) = (laid[i], laid[(i + 1) % m], laid[(i + 2) % m]);
                    let area2 = (b - a).cross(cc - a).length();
                    if area2 < 1e-15 { f64::INFINITY } else { a.distance(b) * b.distance(cc) * cc.distance(a) / (2.0 * area2) }
                }).fold(f64::INFINITY, f64::min);
                out.push(Spine { edges: vec![edge], up: None, laid, normals, start, end, t0, t1, closed: whole_loop, len, bend });
            }
        }
    }
    Ok(out)
}

/// A free end of a spine: where it is, which way the path leaves the tube there, and
/// whether another end meets it.
struct End { at: DVec3, out: DVec3 }

/// Sections round a laid curve.
const AROUND: usize = 24;

/// A tube round a laid curve, built as a surface through rings of points. The kernel's
/// sweep turns its section with the curve's bend, which flips where the curve changes
/// the way it bends (a wave over a can), and the twisted surface it leaves breaks later
/// Booleans. Here each ring is turned by the body's surface normal, which does not flip.
/// `lead_in` and `lead_out` carry the ends straight on, for a bore that must break through them.
fn laid_tube(s: &Spine, radius: f64, lead_in: f64, lead_out: f64) -> R<Solid> {
    let edge = &s.edges[0];
    let n = s.laid.len();
    let mut pts = s.laid.clone();
    let mut tan: Vec<DVec3> = (0..n).map(|i| {
        let chord = if i + 1 < n { pts[i + 1] - pts[i] } else if s.closed { pts[0] - pts[i] } else { pts[i] - pts[i - 1] };
        let t = g(edge.project(c(pts[i])).1).normalize_or_zero();
        if t == DVec3::ZERO { chord.normalize_or_zero() } else if t.dot(chord) < 0.0 { -t } else { t }
    }).collect();
    if lead_in > 0.0 {
        let (p, t) = (pts[0], tan[0]);
        for k in 1..=2 { pts.insert(0, p - t * lead_in * k as f64 / 2.0); tan.insert(0, t); }
    }
    if lead_out > 0.0 {
        let (p, t) = (*pts.last().unwrap(), *tan.last().unwrap());
        for k in 1..=2 { pts.push(p + t * lead_out * k as f64 / 2.0); tan.push(t); }
    }
    let mut up = s.normals.clone();
    if lead_in > 0.0 { let u = up[0]; up.splice(0..0, [u, u]); }
    if lead_out > 0.0 { let u = *up.last().unwrap(); up.extend([u, u]); }
    let m = pts.len();
    // Each ring starts from the body's normal there, square to the curve: the ring's
    // seam then runs along the top of the tube, as far as it can be from where the
    // body's surface cuts through it. The kernel's Booleans fail when the two meet.
    let r: Vec<DVec3> = (0..m).map(|i| (up[i] - tan[i] * up[i].dot(tan[i])).normalize_or_zero()).collect();
    if r.iter().any(|v| !v.is_finite() || v.length() < 0.5) { return Err("the path laid onto the body doubles back on itself".into()); }
    let made = Solid::bspline(m, AROUND, s.closed, |i, j| {
        let a = std::f64::consts::TAU * j as f64 / AROUND as f64;
        c(pts[i] + (r[i] * a.cos() + tan[i].cross(r[i]) * a.sin()) * radius)
    }).map_err(|e| format!("the kernel could not build the pipe along the laid path: {e}"))?;
    // The kernel may build the surface facing inward.
    let full = std::f64::consts::PI * radius * radius * (s.len + lead_in + lead_out);
    if (made.volume().abs() - full).abs() > 0.03 * full {
        return Err("the kernel built a wrong pipe along the laid path; it may bend too tightly for the diameter".into());
    }
    Ok(made)
}

/// The round section of `radius` swept along every spine, with a ball at each corner
/// and, where `lead` is set, each free end carried that much past where the path stops.
fn rod(spines: &[Spine], radius: f64, lead: f64, laid_by_sweep: bool) -> R<(Lumps, usize)> {
    let fail = |e: cadrum::Error| format!("the kernel could not sweep the pipe: {e}");
    let mut ends: Vec<End> = Vec::new();
    for s in spines.iter().filter(|s| !s.closed) {
        ends.push(End { at: s.start, out: -s.t0 });
        ends.push(End { at: s.end, out: s.t1 });
    }
    // Ends that meet: smooth joins need nothing, corners get a ball; the rest are free.
    let mut free = vec![true; ends.len()];
    let mut balls = Vec::new();
    for i in 0..ends.len() {
        for j in i + 1..ends.len() {
            if ends[i].at.distance(ends[j].at) > MEET { continue; }
            free[i] = false;
            free[j] = false;
            // At a smooth join one tube leaves the point the way the other arrives.
            if ends[i].out.angle_between(-ends[j].out) > KINK { balls.push(ends[i].at); }
        }
    }
    let mut parts = Vec::new();
    for (k, s) in spines.iter().enumerate() {
        let (lead_in, lead_out) = if s.closed { (false, false) } else { (free[2 * spines[..k].iter().filter(|s| !s.closed).count()], free[2 * spines[..k].iter().filter(|s| !s.closed).count() + 1]) };
        let before = (lead > 0.0 && lead_in).then(|| Edge::line(c(s.start - s.t0 * lead), c(s.start))).transpose().map_err(fail)?;
        let after = (lead > 0.0 && lead_out).then(|| Edge::line(c(s.end), c(s.end + s.t1 * lead))).transpose().map_err(fail)?;
        let from = if before.is_some() { s.start - s.t0 * lead } else { s.start };
        let ring = Edge::circle(radius, c(s.t0)).map_err(fail)?.translate(c(from));
        let spine: Vec<&Edge> = before.iter().chain(&s.edges).chain(after.iter()).collect();
        let swept = match s.up {
            Some(up) => Solid::sweep(std::slice::from_ref(&ring), spine, ProfileOrient::Up(c(up))).map_err(fail)?,
            None if !laid_by_sweep => laid_tube(s, radius, if before.is_some() { lead } else { 0.0 }, if after.is_some() { lead } else { 0.0 })?,
            None => {
                // The kernel's own sweep, turning with the curve's bend.
                let made = Solid::sweep(std::slice::from_ref(&ring), spine, ProfileOrient::Torsion).map_err(fail)?;
                let full = std::f64::consts::PI * radius * radius * (s.len + if before.is_some() { lead } else { 0.0 } + if after.is_some() { lead } else { 0.0 });
                if (made.volume() - full).abs() > 0.02 * full { return Err("the kernel built a wrong pipe along the laid path".into()); }
                made
            }
        };
        if !(swept.volume() > 0.0) { return Err("the kernel returned an empty pipe".into()); }
        parts.push(swept);
    }
    for at in &balls {
        parts.push(Solid::sphere(radius).translate(c(*at)));
    }
    Ok((fuse(parts)?, balls.len()))
}

/// A pipe of `diameter` along the path parts, hollow when `bore` is given: every way it
/// can be built, best first. A path laid onto a body is a freeform curve, and the kernel's
/// Booleans against freeform tubes sometimes go wrong, so the caller checks each against
/// the body and takes the first that combines with it correctly. Other paths are built one way.
pub fn tubes(parts: &[PathPart], diameter: f64, bore: Option<f64>) -> R<Vec<Lumps>> {
    let radius = diameter / 2.0;
    if !(radius > 0.0) { return Err("the pipe's diameter must be more than zero".into()); }
    if let Some(b) = bore && !(b > 0.0 && b < diameter) { return Err("the bore must be more than zero and less than the pipe's diameter".into()); }
    let mut all = Vec::new();
    for part in parts { all.extend(spines(part)?); }
    if let Some(s) = all.iter().find(|s| s.bend <= radius * 1.0001) {
        return Err(format!("the pipe is too thick for its path: a bend there has a radius of {} mm, so the diameter must be less than {} mm", crate::units::trim_num(s.bend, 4), crate::units::trim_num(2.0 * s.bend, 4)));
    }
    let laid = all.iter().any(|s| s.up.is_none());
    let mut made = Vec::new();
    let mut first_error = None;
    for by_sweep in if laid { vec![false, true] } else { vec![false] } {
        match one_tube(&all, radius, bore, by_sweep) {
            Ok(l) => made.push(l),
            Err(e) => { first_error.get_or_insert(e); }
        }
    }
    if made.is_empty() { return Err(first_error.unwrap_or_else(|| "the pipe could not be built".into())); }
    Ok(made)
}

fn one_tube(all: &[Spine], radius: f64, bore: Option<f64>, by_sweep: bool) -> R<Lumps> {
    let (outer, balls) = rod(all, radius, 0.0, by_sweep)?;
    // The section's area times the length of path, give or take the balls at corners.
    let length: f64 = all.iter().map(|s| s.len).sum();
    let expected = std::f64::consts::PI * radius * radius * length;
    let got = volume(&outer);
    let ball = 4.0 / 3.0 * std::f64::consts::PI * radius.powi(3);
    if got < expected * 0.97 - 1e-9 || got > expected * 1.03 + balls as f64 * ball + 1e-9 {
        return Err("the kernel built a wrong pipe; the path may cross itself or bend too tightly for the diameter".into());
    }
    match bore {
        None => Ok(outer),
        Some(b) => {
            let (inner, _) = rod(all, b / 2.0, radius, by_sweep)?;
            boolean(&outer, &inner, Bool::Subtract)
        }
    }
}

/// Shapes projected onto a body along `from` and raised off its surface (or, without
/// `raise`, sunk into it) by `depth`. Returns the material to add or take away.
///
/// `from` points from the body toward the side the shapes are seen from; only the
/// surface facing that way is marked. The shapes' sides run along `from`, and their
/// tops (or floors) follow the surface at `depth` from it.
pub fn emboss(profiles: &[&Profile], plane: &Plane, solids: &[Solid], mesh: &Mesh, depth: f64, raise: bool, from: DVec3) -> R<Lumps> {
    if !(depth > 0.0) { return Err("the depth must be more than zero".into()); }
    let onto = Onto { solids, mesh, toward: -from };
    let n = plane.normal();
    // Every outline must land on surface facing the viewer. Where they land says how
    // far along the normal the marked surface lies.
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut lo2, mut hi2) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for p in profiles {
        for ring in std::iter::once(&p.outer).chain(&p.holes) {
            for q in ring { (lo2, hi2) = (lo2.min(*q), hi2.max(*q)); }
            let step = (ring.len() / 64).max(1);
            for q in ring.iter().step_by(step) {
                match land(&onto, plane.to_world(*q)) {
                    Some((at, normal)) if normal.dot(from) > 0.05 => {
                        let z = (at - plane.origin).dot(n);
                        (zmin, zmax) = (zmin.min(z), zmax.max(z));
                    }
                    _ => return Err("the shapes reach past the body, or onto surface facing away from them; move them, make them smaller, or choose the other side".into()),
                }
            }
        }
    }
    if !zmin.is_finite() { return Err("there are no shapes to emboss".into()); }
    // Offsetting the whole body is slow and, on a busy body, fragile. Only the patch
    // under the shapes is needed: cut out a block a margin wider than them, reaching
    // a margin past the surface on both sides, and offset just that. A wide patch can
    // take in a rim or a step near the shapes that cannot be moved; a narrower one,
    // still wider than the depth, is tried before giving up.
    let margin = 2.0 * depth + 0.5;
    let (lo_z, hi_z) = (zmin - 2.0 * depth - margin, zmax + 2.0 * depth + margin);
    // The layer between the surface and the surface moved out (or in) by the depth.
    let thickness = if raise { depth } else { -depth };
    let mut skin = None;
    for side in [margin, depth + 0.25, depth + 0.05] {
        let (lo, hi) = (lo2 - DVec2::splat(side), hi2 + DVec2::splat(side));
        let corners = [lo, DVec2::new(hi.x, lo.y), hi, DVec2::new(lo.x, hi.y)].map(|q| c(plane.to_world(q) + n * lo_z));
        let rim = Edge::polygon(corners.iter()).map_err(|e| format!("the kernel could not outline the shapes: {e}"))?;
        let block = Solid::extrude(&rim, c(n * (hi_z - lo_z))).map_err(|e| format!("the kernel could not outline the shapes: {e}"))?;
        let patch = boolean(solids, std::slice::from_ref(&block), Bool::Intersect)?;
        let layers: R<Vec<Lumps>> = patch.iter().map(|s| super::sealed_shell(s, thickness)).collect();
        if let Ok(layers) = layers { skin = Some(layers.concat()); break; }
    }
    let skin = skin.ok_or_else(|| format!("the body's surface cannot be moved {} by {} mm there; use a smaller depth, or keep the shapes clear of edges and rims", if raise { "out" } else { "in" }, crate::units::trim_num(depth, 4)))?;
    // The shapes as a prism through that layer, clear of the block's cut face on the far side.
    let (z0, z1) = if from.dot(n) > 0.0 { (lo_z + depth + margin / 2.0, hi_z + depth + 1.0) } else { (lo_z - depth - 1.0, hi_z - depth - margin / 2.0) };
    let prism = extrude(profiles, plane, z0, z1)?;
    let pieces = boolean(&prism, &skin, Bool::Intersect)?;
    // The prism passes through the body and marks its far side too; keep the near side.
    let kept: Vec<Solid> = pieces.into_iter().filter(|piece| nearest(solids, g(piece.center())).is_some_and(|(_, normal, _)| normal.dot(from) > 0.0)).collect();
    if kept.is_empty() { return Err("the shapes do not land on the body".into()); }
    Ok(kept)
}
