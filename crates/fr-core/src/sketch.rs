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
    /// Native interpolating B-spline through four editable fit points.
    Spline { a: Id, b: Id, c: Id, d: Id },
}

/// Extra construction intent for arcs drawn without a centre.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArcGuide {
    Through { point: Id },
    Tangent { source: Id, start: Id },
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
    PositionX,
    PositionY,
}

impl CKind {
    pub const ALL: [CKind; 18] = [
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
        CKind::PositionX,
        CKind::PositionY,
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
            CKind::PositionX => "position_x",
            CKind::PositionY => "position_y",
        }
    }

    pub fn parse(s: &str) -> Option<CKind> {
        CKind::ALL.into_iter().find(|k| k.name() == s)
    }

    /// The kind of value a dimension carries; `None` for geometric constraints.
    pub fn value_kind(self) -> Option<Kind> {
        match self {
            CKind::Distance | CKind::Radius | CKind::Diameter | CKind::PositionX | CKind::PositionY => Some(Kind::Length),
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
            CKind::Angle => "a line, a circular arc, or two lines",
            CKind::PositionX | CKind::PositionY => "one point",
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
    Spline,
}

/// Copied sketch geometry, as it travels through the clipboard.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub points: Vec<(Id, DVec2)>,
    pub entities: Vec<(Id, Entity)>,
    pub constraints: Vec<Constraint>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arc_guides: BTreeMap<Id, ArcGuide>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<Id>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<crate::reference::ReferenceImage>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arc_guides: BTreeMap<Id, ArcGuide>,
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
        Sketch { plane, on: None, points: BTreeMap::from([(ORIGIN, DVec2::ZERO)]), entities: BTreeMap::new(), constraints: BTreeMap::new(), next: 1, visible: true, fixed: BTreeSet::new(), reference: None, arc_guides: BTreeMap::new() }
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
        self.entities.get(&id).map(|e| match e.geom { Geom::Line { .. } => Ref::Line, Geom::Spline { .. } => Ref::Spline, _ => Ref::Curve })
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
            CKind::Parallel | CKind::Perpendicular | CKind::Collinear => pat == [L, L],
            CKind::Angle => pat == [L] || pat == [L, L] || (pat == [C] && matches!(self.entities[&r[0].1].geom, Geom::Arc { .. })),
            CKind::Tangent => pat == [L, C] || pat == [C, C],
            CKind::Equal => pat == [L, L] || pat == [C, C],
            CKind::Midpoint => pat == [P, L],
            CKind::Concentric => pat == [C, C],
            CKind::Symmetric => pat == [P, P, L],
            CKind::Fix => pat == [P] || pat == [L],
            CKind::Distance => pat == [L] || pat == [P, P] || pat == [P, L] || pat == [L, L],
            CKind::Radius | CKind::Diameter => pat == [C],
            CKind::PositionX | CKind::PositionY => pat == [P],
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
        if let Some(v) = &value { self.validate_dimension(kind, &refs, v.v)?; }
        if self.constraints.values().any(|c| c.kind == kind && c.refs == refs) {
            return Err(format!("that {} constraint already exists", kind.name()));
        }
        let id = self.id();
        self.constraints.insert(id, Constraint { kind, refs, value });
        Ok(id)
    }

    /// Dimensional domains shared by interactive entry, the API and native-file validation.
    pub fn validate_dimension(&self, kind: CKind, refs: &[Id], value: f64) -> Result<(), String> {
        if !value.is_finite() { return Err("a dimension must be finite".into()); }
        if kind == CKind::Angle && let [id] = refs {
            match self.entities.get(id).map(|e| e.geom) {
                Some(Geom::Line { .. }) => {}, // Absolute line direction is signed; zero is horizontal.
                Some(Geom::Arc { .. }) if value > 0.0 && value < 360.0 => {},
                Some(Geom::Arc { .. }) => return Err("an arc sweep must be greater than 0° and less than 360°".into()),
                _ => return Err("an angle needs a line or circular arc".into()),
            }
        }
        Ok(())
    }

    /// The points an entity is defined by.
    pub fn ent_points(&self, id: Id) -> Vec<Id> {
        match self.entities.get(&id).map(|e| e.geom) {
            Some(Geom::Line { a, b }) => vec![a, b],
            Some(Geom::Circle { c, .. }) => vec![c],
            Some(Geom::Arc { c, s, e }) => {
                let mut points = vec![c,s,e];
                if let Some(ArcGuide::Through { point }) = self.arc_guides.get(&id) { points.push(*point); }
                points
            },
            Some(Geom::Spline { a, b, c, d }) => vec![a, b, c, d],
            None => vec![],
        }
    }

    /// Centre and radius of a circle or arc.
    pub fn curve(&self, id: Id) -> Option<(DVec2, f64)> {
        match self.entities.get(&id)?.geom {
            Geom::Circle { c, r } => Some((self.pos(c), r)),
            Geom::Arc { c, s, .. } => Some((self.pos(c), self.pos(c).distance(self.pos(s)))),
            Geom::Line { .. } | Geom::Spline { .. } => None,
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
            Geom::Spline { a, b, c, d } => spline_polyline([a,b,c,d].map(|p| self.pos(p))),
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
        self.prune_arc_guides();
    }

    /// The current size of what a dimension on `refs` would measure (mm or degrees).
    pub fn measure(&self, kind: CKind, refs: &[Id]) -> f64 {
        let perp = |p: DVec2, l: (DVec2, DVec2)| ((l.1 - l.0).perp_dot(p - l.0) / (l.1 - l.0).length().max(1e-12)).abs();
        match (kind, refs) {
            (CKind::PositionX, [p]) => self.pos(*p).x,
            (CKind::PositionY, [p]) => self.pos(*p).y,
            (CKind::Radius, [e]) => self.curve(*e).map_or(0.0, |c| c.1),
            (CKind::Diameter, [e]) => self.curve(*e).map_or(0.0, |c| c.1 * 2.0),
            (CKind::Angle, [id]) => match self.entities.get(id).map(|e| e.geom) {
                Some(Geom::Line { a, b }) => (self.pos(b) - self.pos(a)).to_angle().to_degrees(),
                Some(Geom::Arc { .. }) => self.arc_angles(*id).map_or(0.0, |(_, sweep)| sweep.to_degrees()),
                _ => 0.0,
            },
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
            arc_guides: self.arc_guides.iter().filter(|(id,guide)| ents.iter().any(|e| e.0 == **id) && match guide {
                ArcGuide::Through { .. } => true,
                ArcGuide::Tangent { source,.. } => ents.iter().any(|e| e.0 == *source),
            }).map(|(id,guide)| (*id,*guide)).collect(),
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
                Geom::Spline { a,b,c,d } => Geom::Spline { a:m(&map,a),b:m(&map,b),c:m(&map,c),d:m(&map,d) },
            };
            let id = self.add(geom, e.construction);
            map.insert(*old, id);
            out.push(id);
        }
        for (arc,guide) in &clip.arc_guides {
            let guide = match *guide {
                ArcGuide::Through { point } => ArcGuide::Through { point: m(&map,point) },
                ArcGuide::Tangent { source,start } => ArcGuide::Tangent { source:m(&map,source),start:m(&map,start) },
            };
            self.arc_guides.insert(m(&map,*arc),guide);
        }
        // Points that belong to a pasted entity come along with it; the rest
        // were copied on their own and are selected directly.
        let owned: BTreeSet<Id> = out.iter().flat_map(|e| self.ent_points(*e)).collect();
        out.extend(clip.points.iter().map(|(old, _)| map[old]).filter(|p| !owned.contains(p)));
        for c in &clip.constraints {
            let id = self.id();
            let mut value = c.value.clone();
            if let Some(v) = &mut value {
                let shift = match c.kind { CKind::PositionX => offset.x, CKind::PositionY => offset.y, _ => 0.0 };
                if shift != 0.0 { v.expr = format!("({}) + ({shift} mm)", v.expr); v.v += shift; }
            }
            self.constraints.insert(id, Constraint { kind: c.kind, refs: c.refs.iter().map(|r| m(&map, *r)).collect(), value });
        }
        out
    }

    /// Shifts entities and points by `by`. An entity standing on the origin
    /// or on a fixed point is given a point of its own, so that it can leave.
    pub fn translate(&mut self, ids: &[Id], by: DVec2) {
        let stuck = |sk: &Sketch, p: Id| p == ORIGIN || sk.fixed.contains(&p);
        let mut pts: BTreeSet<Id> = ids.iter().copied().filter(|i| self.points.contains_key(i) && !stuck(self, *i)).collect();
        let mut rebound = BTreeMap::new();
        let ids: BTreeSet<Id> = ids.iter().copied().collect();
        for id in &ids {
            for p in self.ent_points(*id) {
                if !stuck(self, p) {
                    pts.insert(p);
                    continue;
                }
                let free = *rebound.entry(p).or_insert_with(|| self.add_point(self.pos(p) + by));
                let swap = |x: &mut Id| {
                    if *x == p {
                        *x = free;
                    }
                };
                if let Some(guide) = self.arc_guides.get_mut(id) {
                    match guide {
                        ArcGuide::Through { point } => swap(point),
                        ArcGuide::Tangent { start,.. } => swap(start),
                    }
                }
                match &mut self.entities.get_mut(id).unwrap().geom {
                    Geom::Line { a, b } => [a, b].into_iter().for_each(swap),
                    Geom::Circle { c, .. } => swap(c),
                    Geom::Arc { c, s, e } => [c, s, e].into_iter().for_each(swap),
                    Geom::Spline { a,b,c,d } => [a,b,c,d].into_iter().for_each(swap),
                }
            }
        }
        self.prune_arc_guides();
        for constraint in self.constraints.values_mut().filter(|c| c.refs.len() == 1 && pts.contains(&c.refs[0])) {
            let delta = match constraint.kind { CKind::PositionX => by.x, CKind::PositionY => by.y, _ => 0.0 };
            if delta != 0.0 && let Some(value) = &mut constraint.value {
                value.expr = format!("({}) + ({delta} mm)", value.expr);
                value.v += delta;
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

/// The circumcentre, translated before solving to avoid cancellation far from the origin.
pub fn arc3_center(start: DVec2, through: DVec2, end: DVec2) -> Result<DVec2, String> {
    if [start, through, end].iter().any(|p| !p.is_finite()) { return Err("arc points must be finite".into()); }
    let (u, v) = (through - start, end - start);
    let cross = u.perp_dot(v);
    if u.length() < 1e-6 || v.length() < 1e-6 || through.distance(end) < 1e-6 || cross.abs() < 1e-9 * u.length() * v.length() {
        return Err("a three-point arc needs three distinct points that are not on one line".into());
    }
    let centre = start + (v.perp() * -u.length_squared() + u.perp() * v.length_squared()) / (2.0 * cross);
    if !centre.is_finite() || !centre.as_vec2().is_finite() { return Err("the arc centre is out of range".into()); }
    Ok(centre)
}

/// Centre of an arc leaving `start` along `tangent` and arriving at `end`.
pub fn tangent_arc_center(start: DVec2, tangent: DVec2, end: DVec2) -> Result<DVec2, String> {
    let t = tangent.try_normalize().filter(|t| t.is_finite()).ok_or("the source has no usable tangent")?;
    let d = end - start;
    let den = 2.0 * d.dot(t.perp());
    if !start.is_finite() || !end.is_finite() || d.length() < 1e-6 || den.abs() < 1e-9 * d.length() {
        return Err("move the arc end away from the start and its tangent line".into());
    }
    let centre = start + t.perp() * (d.length_squared() / den);
    if !centre.is_finite() || !centre.as_vec2().is_finite() { return Err("the arc centre is out of range".into()); }
    Ok(centre)
}

pub(crate) fn validate_spline_points(points: [DVec2; 4]) -> Result<(), String> {
    if points.iter().any(|p| !p.is_finite() || !p.as_vec2().is_finite()) { return Err("spline points must be finite and representable".into()); }
    if points.windows(2).any(|p| p[0].distance(p[1]) < 1e-6) {
        return Err("neighbouring spline fit points must be distinct".into());
    }
    Ok(())
}

fn spline_edge(points: [DVec2; 4]) -> Result<cadrum::Edge, String> {
    validate_spline_points(points)?;
    let points = points.map(|p| cadrum::DVec3::new(p.x, p.y, 0.0));
    cadrum::Edge::bspline(points.iter(), cadrum::BSplineEnd::NotAKnot).map_err(|e| format!("the spline could not be made: {e}"))
}

/// Preview the same interpolating curve used by the solid kernel, with a bounded cache.
/// Invalid transient edits return an empty curve instead of a misleading straight-line substitute.
pub fn spline_polyline(points: [DVec2; 4]) -> Vec<DVec2> {
    type Entry = ([u64; 8], Vec<DVec2>);
    thread_local! { static CACHE: std::cell::RefCell<std::collections::VecDeque<Entry>> = const { std::cell::RefCell::new(std::collections::VecDeque::new()) }; }
    let key = std::array::from_fn(|i| points[i / 2][i % 2].to_bits());
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(i) = cache.iter().position(|e| e.0 == key) {
            let entry = cache.remove(i).unwrap();
            let result = entry.1.clone();
            cache.push_back(entry);
            return result;
        }
        let result: Vec<_> = spline_edge(points).map(|edge| edge.approximation_segments(cadrum::Tessellation {
            deflection_linear: 0.0005, deflection_angular: 0.1, relative_linear: true,
        }).into_iter().map(|p| DVec2::new(p.x, p.y)).collect()).unwrap_or_default();
        if cache.len() == 128 { cache.pop_front(); }
        cache.push_back((key, result.clone()));
        result
    })
}

impl Sketch {
    pub fn add_spline(&mut self, points: [Id; 4], construction: bool) -> Result<Id, String> {
        if points.iter().any(|p| !self.points.contains_key(p)) { return Err("a spline fit point is missing".into()); }
        spline_edge(points.map(|p| self.pos(p)))?;
        let [a,b,c,d] = points;
        Ok(self.add(Geom::Spline { a,b,c,d }, construction))
    }

    pub fn add_arc3(&mut self, start: Id, through: Id, end: Id, construction: bool) -> Result<Id, String> {
        if [start,through,end].iter().any(|p| !self.points.contains_key(p)) { return Err("an arc point is missing".into()); }
        let centre = arc3_center(self.pos(start), self.pos(through), self.pos(end))?;
        let c = self.add_point(centre);
        let (s,e) = if (self.pos(through)-self.pos(start)).perp_dot(self.pos(end)-self.pos(start)) > 0.0 { (start,end) } else { (end,start) };
        let arc = self.add(Geom::Arc { c,s,e }, construction);
        self.arc_guides.insert(arc,ArcGuide::Through { point:through });
        Ok(arc)
    }

    /// Tangent pointing out of an endpoint, extending a line or circular arc.
    pub fn endpoint_tangent(&self, entity: Id, endpoint: Id) -> Option<DVec2> {
        let direction = match self.entities.get(&entity)?.geom {
            Geom::Line { a,b } if endpoint == a => self.pos(a)-self.pos(b),
            Geom::Line { a,b } if endpoint == b => self.pos(b)-self.pos(a),
            Geom::Arc { c,s,.. } if endpoint == s => -(self.pos(s)-self.pos(c)).perp(),
            Geom::Arc { c,e,.. } if endpoint == e => (self.pos(e)-self.pos(c)).perp(),
            _ => return None,
        };
        direction.try_normalize()
    }

    pub fn add_tangent_arc(&mut self, source: Id, start: Id, end: Id, construction: bool) -> Result<Id, String> {
        if !self.points.contains_key(&end) { return Err("the arc end is missing".into()); }
        let tangent = self.endpoint_tangent(source,start).ok_or("choose the endpoint of a line or circular arc")?;
        let centre = tangent_arc_center(self.pos(start),tangent,self.pos(end))?;
        let c = self.add_point(centre);
        let (s,e) = if (self.pos(start)-centre).perp().dot(tangent) > 0.0 { (start,end) } else { (end,start) };
        let arc = self.add(Geom::Arc { c,s,e }, construction);
        self.add_constraint(CKind::Tangent, &[source,arc], None)?;
        self.arc_guides.insert(arc,ArcGuide::Tangent { source,start });
        Ok(arc)
    }

    /// Set signed sketch coordinates as persistent, editable dimensions.
    /// Call inside a document transaction and solve afterwards to reject conflicts atomically.
    pub fn set_point_coordinates(&mut self, point: Id, x: Value, y: Value) -> Result<(), String> {
        if !self.points.contains_key(&point) { return Err("the point is missing".into()); }
        if point == ORIGIN || self.fixed.contains(&point) || self.constraints.values().any(|c| c.kind == CKind::Fix && (c.refs == [point] || c.refs.iter().any(|id| self.ent_points(*id).contains(&point)))) {
            return Err("a fixed point cannot be positioned; remove its Fix constraint first".into());
        }
        let pos = DVec2::new(x.v,y.v);
        if !pos.is_finite() || !pos.as_vec2().is_finite() { return Err("point coordinates must be finite and representable".into()); }
        for (kind,value) in [(CKind::PositionX,x),(CKind::PositionY,y)] {
            if let Some(existing) = self.constraints.values_mut().find(|c| c.kind == kind && c.refs == [point]) { existing.value = Some(value); }
            else { self.add_constraint(kind, &[point], Some(value))?; }
        }
        self.points.insert(point,pos);
        Ok(())
    }

    /// Endpoints of drawn outlines that have only one incident segment.
    /// Coordinate coincidence and explicit point coincidence use the profile closure tolerance.
    pub fn open_endpoints(&self) -> Vec<Id> {
        let nodes = crate::profile::point_nodes(self);
        let mut degree = BTreeMap::<Id, usize>::new();
        let mut endpoint_ids = BTreeSet::new();
        for e in self.entities.values().filter(|e| !e.construction) {
            let pair = match e.geom {
                Geom::Line { a,b } => [a,b], Geom::Arc { s,e,.. } => [s,e], Geom::Spline { a,d,.. } => [a,d],
                Geom::Circle { .. } => continue,
            };
            for p in pair { *degree.entry(nodes[&p]).or_default() += 1; endpoint_ids.insert(p); }
        }
        endpoint_ids.into_iter().filter(|p| degree.get(&nodes[p]) == Some(&1)).collect()
    }
}

impl Sketch {
    /// Drop construction intent whose supporting geometry or tangent constraint was removed.
    pub(crate) fn prune_arc_guides(&mut self) {
        let valid: Vec<Id> = self.arc_guides.iter().filter_map(|(id,guide)| {
            let Some(Geom::Arc { s,e,.. }) = self.entities.get(id).map(|e| e.geom) else { return Some(*id) };
            let valid = match guide {
                ArcGuide::Through { point } => self.points.contains_key(point),
                ArcGuide::Tangent { source,start } => (*start == s || *start == e)
                    && self.endpoint_tangent(*source,*start).is_some()
                    && self.constraints.values().any(|c| c.kind == CKind::Tangent && c.refs.contains(id) && c.refs.contains(source)),
            };
            (!valid).then_some(*id)
        }).collect();
        for id in valid { self.arc_guides.remove(&id); }
    }

    /// Topological order of construction dependencies, without recursive stack growth.
    pub(crate) fn arc_guide_order(&self) -> Result<Vec<Id>, String> {
        let mut ordered = Vec::new();
        let mut done = BTreeSet::new();
        for root in self.arc_guides.keys().copied() {
            let mut path = Vec::new();
            let mut visited = BTreeSet::new();
            let mut at = root;
            while !done.contains(&at) {
                if !visited.insert(at) { return Err("tangent arc guides contain a dependency cycle".into()); }
                path.push(at);
                match self.arc_guides[&at] {
                    ArcGuide::Tangent { source,.. } if self.arc_guides.contains_key(&source) => at = source,
                    _ => break,
                }
            }
            for id in path.into_iter().rev() { done.insert(id); ordered.push(id); }
        }
        Ok(ordered)
    }

    /// Start the nonlinear solve near the analytic construction, especially when a
    /// through point crosses the chord or a tangent end crosses its source line.
    pub(crate) fn seed_guided_arcs(&mut self, fixed: &BTreeSet<Id>) {
        for constraint in self.constraints.values() {
            if let [point] = constraint.refs.as_slice() && !fixed.contains(point)
                && let Some(value) = &constraint.value && let Some(p) = self.points.get_mut(point) {
                match constraint.kind { CKind::PositionX => p.x = value.v, CKind::PositionY => p.y = value.v, _ => {} }
            }
        }
        for arc in self.arc_guide_order().unwrap_or_default() {
            let guide = self.arc_guides[&arc];
            let Some(Geom::Arc { c,s,e }) = self.entities.get(&arc).map(|e| e.geom) else { continue };
            if !fixed.contains(&c) {
                let centre = match guide {
                    ArcGuide::Through { point } => arc3_center(self.pos(s),self.pos(point),self.pos(e)).ok(),
                    ArcGuide::Tangent { source,start } => self.endpoint_tangent(source,start).and_then(|t| tangent_arc_center(self.pos(start),t,self.pos(if start == s { e } else { s })).ok()),
                };
                if let Some(centre) = centre { self.points.insert(c,centre); }
            }
            self.orient_guided_arc(arc);
        }
    }

    fn orient_guided_arc(&mut self, id: Id) {
        let Some(Geom::Arc { c,s,e }) = self.entities.get(&id).map(|e| e.geom) else { return };
        let flip = match self.arc_guides[&id] {
            ArcGuide::Through { point } => (self.pos(point)-self.pos(s)).perp_dot(self.pos(e)-self.pos(s)) < 0.0,
            ArcGuide::Tangent { source,start } => self.endpoint_tangent(source,start).zip(self.endpoint_tangent(id,start)).is_some_and(|(a,b)| a.dot(b) > 0.0),
        };
        if flip { self.entities.get_mut(&id).unwrap().geom = Geom::Arc { c,s:e,e:s }; }
    }

    /// Arc storage is CCW; keep the intended side after fit-point or tangent-end edits.
    pub(crate) fn orient_guided_arcs(&mut self) {
        for id in self.arc_guide_order().unwrap_or_default() { self.orient_guided_arc(id); }
    }
}
