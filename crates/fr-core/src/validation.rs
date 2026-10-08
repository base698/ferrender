//! Invariants checked before native documents or clipboard geometry reach the solver.
use std::collections::BTreeSet;

use crate::doc::{Document, FeatureKind};
use crate::sketch::{ArcGuide, CKind, Clip, Geom, Id, ORIGIN, Plane, Sketch};

const MAX_FEATURES: usize = 10_000;
const MAX_SKETCH_ITEMS: usize = 10_000;
const MAX_EXPR_BYTES: usize = 4096;

fn expression(text: &str) -> Result<(), String> {
    if text.len() > MAX_EXPR_BYTES { return Err("an expression is too long (maximum 4096 bytes)".into()); }
    Ok(())
}

impl Sketch {
    /// Validates references and types without solving or changing geometry.
    pub fn validate(&self) -> Result<(), String> {
        let p = self.plane;
        if !p.origin.is_finite() || !p.x.is_finite() || !p.y.is_finite()
            || (p.x.length_squared() - 1.0).abs() > 1e-6
            || (p.y.length_squared() - 1.0).abs() > 1e-6 || p.x.dot(p.y).abs() > 1e-6
        { return Err("the sketch plane needs finite, perpendicular unit axes".into()); }
        if self.points.get(&ORIGIN).is_none_or(|p| *p != glam::DVec2::ZERO) {
            return Err("the sketch is missing its fixed origin".into());
        }
        if let Some(reference) = &self.reference { reference.validate()?; }
        validate_contents(self)
    }
}

fn validate_contents(s: &Sketch) -> Result<(), String> {
    if s.points.len() + s.entities.len() + s.constraints.len() + s.arc_guides.len() > MAX_SKETCH_ITEMS {
        return Err("the sketch has too many items (maximum 10000)".into());
    }
    let mut ids = BTreeSet::new();
    for &id in s.points.keys().chain(s.entities.keys()).chain(s.constraints.keys()) {
        if !ids.insert(id) { return Err(format!("duplicate sketch id {id}")); }
        if id >= s.next { return Err("the sketch's next id would overwrite an existing item".into()); }
    }
    if s.next > Id::MAX - MAX_SKETCH_ITEMS as Id { return Err("the sketch id range is exhausted".into()); }
    if s.points.values().any(|p| !p.is_finite() || !p.as_vec2().is_finite()) {
        return Err("sketch coordinates must be finite and representable".into());
    }
    for (&id, ent) in &s.entities {
        if s.ent_points(id).iter().any(|p| !s.points.contains_key(p)) {
            return Err(format!("entity {id} refers to a missing point"));
        }
        if let Geom::Circle { r, .. } = ent.geom
            && (!r.is_finite() || r <= 0.0 || !(r as f32).is_finite())
        { return Err(format!("circle {id} needs a finite positive radius")); }
    }
    for (&id, ent) in &s.entities {
        if let Geom::Spline { a,b,c,d } = ent.geom {
            crate::sketch::validate_spline_points([a,b,c,d].map(|p| s.pos(p))).map_err(|e| format!("spline {id}: {e}"))?;
        }
    }
    s.arc_guide_order()?;
    for (&id,guide) in &s.arc_guides {
        let Some(Geom::Arc { c,s:start,e:end }) = s.entities.get(&id).map(|e| e.geom) else { return Err("an arc guide refers to a missing arc".into()) };
        match *guide {
            ArcGuide::Through { point } => {
                if !s.points.contains_key(&point) || [c,start,end].contains(&point) { return Err("a three-point arc needs its own through point".into()); }
                crate::sketch::arc3_center(s.pos(start),s.pos(point),s.pos(end))?;
            }
            ArcGuide::Tangent { source,start:point } => {
                if source == id || ![start,end].contains(&point) || s.endpoint_tangent(source,point).is_none()
                    || !s.constraints.values().any(|c| c.kind == CKind::Tangent && c.refs.contains(&id) && c.refs.contains(&source)) {
                    return Err("a tangent arc guide needs its source endpoint and tangent constraint".into());
                }
            }
        }
    }
    for (&id, c) in &s.constraints {
        let canonical = s.normalize(c.kind, &c.refs).map_err(|e| format!("constraint {id}: {e}"))?;
        if canonical != c.refs { return Err(format!("constraint {id} has references in the wrong order")); }
        if c.kind.value_kind().is_some() != c.value.is_some() {
            return Err(format!("constraint {id} has a missing or unexpected dimension value"));
        }
        if let Some(v) = &c.value {
            expression(&v.expr)?;
            if !v.v.is_finite() { return Err(format!("constraint {id} has a non-finite value")); }
            s.validate_dimension(c.kind, &c.refs, v.v).map_err(|e| format!("constraint {id}: {e}"))?;
        }
    }
    if s.fixed.iter().any(|id| !s.points.contains_key(id) && !matches!(s.entities.get(id).map(|e|e.geom), Some(Geom::Circle { .. }))) {
        return Err("a fixed sketch reference is missing or is not a point or circle".into());
    }
    Ok(())
}

impl Clip {
    /// Clipboard data is untrusted, even when it carries Ferrender's marker.
    pub fn validate(&self) -> Result<(), String> {
        if self.points.len() + self.entities.len() + self.constraints.len() + self.arc_guides.len() > MAX_SKETCH_ITEMS {
            return Err("the clipboard has too many sketch items".into());
        }
        let mut ids = BTreeSet::new();
        for id in self.points.iter().map(|p|p.0).chain(self.entities.iter().map(|e|e.0)) {
            if !ids.insert(id) { return Err(format!("duplicate clipboard id {id}")); }
        }
        let next = ids.last().copied().unwrap_or(0).checked_add(1).ok_or("clipboard ids are out of range")?;
        let mut s = Sketch { plane: Plane::XY, on: None, points: self.points.iter().copied().collect(), entities: self.entities.iter().copied().collect(), constraints: Default::default(), next, visible: true, fixed: Default::default(), reference: None, arc_guides: self.arc_guides.clone() };
        for c in &self.constraints {
            let id = s.next;
            s.next = s.next.checked_add(1).ok_or("clipboard ids are out of range")?;
            s.constraints.insert(id, c.clone());
        }
        validate_contents(&s)
    }
}

/// Broken feature dependencies remain loadable so they can be repaired in the timeline.
/// Structural invariants needed for safe solver/kernel access must hold first.
pub fn document(d: &Document) -> Result<(), String> {
    if d.features.len() > MAX_FEATURES || d.params.len() > 1024 {
        return Err("the document exceeds the feature or parameter limit".into());
    }
    // Bound aggregate decoded image memory before validating (and decoding) any image.
    let mut reference_pixels = 0u64;
    for f in &d.features {
        if let FeatureKind::Sketch(sketch) = &f.kind && let Some(image) = &sketch.reference {
            reference_pixels += u64::from(image.pixel_width) * u64::from(image.pixel_height);
            if reference_pixels > 16 * 1024 * 1024 { return Err("reference images exceed the document limit of 16 megapixels".into()); }
        }
    }
    let mut names = BTreeSet::new();
    for p in &d.params {
        if !crate::expr::valid_name(&p.name) || !names.insert(&p.name) { return Err("parameter names must be valid and unique".into()); }
        expression(&p.expr)?;
    }
    let mut ids = BTreeSet::new();
    let mut depths = std::collections::BTreeMap::from([(0,0usize)]);
    for f in &d.features {
        if f.id == 0 || !ids.insert(f.id) || f.id >= d.next_id { return Err("feature ids must be unique and below the next id".into()); }
        let depth=*depths.get(&f.owner).ok_or("each feature owner must be root or an earlier component")?;
        if let FeatureKind::Component(c)=&f.kind {
            if depth>=32 {return Err("components can nest at most 32 levels deep".into());}
            depths.insert(f.id,depth+1);
            for v in c.placement.translate.iter().chain(&c.placement.rotate) {
                expression(&v.expr)?;
                if !v.v.is_finite() || !(v.v as f32).is_finite() {return Err("component placement values must be finite and representable".into());}
            }
        }
        plane_dependencies(d,f)?;
        match &f.kind {
            FeatureKind::Sketch(s) => s.validate().map_err(|e|format!("sketch {}: {e}",f.id))?,
            FeatureKind::Import(m) => m.validate()?,
            FeatureKind::Text(t) => text(t)?,
            FeatureKind::Pattern(p) => p.validate()?,
            FeatureKind::Primitive(p) => p.validate()?,
            _ => {}
        }
    }
    if !depths.contains_key(&d.active_component) {return Err("the active component does not exist".into());}
    for f in d.features.iter().filter(|f|matches!(f.kind,FeatureKind::Pattern(_))) {
        if let Some(start)=f.id.checked_mul(1000) {
            if ids.range(start.saturating_add(1)..=start.saturating_add(999)).next().is_some() {
                return Err("a feature id collides with a pattern's reserved body ids".into());
            }
        } else {return Err("pattern body ids are out of range".into());}
    }
    // Pattern bodies reserve ids at feature_id * 1000 + copy_index.
    if d.next_id == 0 || d.next_id > Id::MAX / 1000 - MAX_FEATURES as Id {
        return Err("the document id range is exhausted".into());
    }
    Ok(())
}

fn plane_dependencies(d: &Document, f: &crate::Feature) -> Result<(),String> {
    use crate::planes::{PlaneRef,PointRef,PlaneKind};
    let position=d.features.iter().position(|other|other.id==f.id).unwrap();
    let earlier=|id:Id,what:&str,accept:fn(&FeatureKind)->bool| -> Result<(),String> {
        if let Some((index,feature))=d.features.iter().enumerate().find(|(_,feature)|feature.id==id) {
            if index>=position || !accept(&feature.kind) {return Err(format!("{} {id} must name an earlier {what}",f.name));}
        }
        // Missing dependencies remain repairable timeline errors after deletion.
        Ok(())
    };
    let anchor=|at:glam::DVec3,frame:Option<[glam::DVec3;2]>| -> Result<(),String> {
        if !at.is_finite() || !at.as_vec3().is_finite() || frame.is_some_and(|f| f.iter().any(|p|!p.is_finite()) || f[0].cmpgt(f[1]).any()) {
            return Err("plane references must be finite and have valid bounds".into());
        }
        Ok(())
    };
    let body=|id:Id| -> Result<(),String> {
        if d.feature(id).is_some() { earlier(id,"body-making feature",|k|matches!(k,FeatureKind::Extrude(_)|FeatureKind::Revolve(_)|FeatureKind::Primitive(_)|FeatureKind::Import(_)|FeatureKind::Text(_))) }
        else if id>=1000 && d.feature(id/1000).is_some() {earlier(id/1000,"pattern",|k|matches!(k,FeatureKind::Pattern(_)))}
        else {Ok(())}
    };
    let plane=|r:&PlaneRef| -> Result<(),String> {match r {
        PlaneRef::Origin(_)=>Ok(()),
        PlaneRef::Plane(id)=>earlier(*id,"construction plane",|k|matches!(k,FeatureKind::Plane(_))),
        PlaneRef::Face {body:id,at,frame}=>{body(*id)?;anchor(*at,*frame)}
    }};
    match &f.kind {
        FeatureKind::Sketch(s)=>{if let Some(id)=s.on {earlier(id,"construction plane",|k|matches!(k,FeatureKind::Plane(_)))?;}},
        FeatureKind::Plane(p)=>match &p.kind {
            PlaneKind::Offset {base,distance}=>{
                plane(base)?; expression(&distance.expr)?;
                if !distance.v.is_finite() || !(distance.v as f32).is_finite() {return Err("plane distance must be finite and representable".into());}
            }
            PlaneKind::Midplane {a,b,..}=>{plane(a)?;plane(b)?;}
            PlaneKind::ThreePoint {points}=>for p in points {match p {
                PointRef::World(p)=>anchor(*p,None)?,
                PointRef::Vertex {body:id,at,frame}=>{body(*id)?;anchor(*at,*frame)?;}
                PointRef::SketchPoint {sketch,point}=>{
                    earlier(*sketch,"sketch",|k|matches!(k,FeatureKind::Sketch(_)))?;
                    if d.sketch(*sketch).is_some_and(|s|!s.points.contains_key(point)) {return Err("a plane refers to a missing sketch point".into());}
                }
            }},
        },
        _=>{}
    }
    Ok(())
}

/// Bounds for text before the font parser or solid kernel is entered.
pub fn text(t: &crate::doc::Text) -> Result<(), String> {
    if t.text.chars().count() > 128 { return Err("text is limited to 128 characters".into()); }
    Sketch::new(t.plane).validate().map_err(|_| "the text plane needs finite, perpendicular unit axes")?;
    for v in [&t.height, &t.depth, &t.spacing, &t.angle, &t.x, &t.y] {
        expression(&v.expr)?;
        if !v.v.is_finite() || !(v.v as f32).is_finite() { return Err("text dimensions must be finite and representable".into()); }
    }
    if t.face.is_some_and(|p| !p.is_finite()) || t.frame.is_some_and(|f| f.iter().any(|p| !p.is_finite()) || f[0].cmpgt(f[1]).any()) {
        return Err("text face references must be finite and have valid bounds".into());
    }
    match t.op {
        crate::doc::Op::New if t.body.is_none() && t.face.is_none() && t.frame.is_none() => Ok(()),
        crate::doc::Op::Join | crate::doc::Op::Cut if t.body.is_some() && t.face.is_some() => Ok(()),
        _ => Err("text needs either New Body with a plane, or Raise/Engrave with a body and flat face".into()),
    }
}
