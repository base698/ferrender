//! Sketch constraint solver: damped least squares over point coordinates
//! and circle radii, starting from where the geometry already is so that
//! under-constrained sketches move as little as possible.

use std::collections::{BTreeSet, HashMap};

use glam::DVec2;

use crate::sketch::{CKind, Geom, Id, ORIGIN, Ref, Sketch};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Every constraint is satisfied.
    pub ok: bool,
    /// Degrees of freedom left; zero means fully constrained.
    pub dof: usize,
    /// Constraints that could not be satisfied.
    pub bad: Vec<Id>,
}

const TOL: f64 = 1e-7;

struct Sys<'a> {
    sk: &'a Sketch,
    pidx: HashMap<Id, usize>,
    ridx: HashMap<Id, usize>,
    drags: &'a [(Id, DVec2)],
    /// A typical length in the sketch, fixed for the whole solve.
    size: f64,
}

impl Sys<'_> {
    fn p(&self, x: &[f64], id: Id) -> DVec2 {
        match self.pidx.get(&id) {
            Some(i) => DVec2::new(x[*i], x[*i + 1]),
            None => self.sk.pos(id),
        }
    }

    fn line(&self, x: &[f64], id: Id) -> (DVec2, DVec2) {
        match self.sk.entities[&id].geom {
            Geom::Line { a, b } => (self.p(x, a), self.p(x, b)),
            _ => unreachable!("constraint refs are validated when added"),
        }
    }

    fn curve(&self, x: &[f64], id: Id) -> (DVec2, f64) {
        match self.sk.entities[&id].geom {
            Geom::Circle { c, r } => (self.p(x, c), self.ridx.get(&id).map_or(r, |i| x[*i])),
            Geom::Arc { c, s, .. } => (self.p(x, c), self.p(x, c).distance(self.p(x, s))),
            Geom::Line { .. } => unreachable!("constraint refs are validated when added"),
        }
    }

    /// Appends one residual per equation; `owners` records which constraint each came from.
    fn residuals(&self, x: &[f64], drag_weight: f64, out: &mut Vec<f64>, mut owners: Option<&mut Vec<Id>>) {
        out.clear();
        let sk = self.sk;
        let len = |d: DVec2| d.length().max(1e-12);
        let perp = |p: DVec2, l: (DVec2, DVec2)| (l.1 - l.0).perp_dot(p - l.0) / len(l.1 - l.0);
        for (cid, c) in &sk.constraints {
            let start = out.len();
            let r = &c.refs;
            let kinds: Vec<Ref> = r.iter().filter_map(|i| sk.ref_kind(*i)).collect();
            let v = c.value.as_ref().map_or(0.0, |v| v.v);
            match (c.kind, kinds.as_slice()) {
                (CKind::Coincident, [Ref::Point, Ref::Point]) => {
                    let d = self.p(x, r[0]) - self.p(x, r[1]);
                    out.extend([d.x, d.y]);
                }
                (CKind::Coincident, [Ref::Point, Ref::Line]) => out.push(perp(self.p(x, r[0]), self.line(x, r[1]))),
                (CKind::Coincident, [Ref::Point, Ref::Curve]) => {
                    let (c, rad) = self.curve(x, r[1]);
                    out.push(self.p(x, r[0]).distance(c) - rad);
                }
                (CKind::Horizontal | CKind::Vertical, _) => {
                    let (a, b) = if r.len() == 1 { self.line(x, r[0]) } else { (self.p(x, r[0]), self.p(x, r[1])) };
                    out.push(if c.kind == CKind::Horizontal { a.y - b.y } else { a.x - b.x });
                }
                // Angles are measured between directions, so that shortening a line
                // cannot pass for turning it, then scaled to weigh like distances.
                (CKind::Parallel, _) => {
                    let (a, b) = (self.line(x, r[0]), self.line(x, r[1]));
                    out.push((a.1 - a.0).perp_dot(b.1 - b.0) / (len(a.1 - a.0) * len(b.1 - b.0)) * self.size);
                }
                (CKind::Perpendicular, _) => {
                    let (a, b) = (self.line(x, r[0]), self.line(x, r[1]));
                    out.push((a.1 - a.0).dot(b.1 - b.0) / (len(a.1 - a.0) * len(b.1 - b.0)) * self.size);
                }
                (CKind::Tangent, [Ref::Line, Ref::Curve]) => {
                    let (c, rad) = self.curve(x, r[1]);
                    out.push(perp(c, self.line(x, r[0])).abs() - rad);
                }
                (CKind::Tangent, _) => {
                    let ((c1, r1), (c2, r2)) = (self.curve(x, r[0]), self.curve(x, r[1]));
                    let d = c1.distance(c2);
                    // Touching from outside or from inside, whichever it is nearer to now.
                    out.push(if d > r1.max(r2) { d - (r1 + r2) } else { d - (r1 - r2).abs() });
                }
                (CKind::Equal, [Ref::Line, Ref::Line]) => {
                    let (a, b) = (self.line(x, r[0]), self.line(x, r[1]));
                    out.push(a.0.distance(a.1) - b.0.distance(b.1));
                }
                (CKind::Equal, _) => out.push(self.curve(x, r[0]).1 - self.curve(x, r[1]).1),
                (CKind::Midpoint, _) => {
                    let l = self.line(x, r[1]);
                    let d = self.p(x, r[0]) - (l.0 + l.1) / 2.0;
                    out.extend([d.x, d.y]);
                }
                (CKind::Concentric, _) => {
                    let d = self.curve(x, r[0]).0 - self.curve(x, r[1]).0;
                    out.extend([d.x, d.y]);
                }
                (CKind::Collinear, _) => {
                    let (a, b) = (self.line(x, r[0]), self.line(x, r[1]));
                    out.extend([perp(b.0, a), perp(b.1, a)]);
                }
                (CKind::Symmetric, _) => {
                    let (p, q, l) = (self.p(x, r[0]), self.p(x, r[1]), self.line(x, r[2]));
                    out.extend([perp((p + q) / 2.0, l), (q - p).dot(l.1 - l.0) / len(l.1 - l.0)]);
                }
                (CKind::Distance, [Ref::Line]) => {
                    let l = self.line(x, r[0]);
                    out.push(l.0.distance(l.1) - v);
                }
                (CKind::Distance, [Ref::Point, Ref::Point]) => out.push(self.p(x, r[0]).distance(self.p(x, r[1])) - v),
                (CKind::Distance, [Ref::Point, Ref::Line]) => out.push(perp(self.p(x, r[0]), self.line(x, r[1])).abs() - v),
                (CKind::Distance, _) => out.push(perp(self.line(x, r[1]).0, self.line(x, r[0])).abs() - v),
                (CKind::Radius, _) => out.push(self.curve(x, r[0]).1 - v),
                (CKind::Diameter, _) => out.push(self.curve(x, r[0]).1 * 2.0 - v),
                (CKind::Angle, _) => {
                    let (a, b) = (self.line(x, r[0]), self.line(x, r[1]));
                    let (d1, d2) = (a.1 - a.0, b.1 - b.0);
                    out.push((d1.perp_dot(d2).abs().atan2(d1.dot(d2)) - v.to_radians()) * self.size);
                }
                // Fixed points are simply not variables.
                (CKind::Fix, _) | (CKind::Coincident, _) => {}
            }
            if let Some(o) = owners.as_deref_mut() {
                o.extend(std::iter::repeat_n(*cid, out.len() - start));
            }
        }
        for (eid, e) in &sk.entities {
            if let Geom::Arc { c, s, e } = e.geom {
                let c = self.p(x, c);
                out.push(c.distance(self.p(x, s)) - c.distance(self.p(x, e)));
                if let Some(o) = owners.as_deref_mut() {
                    o.push(*eid);
                }
            }
            if let Some(i) = self.ridx.get(eid) {
                // Keeps radii from collapsing through zero.
                out.push((1e-6 - x[*i]).max(0.0) * 1e3);
                if let Some(o) = owners.as_deref_mut() {
                    o.push(*eid);
                }
            }
        }
        if drag_weight > 0.0 {
            for (id, target) in self.drags {
                if let Some(i) = self.ridx.get(id) {
                    out.push((x[*i] - target.x) * drag_weight);
                } else if self.pidx.contains_key(id) {
                    let d = (self.p(x, *id) - *target) * drag_weight;
                    out.extend([d.x, d.y]);
                }
            }
        }
    }

    fn jacobian(&self, x: &mut [f64], drag_weight: f64, r0: &[f64]) -> Vec<Vec<f64>> {
        let mut cols = Vec::with_capacity(x.len());
        let mut r = Vec::with_capacity(r0.len());
        for j in 0..x.len() {
            let (old, h) = (x[j], 1e-7 * x[j].abs().max(1.0));
            x[j] = old + h;
            self.residuals(x, drag_weight, &mut r, None);
            x[j] = old;
            cols.push(r.iter().zip(r0).map(|(a, b)| (a - b) / h).collect());
        }
        cols
    }

    /// Levenberg-Marquardt from `x`; leaves the best point found in `x`.
    fn minimize(&self, x: &mut Vec<f64>, drag_weight: f64) {
        let n = x.len();
        let mut r = Vec::new();
        self.residuals(x, drag_weight, &mut r, None);
        let mut cost: f64 = r.iter().map(|v| v * v).sum();
        let mut lambda = 1e-4;
        for _ in 0..60 {
            if cost < 1e-22 || n == 0 {
                break;
            }
            let j = self.jacobian(x, drag_weight, &r);
            let mut a = vec![0.0; n * n];
            let mut g = vec![0.0; n];
            for c in 0..n {
                g[c] = j[c].iter().zip(&r).map(|(a, b)| a * b).sum();
                for d in c..n {
                    let s: f64 = j[c].iter().zip(&j[d]).map(|(a, b)| a * b).sum();
                    a[c * n + d] = s;
                    a[d * n + c] = s;
                }
            }
            let scale = (0..n).map(|i| a[i * n + i]).fold(0.0, f64::max).max(1e-12);
            let mut stepped = false;
            for _ in 0..12 {
                let mut m = a.clone();
                for i in 0..n {
                    // The same damping on every variable, so that a free sketch takes the
                    // shortest move that satisfies it instead of sliding where it is loose.
                    m[i * n + i] += lambda * scale;
                }
                let mut d: Vec<f64> = g.iter().map(|v| -v).collect();
                if !solve_linear(&mut m, &mut d, n) {
                    lambda *= 10.0;
                    continue;
                }
                let xn: Vec<f64> = x.iter().zip(&d).map(|(a, b)| a + b).collect();
                let mut rn = Vec::new();
                self.residuals(&xn, drag_weight, &mut rn, None);
                let cn: f64 = rn.iter().map(|v| v * v).sum();
                if cn < cost {
                    let gain = cost - cn;
                    *x = xn;
                    r = rn;
                    cost = cn;
                    lambda = (lambda / 5.0).max(1e-12);
                    stepped = gain > 1e-24;
                    break;
                }
                lambda *= 5.0;
            }
            if !stepped {
                break;
            }
        }
    }
}

/// Gaussian elimination with partial pivoting; `b` becomes the solution.
fn solve_linear(a: &mut [f64], b: &mut [f64], n: usize) -> bool {
    for c in 0..n {
        let piv = (c..n).max_by(|i, j| a[i * n + c].abs().total_cmp(&a[j * n + c].abs())).unwrap();
        if a[piv * n + c].abs() < 1e-300 {
            return false;
        }
        if piv != c {
            for k in 0..n {
                a.swap(c * n + k, piv * n + k);
            }
            b.swap(c, piv);
        }
        for r in c + 1..n {
            let f = a[r * n + c] / a[c * n + c];
            if f != 0.0 {
                for k in c..n {
                    a[r * n + k] -= f * a[c * n + k];
                }
                b[r] -= f * b[c];
            }
        }
    }
    for c in (0..n).rev() {
        let s: f64 = (c + 1..n).map(|k| a[c * n + k] * b[k]).sum();
        b[c] = (b[c] - s) / a[c * n + c];
    }
    b.iter().all(|v| v.is_finite())
}

/// Rank of the matrix whose columns are `cols`.
fn rank(mut cols: Vec<Vec<f64>>) -> usize {
    let m = cols.first().map_or(0, Vec::len);
    let mut rank = 0;
    for row in 0..m {
        if rank == cols.len() {
            break;
        }
        let piv = (rank..cols.len()).max_by(|a, b| cols[*a][row].abs().total_cmp(&cols[*b][row].abs())).unwrap();
        if cols[piv][row].abs() < 1e-6 {
            continue;
        }
        cols.swap(rank, piv);
        let (head, tail) = cols.split_at_mut(rank + 1);
        let p = &head[rank];
        for c in tail {
            let f = c[row] / p[row];
            if f != 0.0 {
                for k in row..m {
                    c[k] -= f * p[k];
                }
            }
        }
        rank += 1;
    }
    rank
}

/// Moves the sketch to satisfy its constraints. `drags` pulls points toward
/// positions (or, for a circle id, its radius toward `x`) as far as the
/// constraints allow.
pub fn solve(sk: &mut Sketch, drags: &[(Id, DVec2)]) -> Report {
    let mut fixed: BTreeSet<Id> = sk.fixed.iter().copied().chain([ORIGIN]).collect();
    for c in sk.constraints.values().filter(|c| c.kind == CKind::Fix) {
        for r in &c.refs {
            fixed.insert(*r);
            fixed.extend(sk.ent_points(*r));
        }
    }
    let mut x = Vec::new();
    let (mut pidx, mut ridx) = (HashMap::new(), HashMap::new());
    for (id, p) in &sk.points {
        if !fixed.contains(id) {
            pidx.insert(*id, x.len());
            x.extend([p.x, p.y]);
        }
    }
    for (id, e) in &sk.entities {
        if let Geom::Circle { r, .. } = e.geom
            && !fixed.contains(id)
        {
            ridx.insert(*id, x.len());
            x.push(r);
        }
    }
    let size = sk.bbox().map_or(10.0, |(lo, hi)| (hi - lo).max_element()).clamp(1.0, 1e4);
    let sys = Sys { sk, pidx, ridx, drags, size };
    if !drags.is_empty() {
        sys.minimize(&mut x, 0.05);
    }
    sys.minimize(&mut x, 0.0);

    let (mut r, mut owners) = (Vec::new(), Vec::new());
    sys.residuals(&x, 0.0, &mut r, Some(&mut owners));
    let mut bad: Vec<Id> = r.iter().zip(&owners).filter(|(v, _)| v.abs() > TOL * 100.0).map(|(_, o)| *o).collect();
    bad.dedup();
    // Squeezing a line down to a point can satisfy almost anything; that is not a solution.
    for (id, e) in &sk.entities {
        if let Geom::Line { a, b } = e.geom
            && sys.p(&x, a).distance(sys.p(&x, b)) < 1e-6
            && sk.pos(a).distance(sk.pos(b)) >= 1e-6
        {
            bad.push(*id);
        }
    }
    let dof = x.len() - rank(sys.jacobian(&mut x, 0.0, &r));
    let (pidx, ridx) = (sys.pidx, sys.ridx);
    for (id, i) in pidx {
        sk.points.insert(id, DVec2::new(x[i], x[i + 1]));
    }
    for (id, i) in ridx {
        if let Some(Geom::Circle { r, .. }) = sk.entities.get_mut(&id).map(|e| &mut e.geom) {
            *r = x[i].max(1e-6);
        }
    }
    Report { ok: bad.is_empty(), dof, bad }
}
