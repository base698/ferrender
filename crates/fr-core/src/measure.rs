//! Measuring between two picked things: points, edges and faces.

use glam::DVec3;

/// Something that can be measured from or to.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Point(DVec3),
    /// An edge or sketch line, as a polyline.
    Path(Vec<DVec3>),
    /// A face, as its triangles.
    Surface(Vec<[DVec3; 3]>),
}

/// What lies between two items.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    /// The shortest distance between them, and the points on each where it is found.
    pub distance: f64,
    pub from: DVec3,
    pub to: DVec3,
    /// How far apart they are square to each other, when they are parallel: two flat
    /// faces, two straight edges, an edge along a face, or a point off a face or edge.
    /// Unlike `distance` it does not depend on where the two happen to end.
    pub apart: Option<f64>,
    /// The angle between two straight or flat items, in degrees from 0 to 90.
    pub angle: Option<f64>,
}

/// A straight item's line or a flat item's plane.
#[derive(Clone, Copy)]
enum Flat {
    Line(DVec3, DVec3),
    Plane(DVec3, DVec3),
}

impl Item {
    /// The direction of a straight path, as a point on it and a unit vector.
    pub fn line(&self) -> Option<(DVec3, DVec3)> {
        let Item::Path(p) = self else { return None };
        let (a, b) = (*p.first()?, *p.last()?);
        let dir = (b - a).try_normalize()?;
        p.iter().all(|q| (*q - a).cross(dir).length() < 1e-6 * (1.0 + a.distance(b))).then_some((a, dir))
    }

    /// The plane of a flat surface, as a point on it and its unit normal.
    pub fn plane(&self) -> Option<(DVec3, DVec3)> {
        let Item::Surface(tris) = self else { return None };
        let normal = |t: &[DVec3; 3]| (t[1] - t[0]).cross(t[2] - t[0]).try_normalize();
        let n = tris.iter().find_map(normal)?;
        let at = tris.first()?[0];
        tris.iter().all(|t| t.iter().all(|v| (*v - at).dot(n).abs() < 1e-6 * (1.0 + at.length()))).then_some((at, n))
    }

    fn flat(&self) -> Option<Flat> {
        self.line().map(|(p, d)| Flat::Line(p, d)).or(self.plane().map(|(p, n)| Flat::Plane(p, n)))
    }

    /// The length of a path, the area of a surface.
    pub fn size(&self) -> Option<f64> {
        match self {
            Item::Point(_) => None,
            Item::Path(p) => Some(p.windows(2).map(|w| w[0].distance(w[1])).sum()),
            Item::Surface(t) => Some(t.iter().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).length() / 2.0).sum()),
        }
    }

    fn bounds(&self) -> (DVec3, DVec3) {
        let pts: Box<dyn Iterator<Item = DVec3> + '_> = match self {
            Item::Point(p) => Box::new(std::iter::once(*p)),
            Item::Path(p) => Box::new(p.iter().copied()),
            Item::Surface(t) => Box::new(t.iter().flatten().copied()),
        };
        pts.fold((DVec3::MAX, DVec3::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)))
    }
}

/// One piece of an item: a point, a segment or a triangle, as its corners.
type Piece = Vec<DVec3>;

fn pieces(item: &Item) -> Vec<Piece> {
    match item {
        Item::Point(p) => vec![vec![*p]],
        Item::Path(p) if p.len() == 1 => vec![vec![p[0]]],
        Item::Path(p) => p.windows(2).map(|w| w.to_vec()).collect(),
        Item::Surface(t) => t.iter().map(|t| t.to_vec()).collect(),
    }
}

fn on_segment(p: DVec3, a: DVec3, b: DVec3) -> DVec3 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

/// The closest points of two segments (Ericson, Real-Time Collision Detection 5.1.9).
fn segments(p1: DVec3, q1: DVec3, p2: DVec3, q2: DVec3) -> (DVec3, DVec3) {
    let (d1, d2, r) = (q1 - p1, q2 - p2, p1 - p2);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    let (s, t);
    if a <= 1e-18 && e <= 1e-18 {
        return (p1, p2);
    }
    if a <= 1e-18 {
        (s, t) = (0.0, (f / e).clamp(0.0, 1.0));
    } else {
        let c = d1.dot(r);
        if e <= 1e-18 {
            (s, t) = ((-c / a).clamp(0.0, 1.0), 0.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let s0 = if denom > 1e-18 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let t0 = (b * s0 + f) / e;
            (s, t) = if t0 < 0.0 {
                ((-c / a).clamp(0.0, 1.0), 0.0)
            } else if t0 > 1.0 {
                (((b - c) / a).clamp(0.0, 1.0), 1.0)
            } else {
                (s0, t0)
            };
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

/// The point of a triangle closest to `p` (Ericson 5.1.5).
fn on_triangle(p: DVec3, t: &[DVec3]) -> DVec3 {
    let (a, b, c) = (t[0], t[1], t[2]);
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Where a segment passes through a triangle, if it does.
fn pierces(a: DVec3, b: DVec3, t: &[DVec3]) -> Option<DVec3> {
    let n = (t[1] - t[0]).cross(t[2] - t[0]);
    let (da, db) = ((a - t[0]).dot(n), (b - t[0]).dot(n));
    if da * db > 0.0 || (da - db).abs() < 1e-18 {
        return None;
    }
    let p = a + (b - a) * (da / (da - db));
    (on_triangle(p, t).distance(p) < 1e-9).then_some(p)
}

fn segment_triangle(a: DVec3, b: DVec3, t: &[DVec3]) -> (DVec3, DVec3) {
    if let Some(p) = pierces(a, b, t) {
        return (p, p);
    }
    let mut pairs = vec![(a, on_triangle(a, t)), (b, on_triangle(b, t))];
    pairs.extend((0..3).map(|i| segments(a, b, t[i], t[(i + 1) % 3])));
    pairs.into_iter().min_by(|x, y| x.0.distance(x.1).total_cmp(&y.0.distance(y.1))).expect("there are five pairs")
}

/// The closest points of two pieces, the first on `a`.
fn closest(a: &Piece, b: &Piece) -> (DVec3, DVec3) {
    let swap = |(x, y): (DVec3, DVec3)| (y, x);
    match (a.len(), b.len()) {
        (1, 1) => (a[0], b[0]),
        (1, 2) => (a[0], on_segment(a[0], b[0], b[1])),
        (1, _) => (a[0], on_triangle(a[0], b)),
        (2, 2) => segments(a[0], a[1], b[0], b[1]),
        (2, 3) => segment_triangle(a[0], a[1], b),
        (3, 3) => (0..3)
            .map(|i| segment_triangle(a[i], a[(i + 1) % 3], b))
            .chain((0..3).map(|i| swap(segment_triangle(b[i], b[(i + 1) % 3], a))))
            .min_by(|x, y| x.0.distance(x.1).total_cmp(&y.0.distance(y.1)))
            .expect("there are six pairs"),
        _ => swap(closest(b, a)),
    }
}

/// The distance, and where they apply the perpendicular gap and angle, between two items.
pub fn between(a: &Item, b: &Item) -> Measure {
    let (pa, pb) = (pieces(a), pieces(b));
    let reach = |p: &Piece| p.iter().fold((DVec3::MAX, DVec3::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let boxes: Vec<(DVec3, DVec3)> = pb.iter().map(reach).collect();
    let mut best = (f64::MAX, DVec3::ZERO, DVec3::ZERO);
    for x in &pa {
        let (lo, hi) = reach(x);
        for (y, (blo, bhi)) in pb.iter().zip(&boxes) {
            // Pieces whose boxes are already further apart than the best cannot beat it.
            let gap = (*blo - hi).max(lo - *bhi).max(DVec3::ZERO).length();
            if gap >= best.0 {
                continue;
            }
            let (p, q) = closest(x, y);
            if p.distance(q) < best.0 {
                best = (p.distance(q), p, q);
            }
        }
    }
    if best.0 == f64::MAX {
        let (p, q) = (a.bounds().0, b.bounds().0);
        best = (p.distance(q), p, q);
    }
    let parallel = |u: DVec3, v: DVec3| u.cross(v).length() < 1e-9;
    let degrees = |cos: f64| cos.abs().clamp(0.0, 1.0).acos().to_degrees();
    let point = |i: &Item| if let Item::Point(p) = i { Some(*p) } else { None };
    let (apart, angle) = match (a.flat(), b.flat(), point(a), point(b)) {
        (Some(Flat::Plane(p, n)), Some(Flat::Plane(q, m)), ..) => (parallel(n, m).then(|| (q - p).dot(n).abs()), Some(degrees(n.dot(m)))),
        (Some(Flat::Line(p, d)), Some(Flat::Line(q, e)), ..) => (parallel(d, e).then(|| (q - p).cross(d).length()), Some(degrees(d.dot(e)))),
        (Some(Flat::Line(p, d)), Some(Flat::Plane(q, n)), ..) | (Some(Flat::Plane(q, n)), Some(Flat::Line(p, d)), ..) => ((d.dot(n).abs() < 1e-9).then(|| (p - q).dot(n).abs()), Some(90.0 - degrees(d.dot(n)))),
        (Some(Flat::Plane(q, n)), None, _, Some(p)) | (None, Some(Flat::Plane(q, n)), Some(p), _) => (Some((p - q).dot(n).abs()), None),
        (Some(Flat::Line(q, d)), None, _, Some(p)) | (None, Some(Flat::Line(q, d)), Some(p), _) => (Some((p - q).cross(d).length()), None),
        _ => (None, None),
    };
    Measure { distance: best.0, from: best.1, to: best.2, apart, angle }
}
