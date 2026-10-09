//! Sweeps: a profile carried along a path drawn in another sketch.
//!
//! The kernel's pipe is exact along a path whose pieces meet tangentially,
//! and it finds for itself where on the path the profile sits. At a sharp
//! corner it returns a wrong solid without an error, so a path with corners
//! is cut into its smooth runs: each run is swept on its own, a little past
//! its ends, trimmed on the plane that bisects the corner, and the pieces are
//! joined. That is a mitred corner, as on a picture frame.
//!
//! Every result is checked against the volume the sweep must have: the
//! profile's area times the distance its centroid travels.

use cadrum::ProfileOrient;
use glam::DQuat;

use super::*;
use crate::profile::Chain;

const PATH: Tessellation = Tessellation { deflection_linear: 0.005, deflection_angular: 0.05, relative_linear: false };
/// Pieces whose tangents differ by more than this (radians) meet at a corner.
const KINK: f64 = 1e-3;
/// A corner may turn the path by at most this much (radians); beyond it the mitre runs away.
const SHARPEST: f64 = 150.0 * std::f64::consts::PI / 180.0;

/// One piece of the path in the feature's frame.
struct Leg {
    edge: Edge,
    /// The sketch segment the piece was made from, for cutting part of it out.
    seg: Seg,
    /// The piece as points from its start to its end.
    pts: Vec<DVec3>,
    /// Unit tangents at the start and the end, pointing along the path.
    t0: DVec3,
    t1: DVec3,
    id: Id,
    len: f64,
    /// How far the piece turns to the left about the path plane's normal, signed.
    turning: f64,
}

struct Setup {
    legs: Vec<Leg>,
    /// The signed turn at the joint after each leg, where that joint is a corner.
    after: Vec<Option<f64>>,
    closed: bool,
    /// The path plane's normal.
    n: DVec3,
    follow: bool,
    /// Where on the path the profile sits, and the path's direction there.
    at: DVec3,
    tangent: DVec3,
    /// The profile plane's normal.
    normal: DVec3,
    /// How far the profiles reach to the right (negative) and left of the path.
    reach: (f64, f64),
    /// The legs are only part of the path, so the profile need not sit on them:
    /// it is carried to where they start before it is swept.
    placed: bool,
}

/// The angle from `a` to `b` about `n`, positive anticlockwise seen from `n`'s tip.
fn turn(n: DVec3, a: DVec3, b: DVec3) -> f64 {
    n.dot(a.cross(b)).atan2(a.dot(b))
}

fn facing(t: DVec3, along: DVec3) -> DVec3 {
    let t = t.normalize_or_zero();
    if t == DVec3::ZERO { along.normalize_or_zero() } else if t.dot(along) < 0.0 { -t } else { t }
}

/// Area and first moment of a polygon, signed by its winding.
fn moment(p: &[DVec2]) -> (f64, DVec2) {
    let (mut area, mut first) = (0.0, DVec2::ZERO);
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let cross = a.perp_dot(b);
        area += cross / 2.0;
        first += (a + b) * cross / 6.0;
    }
    (area, first)
}

/// The centre of area of a profile, holes left out.
fn centre(p: &Profile) -> DVec2 {
    let (mut area, mut first) = moment(&p.outer);
    for h in &p.holes {
        let (a, f) = moment(h);
        area += a;
        first += f;
    }
    if area.abs() < 1e-12 { p.centroid() } else { first / area }
}

/// Measures a kernel edge as a piece of the path.
fn leg(edge: Edge, seg: Seg, id: Id, n: DVec3, measured: Option<f64>) -> R<Leg> {
    let pts: Vec<DVec3> = edge.approximation_segments(PATH).into_iter().map(g).collect();
    if pts.len() < 2 { return Err("a piece of the path has no length".to_owned()); }
    let chords: Vec<DVec3> = pts.windows(2).map(|w| w[1] - w[0]).filter(|d| d.length() > 1e-12).collect();
    let (Some(first), Some(last)) = (chords.first().copied(), chords.last().copied()) else { return Err("a piece of the path has no length".to_owned()) };
    let (t0, t1) = (facing(g(edge.start_tangent()), first), facing(g(edge.end_tangent()), last));
    let turning = turn(n, t0, first) + chords.windows(2).map(|w| turn(n, w[0], w[1])).sum::<f64>() + turn(n, last, t1);
    Ok(Leg { edge, seg, len: measured.unwrap_or_else(|| chords.iter().map(|d| d.length()).sum()), pts, t0, t1, id, turning })
}

fn legs(path: &Chain, plane: &Plane) -> R<Vec<Leg>> {
    let n = plane.normal();
    path.segs.iter().zip(&path.ids).map(|(seg, id)| {
        let edge = seg_edge(seg, plane, 0.0).map_err(|e| format!("the path has an edge the kernel rejects: {e}"))?;
        leg(edge, *seg, *id, n, seg_length(seg))
    }).collect()
}

/// The point a fraction `t` of the way along a line, arc or circle, in sketch
/// coordinates. A circle starts at the sketch's +X side and runs anticlockwise.
fn along_seg(seg: &Seg, t: f64) -> Option<DVec2> {
    use std::f64::consts::TAU;
    match *seg {
        Seg::Line(a, b) => Some(a.lerp(b, t)),
        Seg::Circle(centre, r) => Some(centre + DVec2::from_angle(TAU * t) * r),
        Seg::Arc(a, m, b) => {
            let (centre, radius, start, sweep) = arc_of(a, m, b)?;
            Some(centre + DVec2::from_angle(start + sweep * t) * radius)
        }
        Seg::Spline(_) => None,
    }
}

/// An arc through three points as its centre, radius, start angle and signed sweep.
fn arc_of(a: DVec2, m: DVec2, b: DVec2) -> Option<(DVec2, f64, f64, f64)> {
    use std::f64::consts::TAU;
    let centre = crate::sketch::arc3_center(a, m, b).ok()?;
    let start = (a - centre).y.atan2((a - centre).x);
    let end = (b - centre).y.atan2((b - centre).x);
    let anticlockwise = (m - a).perp_dot(b - m) > 0.0;
    let mut sweep = (end - start).rem_euclid(TAU);
    if sweep < 1e-12 { sweep = TAU; }
    if !anticlockwise { sweep -= TAU; }
    Some((centre, centre.distance(a), start, sweep))
}

/// The exact length of a line, arc or circle; a spline is measured along its points instead.
fn seg_length(seg: &Seg) -> Option<f64> {
    match *seg {
        Seg::Line(a, b) => Some(a.distance(b)),
        Seg::Circle(_, r) => Some(std::f64::consts::TAU * r),
        Seg::Arc(a, m, b) => arc_of(a, m, b).map(|(_, radius, _, sweep)| radius * sweep.abs()),
        Seg::Spline(_) => None,
    }
}

/// The point at distance `d` along a run of points.
fn along_points(pts: &[DVec3], d: f64) -> DVec3 {
    let mut left = d;
    for w in pts.windows(2) {
        let step = w[0].distance(w[1]);
        if left <= step && step > 0.0 { return w[0].lerp(w[1], left / step); }
        left -= step;
    }
    *pts.last().unwrap()
}

/// The part of a piece between two fractions of its length. Lines, arcs and
/// circles are cut exactly. A spline is refitted through points taken from the
/// original curve, so a cut spline is a very close copy rather than the same curve.
fn part_of(whole: &Leg, plane: &Plane, t0: f64, t1: f64, n: DVec3) -> R<Leg> {
    let fail = |e: cadrum::Error| format!("the kernel rejects that part of the path: {e}");
    match (along_seg(&whole.seg, t0), along_seg(&whole.seg, (t0 + t1) / 2.0), along_seg(&whole.seg, t1)) {
        (Some(a), Some(m), Some(b)) => {
            let seg = if matches!(whole.seg, Seg::Line(..)) { Seg::Line(a, b) } else { Seg::Arc(a, m, b) };
            leg(seg_edge(&seg, plane, 0.0).map_err(fail)?, seg, whole.id, n, seg_length(&seg))
        }
        _ => {
            const SAMPLES: usize = 24;
            let points: Vec<cadrum::DVec3> = (0..=SAMPLES).map(|k| {
                let d = whole.len * (t0 + (t1 - t0) * k as f64 / SAMPLES as f64);
                whole.edge.project(c(along_points(&whole.pts, d))).0
            }).collect();
            leg(Edge::bspline(points.iter(), cadrum::BSplineEnd::NotAKnot).map_err(fail)?, whole.seg, whole.id, n, None)
        }
    }
}

/// The pieces of a path between two fractions of its whole length.
fn between(full: Vec<Leg>, plane: &Plane, from: f64, to: f64, n: DVec3) -> R<Vec<Leg>> {
    let total: f64 = full.iter().map(|l| l.len).sum();
    let (from, to) = (from * total, to * total);
    // A cut that lands within this of a joint is taken to be at the joint.
    let sliver = 1e-6 * total.max(1.0);
    let mut out = Vec::new();
    let mut start = 0.0;
    for whole in full {
        let end = start + whole.len;
        let (lo, hi) = (from.max(start), to.min(end));
        if hi - lo > sliver {
            let (t0, t1) = ((lo - start) / whole.len, (hi - start) / whole.len);
            let (t0, t1) = (if lo - start < sliver { 0.0 } else { t0 }, if end - hi < sliver { 1.0 } else { t1 });
            out.push(if t0 == 0.0 && t1 == 1.0 { whole } else { part_of(&whole, plane, t0, t1, n)? });
        }
        start = end;
    }
    if out.is_empty() { return Err("that part of the path has no length".into()); }
    Ok(out)
}

/// The parts of a path to sweep, as fractions of its length in order. An empty
/// list is the whole path. Parts that touch or overlap are joined.
pub fn sweep_spans(spans: &[[f64; 2]]) -> R<Vec<Option<(f64, f64)>>> {
    if spans.is_empty() { return Ok(vec![None]); }
    let mut list = Vec::new();
    for [a, b] in spans {
        if !a.is_finite() || !b.is_finite() || *a < -1e-9 || *b > 1.0 + 1e-9 { return Err("each part of the path is given as two fractions between 0 and 1".into()); }
        let (a, b) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
        if b - a < 1e-6 { return Err("each part of the path needs its end after its start".into()); }
        list.push((a, b));
    }
    list.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut joined: Vec<(f64, f64)> = Vec::new();
    for (a, b) in list {
        match joined.last_mut() {
            Some(last) if a <= last.1 + 1e-9 => last.1 = last.1.max(b),
            _ => joined.push((a, b)),
        }
    }
    Ok(joined.into_iter().map(|(a, b)| if a <= 1e-9 && b >= 1.0 - 1e-9 { None } else { Some((a, b)) }).collect())
}

fn joints(legs: &[Leg], closed: bool, n: DVec3) -> Vec<Option<f64>> {
    let m = legs.len();
    (0..m).map(|i| {
        if i + 1 == m && !closed { return None; }
        // One circle closes on itself smoothly.
        if m == 1 { return None; }
        let angle = turn(n, legs[i].t1, legs[(i + 1) % m].t0);
        (angle.abs() > KINK).then_some(angle)
    }).collect()
}

impl Setup {
    /// `span` limits the sweep to part of the path, as two fractions of its length.
    /// Where the profile sits, and so how it is carried, is always taken from the whole path.
    fn new(profiles: &[&Profile], plane: &Plane, path: &Chain, path_plane: &Plane, follow: bool, span: Option<(f64, f64)>) -> R<Setup> {
        if profiles.is_empty() { return Err("no profile is selected".into()); }
        if path.segs.is_empty() { return Err("the path is empty".into()); }
        let n = path_plane.normal();
        let legs = legs(path, path_plane)?;
        let normal = plane.normal();
        let centroid = plane.to_world(centre(profiles[0]));
        // Where the profile's plane meets the path, nearest the profile; failing that, the nearest point of the path.
        let off = |p: DVec3| (p - plane.origin).dot(normal);
        let mut best: Option<(f64, f64, usize, DVec3, DVec3)> = None;
        let mut offer = |gap: f64, at: DVec3, along: DVec3, leg: usize| {
            let key = (if gap < 1e-7 { 0.0 } else { gap }, at.distance(centroid));
            if best.as_ref().is_none_or(|b| key < (b.0, b.1)) { best = Some((key.0, key.1, leg, at, along)); }
        };
        for (i, leg) in legs.iter().enumerate() {
            for w in leg.pts.windows(2) {
                let (sa, sb) = (off(w[0]), off(w[1]));
                offer(sa.abs(), w[0], w[1] - w[0], i);
                offer(sb.abs(), w[1], w[1] - w[0], i);
                if sa * sb < 0.0 { offer(0.0, w[0] + (w[1] - w[0]) * (sa / (sa - sb)), w[1] - w[0], i); }
            }
        }
        let (_, _, leg, near, along) = best.ok_or("the path has no length")?;
        let (at, tangent) = legs[leg].edge.project(c(near));
        let (at, tangent) = (g(at), facing(g(tangent), along));
        if normal.dot(tangent).abs() < 0.05 {
            return Err("the profile's plane runs along the path where they meet; draw the profile on a plane that crosses the path".into());
        }
        let left = n.cross(tangent).normalize_or_zero();
        let reach = profiles.iter().flat_map(|p| p.outer.iter()).map(|q| (plane.to_world(*q) - at).dot(left)).fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)));
        let (mut legs, closed, placed) = match span {
            Some((from, to)) => (between(legs, path_plane, from, to, n)?, false, true),
            None => (legs, path.closed, false),
        };
        let mut after = joints(&legs, closed, n);
        // A closed path with corners is walked from just after one of them, so every run ends at a corner.
        if closed && follow && let Some(k) = after.iter().position(Option::is_some) {
            let first = (k + 1) % legs.len();
            legs.rotate_left(first);
            after = joints(&legs, true, n);
        }
        let setup = Setup { legs, after, closed, n, follow, at, tangent, normal, reach, placed };
        setup.check()?;
        Ok(setup)
    }

    /// Refuses the paths the kernel would sweep into a wrong or self-crossing solid.
    fn check(&self) -> R<()> {
        if !self.follow {
            let side = self.normal.dot(self.tangent).signum();
            let sideways = self.legs.iter().flat_map(|l| l.pts.windows(2)).map(|w| (w[1] - w[0]).normalize_or_zero()).any(|d| d != DVec3::ZERO && self.normal.dot(d) * side < 1e-3);
            if sideways {
                return Err("with a fixed orientation the path may not run sideways to the profile or back across it; let the profile follow the path instead".into());
            }
            return Ok(());
        }
        for leg in &self.legs {
            for w in leg.pts.windows(3) {
                let (a, b) = (w[1] - w[0], w[2] - w[1]);
                let scale = a.length() * b.length() * (w[2] - w[0]).length();
                if scale < 1e-18 { continue; }
                // Positive when the path bends to the left, where `reach.1` is.
                let bend = 2.0 * self.n.dot(a.cross(b)) / scale;
                for d in [self.reach.0, self.reach.1] {
                    if 1.0 - bend * d < 0.02 {
                        return Err(format!("the path bends more tightly (radius {:.3}) than the profile reaches on the inside of the bend ({:.3}); use a wider bend or a narrower profile", 1.0 / bend.abs(), d.abs()));
                    }
                }
            }
        }
        if self.after.iter().flatten().any(|a| a.abs() > SHARPEST) {
            return Err("the path doubles back at a corner; a corner may turn by at most 150 degrees".into());
        }
        for (run, start, end) in self.runs() {
            let length: f64 = self.legs[run].iter().map(|l| l.len).sum();
            let need = start.map_or(0.0, |a| self.inside(a)) + end.map_or(0.0, |a| self.inside(a));
            if need >= length - 1e-6 {
                return Err("a stretch of the path between two corners is too short for the profile to turn them; lengthen it or narrow the profile".into());
            }
        }
        Ok(())
    }

    /// How much of a run a mitre takes up on the inside of a corner of this angle.
    fn inside(&self, angle: f64) -> f64 {
        let reach = if angle > 0.0 { self.reach.1.max(0.0) } else { (-self.reach.0).max(0.0) };
        reach * (angle.abs() / 2.0).tan()
    }

    /// The smooth runs of the path as leg ranges, each with the corner angle at its start and end.
    fn runs(&self) -> Vec<(std::ops::Range<usize>, Option<f64>, Option<f64>)> {
        let m = self.legs.len();
        let mut runs = Vec::new();
        let mut start = 0;
        for i in 0..m {
            if self.after[i].is_some() || i + 1 == m {
                let before = if start > 0 { self.after[start - 1] } else if self.closed { self.after[m - 1] } else { None };
                runs.push((start..i + 1, before, self.after[i]));
                start = i + 1;
            }
        }
        runs
    }

    /// Where a point of the profile is when the profile has travelled to `to`, heading `along`.
    fn carry(&self, p: DVec3, to: DVec3, along: DVec3) -> DVec3 {
        to + self.rotation(along) * (p - self.at)
    }

    fn rotation(&self, along: DVec3) -> DQuat {
        if self.follow { DQuat::from_axis_angle(self.n, turn(self.n, self.tangent, along)) } else { DQuat::IDENTITY }
    }

    fn start(&self) -> (DVec3, DVec3) { (self.legs[0].pts[0], self.legs[0].t0) }

    fn end(&self) -> (DVec3, DVec3) {
        let last = self.legs.last().unwrap();
        (*last.pts.last().unwrap(), last.t1)
    }

    /// The volume a profile must sweep out: its area, seen along the path, times how far its centre travels.
    fn expected(&self, p: &Profile, plane: &Plane) -> f64 {
        let area = p.area().abs();
        if !self.follow {
            return area * self.normal.dot(self.end().0 - self.start().0).abs();
        }
        let left = self.n.cross(self.tangent).normalize_or_zero();
        let d = (plane.to_world(centre(p)) - self.at).dot(left);
        let length: f64 = self.legs.iter().map(|l| l.len - d * l.turning).sum::<f64>() - self.after.iter().flatten().map(|a| 2.0 * d * (a / 2.0).tan()).sum::<f64>();
        area * self.normal.dot(self.tangent).abs() * length
    }
}

/// Removes what lies on the `away` side of the plane through `origin`.
fn trim(lumps: Lumps, origin: DVec3, away: DVec3) -> R<Lumps> {
    let half = Solid::half_space(c(origin), c(away));
    let mut out = Vec::new();
    for lump in &lumps {
        let expr: Boolean<Solid> = lump.into();
        out.extend((expr - &half).build_vec().map_err(|e| format!("the kernel could not mitre a corner of the sweep: {e}"))?);
    }
    Ok(out)
}

/// One closed boundary swept along the whole path.
fn pipe(s: &Setup, boundary: &[Seg], plane: &Plane) -> R<Lumps> {
    if boundary.is_empty() { return Err("the profile has no exact outline".into()); }
    let base = ring(boundary, plane, 0.0)?;
    let fail = |e: cadrum::Error| format!("the kernel could not sweep the profile: {e}");
    let solid = |l: Solid| if l.volume() > 0.0 { Ok(l) } else { Err("the kernel returned an empty sweep".to_owned()) };
    if !s.follow {
        // Part of a path: the profile slides, unturned, to where that part starts.
        let base: Vec<Edge> = if s.placed { base.into_iter().map(|e| e.translate(c(s.start().0 - s.at))).collect() } else { base };
        return Ok(vec![solid(Solid::sweep(&base, s.legs.iter().map(|l| &l.edge), ProfileOrient::Fixed).map_err(fail)?)?]);
    }
    let up = ProfileOrient::Up(c(s.n));
    // On the whole path the kernel finds the profile's place itself. On part of
    // one, each run below is given the profile where the run begins.
    if !s.placed && s.after.iter().all(Option::is_none) {
        return Ok(vec![solid(Solid::sweep(&base, s.legs.iter().map(|l| &l.edge), up).map_err(fail)?)?]);
    }
    let reach = s.reach.0.abs().max(s.reach.1.abs());
    let past = |angle: f64| reach * (angle.abs() / 2.0).tan() + reach.max(1.0);
    let m = s.legs.len();
    let mut parts = Vec::new();
    for (run, before, after) in s.runs() {
        let (first, last) = (&s.legs[run.start], &s.legs[run.end - 1]);
        let (from, heading) = (first.pts[0], first.t0);
        let (to, leaving) = (*last.pts.last().unwrap(), last.t1);
        // The run goes a little past each corner so the mitre has something to cut.
        let lead = before.map(|a| Edge::line(c(from - heading * past(a)), c(from))).transpose().map_err(fail)?;
        let tail = after.map(|a| Edge::line(c(to), c(to + leaving * past(a)))).transpose().map_err(fail)?;
        let spine: Vec<&Edge> = lead.iter().chain(s.legs[run.clone()].iter().map(|l| &l.edge)).chain(tail.iter()).collect();
        // The profile as it stands where this run begins.
        let angle = turn(s.n, s.tangent, heading);
        let placed: Vec<Edge> = base.iter().cloned().map(|e| e.rotate(c(s.at), c(s.n), angle).translate(c(from - s.at))).collect();
        let mut lumps = vec![solid(Solid::sweep(&placed, spine, up).map_err(fail)?)?];
        if before.is_some() {
            let arriving = s.legs[(run.start + m - 1) % m].t1;
            lumps = trim(lumps, from, -(arriving + heading).normalize_or_zero())?;
        }
        if after.is_some() {
            let departing = s.legs[run.end % m].t0;
            lumps = trim(lumps, to, (leaving + departing).normalize_or_zero())?;
        }
        parts.extend(lumps);
    }
    fuse(parts)
}

/// Carries profiles along a path. `path` is in `path_plane`'s coordinates and the
/// profiles in `plane`'s. With `follow` the profile turns with the path, staying
/// square to it as it was where they meet; without, it keeps its orientation.
///
/// `spans` limits the sweep to parts of the path, each two fractions of its
/// length (`[[0.1, 0.3], [0.6, 0.7]]`); empty sweeps all of it. Each part is the
/// piece of the whole sweep that lies there: the profile is where it would be
/// had it travelled from where it was drawn. Separate parts are separate lumps.
pub fn sweep(profiles: &[&Profile], plane: &Plane, path: &Chain, path_plane: &Plane, follow: bool, spans: &[[f64; 2]]) -> R<Lumps> {
    let mut parts = Vec::new();
    let mut expected = 0.0;
    for span in sweep_spans(spans)? {
        let s = Setup::new(profiles, plane, path, path_plane, follow, span)?;
        for p in profiles {
            let mut body = pipe(&s, &p.path, plane)?;
            for hole in &p.hole_paths {
                let tool = pipe(&s, hole, plane)?;
                body = boolean_impl(&body, &tool, Bool::Subtract, false)?;
            }
            parts.extend(body);
            expected += s.expected(p, plane);
        }
    }
    let out = fuse(parts)?;
    let v = volume(&out);
    if !v.is_finite() || (v - expected).abs() > 0.01 * expected.abs() + 1e-9 {
        return Err(format!("the kernel returned a sweep with the wrong volume ({v:.4} where {expected:.4} was expected); the profile may cross itself along this path"));
    }
    let (mesh, _) = tessellate(&out)?;
    let tolerance = 2.0 * FINE.deflection_linear * out.iter().map(Solid::area).sum::<f64>() + 1e-6 * v.max(1.0);
    if !mesh.volume().is_finite() || (mesh.volume() - v).abs() > tolerance {
        return Err("the kernel could not make a reliable closed surface for that sweep; try a simpler path or profile".into());
    }
    Ok(out)
}

/// Tags for the result of [`sweep`] with the same arguments. A side face is named
/// by the profile entity and the path entity that made it; along a path of one
/// piece that is the plain `Swept` tag an extrude gives. When only parts of the
/// path are swept, each name also carries the number of its part, counted from
/// the start of the path, so two parts on one path entity stay distinct.
#[allow(clippy::too_many_arguments)]
pub fn tag_swept(lumps: &[Solid], profiles: &[&Profile], plane: &Plane, path: &Chain, path_plane: &Plane, follow: bool, spans: &[[f64; 2]], feature: Id) -> Tags {
    let plain = || lumps.iter().map(|s| s.iter_face().map(|f| Some(surface_tag(f, feature))).collect()).collect();
    let Ok(spans) = sweep_spans(spans) else { return plain() };
    let partial = spans.iter().any(Option::is_some);
    // A point on each side face: the middle of a profile segment, carried to the middle of a path piece.
    // With it, the profile entity, the path entity and the part of the path.
    let mut probes: Vec<(DVec3, Id, Id, usize)> = Vec::new();
    // The flat ends of each open part: which end, a point on each profile's outline there, the
    // normal and the part. The outline is used because a profile's centre can lie in a hole.
    let mut caps: Vec<(bool, Vec<DVec3>, DVec3, usize)> = Vec::new();
    let mut single = false;
    let outline: Vec<DVec3> = profiles.iter().filter_map(|p| p.path.first()).map(|seg| probe(seg, plane, 0.0)).collect();
    if outline.is_empty() { return plain(); }
    for (part, span) in spans.into_iter().enumerate() {
        let Ok(s) = Setup::new(profiles, plane, path, path_plane, follow, span) else { return plain() };
        single = !partial && s.legs.len() == 1;
        for leg in &s.legs {
            let k = leg.pts.len() / 2;
            let (guess, along) = if leg.pts.len() == 2 { ((leg.pts[0] + leg.pts[1]) / 2.0, leg.pts[1] - leg.pts[0]) } else { (leg.pts[k], leg.pts[k + 1 - usize::from(k + 1 == leg.pts.len())] - leg.pts[k - 1]) };
            let (mid, tangent) = leg.edge.project(c(guess));
            let (mid, tangent) = (g(mid), facing(g(tangent), along));
            for (seg, entity) in segments(profiles) {
                if entity != 0 { probes.push((s.carry(probe(seg, plane, 0.0), mid, tangent), entity, leg.id, part)); }
            }
        }
        if !s.closed {
            for (end, (to, along)) in [(false, s.start()), (true, s.end())] {
                caps.push((end, outline.iter().map(|p| s.carry(*p, to, along)).collect(), s.rotation(along) * s.normal, part));
            }
        }
    }
    // Per face: the profile entities that made it, and for a cap which part it closes.
    let mut sources: Vec<Vec<Vec<Id>>> = Vec::new();
    let mut closes: Vec<Vec<Option<usize>>> = Vec::new();
    let mut tags: Tags = lumps.iter().map(|lump| {
        let (mut made, mut ends) = (Vec::new(), Vec::new());
        let row = lump.iter_face().map(|f| {
            let kind = kind_of(f);
            if kind == Kind::Plane && let Some(surface) = f.surface() {
                // Two parts can end on the same plane (a part that stops and one that starts on one straight
                // piece do not, but parts of a ring can); the cap belongs to the end whose point lies on the face.
                let on_face = |point: &DVec3| g(f.project(c(*point)).0).distance(*point) < 1e-5;
                let flat = |normal: &DVec3, point: &DVec3| g(surface.axis_z).dot(*normal).abs() > 1.0 - 1e-8 && (g(surface.origin) - *point).dot(*normal).abs() < 1e-6;
                if let Some((end, _, _, part)) = caps.iter().find(|(_, points, normal, _)| flat(normal, &points[0]) && (!partial || points.iter().any(on_face))) {
                    made.push(Vec::new());
                    ends.push(Some(*part));
                    return Some(Tag::new(Origin::Cap { feature, end: *end }, kind));
                }
            }
            let hits: Vec<&(DVec3, Id, Id, usize)> = probes.iter().filter(|(p, ..)| g(f.project(c(*p)).0).distance(*p) < 1e-5).collect();
            let mut source: Vec<Tag> = hits.iter().map(|(_, entity, leg, part)| {
                let origin = if single { Origin::Swept { feature, entity: *entity } }
                    else if partial { Origin::Semantic { feature, role: format!("sweep:{entity}:{leg}:{part}") } }
                    else { Origin::Semantic { feature, role: format!("sweep:{entity}:{leg}") } };
                Tag::new(origin, kind)
            }).collect();
            source.sort();
            source.dedup();
            let mut entities: Vec<Id> = hits.iter().map(|(_, entity, ..)| *entity).collect();
            entities.sort();
            entities.dedup();
            made.push(entities);
            ends.push(None);
            Some(match source.len() { 0 => surface_tag(f, feature), 1 => source.pop().unwrap(), _ => Tag::new(Origin::Merged { sources: source }, kind) })
        }).collect();
        sources.push(made);
        closes.push(ends);
        row
    }).collect();
    // A cap is named by the profile entities around it, so two profiles' caps differ.
    for (li, lump) in lumps.iter().enumerate() {
        let mut edges = std::collections::HashMap::new();
        for (fi, f) in lump.iter_face().enumerate() { for e in f.iter_edge() { edges.entry(edge_key(e)).or_insert_with(Vec::new).push(fi); } }
        for (fi, f) in lump.iter_face().enumerate() {
            let Some(Tag { origin: Origin::Cap { end, .. }, .. }) = tags[li][fi].clone() else { continue };
            let mut entities: Vec<Id> = f.iter_edge().flat_map(|e| edges.get(&edge_key(e)).into_iter().flatten()).filter(|&&i| i != fi).flat_map(|&i| sources[li][i].iter().copied()).collect();
            entities.sort();
            entities.dedup();
            if entities.is_empty() { continue; }
            tags[li][fi] = Some(match closes[li][fi] {
                // Several parts each have a start and an end cap around the same entities: the part tells them apart.
                Some(part) if partial => {
                    let around: Vec<String> = entities.iter().map(|e| e.to_string()).collect();
                    Tag::new(Origin::Semantic { feature, role: format!("sweep:cap:{part}:{}:{}", if end { "end" } else { "start" }, around.join(",")) }, Kind::Plane)
                }
                _ => Tag::new(Origin::ProfileCap { feature, end, entities }, Kind::Plane),
            });
        }
    }
    tags
}
