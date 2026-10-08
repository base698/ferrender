//! Native, parameterized solids in their owning component's local frame.
use serde::{Deserialize, Serialize};
use crate::{Kind, Op, Value};

const MIN_SIZE: f64 = 0.001;
const MAX_SIZE: f64 = 10_000.;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveShape {
    Box { width: Value, depth: Value, height: Value },
    Cylinder { diameter: Value, height: Value },
    Sphere { diameter: Value },
    Cone { bottom_diameter: Value, top_diameter: Value, height: Value },
    Torus { major_radius: Value, tube_radius: Value },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Primitive {
    pub shape: PrimitiveShape,
    pub position: [Value;3],
    pub rotate: [Value;3],
    pub op: Op,
}

impl PrimitiveShape {
    pub fn label(&self) -> &'static str {
        match self { Self::Box {..} => "Box", Self::Cylinder {..} => "Cylinder", Self::Sphere {..} => "Sphere", Self::Cone {..} => "Cone", Self::Torus {..} => "Torus" }
    }
    pub fn name(&self) -> &'static str {
        match self { Self::Box {..} => "box", Self::Cylinder {..} => "cylinder", Self::Sphere {..} => "sphere", Self::Cone {..} => "cone", Self::Torus {..} => "torus" }
    }
    pub fn dimensions(&self) -> Vec<(&'static str, &Value)> {
        match self {
            Self::Box {width,depth,height} => vec![("width",width),("depth",depth),("height",height)],
            Self::Cylinder {diameter,height} => vec![("diameter",diameter),("height",height)],
            Self::Sphere {diameter} => vec![("diameter",diameter)],
            Self::Cone {bottom_diameter,top_diameter,height} => vec![("bottom_diameter",bottom_diameter),("top_diameter",top_diameter),("height",height)],
            Self::Torus {major_radius,tube_radius} => vec![("major_radius",major_radius),("tube_radius",tube_radius)],
        }
    }
    pub(crate) fn dimensions_mut(&mut self) -> Vec<(&'static str, &mut Value)> {
        match self {
            Self::Box {width,depth,height} => vec![("width",width),("depth",depth),("height",height)],
            Self::Cylinder {diameter,height} => vec![("diameter",diameter),("height",height)],
            Self::Sphere {diameter} => vec![("diameter",diameter)],
            Self::Cone {bottom_diameter,top_diameter,height} => vec![("bottom_diameter",bottom_diameter),("top_diameter",top_diameter),("height",height)],
            Self::Torus {major_radius,tube_radius} => vec![("major_radius",major_radius),("tube_radius",tube_radius)],
        }
    }
}

impl Primitive {
    /// Bounds are checked before calling the kernel's infallible constructors.
    pub fn validate(&self) -> Result<(),String> {
        let value = |v: &Value| {
            if v.expr.len() > 4096 { return Err("an expression is too long (maximum 4096 bytes)".to_owned()); }
            if !v.v.is_finite() { return Err("primitive values must be finite".to_owned()); }
            Ok(())
        };
        for (key,v) in self.shape.dimensions() {
            value(v)?;
            let tip = matches!(self.shape,PrimitiveShape::Cone {..}) && matches!(key,"bottom_diameter"|"top_diameter") && v.v == 0.;
            if !tip && !(MIN_SIZE..=MAX_SIZE).contains(&v.v) {
                return Err(format!("{key} must be between 0.001 and 10000 mm"));
            }
        }
        match &self.shape {
            PrimitiveShape::Cone {bottom_diameter,top_diameter,..} => {
                if bottom_diameter.v == 0. && top_diameter.v == 0. { return Err("a cone needs at least one nonzero diameter".into()); }
                let difference = (bottom_diameter.v - top_diameter.v).abs();
                if difference > 0. && difference <= 2e-7 { return Err("cone diameters are too close; use exactly equal diameters for a cylinder".into()); }
            }
            PrimitiveShape::Torus {major_radius,tube_radius} if major_radius.v < tube_radius.v + MIN_SIZE => {
                return Err("the major radius must exceed the tube radius by at least 0.001 mm".into());
            }
            _ => {}
        }
        for v in &self.position { value(v)?; if v.v.abs() > 1_000_000. { return Err("primitive position must be within 1000000 mm of the origin".into()); } }
        for v in &self.rotate { value(v)?; }
        Ok(())
    }

    pub(crate) fn evaluate(&mut self, mut set: impl FnMut(&mut Value, Kind)) {
        for (_,v) in self.shape.dimensions_mut() { set(v,Kind::Length); }
        for v in &mut self.position { set(v,Kind::Length); }
        for v in &mut self.rotate { set(v,Kind::Angle); }
    }

    pub(crate) fn solids(&self) -> Result<crate::exact::Lumps,String> {
        self.validate()?;
        use cadrum::{DVec3, Solid};
        use std::f64::consts::PI;
        let (solid, expected) = match &self.shape {
            PrimitiveShape::Box {width,depth,height} => (Solid::cube(DVec3::ZERO,DVec3::new(width.v,depth.v,height.v)), width.v * depth.v * height.v),
            PrimitiveShape::Cylinder {diameter,height} => (Solid::cylinder(diameter.v/2.,DVec3::Z*height.v), PI*diameter.v.powi(2)*height.v/4.),
            PrimitiveShape::Sphere {diameter} => (Solid::sphere(diameter.v/2.), PI*diameter.v.powi(3)/6.),
            PrimitiveShape::Cone {bottom_diameter,top_diameter,height} => {
                let (a,b,h) = (bottom_diameter.v/2.,top_diameter.v/2.,height.v);
                (if a == b { Solid::cylinder(a,DVec3::Z*h) } else { Solid::cone(a,b,DVec3::Z*h) }, PI*h*(a*a+a*b+b*b)/3.)
            }
            PrimitiveShape::Torus {major_radius,tube_radius} => (Solid::torus(major_radius.v,tube_radius.v,DVec3::Z), 2.*PI*PI*major_radius.v*tube_radius.v.powi(2)),
        };
        let volume = solid.volume();
        if !volume.is_finite() || volume <= 0. || (volume-expected).abs() > expected*1e-6 {
            return Err("the kernel did not produce the expected primitive volume".into());
        }
        let placement = crate::components::Placement {translate:self.position.clone(),rotate:self.rotate.clone()};
        let mut solids = vec![solid];
        for step in crate::components::steps(placement.affine()) { solids = crate::exact::place(solids,&step); }
        Ok(solids)
    }
}
