//! Finds the closed regions of a sketch that can be extruded or revolved.
//!
//! Lines and arcs that meet at their ends are traced into faces, so a
//! rectangle split by a line gives two regions. Lines are also split where a
//! sketch point lies along them and where they cross another line, so a
//! rectangle drawn against another's edge, or a line drawn across a shape,
//! bounds regions the way it looks. A circle, arc or ellipse is cut where other
//! geometry ends at a sketch point on it, so a shape drawn up to a curve closes
//! against it. Splines split only at their ends.

use std::collections::BTreeMap;

use glam::DVec2;

use crate::sketch::{CKind, Geom, Id, Ref, Sketch};

#[derive(Clone, Debug)]
pub struct Profile {
    /// Counter-clockwise boundary.
    pub outer: Vec<DVec2>,
    /// Clockwise boundaries of the regions nested directly inside.
    pub holes: Vec<Vec<DVec2>>,
    /// Entities on the outer boundary, sorted; identifies the profile when the sketch changes size.
    pub edges: Vec<Id>,
    /// How many other profiles this one is nested inside.
    pub depth: usize,
    /// The outer boundary as exact lines and arcs, end to end, for the solid kernel.
    pub path: Vec<Seg>,
    /// The hole boundaries likewise.
    pub hole_paths: Vec<Vec<Seg>>,
    /// The sketch entity each segment of `path` came from, aligned with it.
    pub path_ids: Vec<Id>,
    /// Likewise for each hole path.
    pub hole_path_ids: Vec<Vec<Id>>,
}

/// One exact piece of a profile's boundary, in sketch coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line(DVec2, DVec2),
    /// Start, a point on the arc, and end.
    Arc(DVec2, DVec2, DVec2),
    /// Centre and radius.
    Circle(DVec2, f64),
    Spline([DVec2; 4]),
    /// Centre and the two semi-axis vectors, the second a quarter turn anticlockwise from the first.
    Ellipse(DVec2, DVec2, DVec2),
    /// Part of that ellipse, `centre + u cos t + v sin t`, from the first angle to the second:
    /// anticlockwise when the second is the greater.
    EllipseArc(DVec2, DVec2, DVec2, f64, f64),
}

impl Seg {
    pub(crate) fn reversed(self) -> Seg {
        match self {
            Seg::Line(a, b) => Seg::Line(b, a),
            Seg::Arc(a, m, b) => Seg::Arc(b, m, a),
            Seg::Spline([a,b,c,d]) => Seg::Spline([d,c,b,a]),
            Seg::EllipseArc(c, u, v, t0, t1) => Seg::EllipseArc(c, u, v, t1, t0),
            c => c,
        }
    }

    /// Where the piece starts and ends; a circle or ellipse gives its centre twice.
    pub fn ends(&self) -> (DVec2, DVec2) {
        match *self {
            Seg::Line(a, b) | Seg::Arc(a, _, b) => (a, b),
            Seg::Spline(p) => (p[0], p[3]),
            Seg::Circle(c, _) | Seg::Ellipse(c, ..) => (c, c),
            Seg::EllipseArc(c, u, v, t0, t1) => (c + u * t0.cos() + v * t0.sin(), c + u * t1.cos() + v * t1.sin()),
        }
    }
}

/// A run of sketch entities joined end to end: the path of a sweep.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    /// The pieces in order, each starting where the one before ended.
    pub segs: Vec<Seg>,
    /// The sketch entity each piece came from, aligned with `segs`.
    pub ids: Vec<Id>,
    /// The last piece ends where the first began (or the path is one circle).
    pub closed: bool,
}

/// Orders sketch entities into one path. `pick` names the entities to use, in
/// any order; empty means every non-construction entity of the sketch. An open
/// path starts at the free end of the first entity named (or of the lowest id).
pub fn chain(sk: &Sketch, pick: &[Id]) -> Result<Chain, String> {
    let mut wanted: Vec<Id> = if pick.is_empty() {
        sk.entities.iter().filter(|(_, e)| !e.construction).map(|(id, _)| *id).collect()
    } else {
        pick.to_vec()
    };
    let before = wanted.len();
    let mut seen = std::collections::BTreeSet::new();
    wanted.retain(|id| seen.insert(*id));
    if wanted.len() != before { return Err("the path names an entity twice".into()); }
    if wanted.is_empty() { return Err("the path sketch has no lines, arcs or splines to follow".into()); }
    let nodes = point_nodes(sk);
    struct Piece { id: Id, a: Id, b: Id, seg: Seg }
    let mut pieces = Vec::new();
    for id in &wanted {
        let e = sk.entities.get(id).ok_or(format!("the path's entity {id} is no longer in its sketch"))?;
        let (a, b, seg) = match e.geom {
            Geom::Circle { c, r } => {
                if wanted.len() > 1 { return Err("a circle is a whole path by itself; it cannot be joined to other entities".into()); }
                if r < 1e-9 { return Err("the path circle has no radius".into()); }
                return Ok(Chain { segs: vec![Seg::Circle(sk.pos(c), r)], ids: vec![*id], closed: true });
            }
            Geom::Ellipse { .. } => {
                if wanted.len() > 1 { return Err("an ellipse is a whole path by itself; it cannot be joined to other entities".into()); }
                let (c, u, v) = sk.ellipse(*id).ok_or("the path ellipse has no size")?;
                return Ok(Chain { segs: vec![Seg::Ellipse(c, u, v)], ids: vec![*id], closed: true });
            }
            Geom::Line { a, b } => (a, b, Seg::Line(sk.pos(a), sk.pos(b))),
            Geom::Arc { s, e, .. } => { let pts = sk.polyline(*id); (s, e, Seg::Arc(sk.pos(s), pts[pts.len() / 2], sk.pos(e))) }
            Geom::Spline { a, b, c, d } => (a, d, Seg::Spline([a, b, c, d].map(|p| sk.pos(p)))),
        };
        let (a, b) = (nodes[&a], nodes[&b]);
        if a == b { return Err(format!("the path's entity {id} starts and ends at the same point")); }
        pieces.push(Piece { id: *id, a, b, seg });
    }
    let mut degree: BTreeMap<Id, usize> = BTreeMap::new();
    for p in &pieces { *degree.entry(p.a).or_insert(0) += 1; *degree.entry(p.b).or_insert(0) += 1; }
    if degree.values().any(|d| *d > 2) { return Err("the path branches; pick one run of entities joined end to end".into()); }
    let free: Vec<Id> = degree.iter().filter(|(_, d)| **d == 1).map(|(n, _)| *n).collect();
    let closed = free.is_empty();
    // Start at a free end: the first named entity's if it has one.
    let mut at = if closed { pieces[0].a } else {
        let first = &pieces[0];
        if free.contains(&first.a) { first.a } else if free.contains(&first.b) { first.b } else { free[0] }
    };
    let mut used = vec![false; pieces.len()];
    let (mut segs, mut ids) = (Vec::new(), Vec::new());
    // A closed path walks its first entity forwards.
    let mut next = if closed { Some(0) } else { pieces.iter().position(|p| p.a == at || p.b == at) };
    while let Some(i) = next {
        used[i] = true;
        let p = &pieces[i];
        let forward = p.a == at;
        segs.push(if forward { p.seg } else { p.seg.reversed() });
        ids.push(p.id);
        at = if forward { p.b } else { p.a };
        next = pieces.iter().enumerate().position(|(j, q)| !used[j] && (q.a == at || q.b == at));
    }
    if used.iter().any(|u| !u) { return Err("the path is in separate pieces; pick one run of entities joined end to end".into()); }
    // Neighbours share a solved node, but their own end points can differ by the
    // solver's tolerance. Make each piece start exactly where the last one ended.
    for i in 1..segs.len() {
        let join = segs[i - 1].ends().1;
        match &mut segs[i] { Seg::Line(a, _) | Seg::Arc(a, _, _) => *a = join, Seg::Spline(p) => p[0] = join, Seg::Circle(..) | Seg::Ellipse(..) | Seg::EllipseArc(..) => {} }
    }
    if closed {
        let join = segs[0].ends().0;
        match segs.last_mut().unwrap() { Seg::Line(_, b) | Seg::Arc(_, _, b) => *b = join, Seg::Spline(p) => p[3] = join, Seg::Circle(..) | Seg::Ellipse(..) | Seg::EllipseArc(..) => {} }
    }
    Ok(Chain { segs, ids, closed })
}

pub fn signed_area(p: &[DVec2]) -> f64 {
    (0..p.len()).map(|i| p[i].perp_dot(p[(i + 1) % p.len()])).sum::<f64>() / 2.0
}

pub fn inside(poly: &[DVec2], p: DVec2) -> bool {
    let mut odd = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
    }
    odd
}

/// Moves a path sideways by `d`, to the right of its direction of travel,
/// joining neighbouring sides where their offsets meet.
pub fn offset_path(p: &[DVec2], d: f64, closed: bool) -> Vec<DVec2> {
    let n = p.len();
    let normal = |a: DVec2, b: DVec2| -(b - a).normalize_or_zero().perp();
    (0..n)
        .map(|i| {
            let before = (closed || i > 0).then(|| normal(p[(i + n - 1) % n], p[i]));
            let after = (closed || i + 1 < n).then(|| normal(p[i], p[(i + 1) % n]));
            match (before, after) {
                (Some(a), Some(b)) => {
                    // The mitre runs along the bisector, stretched so both sides sit `d` away.
                    let m = (a + b).normalize_or(a);
                    p[i] + m * (d / m.dot(a).max(0.2))
                }
                (Some(a), None) | (None, Some(a)) => p[i] + a * d,
                (None, None) => p[i],
            }
        })
        .collect()
}

impl Profile {
    pub fn contains(&self, p: DVec2) -> bool {
        inside(&self.outer, p) && !self.holes.iter().any(|h| inside(h, p))
    }

    /// Area of the region, holes excluded.
    pub fn area(&self) -> f64 {
        signed_area(&self.outer) + self.holes.iter().map(|h| signed_area(h)).sum::<f64>()
    }

    /// A point inside the outer boundary, for labels.
    pub fn centroid(&self) -> DVec2 {
        self.outer.iter().sum::<DVec2>() / self.outer.len().max(1) as f64
    }
}

fn find(uf: &mut [usize], mut i: usize) -> usize {
    while uf[i] != i {
        uf[i] = uf[uf[i]];
        i = uf[i];
    }
    i
}

struct Face {
    poly: Vec<DVec2>,
    edges: Vec<Id>,
    comp: usize,
    path: Vec<Seg>,
    path_ids: Vec<Id>,
    /// The points at which this face's pieces of cut curves start, going anticlockwise round each curve.
    marks: Vec<Id>,
}

pub(crate) fn point_nodes(sk: &Sketch) -> BTreeMap<Id, Id> {
    // Points that coincide act as one node.
    let ids: Vec<Id> = sk.points.keys().copied().collect();
    let index: BTreeMap<Id, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut uf: Vec<usize> = (0..ids.len()).collect();
    for c in sk.constraints.values() {
        if c.kind == CKind::Coincident && sk.ref_kind(c.refs[1]) == Some(Ref::Point) {
            let (a, b) = (find(&mut uf, index[&c.refs[0]]), find(&mut uf, index[&c.refs[1]]));
            uf[a] = b;
        }
    }
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            if sk.points[&ids[i]].distance(sk.points[&ids[j]]) < 1e-6 {
                let (a, b) = (find(&mut uf, i), find(&mut uf, j));
                uf[a] = b;
            }
        }
    }
    ids.iter().map(|id| (*id, ids[find(&mut uf, index[id])])).collect()

}

pub fn profiles(sk: &Sketch) -> Vec<Profile> {
    let ids: Vec<Id> = sk.points.keys().copied().collect();
    let index: BTreeMap<Id, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let nodes = point_nodes(sk);
    let node = |id: Id| index[&nodes[&id]];

    #[derive(Clone)]
    struct Edge {
        /// The entity the piece is named by, and every entity it lies on.
        id: Id,
        ids: Vec<Id>,
        a: usize,
        b: usize,
        pts: Vec<DVec2>,
        seg: Seg,
        /// A piece of a circle, arc or ellipse that was cut, running anticlockwise from `a`.
        cut: bool,
    }
    // Edges that lead nowhere cannot bound a region.
    fn prune(edges: &mut Vec<Edge>) {
        loop {
            let mut deg = BTreeMap::new();
            for e in edges.iter() {
                *deg.entry(e.a).or_insert(0) += 1;
                *deg.entry(e.b).or_insert(0) += 1;
            }
            let before = edges.len();
            edges.retain(|e| deg[&e.a] > 1 && deg[&e.b] > 1);
            if edges.len() == before {
                break;
            }
        }
    }
    let mut edges: Vec<Edge> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    // Each closed curve is a face and a component of its own.
    let mut hulls: Vec<(usize, Vec<DVec2>, Vec<Seg>, Vec<Id>)> = Vec::new();
    // Node positions: the sketch's points first, then where lines cross each other.
    let mut pos: Vec<DVec2> = ids.iter().map(|id| sk.points[id]).collect();
    let representative: Vec<usize> = (0..ids.len()).filter(|n| index[&nodes[&ids[*n]]] == *n).collect();
    const NEAR: f64 = 1e-6;

    // Lines: split where a point lies along them and where they cross another line, then
    // pieces that lie on two entities (a square's side drawn along a rectangle's edge) are one piece.
    let lines: Vec<(Id, usize, usize)> = sk.entities.iter().filter(|(_, e)| !e.construction).filter_map(|(id, e)| {
        let Geom::Line { a, b } = e.geom else { return None };
        let (a, b) = (node(a), node(b));
        (a != b && pos[a].distance(pos[b]) > NEAR).then_some((*id, a, b))
    }).collect();
    let mut cuts: Vec<Vec<(f64, usize)>> = vec![Vec::new(); lines.len()];
    for (i, &(_, a, b)) in lines.iter().enumerate() {
        let (pa, pb) = (pos[a], pos[b]);
        let d = pb - pa;
        let len2 = d.length_squared();
        for &n in &representative {
            if n == a || n == b { continue; }
            let t = (pos[n] - pa).dot(d) / len2;
            if t > 1e-9 && t < 1.0 - 1e-9 && (pa + d * t).distance(pos[n]) < NEAR {
                cuts[i].push((t, n));
            }
        }
    }
    for i in 0..lines.len() {
        for j in i + 1..lines.len() {
            let (pa, pb) = (pos[lines[i].1], pos[lines[i].2]);
            let (qa, qb) = (pos[lines[j].1], pos[lines[j].2]);
            let (d, e) = (pb - pa, qb - qa);
            let cross = d.perp_dot(e);
            if cross.abs() < 1e-12 * d.length() * e.length() { continue; }
            let t = (qa - pa).perp_dot(e) / cross;
            let u = (qa - pa).perp_dot(d) / cross;
            if t <= 1e-9 || t >= 1.0 - 1e-9 || u <= 1e-9 || u >= 1.0 - 1e-9 { continue; }
            let p = pa + d * t;
            let n = match pos.iter().position(|q| q.distance(p) < NEAR) { Some(n) => n, None => { pos.push(p); pos.len() - 1 } };
            if n != lines[i].1 && n != lines[i].2 { cuts[i].push((t, n)); }
            if n != lines[j].1 && n != lines[j].2 { cuts[j].push((u, n)); }
        }
    }
    for (i, &(id, a, b)) in lines.iter().enumerate() {
        let mut along = std::mem::take(&mut cuts[i]);
        along.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut run: Vec<usize> = vec![a];
        for (_, n) in along { if !run.contains(&n) && n != b { run.push(n); } }
        run.push(b);
        for w in run.windows(2) {
            let (u, v) = (w[0], w[1]);
            if let Some(same) = edges.iter_mut().find(|e| matches!(e.seg, Seg::Line(..)) && ((e.a == u && e.b == v) || (e.a == v && e.b == u))) {
                if !same.ids.contains(&id) { same.ids.push(id); }
                continue;
            }
            edges.push(Edge { id, ids: vec![id], a: u, b: v, pts: vec![pos[u], pos[v]], seg: Seg::Line(pos[u], pos[v]), cut: false });
        }
    }

    // A circle, arc or ellipse is cut where other geometry ends at a sketch point on it.
    // First every place it could be cut is found; a cut is then kept only where something
    // other than the curve itself still ends there once dangling pieces are gone, so that
    // a stray point on a circle leaves it one whole circle.
    use std::f64::consts::TAU;
    struct Round { id: Id, centre: DVec2, u: DVec2, v: DVec2, t0: f64, sweep: f64, ends: Option<(usize, usize)>, ellipse: bool, cuts: Vec<(f64, usize)> }
    impl Round {
        /// The point an angle `o` on from where the curve starts.
        fn at(&self, o: f64) -> DVec2 { let t = self.t0 + o; self.centre + self.u * t.cos() + self.v * t.sin() }
    }
    let mut rounds: Vec<Round> = Vec::new();
    for (id, e) in sk.entities.iter().filter(|(_, e)| !e.construction) {
        let whole = |centre: DVec2, u: DVec2, v: DVec2, ellipse: bool| Round { id: *id, centre, u, v, t0: 0.0, sweep: TAU, ends: None, ellipse, cuts: Vec::new() };
        let round = match e.geom {
            Geom::Circle { c, r } if r > NEAR => Some(whole(sk.pos(c), DVec2::X * r, DVec2::Y * r, false)),
            Geom::Ellipse { .. } => sk.ellipse(*id).filter(|(_, u, v)| u.length() > NEAR && v.length() > NEAR).map(|(c, u, v)| whole(c, u, v, true)),
            Geom::Arc { c, s, e } if node(s) != node(e) => sk.arc_angles(*id).and_then(|(t0, sweep)| {
                let r = sk.pos(c).distance(sk.pos(s));
                (r > NEAR).then(|| Round { t0, sweep, ends: Some((node(s), node(e))), ..whole(sk.pos(c), DVec2::X * r, DVec2::Y * r, false) })
            }),
            _ => None,
        };
        let Some(mut round) = round else { continue };
        for &n in &representative {
            if round.ends.is_some_and(|(a, b)| n == a || n == b) { continue; }
            let d = pos[n] - round.centre;
            let (x, y) = (d.dot(round.u) / round.u.length_squared(), d.dot(round.v) / round.v.length_squared());
            let k = (x * x + y * y).sqrt();
            // How far the point is from the curve, along its own ray from the centre.
            if k < 1e-9 || (d.length() * (1.0 - 1.0 / k)).abs() > NEAR { continue; }
            let o = (y.atan2(x) - round.t0).rem_euclid(TAU);
            if round.ends.is_some() && !(o > 1e-9 && o < round.sweep - 1e-9) { continue; }
            round.cuts.push((o, n));
        }
        round.cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        rounds.push(round);
    }
    // The pieces of a curve between its cuts: none for a whole circle or ellipse with fewer
    // than two, which stays a closed curve.
    let pieces = |round: &Round| -> Vec<Edge> {
        let stops: Vec<(f64, usize)> = match round.ends {
            Some((a, b)) => std::iter::once((0.0, a)).chain(round.cuts.iter().copied()).chain(std::iter::once((round.sweep, b))).collect(),
            None if round.cuts.len() >= 2 => round.cuts.iter().copied().chain(std::iter::once((round.cuts[0].0 + TAU, round.cuts[0].1))).collect(),
            None => return Vec::new(),
        };
        stops.windows(2).filter(|w| w[0].1 != w[1].1).map(|w| {
            let ((o0, a), (o1, b)) = (w[0], w[1]);
            let n = (((o1 - o0) / TAU * crate::sketch::CIRCLE_SEGS as f64).ceil() as usize).max(2);
            let mut pts: Vec<DVec2> = (0..=n).map(|i| round.at(o0 + (o1 - o0) * i as f64 / n as f64)).collect();
            // Ends are taken from the shared node, so neighbouring pieces meet exactly.
            (pts[0], pts[n]) = (pos[a], pos[b]);
            let seg = if round.ellipse { Seg::EllipseArc(round.centre, round.u, round.v, round.t0 + o0, round.t0 + o1) } else { Seg::Arc(pos[a], round.at((o0 + o1) / 2.0), pos[b]) };
            Edge { id: round.id, ids: vec![round.id], a, b, pts, seg, cut: true }
        }).collect()
    };
    // Splines join the lines for the trial; an uncut arc is one piece from end to end.
    let mut trial = edges.clone();
    for (id, e) in sk.entities.iter().filter(|(_, e)| !e.construction) {
        if let Geom::Spline { a, d, .. } = e.geom && node(a) != node(d) {
            trial.push(Edge { id: *id, ids: vec![*id], a: node(a), b: node(d), pts: Vec::new(), seg: Seg::Line(DVec2::ZERO, DVec2::ZERO), cut: false });
        }
    }
    for round in &rounds { trial.extend(pieces(round)); }
    prune(&mut trial);
    for round in &mut rounds {
        let id = round.id;
        round.cuts.retain(|(_, n)| trial.iter().any(|e| (e.a == *n || e.b == *n) && e.id != id));
    }
    // What is still cut is made of its pieces; the rest is handled whole, as it always was.
    let mut cut: BTreeMap<Id, Vec<Edge>> = rounds.iter().filter(|r| !r.cuts.is_empty() && (r.ends.is_some() || r.cuts.len() >= 2)).map(|r| (r.id, pieces(r))).collect();

    let mut next_comp = pos.len();
    for (id, e) in sk.entities.iter().filter(|(_, e)| !e.construction && !matches!(e.geom, Geom::Line { .. })) {
        if let Some(made) = cut.remove(id) {
            edges.extend(made);
            continue;
        }
        let (a, b) = match e.geom {
            Geom::Arc { s, e, .. } => (node(s), node(e)),
            Geom::Spline { a,d,.. } => (node(a),node(d)),
            _ => (0, 0),
        };
        let mut pts = sk.polyline(*id);
        if pts.len() < 2 { continue; }
        if a != b {
            // Ends are taken from the shared node, so neighbouring pieces meet exactly.
            let seg = match e.geom {
                Geom::Spline { b:fit_b,c:fit_c,.. } => Seg::Spline([sk.points[&ids[a]],sk.pos(fit_b),sk.pos(fit_c),sk.points[&ids[b]]]),
                _ => Seg::Arc(sk.points[&ids[a]], pts[pts.len() / 2], sk.points[&ids[b]]),
            };
            edges.push(Edge { id: *id, ids: vec![*id], a, b, pts, seg, cut: false });
        } else {
            pts.pop();
            if signed_area(&pts).abs() > 1e-9 {
                let seg = match e.geom {
                    Geom::Spline { a,b,c,d } => Seg::Spline([a,b,c,d].map(|p| sk.pos(p))),
                    Geom::Ellipse { .. } => { let Some((c, u, v)) = sk.ellipse(*id) else { continue }; Seg::Ellipse(c, u, v) },
                    _ => { let Some((c,r)) = sk.curve(*id) else { continue }; Seg::Circle(c,r) },
                };
                let path = vec![if signed_area(&pts) < 0.0 { pts.reverse(); seg.reversed() } else { seg }];
                faces.push(Face { poly: pts.clone(), edges: vec![*id], comp: next_comp, path: path.clone(), path_ids: vec![*id], marks: Vec::new() });
                hulls.push((next_comp, pts, path, vec![*id]));
                next_comp += 1;
            }
        }
    }

    prune(&mut edges);

    // Half-edge 2i runs along edge i, 2i+1 runs back.
    let from = |h: usize| if h % 2 == 0 { edges[h / 2].a } else { edges[h / 2].b };
    let dir = |h: usize| {
        let p = &edges[h / 2].pts;
        let d = if h % 2 == 0 { p[1] - p[0] } else { p[p.len() - 2] - p[p.len() - 1] };
        d.y.atan2(d.x)
    };
    let mut out: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for h in 0..edges.len() * 2 {
        out.entry(from(h)).or_default().push(h);
    }
    for list in out.values_mut() {
        list.sort_by(|a, b| dir(*a).total_cmp(&dir(*b)));
    }
    let mut cuf: Vec<usize> = (0..pos.len()).collect();
    for e in &edges {
        let (a, b) = (find(&mut cuf, e.a), find(&mut cuf, e.b));
        cuf[a] = b;
    }
    let mut seen = vec![false; edges.len() * 2];
    for start in 0..edges.len() * 2 {
        if seen[start] {
            continue;
        }
        let (mut h, mut poly, mut used, mut path, mut path_ids, mut marks) = (start, Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        while !seen[h] {
            seen[h] = true;
            let e = &edges[h / 2];
            let n = e.pts.len() - 1;
            if h % 2 == 0 {
                poly.extend_from_slice(&e.pts[..n]);
            } else {
                poly.extend(e.pts[1..].iter().rev());
            }
            used.extend(e.ids.iter().copied());
            if e.cut { marks.push(ids[e.a]); }
            path_ids.push(e.id);
            path.push(if h % 2 == 0 { e.seg } else { e.seg.reversed() });
            // Turn as far left as possible at the far end, keeping the face on the left.
            let twin = h ^ 1;
            let list = &out[&from(twin)];
            let at = list.iter().position(|x| *x == twin).unwrap();
            h = list[(at + list.len() - 1) % list.len()];
        }
        let comp = find(&mut cuf, from(start));
        let area = signed_area(&poly);
        if area > 1e-9 {
            used.sort();
            used.dedup();
            faces.push(Face { poly, edges: used, comp, path, path_ids, marks });
        } else if area < -1e-9 {
            poly.reverse();
            hulls.push((comp, poly, path, path_ids));
        }
    }
    // A region is known by the entities around it. Where two are bounded by exactly the
    // same ones, as the two sides of a chord across a circle are, each also names the
    // points at which its pieces of the cut curves start.
    let plain: Vec<Vec<Id>> = faces.iter().map(|f| f.edges.clone()).collect();
    for (i, face) in faces.iter_mut().enumerate() {
        if plain.iter().enumerate().any(|(j, other)| j != i && *other == plain[i]) {
            face.edges.extend(face.marks.iter().copied());
            face.edges.sort();
            face.edges.dedup();
        }
    }
    // A component sitting inside a face of another component is a hole in it.
    let mut holes: Vec<Vec<Vec<DVec2>>> = vec![Vec::new(); faces.len()];
    let mut parent: Vec<Option<usize>> = vec![None; faces.len()];
    let mut hole_paths: Vec<Vec<Vec<Seg>>> = vec![Vec::new(); faces.len()];
    let mut hole_path_ids: Vec<Vec<Vec<Id>>> = vec![Vec::new(); faces.len()];
    for (comp, hull, path, ids) in &hulls {
        let host = (0..faces.len())
            .filter(|i| faces[*i].comp != *comp && hull.iter().all(|p| inside(&faces[*i].poly, *p) || faces[*i].poly.contains(p)))
            .filter(|i| hull.iter().any(|p| inside(&faces[*i].poly, *p)))
            .min_by(|a, b| signed_area(&faces[*a].poly).total_cmp(&signed_area(&faces[*b].poly)));
        if let Some(host) = host {
            let mut h = hull.clone();
            h.reverse();
            holes[host].push(h);
            hole_paths[host].push(path.clone());
            hole_path_ids[host].push(ids.clone());
            for (i, f) in faces.iter().enumerate() {
                if f.comp == *comp {
                    parent[i] = Some(host);
                }
            }
        }
    }
    (0..faces.len())
        .map(|i| {
            let mut depth = 0;
            let mut at = parent[i];
            while let Some(p) = at {
                depth += 1;
                at = parent[p];
                if depth > faces.len() {
                    break;
                }
            }
            Profile { outer: faces[i].poly.clone(), holes: holes[i].clone(), edges: faces[i].edges.clone(), depth, path: faces[i].path.clone(), hole_paths: hole_paths[i].clone(), path_ids: faces[i].path_ids.clone(), hole_path_ids: hole_path_ids[i].clone() }
        })
        .collect()
}
