//! Finds the closed regions of a sketch that can be extruded or revolved.
//!
//! Lines and arcs that meet at their ends are traced into faces, so a
//! rectangle split by a line gives two regions. Shapes that cross without
//! sharing a point are not split where they cross.

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
}

impl Seg {
    pub(crate) fn reversed(self) -> Seg {
        match self {
            Seg::Line(a, b) => Seg::Line(b, a),
            Seg::Arc(a, m, b) => Seg::Arc(b, m, a),
            Seg::Spline([a,b,c,d]) => Seg::Spline([d,c,b,a]),
            c => c,
        }
    }

    /// Where the piece starts and ends; a circle gives its centre twice.
    pub fn ends(&self) -> (DVec2, DVec2) {
        match *self {
            Seg::Line(a, b) | Seg::Arc(a, _, b) => (a, b),
            Seg::Spline(p) => (p[0], p[3]),
            Seg::Circle(c, _) => (c, c),
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
        match &mut segs[i] { Seg::Line(a, _) | Seg::Arc(a, _, _) => *a = join, Seg::Spline(p) => p[0] = join, Seg::Circle(..) => {} }
    }
    if closed {
        let join = segs[0].ends().0;
        match segs.last_mut().unwrap() { Seg::Line(_, b) | Seg::Arc(_, _, b) => *b = join, Seg::Spline(p) => p[3] = join, Seg::Circle(..) => {} }
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

    struct Edge {
        id: Id,
        a: usize,
        b: usize,
        pts: Vec<DVec2>,
        seg: Seg,
    }
    let mut edges = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    // Each closed curve is a face and a component of its own.
    let mut hulls: Vec<(usize, Vec<DVec2>, Vec<Seg>, Vec<Id>)> = Vec::new();
    let mut next_comp = ids.len();
    for (id, e) in sk.entities.iter().filter(|(_, e)| !e.construction) {
        let (a, b) = match e.geom {
            Geom::Line { a, b } => (node(a), node(b)),
            Geom::Arc { s, e, .. } => (node(s), node(e)),
            Geom::Spline { a,d,.. } => (node(a),node(d)),
            Geom::Circle { .. } => (0, 0),
        };
        let mut pts = sk.polyline(*id);
        if pts.len() < 2 { continue; }
        if a != b {
            // Ends are taken from the shared node, so neighbouring pieces meet exactly.
            let seg = match e.geom {
                Geom::Line { .. } => Seg::Line(sk.points[&ids[a]], sk.points[&ids[b]]),
                Geom::Spline { b:fit_b,c:fit_c,.. } => Seg::Spline([sk.points[&ids[a]],sk.pos(fit_b),sk.pos(fit_c),sk.points[&ids[b]]]),
                _ => Seg::Arc(sk.points[&ids[a]], pts[pts.len() / 2], sk.points[&ids[b]]),
            };
            edges.push(Edge { id: *id, a, b, pts, seg });
        } else if !matches!(e.geom, Geom::Line { .. }) {
            pts.pop();
            if signed_area(&pts).abs() > 1e-9 {
                let seg = match e.geom {
                    Geom::Spline { a,b,c,d } => Seg::Spline([a,b,c,d].map(|p| sk.pos(p))),
                    _ => { let Some((c,r)) = sk.curve(*id) else { continue }; Seg::Circle(c,r) },
                };
                let path = vec![if signed_area(&pts) < 0.0 { pts.reverse(); seg.reversed() } else { seg }];
                faces.push(Face { poly: pts.clone(), edges: vec![*id], comp: next_comp, path: path.clone(), path_ids: vec![*id] });
                hulls.push((next_comp, pts, path, vec![*id]));
                next_comp += 1;
            }
        }
    }

    // Edges that lead nowhere cannot bound a region.
    loop {
        let mut deg = BTreeMap::new();
        for e in &edges {
            *deg.entry(e.a).or_insert(0) += 1;
            *deg.entry(e.b).or_insert(0) += 1;
        }
        let before = edges.len();
        edges.retain(|e| deg[&e.a] > 1 && deg[&e.b] > 1);
        if edges.len() == before {
            break;
        }
    }

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
    let mut cuf: Vec<usize> = (0..ids.len()).collect();
    for e in &edges {
        let (a, b) = (find(&mut cuf, e.a), find(&mut cuf, e.b));
        cuf[a] = b;
    }
    let mut seen = vec![false; edges.len() * 2];
    for start in 0..edges.len() * 2 {
        if seen[start] {
            continue;
        }
        let (mut h, mut poly, mut used, mut path, mut path_ids) = (start, Vec::new(), Vec::new(), Vec::new(), Vec::new());
        while !seen[h] {
            seen[h] = true;
            let e = &edges[h / 2];
            let n = e.pts.len() - 1;
            if h % 2 == 0 {
                poly.extend_from_slice(&e.pts[..n]);
            } else {
                poly.extend(e.pts[1..].iter().rev());
            }
            used.push(e.id);
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
            faces.push(Face { poly, edges: used, comp, path, path_ids });
        } else if area < -1e-9 {
            poly.reverse();
            hulls.push((comp, poly, path, path_ids));
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
