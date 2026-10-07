//! Invariants checked before native documents or clipboard geometry reach the solver.
use std::collections::BTreeSet;

use crate::doc::{Document, FeatureKind};
use crate::sketch::{Clip, Geom, Id, ORIGIN, Plane, Sketch};

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
        validate_contents(self)
    }
}

fn validate_contents(s: &Sketch) -> Result<(), String> {
    if s.points.len() + s.entities.len() + s.constraints.len() > MAX_SKETCH_ITEMS {
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
    for (&id, c) in &s.constraints {
        let canonical = s.normalize(c.kind, &c.refs).map_err(|e| format!("constraint {id}: {e}"))?;
        if canonical != c.refs { return Err(format!("constraint {id} has references in the wrong order")); }
        if c.kind.value_kind().is_some() != c.value.is_some() {
            return Err(format!("constraint {id} has a missing or unexpected dimension value"));
        }
        if let Some(v) = &c.value {
            expression(&v.expr)?;
            if !v.v.is_finite() { return Err(format!("constraint {id} has a non-finite value")); }
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
        if self.points.len() + self.entities.len() + self.constraints.len() > MAX_SKETCH_ITEMS {
            return Err("the clipboard has too many sketch items".into());
        }
        let mut ids = BTreeSet::new();
        for id in self.points.iter().map(|p|p.0).chain(self.entities.iter().map(|e|e.0)) {
            if !ids.insert(id) { return Err(format!("duplicate clipboard id {id}")); }
        }
        let next = ids.last().copied().unwrap_or(0).checked_add(1).ok_or("clipboard ids are out of range")?;
        let mut s = Sketch { plane: Plane::XY, points: self.points.iter().copied().collect(), entities: self.entities.iter().copied().collect(), constraints: Default::default(), next, visible: true, fixed: Default::default() };
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
    let mut names = BTreeSet::new();
    for p in &d.params {
        if !crate::expr::valid_name(&p.name) || !names.insert(&p.name) { return Err("parameter names must be valid and unique".into()); }
        expression(&p.expr)?;
    }
    let mut ids = BTreeSet::new();
    for f in &d.features {
        if f.id == 0 || !ids.insert(f.id) || f.id >= d.next_id { return Err("feature ids must be unique and below the next id".into()); }
        match &f.kind {
            FeatureKind::Sketch(s) => s.validate().map_err(|e|format!("sketch {}: {e}",f.id))?,
            FeatureKind::Import(m) => m.validate()?,
            _ => {}
        }
    }
    // Pattern bodies reserve ids at feature_id * 1000 + copy_index.
    if d.next_id == 0 || d.next_id > Id::MAX / 1000 - MAX_FEATURES as Id {
        return Err("the document id range is exhausted".into());
    }
    Ok(())
}
