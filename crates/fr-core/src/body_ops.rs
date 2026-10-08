//! Body operations that preserve their source features in the timeline.
use serde::{Deserialize, Serialize};
use crate::{Body, Id, Plane, Sketch};
use crate::planes::PlaneRef;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Remove { pub bodies: Vec<Id> }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Split { pub body: Id, pub plane: PlaneRef }

impl Remove {
    pub fn validate(&self) -> Result<(),String> {
        if self.bodies.is_empty() || self.bodies.len() > 1000 { return Err("select between 1 and 1000 bodies to remove".into()); }
        let mut ids=std::collections::BTreeSet::new();
        if self.bodies.iter().any(|id|*id==0 || !ids.insert(*id)) { return Err("remove needs distinct, nonzero body IDs".into()); }
        Ok(())
    }
}
impl Split {
    pub fn validate(&self) -> Result<(),String> {
        if self.body==0 {return Err("select a body to split".into());}
        match &self.plane {
            PlaneRef::Origin(_) => {},
            PlaneRef::Plane(id) if *id!=0 => {},
            PlaneRef::Face {body,at,frame} if *body!=0 && at.is_finite() && at.as_vec3().is_finite()
                && frame.is_none_or(|f|f.iter().all(|p|p.is_finite() && p.as_vec3().is_finite()) && f[0].cmple(f[1]).all()) => {},
            _ => return Err("the splitting plane reference is invalid".into()),
        }
        Ok(())
    }
}

/// Negative-side pieces come first, then positive-side pieces, with each side
/// sorted geometrically for stable IDs across repeated builds and round trips.
pub(crate) fn pieces(body: &Body, plane: Plane) -> Result<crate::exact::Lumps,String> {
    if !body.is_exact() { return Err("Split needs an exact body; imported or tapered mesh bodies cannot be split yet".into()); }
    if !body.threads.is_empty() { return Err("Split cannot preserve modeled threads yet; split before adding the threads".into()); }
    Sketch::new(plane).validate()?;
    let original:f64=body.solids.iter().map(|s|s.volume()).sum();
    if !original.is_finite() || original<=0. {return Err("the selected body has no solid volume".into());}
    let vector=|p:glam::DVec3|cadrum::DVec3::from_array(p.to_array());
    let halves=[-plane.normal(),plane.normal()].map(|n|cadrum::Solid::half_space(vector(plane.origin),vector(n)));
    let mut sides:[Vec<cadrum::Solid>;2]=[Vec::new(),Vec::new()];
    let mut cut_material=false;
    for solid in &body.solids {
        let mut amounts=[0.;2];
        for (side,half) in halves.iter().enumerate() {
            let expression:cadrum::Boolean<cadrum::Solid>=solid.into();
            let made=match (expression*half).build_vec() {
                Ok(v)=>v,
                Err(cadrum::Error::NotOne(0))=>Vec::new(),
                Err(e)=>return Err(format!("the kernel could not split the body: {e}")),
            };
            for piece in made {
                let volume=piece.volume();
                if !volume.is_finite() || volume<=0. {return Err("the kernel returned an invalid split piece".into());}
                amounts[side]+=volume;
                sides[side].push(piece);
            }
            if sides[0].len()+sides[1].len()>1000 {return Err("a split can produce at most 1000 pieces".into());}
        }
        let minimum=(solid.volume()*1e-8).max(1e-12);
        cut_material|=amounts.iter().all(|v|*v>minimum);
    }
    let total:f64=sides.iter().flatten().map(|s|s.volume()).sum();
    if !cut_material || sides.iter().any(Vec::is_empty) {return Err("the plane does not cut through the body's material; choose a plane inside the body".into());}
    if !total.is_finite() || (total-original).abs()>original*1e-6 {
        return Err("the kernel did not preserve the body's volume when splitting".into());
    }
    for side in &mut sides {
        side.sort_by(|a,b| {
            let a=a.bounding_box(); let b=b.bounding_box();
            let a=(a[0]+a[1])*0.5;let b=(b[0]+b[1])*0.5;
            a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).then(a.z.total_cmp(&b.z))
        });
    }
    Ok(sides.into_iter().flatten().collect())
}
