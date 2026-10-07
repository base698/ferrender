//! 2D sketches: points, lines, circles and arcs on a plane, with the
//! constraints and dimensions that hold them in place.

use std::collections::{BTreeMap, BTreeSet};

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::expr::{Kind, Value};

pub type Id = u32;

/// Every sketch has a fixed point at its origin.
pub const ORIGIN: Id = 0;

/// Segments used for a full circle when curves become polylines.
pub const CIRCLE_SEGS: usize = 72;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plane {
    pub origin: DVec3,
    pub x: DVec3,
    pub y: DVec3,
}

impl Plane {
    pub const XY: Plane = Plane { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y };
    pub const XZ: Plane = Plane { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Z };
    pub const YZ: Plane = Plane { origin: DVec3::ZERO, x: DVec3::Y, y: DVec3::Z };

    pub fn normal(&self) -> DVec3 {
        self.x.cross(self.y)
    }

    /// A plane through `origin` facing `normal`, with its x axis horizontal
    /// so that it lines up with the screen when viewed head-on.
    pub fn from_normal(origin: DVec3, normal: DVec3) -> Plane {
        let n = normal.normalize();
        let yaw = if n.x.abs() < 1e-9 && n.y.abs() < 1e-9 { -std::f64::consts::FRAC_PI_2 } else { n.y.atan2(n.x) };
        let x = DVec3::new(-yaw.sin(), yaw.cos(), 0.0);
        Plane { origin, x, y: n.cross(x) }
    }

    pub fn offset(&self, d: f64) -> Plane {
        Plane { origin: self.origin + self.normal() * d, ..*self }
    }

    pub fn to_world(&self, p: DVec2) -> DVec3 {
        self.origin + self.x * p.x + self.y * p.y
    }

    pub fn to_local(&self, p: DVec3) -> DVec2 {
        let d = p - self.origin;
        DVec2::new(d.dot(self.x), d.dot(self.y))
    }

    /// Where a ray meets the plane, in plane coordinates.
    pub fn ray_hit(&self, origin: DVec3, dir: DVec3) -> Option<DVec2> {
        let n = self.normal();
        let den = dir.dot(n);
        if den.abs() < 1e-9 {
            return None;
        }
        let t = (self.origin - origin).dot(n) / den;
        Some(self.to_local(origin + dir * t))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Geom {
    Line { a: Id, b: Id },
    Circle { c: Id, r: f64 },
    /// Counter-clockwise from `s` to `e` around `c`.
    Arc { c: Id, s: Id, e: Id },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    #[serde(flatten)]
    pub geom: Geom,
    #[serde(default)]
    pub construction: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CKind {
    Coincident,
    Horizontal,
    Vertical,
    Parallel,
    Perpendicular,
    Tangent,
    Equal,
    Midpoint,
    Concentric,
    Collinear,
    Symmetric,
    Fix,
    Distance,
    Radius,
    Diameter,
    Angle,
}

impl CKind {
    pub const ALL: [CKind; 16] = [
        CKind::Coincident,
        CKind::Horizontal,
        CKind::Vertical,
        CKind::Parallel,
        CKind::Perpendicular,
        CKind::Tangent,
        CKind::Equal,
        CKind::Midpoint,
        CKind::Concentric,
        CKind::Collinear,
        CKind::Symmetric,
        CKind::Fix,
        CKind::Distance,
        CKind::Radius,
        CKind::Diameter,
        CKind::Angle,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CKind::Coincident => "coincident",
            CKind::Horizontal => "horizontal",
            CKind::Vertical => "vertical",
            CKind::Parallel => "parallel",
            CKind::Perpendicular => "perpendicular",
            CKind::Tangent => "tangent",
            CKind::Equal => "equal",
            CKind::Midpoint => "midpoint",
            CKind::Concentric => "concentric",
            CKind::Collinear => "collinear",
            CKind::Symmetric => "symmetric",
            CKind::Fix => "fix",
            CKind::Distance => "distance",
            CKind::Radius => "radius",
            CKind::Diameter => "diameter",
            CKind::Angle => "angle",
        }
    }

    pub fn parse(s: &str) -> Option<CKind> {
        CKind::ALL.into_iter().find(|k| k.name() == s)
    }

    /// The kind of value a dimension carries; `None` for geometric constraints.
    pub fn value_kind(self) -> Option<Kind> {
        match self {
            CKind::Distance | CKind::Radius | CKind::Diameter => Some(Kind::Length),
            CKind::Angle => Some(Kind::Angle),
            _ => None,
        }
    }

    /// What has to be selected for this constraint, for hints and errors.
    pub fn needs(self) -> &'static str {
        match self {
            CKind::Coincident => "two points, or a point and a line or curve",
            CKind::Horizontal | CKind::Vertical => "a line or two points",
            CKind::Parallel | CKind::Perpendicular | CKind::Collinear => "two lines",
            CKind::Tangent => "a line and a curve, or two curves",
            CKind::Equal => "two lines or two curves",
            CKind::Midpoint => "a point and a line",
            CKind::Concentric => "two circles or arcs",
            CKind::Symmetric => "two points and a line",
            CKind::Fix => "a point or a line",
            CKind::Distance => "a line, two points, a point and a line, or two lines",
            CKind::Radius | CKind::Diameter => "a circle or arc",
            CKind::Angle => "two lines",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub kind: CKind,
    /// Points first, then lines, then curves.
    pub refs: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ref {
    Point,
    Line,
    Curve,
}

/// Copied sketch geometry, as it travels through the clipboard.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub points: Vec<(Id, DVec2)>,
    pub entities: Vec<(Id, Entity)>,
    pub constraints: Vec<Constraint>,
}

impl Clip {
    pub fn center(&self) -> DVec2 {
        let (mut lo, mut hi) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
        for (_, p) in &self.points {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        if self.points.is_empty() { DVec2::ZERO } else { (lo + hi) / 2.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sketch {
    pub plane: Plane,
    pub points: BTreeMap<Id, DVec2>,
    pub entities: BTreeMap<Id, Entity>,
    pub constraints: BTreeMap<Id, Constraint>,
    /// Points, entities and constraints share one id space.
    pub next: Id,
    #[serde(default = "yes")]
    pub visible: bool,
    /// Points and circles projected from the model, which the solver leaves alone.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub fixed: BTreeSet<Id>,
}

fn yes() -> bool {
    true
}

pub fn seg_dist(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let d = b - a;
    let l = d.length_squared();
    let t = if l < 1e-18 { 0.0 } else { ((p - a).dot(d) / l).clamp(0.0, 1.0) };
    (a + d * t).distance(p)
}

impl Sketch {
    pub fn new(plane: Plane) -> Sketch {
        Sketch { plane, points: BTreeMap::from([(ORIGIN, DVec2::ZERO)]), entities: BTreeMap::new(), constraints: BTreeMap::new(), next: 1, visible: true, fixed: BTreeSet::new() }
    }

    fn id(&mut self) -> Id {
        self.next += 1;
        self.next - 1
    }

    pub fn add_point(&mut self, p: DVec2) -> Id {
        let id = self.id();
        self.points.insert(id, p);
        id
    }

    /// The existing point within `tol` of `p`, or a new one.
    pub fn point_at(&mut self, p: DVec2, tol: f64) -> Id {
        match self.points.iter().find(|(_, q)| q.distance(p) <= tol) {
            Some((id, _)) => *id,
            None => self.add_point(p),
        }
    }

    pub fn add(&mut self, geom: Geom, construction: bool) -> Id {
        let id = self.id();
        self.entities.insert(id, Entity { geom, construction });
        id
    }

    pub fn add_line(&mut self, a: Id, b: Id) -> Id {
        self.add(Geom::Line { a, b }, false)
    }

    /// An axis-aligned rectangle between two corner points, with its
    /// horizontal and vertical constraints. Returns the four lines.
    pub fn add_rect(&mut self, a: Id, c: Id, construction: bool) -> Vec<Id> {
        let (pa, pc) = (self.pos(a), self.pos(c));
        let b = self.add_point(DVec2::new(pc.x, pa.y));
        let d = self.add_point(DVec2::new(pa.x, pc.y));
        let corners = [a, b, c, d];
        (0..4)
            .map(|i| {
                let l = self.add(Geom::Line { a: corners[i], b: corners[(i + 1) % 4] }, construction);
                let kind = if i % 2 == 0 { CKind::Horizontal } else { CKind::Vertical };
                let id = self.id();
                self.constraints.insert(id, Constraint { kind, refs: vec![l], value: None });
                l
            })
            .collect()
    }

    pub fn pos(&self, id: Id) -> DVec2 {
        self.points.get(&id).copied().unwrap_or(DVec2::ZERO)
    }

    pub fn ref_kind(&self, id: Id) -> Option<Ref> {
        if self.points.contains_key(&id) {
            return Some(Ref::Point);
        }
        self.entities.get(&id).map(|e| if matches!(e.geom, Geom::Line { .. }) { Ref::Line } else { Ref::Curve })
    }

    /// Checks that `refs` suit `kind` and puts them in canonical order.
    pub fn normalize(&self, kind: CKind, refs: &[Id]) -> Result<Vec<Id>, String> {
        let mut r: Vec<(Ref, Id)> = Vec::new();
        for id in refs {
            r.push((self.ref_kind(*id).ok_or(format!("nothing in the sketch has id {id}"))?, *id));
        }
        r.sort_by_key(|x| x.0);
        let pat: Vec<Ref> = r.iter().map(|x| x.0).collect();
        use Ref::{Curve as C, Line as L, Point as P};
        let ok = match kind {
            CKind::Coincident => pat == [P, P] || pat == [P, L] || pat == [P, C],
            CKind::Horizontal | CKind::Vertical => pat == [L] || pat == [P, P],
            CKind::Parallel | CKind::Perpendicular | CKind::Collinear | CKind::Angle => pat == [L, L],
            CKind::Tangent => pat == [L, C] || pat == [C, C],
            CKind::Equal => pat == [L, L] || pat == [C, C],
            CKind::Midpoint => pat == [P, L],
            CKind::Concentric => pat == [C, C],
            CKind::Symmetric => pat == [P, P, L],
            CKind::Fix => pat == [P] || pat == [L],
            CKind::Distance => pat == [L] || pat == [P, P] || pat == [P, L] || pat == [L, L],
            CKind::Radius | CKind::Diameter => pat == [C],
        };
        let ids: Vec<Id> = r.iter().map(|x| x.1).collect();
        if !ok || (ids.len() == 2 && ids[0] == ids[1]) {
            return Err(format!("{} needs {}", kind.name(), kind.needs()));
        }
        Ok(ids)
    }

    pub fn add_constraint(&mut self, kind: CKind, refs: &[Id], value: Option<Value>) -> Result<Id, String> {
        let refs = self.normalize(kind, refs)?;
        if kind.value_kind().is_some() != value.is_some() {
            return Err(if value.is_some() { format!("{} takes no value", kind.name()) } else { format!("{} needs a value", kind.name()) });
        }
        if self.constraints.values().any(|c| c.kind == kind && c.refs == refs) {
            return Err(format!("that {} constraint already exists", kind.name()));
        }
        let id = self.id();
        self.constraints.insert(id, Constraint { kind, refs, value });
        Ok(id)
    }

    /// The points an entity is defined by.
    pub fn ent_points(&self, id: Id) -> Vec<Id> {
        match self.entities.get(&id).map(|e| e.geom) {
            Some(Geom::Line { a, b }) => vec![a, b],
            Some(Geom::Circle { c, .. }) => vec![c],
            Some(Geom::Arc { c, s, e }) => vec![c, s, e],
            None => vec![],
        }
    }

    /// Centre and radius of a circle or arc.
    pub fn curve(&self, id: Id) -> Option<(DVec2, f64)> {
        match self.entities.get(&id)?.geom {
            Geom::Circle { c, r } => Some((self.pos(c), r)),
            Geom::Arc { c, s, .. } => Some((self.pos(c), self.pos(c).distance(self.pos(s)))),
            Geom::Line { .. } => None,
        }
    }

    pub fn line(&self, id: Id) -> Option<(DVec2, DVec2)> {
        match self.entities.get(&id)?.geom {
            Geom::Line { a, b } => Some((self.pos(a), self.pos(b))),
            _ => None,
        }
    }

    /// Start angle and counter-clockwise sweep of an arc.
    pub fn arc_angles(&self, id: Id) -> Option<(f64, f64)> {
        let Geom::Arc { c, s, e } = self.entities.get(&id)?.geom else { return None };
        let (ds, de) = (self.pos(s) - self.pos(c), self.pos(e) - self.pos(c));
        let a0 = ds.y.atan2(ds.x);
        let mut sweep = (de.y.atan2(de.x) - a0).rem_euclid(std::f64::consts::TAU);
        if sweep < 1e-9 {
            sweep = std::f64::consts::TAU;
        }
        Some((a0, sweep))
    }

    /// The entity as a polyline; circles repeat their first point at the end.
    pub fn polyline(&self, id: Id) -> Vec<DVec2> {
        let Some(e) = self.entities.get(&id) else { return vec![] };
        match e.geom {
            Geom::Line { a, b } => vec![self.pos(a), self.pos(b)],
            Geom::Circle { c, r } => {
                let c = self.pos(c);
                (0..=CIRCLE_SEGS).map(|i| c + DVec2::from_angle((i % CIRCLE_SEGS) as f64 / CIRCLE_SEGS as f64 * std::f64::consts::TAU) * r).collect()
            }
            Geom::Arc { c, s, e } => {
                let (a0, sweep) = self.arc_angles(id).unwrap();
                let (c, s, e) = (self.pos(c), self.pos(s), self.pos(e));
                let r = c.distance(s);
                let n = ((sweep / std::f64::consts::TAU * CIRCLE_SEGS as f64).ceil() as usize).max(2);
                (0..=n)
                    .map(|i| match i {
                        0 => s,
                        i if i == n => e,
                        i => c + DVec2::from_angle(a0 + sweep * i as f64 / n as f64) * r,
                    })
                    .collect()
            }
        }
    }

    /// Distance from `p` to the drawn entity.
    pub fn dist_to(&self, id: Id, p: DVec2) -> f64 {
        self.polyline(id).windows(2).map(|w| seg_dist(p, w[0], w[1])).fold(f64::MAX, f64::min)
    }

    /// Removes points, entities and constraints, along with whatever depended on them.
    pub fn remove(&mut self, ids: &[Id]) {
        let ids: BTreeSet<Id> = ids.iter().copied().filter(|i| *i != ORIGIN).collect();
        let mut loose = BTreeSet::new();
        let doomed: Vec<Id> = self.entities.keys().copied().filter(|e| ids.contains(e) || self.ent_points(*e).iter().any(|p| ids.contains(p))).collect();
        for e in doomed {
            loose.extend(self.ent_points(e));
            self.entities.remove(&e);
        }
        for id in &ids {
            self.points.remove(id);
            self.constraints.remove(id);
        }
        // Points that only existed to hold up a removed entity go with it.
        let used: BTreeSet<Id> = self.entities.keys().flat_map(|e| self.ent_points(*e)).collect();
        for p in loose {
            if p != ORIGIN && !used.contains(&p) {
                self.points.remove(&p);
            }
        }
        let (pts, ents) = (&self.points, &self.entities);
        self.fixed.retain(|i| pts.contains_key(i) || ents.contains_key(i));
        self.constraints.retain(|_, c| c.refs.iter().all(|r| pts.contains_key(r) || ents.contains_key(r)));
    }

    /// The current size of what a dimension on `refs` would measure (mm or degrees).
    pub fn measure(&self, kind: CKind, refs: &[Id]) -> f64 {
        let perp = |p: DVec2, l: (DVec2, DVec2)| ((l.1 - l.0).perp_dot(p - l.0) / (l.1 - l.0).length().max(1e-12)).abs();
        match (kind, refs) {
            (CKind::Radius, [e]) => self.curve(*e).map_or(0.0, |c| c.1),
            (CKind::Diameter, [e]) => self.curve(*e).map_or(0.0, |c| c.1 * 2.0),
            (CKind::Angle, [a, b]) => match (self.line(*a), self.line(*b)) {
                (Some(a), Some(b)) => {
                    let (d1, d2) = (a.1 - a.0, b.1 - b.0);
                    d1.perp_dot(d2).abs().atan2(d1.dot(d2)).to_degrees()
                }
                _ => 0.0,
            },
            (CKind::Distance, [l]) => self.line(*l).map_or(0.0, |l| l.0.distance(l.1)),
            (CKind::Distance, [a, b]) => match (self.ref_kind(*a), self.line(*b)) {
                (Some(Ref::Point), None) => self.pos(*a).distance(self.pos(*b)),
                (Some(Ref::Point), Some(l)) => perp(self.pos(*a), l),
                (_, Some(l)) => self.line(*a).map_or(0.0, |l2| perp(l.0, l2)),
                _ => 0.0,
            },
            _ => 0.0,
        }
    }

    /// Copies entities and points, with the constraints that only involve them.
    pub fn copy(&self, ids: &[Id]) -> Clip {
        let ents: Vec<(Id, Entity)> = ids.iter().filter_map(|i| self.entities.get(i).map(|e| (*i, *e))).collect();
        let mut pts: BTreeSet<Id> = ids.iter().copied().filter(|i| self.points.contains_key(i)).collect();
        for (id, _) in &ents {
            pts.extend(self.ent_points(*id));
        }
        let has = |r: &Id| pts.contains(r) || ents.iter().any(|e| e.0 == *r);
        Clip {
            points: pts.iter().map(|p| (*p, self.pos(*p))).collect(),
            constraints: self.constraints.values().filter(|c| c.kind != CKind::Fix && c.refs.iter().all(has)).cloned().collect(),
            entities: ents,
        }
    }

    /// Adds copied geometry shifted by `offset`; returns the new points and entities.
    pub fn paste(&mut self, clip: &Clip, offset: DVec2) -> Vec<Id> {
        let mut map = BTreeMap::new();
        let mut out = Vec::new();
        for (old, p) in &clip.points {
            map.insert(*old, self.add_point(*p + offset));
        }
        let m = |map: &BTreeMap<Id, Id>, id: Id| map.get(&id).copied().unwrap_or(ORIGIN);
        for (old, e) in &clip.entities {
            let geom = match e.geom {
                Geom::Line { a, b } => Geom::Line { a: m(&map, a), b: m(&map, b) },
                Geom::Circle { c, r } => Geom::Circle { c: m(&map, c), r },
                Geom::Arc { c, s, e } => Geom::Arc { c: m(&map, c), s: m(&map, s), e: m(&map, e) },
            };
            let id = self.add(geom, e.construction);
            map.insert(*old, id);
            out.push(id);
        }
        // Points that belong to a pasted entity come along with it; the rest
        // were copied on their own and are selected directly.
        let owned: BTreeSet<Id> = out.iter().flat_map(|e| self.ent_points(*e)).collect();
        out.extend(clip.points.iter().map(|(old, _)| map[old]).filter(|p| !owned.contains(p)));
        for c in &clip.constraints {
            let id = self.id();
            self.constraints.insert(id, Constraint { kind: c.kind, refs: c.refs.iter().map(|r| m(&map, *r)).collect(), value: c.value.clone() });
        }
        out
    }

    /// Shifts entities and points by `by`. An entity standing on the origin
    /// or on a fixed point is given a point of its own, so that it can leave.
    pub fn translate(&mut self, ids: &[Id], by: DVec2) {
        let stuck = |sk: &Sketch, p: Id| p == ORIGIN || sk.fixed.contains(&p);
        let mut pts: BTreeSet<Id> = ids.iter().copied().filter(|i| self.points.contains_key(i) && !stuck(self, *i)).collect();
        for id in ids {
            for p in self.ent_points(*id) {
                if !stuck(self, p) {
                    pts.insert(p);
                    continue;
                }
                let free = self.add_point(self.pos(p) + by);
                let swap = |x: &mut Id| {
                    if *x == p {
                        *x = free;
                    }
                };
                match &mut self.entities.get_mut(id).unwrap().geom {
                    Geom::Line { a, b } => [a, b].into_iter().for_each(swap),
                    Geom::Circle { c, .. } => swap(c),
                    Geom::Arc { c, s, e } => [c, s, e].into_iter().for_each(swap),
                }
            }
        }
        for p in pts {
            *self.points.get_mut(&p).unwrap() += by;
        }
    }

    /// Bounds of everything drawn, in sketch coordinates.
    pub fn bbox(&self) -> Option<(DVec2, DVec2)> {
        let (mut lo, mut hi) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
        let mut any = false;
        let pts = self.entities.keys().flat_map(|e| self.polyline(*e)).chain(self.points.iter().filter(|(i, _)| **i != ORIGIN).map(|(_, p)| *p));
        for p in pts {
            lo = lo.min(p);
            hi = hi.max(p);
            any = true;
        }
        any.then_some((lo, hi))
    }
}
