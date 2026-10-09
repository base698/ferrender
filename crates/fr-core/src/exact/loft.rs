//! Lofts: one solid skinned through closed sections drawn on different planes.
//!
//! The kernel skins whatever wires it is given, in the order and from the
//! start point they are given. Sections drawn independently rarely agree on
//! either, and a loft between a square that starts at one corner and a square
//! that starts at the next is a twisted, self-crossing solid with no error.
//! So every section after the first is turned and, if need be, reversed to
//! put each of its corners nearest the matching corner of the one before.
//!
//! Sections must have the same number of edges. The kernel will split edges
//! to make unequal sections agree, but where it puts the new corners is not
//! something a drawing controls, so that is refused with the counts.

use super::*;

/// One closed outline of a loft and the plane it is drawn on, in the feature's frame.
#[derive(Clone, Copy, Debug)]
pub struct Section<'a> {
    pub profile: &'a Profile,
    pub plane: Plane,
}

/// A section ready for the kernel: its edges in the order and direction that match the section before.
struct Rib {
    segs: Vec<Seg>,
    /// For each edge, the edge of the first section it is joined to.
    ids: Vec<Id>,
    plane: Plane,
    /// The normal a circle is drawn about, so that circles on opposed planes still run the same way.
    about: DVec3,
}

impl Rib {
    /// Where each edge starts.
    fn corners(&self) -> Vec<DVec3> {
        self.segs.iter().map(|s| self.plane.to_world(start(s))).collect()
    }

    fn centre(&self) -> DVec3 {
        let pts: Vec<DVec3> = self.segs.iter().map(|s| self.plane.to_world(middle(s))).collect();
        pts.iter().sum::<DVec3>() / pts.len().max(1) as f64
    }

    fn edges(&self) -> R<Vec<Edge>> {
        let at = |p: DVec2| c(self.plane.to_world(p));
        self.segs.iter().map(|s| match *s {
            Seg::Circle(centre, r) => Edge::circle(r, c(self.about)).map(|e| e.translate(at(centre))),
            ref other => seg_edge(other, &self.plane, 0.0),
        }.map_err(|e| format!("a section has an edge the kernel rejects: {e}"))).collect()
    }
}

/// Where an edge starts; a circle starts on the +X side of its sketch.
fn start(s: &Seg) -> DVec2 {
    match *s {
        Seg::Circle(centre, r) => centre + DVec2::X * r,
        _ => s.ends().0,
    }
}

/// A point on the edge away from its ends.
fn middle(s: &Seg) -> DVec2 {
    match *s {
        Seg::Line(a, b) => (a + b) / 2.0,
        Seg::Arc(_, m, _) => m,
        Seg::Circle(centre, _) => centre,
        Seg::Spline(points) => (points[1] + points[2]) / 2.0,
    }
}

/// The section's edges walked the other way round, still starting at the same corner.
fn reversed(segs: &[Seg]) -> Vec<Seg> {
    segs.iter().rev().map(|s| s.reversed()).collect()
}

/// Checks the sections and lines each one up with the one before.
fn ribs(sections: &[Section]) -> R<Vec<Rib>> {
    if sections.len() < 2 {
        return Err("a loft needs at least two sections on different planes".into());
    }
    let mut out: Vec<Rib> = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        let n = i + 1;
        if s.profile.path.is_empty() {
            return Err(format!("section {n} has no exact outline"));
        }
        if !s.profile.hole_paths.is_empty() {
            return Err(format!("section {n} has a hole in it; a loft section is one closed outline, so loft the hole separately and cut it"));
        }
        let Some(first) = out.first() else {
            out.push(Rib { segs: s.profile.path.clone(), ids: s.profile.path_ids.clone(), plane: s.plane, about: s.plane.normal() });
            continue;
        };
        let (want, have) = (first.segs.len(), s.profile.path.len());
        if want != have {
            let edges = |k: usize| if k == 1 { "1 edge".to_owned() } else { format!("{k} edges") };
            return Err(format!("section {n} has {} where section 1 has {}; every section of a loft needs the same number of edges, so split or remove edges until they agree", edges(have), edges(want)));
        }
        let before = out.last().unwrap();
        let (a, b) = (before.plane, s.plane);
        if a.normal().dot(b.normal()).abs() > 1.0 - 1e-9 && (b.origin - a.origin).dot(a.normal()).abs() < 1e-7 {
            return Err(format!("sections {i} and {n} lie on the same plane; draw each section on its own plane"));
        }
        let ids = first.ids.clone();
        // A circle has no corners to match; it only has to run the same way round.
        if matches!(s.profile.path[..], [Seg::Circle(..)]) {
            let about = if b.normal().dot(before.about) < 0.0 { -b.normal() } else { b.normal() };
            out.push(Rib { segs: s.profile.path.clone(), ids, plane: b, about });
            continue;
        }
        let target = before.corners();
        let shift = before.centre();
        let mut best: Option<(f64, Vec<Seg>)> = None;
        for flipped in [false, true] {
            let walk = if flipped { reversed(&s.profile.path) } else { s.profile.path.clone() };
            let candidate = Rib { segs: walk, ids: Vec::new(), plane: b, about: b.normal() };
            // Compare shapes, not positions: a section may sit well to one side of the last.
            let (corners, centre) = (candidate.corners(), candidate.centre());
            for turn in 0..have {
                let cost: f64 = (0..have).map(|k| ((corners[(k + turn) % have] - centre) - (target[k] - shift)).length_squared()).sum();
                // Ties keep the section as it was drawn.
                if best.as_ref().is_none_or(|(least, _)| cost < *least - 1e-9 * least.abs().max(1.0)) {
                    let mut segs = candidate.segs.clone();
                    segs.rotate_left(turn);
                    best = Some((cost, segs));
                }
            }
        }
        let segs = best.map(|(_, segs)| segs).unwrap_or_else(|| s.profile.path.clone());
        out.push(Rib { segs, ids, plane: b, about: b.normal() });
    }
    Ok(out)
}

/// Skins one solid through the sections in order. `ruled` joins neighbouring
/// sections with straight lines; otherwise the surface is a smooth curve through all of them.
pub fn loft(sections: &[Section], ruled: bool) -> R<Lumps> {
    let ribs = ribs(sections)?;
    let wires: Vec<Vec<Edge>> = ribs.iter().map(Rib::edges).collect::<R<_>>()?;
    let solid = Solid::loft(wires.iter().map(|w| w.iter()), ruled).map_err(|e| format!("the kernel could not loft those sections: {e}"))?;
    let out = vec![solid];
    let v = volume(&out);
    if !v.is_finite() || v <= 1e-9 {
        return Err("the kernel returned a loft with no volume; the sections may cross each other or be given in an order that folds back".into());
    }
    let (mesh, _) = tessellate(&out)?;
    let tolerance = 2.0 * FINE.deflection_linear * out.iter().map(Solid::area).sum::<f64>() + 1e-6 * v.max(1.0);
    if !mesh.volume().is_finite() || (mesh.volume() - v).abs() > tolerance {
        return Err("the kernel could not make a reliable closed surface for that loft; the sections may twist or cross between planes".into());
    }
    // Every section is a slice of the result, so the result reaches at least as far as they do.
    // Straight walls cannot reach any further. The kernel's own box is padded, so measure the triangles.
    let points: Vec<DVec3> = sections.iter().flat_map(|s| s.profile.outer.iter().map(|p| s.plane.to_world(*p))).collect();
    let (lo, hi) = points.iter().fold((DVec3::MAX, DVec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let Some((blo, bhi)) = mesh.bbox() else { return Err("the kernel returned an empty loft".into()) };
    let slack = 1e-4 + 2.0 * FINE.deflection_linear;
    let inside = |a: DVec3, b: DVec3| (a - b).max_element() <= slack;
    if !inside(blo, lo) || !inside(hi, bhi) || (ruled && (!inside(lo, blo) || !inside(bhi, hi))) {
        return Err("the kernel returned a loft that does not pass through its sections; they may cross each other".into());
    }
    Ok(out)
}

/// Tags for the result of [`loft`] with the same arguments. The ends are caps;
/// a side face is named by the edge of the first section it grew from, since
/// every later section's edges are matched to those.
pub fn tag_loft(lumps: &[Solid], sections: &[Section], feature: Id) -> Tags {
    let plain = || lumps.iter().map(|s| s.iter_face().map(|f| Some(surface_tag(f, feature))).collect()).collect();
    let Ok(ribs) = ribs(sections) else { return plain() };
    let (Some(first), Some(last)) = (ribs.first(), ribs.last()) else { return plain() };
    // Each section's edge lies on the side face it bounds.
    let on = |rib: &Rib, s: &Seg| match *s {
        Seg::Circle(centre, r) => rib.plane.to_world(centre + DVec2::X * r),
        _ => probe(s, &rib.plane, 0.0),
    };
    let probes: Vec<(DVec3, Id)> = ribs.iter().flat_map(|rib| rib.segs.iter().zip(rib.ids.iter().copied().chain(std::iter::repeat(0))).map(move |(s, id)| (on(rib, s), id))).collect();
    tag_sweep(lumps, &probes, [(first.plane, first.centre()), (last.plane, last.centre())], feature)
}
