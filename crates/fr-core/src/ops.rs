//! Sketch editing tools that rework existing geometry: trim, mirror,
//! offset, corner fillet and chamfer, and projecting a body's face outline.

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use glam::{DVec2, DVec3};

use crate::expr::Value;
use crate::sketch::{ArcGuide, CKind, Constraint, Geom, Id, ORIGIN, Sketch};

/// A line segment or a circular arc; a circle is an arc that sweeps all the way round.
#[derive(Clone, Copy)]
enum Shape {
    Seg(DVec2, DVec2),
    Arc { c: DVec2, r: f64, a0: f64, sweep: f64 },
}

fn shape(sk: &Sketch, id: Id) -> Option<Shape> {
    match sk.entities.get(&id)?.geom {
        Geom::Line { a, b } => Some(Shape::Seg(sk.pos(a), sk.pos(b))),
        Geom::Spline { .. } => None,
        Geom::Circle { c, r } => Some(Shape::Arc { c: sk.pos(c), r, a0: 0.0, sweep: TAU }),
        Geom::Arc { c, .. } => {
            let (a0, sweep) = sk.arc_angles(id)?;
            Some(Shape::Arc { c: sk.pos(c), r: sk.curve(id)?.1, a0, sweep })
        }
    }
}

impl Shape {
    /// Position along the shape, 0 at its start and 1 at its end.
    fn param(&self, p: DVec2) -> f64 {
        match *self {
            Shape::Seg(a, b) => (p - a).dot(b - a) / (b - a).length_squared().max(1e-18),
            Shape::Arc { c, a0, sweep, .. } => ((p - c).to_angle() - a0).rem_euclid(TAU) / sweep,
        }
    }

    fn at(&self, t: f64) -> DVec2 {
        match *self {
            Shape::Seg(a, b) => a + (b - a) * t,
            Shape::Arc { c, r, a0, sweep } => c + DVec2::from_angle(a0 + sweep * t) * r,
        }
    }

    fn closed(&self) -> bool {
        matches!(self, Shape::Arc { sweep, .. } if *sweep >= TAU - 1e-9)
    }

    fn holds(&self, p: DVec2) -> bool {
        let t = self.param(p);
        self.closed() || (-1e-7..=1.0 + 1e-7).contains(&t)
    }

    /// Where the two shapes cross or touch.
    fn crossings(&self, other: &Shape) -> Vec<DVec2> {
        let found = match (*self, *other) {
            (Shape::Seg(a, b), Shape::Seg(c, d)) => {
                let (r, s) = (b - a, d - c);
                let den = r.perp_dot(s);
                if den.abs() < 1e-12 { vec![] } else { vec![a + r * ((c - a).perp_dot(s) / den)] }
            }
            (Shape::Seg(a, b), Shape::Arc { c, r, .. }) | (Shape::Arc { c, r, .. }, Shape::Seg(a, b)) => {
                let d = (b - a).normalize_or_zero();
                let foot = a + d * (c - a).dot(d);
                let h2 = r * r - foot.distance_squared(c);
                if h2 < -1e-9 {
                    vec![]
                } else {
                    let h = h2.max(0.0).sqrt();
                    if h < 1e-7 { vec![foot] } else { vec![foot - d * h, foot + d * h] }
                }
            }
            (Shape::Arc { c: c1, r: r1, .. }, Shape::Arc { c: c2, r: r2, .. }) => {
                let d = c1.distance(c2);
                if d < 1e-9 || d > r1 + r2 + 1e-9 || d < (r1 - r2).abs() - 1e-9 {
                    vec![]
                } else {
                    let x = (d * d + r1 * r1 - r2 * r2) / (2.0 * d);
                    let h = (r1 * r1 - x * x).max(0.0).sqrt();
                    let u = (c2 - c1) / d;
                    let m = c1 + u * x;
                    if h < 1e-7 { vec![m] } else { vec![m + u.perp() * h, m - u.perp() * h] }
                }
            }
        };
        found.into_iter().filter(|p| self.holds(*p) && other.holds(*p)).collect()
    }
}

/// Adds a point at `p` that stays on entity `on`.
fn pinned(sk: &mut Sketch, p: DVec2, on: Id) -> Id {
    let id = sk.add_point(p);
    let _ = sk.add_constraint(CKind::Coincident, &[id, on], None);
    id
}

impl Sketch {
    /// Removes the stretch of entity `id` under `click`, back to the nearest
    /// crossings with other geometry. With no crossings the whole entity goes.
    pub fn trim(&mut self, id: Id, click: DVec2) -> Result<(), String> {
        let s = shape(self, id).ok_or("Click a line, arc or circle to trim.")?;
        let t0 = s.param(click);
        let mut cuts: Vec<(f64, Id)> = Vec::new();
        for other in self.entities.keys().copied().filter(|o| *o != id) {
            let o = shape(self, other).ok_or("Trim does not yet support intersections with splines. Edit their fit points instead.")?;
            for p in s.crossings(&o) {
                let t = s.param(p);
                if s.closed() || (1e-6..=1.0 - 1e-6).contains(&t) {
                    cuts.push((t, other));
                }
            }
        }
        cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        cuts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-7);
        let before = cuts.iter().rfind(|c| c.0 < t0).copied();
        let after = cuts.iter().find(|c| c.0 > t0).copied();
        let geom = self.entities[&id].geom;
        let construction = self.entities[&id].construction;
        // What a dimension or midpoint on the entity meant no longer holds for the pieces.
        let stale = |c: &Constraint| c.refs.contains(&id) && (
            matches!(c.kind, CKind::Distance | CKind::Midpoint | CKind::Equal | CKind::Symmetric)
            // The original sweep describes the whole arc, not a trimmed fragment.
            || (c.kind == CKind::Angle && c.refs == [id] && matches!(geom, Geom::Arc { .. }))
        );
        // Trim turns a guided arc into ordinary circular pieces; its former fit
        // point does not define either new piece. Keep that point as a sketch point.
        self.arc_guides.remove(&id);
        let ends = self.ent_points(id);

        if s.closed() {
            if cuts.len() < 2 {
                self.remove(&[id]);
                return Ok(());
            }
            // The arc that is left runs from the cut after the click round to the cut before it.
            let (from, to) = (after.unwrap_or(cuts[0]), before.unwrap_or(cuts[cuts.len() - 1]));
            let Geom::Circle { c, .. } = geom else { unreachable!("only circles are closed") };
            let (ps, pe) = (pinned(self, s.at(from.0), from.1), pinned(self, s.at(to.0), to.1));
            self.entities.get_mut(&id).unwrap().geom = Geom::Arc { c, s: ps, e: pe };
            return Ok(());
        }
        if before.is_none() && after.is_none() {
            self.remove(&[id]);
            return Ok(());
        }
        self.constraints.retain(|_, c| !stale(c));
        let (first, last) = (ends[ends.len() - 2], ends[ends.len() - 1]);
        let piece = |g: Geom, from: Id, to: Id| match g {
            Geom::Line { .. } => Geom::Line { a: from, b: to },
            Geom::Arc { c, .. } => Geom::Arc { c, s: from, e: to },
            g => g,
        };
        let head = before.map(|(t, other)| (first, pinned(self, s.at(t), other)));
        let tail = after.map(|(t, other)| (pinned(self, s.at(t), other), last));
        let mut parts = head.into_iter().chain(tail);
        let (from, to) = parts.next().unwrap();
        self.entities.get_mut(&id).unwrap().geom = piece(geom, from, to);
        if let Some((from, to)) = parts.next() {
            let second = self.add(piece(geom, from, to), construction);
            let own: Vec<Constraint> = self.constraints.values().filter(|c| c.refs == [id]
                && (matches!(c.kind, CKind::Horizontal | CKind::Vertical)
                    || (c.kind == CKind::Angle && matches!(geom, Geom::Line { .. })))).cloned().collect();
            for c in own {
                let _ = self.add_constraint(c.kind, &[second], c.value);
            }
        }
        self.drop_unused(&[first, last]);
        Ok(())
    }

    /// Removes the given points if no entity uses them any more.
    pub fn drop_unused(&mut self, points: &[Id]) {
        let gone: Vec<Id> = points.iter().copied().filter(|p| *p != ORIGIN && !self.entities.keys().any(|e| self.ent_points(*e).contains(p))).collect();
        if !gone.is_empty() {
            self.remove(&gone);
        }
    }

    /// Adds mirror images of `ids` across line `axis`, held in place by symmetry constraints.
    pub fn mirror(&mut self, ids: &[Id], axis: Id) -> Result<Vec<Id>, String> {
        let (a, b) = self.line(axis).ok_or("The mirror line must be a line.")?;
        let ids: Vec<Id> = ids.iter().copied().filter(|i| *i != axis).collect();
        let clip = self.copy(&ids);
        if clip.points.is_empty() {
            return Err("Select what to mirror, then the mirror line last.".into());
        }
        let d = (b - a).normalize_or_zero();
        let reflect = |p: DVec2| a + d * (p - a).dot(d) * 2.0 - (p - a);
        let mut map = BTreeMap::new();
        for (old, p) in &clip.points {
            // A point on the mirror line is its own image.
            if d.perp_dot(*p - a).abs() < 1e-7 {
                map.insert(*old, *old);
            } else {
                let new = self.add_point(reflect(*p));
                map.insert(*old, new);
                let _ = self.add_constraint(CKind::Symmetric, &[*old, new, axis], None);
            }
        }
        let mut out = Vec::new();
        for (old, e) in &clip.entities {
            let geom = match e.geom {
                Geom::Line { a, b } => Geom::Line { a: map[&a], b: map[&b] },
                Geom::Spline { a,b,c,d } => Geom::Spline { a:map[&a], b:map[&b], c:map[&c], d:map[&d] },
                Geom::Circle { c, r } => Geom::Circle { c: map[&c], r },
                // A reflection turns counter-clockwise into clockwise, so the ends swap.
                Geom::Arc { c, s, e } => Geom::Arc { c: map[&c], s: map[&e], e: map[&s] },
            };
            let new = self.add(geom, e.construction);
            if matches!(geom, Geom::Circle { .. }) {
                let _ = self.add_constraint(CKind::Equal, &[*old, new], None);
            }
            map.insert(*old, new);
            out.push(new);
        }
        for (arc,guide) in &clip.arc_guides {
            match *guide {
                ArcGuide::Through { point } => { self.arc_guides.insert(map[arc],ArcGuide::Through { point:map[&point] }); }
                ArcGuide::Tangent { source,start } => {
                    let (arc,source,start) = (map[arc],map[&source],map[&start]);
                    self.add_constraint(CKind::Tangent,&[source,arc],None)?;
                    self.arc_guides.insert(arc,ArcGuide::Tangent { source,start });
                }
            }
        }
        out.extend(clip.points.iter().map(|(old, _)| map[old]).filter(|p| !ids.contains(p) && self.points.contains_key(p) && clip.entities.is_empty()));
        Ok(out)
    }

    /// The corner a fillet or chamfer applies to: a point where exactly two lines end.
    fn corner(&self, p: Id) -> Result<[(Id, Id); 2], String> {
        let lines: Vec<(Id, Id)> = self
            .entities
            .iter()
            .filter_map(|(id, e)| match e.geom {
                Geom::Line { a, b } if a == p => Some((*id, b)),
                Geom::Line { a, b } if b == p => Some((*id, a)),
                _ => None,
            })
            .collect();
        let touches_curve = self.entities.values().any(|e| !matches!(e.geom, Geom::Line { .. }) && [e.geom].iter().any(|g| matches!(g, Geom::Arc { s, e, .. } if *s == p || *e == p) || matches!(g, Geom::Spline { a, d, .. } if *a == p || *d == p)));
        match lines.as_slice() {
            [a, b] if !touches_curve => Ok([*a, *b]),
            _ => Err("Pick a corner where exactly two lines meet.".into()),
        }
    }

    /// Replaces corner `p` with an arc of `radius` tangent to both lines
    /// (or, with `chamfer`, a straight cut `radius` back along each line).
    pub fn round_corner(&mut self, p: Id, size: Value, chamfer: bool) -> Result<Id, String> {
        let [(l1, far1), (l2, far2)] = self.corner(p)?;
        let (at, a, b) = (self.pos(p), self.pos(far1), self.pos(far2));
        let (u1, u2) = ((a - at).normalize_or_zero(), (b - at).normalize_or_zero());
        let half = u1.angle_to(u2).abs() / 2.0;
        if half < 0.02 || half > std::f64::consts::FRAC_PI_2 - 0.01 {
            return Err("The lines are too close to parallel for that.".into());
        }
        let back = if chamfer { size.v } else { size.v / half.tan() };
        if size.v <= 0.0 || back >= at.distance(a) - 1e-6 || back >= at.distance(b) - 1e-6 {
            return Err("That is too big for this corner.".into());
        }
        let (p1, p2) = (self.add_point(at + u1 * back), self.add_point(at + u2 * back));
        for (line, new) in [(l1, p1), (l2, p2)] {
            if let Some(Geom::Line { a, b }) = self.entities.get_mut(&line).map(|e| &mut e.geom) {
                if *a == p { *a = new } else { *b = new }
            }
        }
        let construction = self.entities[&l1].construction;
        let made = if chamfer {
            self.add(Geom::Line { a: p1, b: p2 }, construction)
        } else {
            let centre = self.add_point(at + (u1 + u2).normalize() * (size.v / half.sin()));
            let c = self.pos(centre);
            let (s, e) = if (self.pos(p1) - c).perp_dot(self.pos(p2) - c) > 0.0 { (p1, p2) } else { (p2, p1) };
            let arc = self.add(Geom::Arc { c: centre, s, e }, construction);
            self.add_constraint(CKind::Tangent, &[l1, arc], None)?;
            self.add_constraint(CKind::Tangent, &[l2, arc], None)?;
            self.add_constraint(CKind::Radius, &[arc], Some(size))?;
            arc
        };
        self.drop_unused(&[p]);
        Ok(made)
    }

    /// Adds copies of lines and circles `ids` set off by `distance`: outward
    /// for a closed outline or a circle, to the left of an open run of lines.
    /// Negative goes the other way. The copies stay parallel at that distance.
    pub fn offset(&mut self, ids: &[Id], distance: Value) -> Result<Vec<Id>, String> {
        let d = distance.v;
        let mut out = Vec::new();
        let mut lines: Vec<(Id, Id, Id)> = Vec::new();
        for id in ids {
            match self.entities.get(id).map(|e| e.geom) {
                Some(Geom::Circle { c, r }) => {
                    if r + d <= 1e-6 {
                        return Err("That offset is larger than the circle.".into());
                    }
                    let new = self.add(Geom::Circle { c, r: r + d }, false);
                    out.push(new);
                }
                Some(Geom::Line { a, b }) => lines.push((*id, a, b)),
                Some(Geom::Arc { .. } | Geom::Spline { .. }) => return Err("Offset works on lines and circles; arcs and splines are not supported yet.".into()),
                None => {}
            }
        }
        // Follow the lines end to end, one run at a time.
        while !lines.is_empty() {
            let degree = |p: Id, lines: &[(Id, Id, Id)]| lines.iter().filter(|l| l.1 == p || l.2 == p).count();
            let start = lines.iter().position(|l| degree(l.1, &lines) == 1 || degree(l.2, &lines) == 1).unwrap_or(0);
            let (first, mut a, mut b) = lines.remove(start);
            if degree(a, &lines) > 0 && degree(b, &lines) == 0 {
                std::mem::swap(&mut a, &mut b);
            }
            let mut run = vec![(first, a, b)];
            while let Some(i) = lines.iter().position(|l| l.1 == run[run.len() - 1].2 || l.2 == run[run.len() - 1].2) {
                let (id, x, y) = lines.remove(i);
                let end = run[run.len() - 1].2;
                run.push(if x == end { (id, x, y) } else { (id, y, x) });
            }
            let closed = run.len() > 2 && run[0].1 == run[run.len() - 1].2;
            let pts: Vec<DVec2> = run.iter().map(|l| self.pos(l.1)).chain((!closed).then(|| self.pos(run[run.len() - 1].2))).collect();
            // Right of travel is outside a counter-clockwise outline; open runs go left.
            let side = if closed { if crate::profile::signed_area(&pts) > 0.0 { d } else { -d } } else { -d };
            let moved = crate::profile::offset_path(&pts, side, closed);
            let new: Vec<Id> = moved.iter().map(|p| self.add_point(*p)).collect();
            for (i, (orig, ..)) in run.iter().enumerate() {
                let line = self.add(Geom::Line { a: new[i], b: new[(i + 1) % new.len()] }, false);
                self.add_constraint(CKind::Parallel, &[*orig, line], None)?;
                self.add_constraint(CKind::Distance, &[*orig, line], Some(Value { expr: distance.expr.clone(), v: d.abs() }))?;
                out.push(line);
            }
        }
        if out.is_empty() { Err("Select lines or circles to offset.".into()) } else { Ok(out) }
    }

    /// Adds outlines from the model, flattened onto the sketch plane and
    /// fixed in place. Round outlines become true circles.
    pub fn project(&mut self, loops: &[Vec<DVec3>]) -> Vec<Id> {
        let mut out = Vec::new();
        for ring in loops {
            let pts: Vec<DVec2> = ring.iter().map(|p| self.plane.to_local(*p)).collect();
            let centre = pts.iter().sum::<DVec2>() / pts.len().max(1) as f64;
            let r = pts.first().map_or(0.0, |p| p.distance(centre));
            if pts.len() >= 16 && r > 1e-6 && pts.iter().all(|p| (p.distance(centre) - r).abs() < r * 1e-5) {
                let c = self.point_at(centre, 1e-6);
                let id = self.add(Geom::Circle { c, r }, false);
                self.fixed.extend([c, id]);
                out.push(id);
                continue;
            }
            let ids: Vec<Id> = pts.iter().map(|p| self.point_at(*p, 1e-6)).collect();
            for i in 0..ids.len() {
                let (a, b) = (ids[i], ids[(i + 1) % ids.len()]);
                let exists = self.entities.values().any(|e| matches!(e.geom, Geom::Line { a: x, b: y } if (x, y) == (a, b) || (x, y) == (b, a)));
                if a != b && !exists {
                    out.push(self.add_line(a, b));
                    self.fixed.extend([a, b]);
                }
            }
        }
        out
    }

    /// A regular polygon around point `centre` with one corner at `corner`.
    /// It is inscribed in a construction circle, with equal sides.
    pub fn add_polygon(&mut self, centre: Id, corner: DVec2, sides: usize, construction: bool) -> Vec<Id> {
        let c = self.pos(centre);
        let (r, a0) = (c.distance(corner), (corner - c).to_angle());
        let circle = self.add(Geom::Circle { c: centre, r }, true);
        let pts: Vec<Id> = (0..sides).map(|i| self.add_point(c + DVec2::from_angle(a0 + TAU * i as f64 / sides as f64) * r)).collect();
        let lines: Vec<Id> = (0..sides).map(|i| self.add(Geom::Line { a: pts[i], b: pts[(i + 1) % sides] }, construction)).collect();
        for p in &pts {
            let _ = self.add_constraint(CKind::Coincident, &[*p, circle], None);
        }
        for pair in lines.windows(2) {
            let _ = self.add_constraint(CKind::Equal, pair, None);
        }
        lines
    }
}
