use super::*;
use crate::body_ops::{Remove,Split};

fn targets(s: &Session,c: &J) -> R<Vec<Id>> {
    if !c["bodies"].is_null() && !c["body"].is_null() {return Err("use bodies or body, not both".into());}
    let bodies=if c["bodies"].is_null() {vec![id_of(c,"body")?]} else {ids_of(c,"bodies")?};
    Remove {bodies:bodies.clone()}.validate()?;
    for id in &bodies {if s.built.body(*id).is_none() {return Err(format!("body {id} is not available at this history position"));}}
    Ok(bodies)
}

fn split(s: &Session,c: &J,old: Option<&Split>) -> R<Split> {
    let body=if c["body"].is_null() {old.map(|s|s.body).ok_or("split_body needs a body")?} else {id_of(c,"body")?};
    if s.built.body(body).is_none() {return Err("the body to split is not available at this history position".into());}
    let plane=if c["plane"].is_null() {old.map(|s|s.plane.clone()).ok_or("split_body needs a plane")?} else {planes_api::base(s,&c["plane"])?};
    let split=Split {body,plane};split.validate()?;Ok(split)
}

pub(super) fn create(s: &mut Session,c: &J) -> R<Id> {
    let kind=if c["op"]=="remove_body" {FeatureKind::Remove(Remove {bodies:targets(s,c)?})} else {FeatureKind::Split(split(s,c,None)?)};
    s.edit_feature(|d| {
        let id=d.add_feature(kind);
        crate::validation::document(d)?;
        Ok((id,id))
    })
}

/// Parse picks against the model before the existing operation. The original
/// body may be absent (Remove) or already split in the current model.
pub(super) fn update(s: &Session,id:Id,c:&J) -> R<Option<FeatureKind>> {
    let Some(feature)=s.doc.feature(id) else {return Ok(None)};
    let geometry=match &feature.kind {
        FeatureKind::Remove(_)=>["body","bodies"].iter().any(|k|!c[*k].is_null()),
        FeatureKind::Split(_)=>["body","plane"].iter().any(|k|!c[*k].is_null()),
        _=>false,
    };
    if !geometry {return Ok(None);}
    let index=s.doc.features.iter().position(|f|f.id==id).unwrap();
    let mut prefix=s.doc.clone();prefix.roll_to(index);
    let prefix=Session::new(prefix);
    let kind=match &feature.kind {
        FeatureKind::Remove(_)=>FeatureKind::Remove(Remove {bodies:targets(&prefix,c)?}),
        FeatureKind::Split(old)=>FeatureKind::Split(split(&prefix,c,Some(old))?),
        _=>unreachable!(),
    };
    let mut trial=s.doc.clone();
    let changed=trial.feature_mut(id).unwrap();changed.kind=kind.clone();changed.suppressed=false;
    trial.roll_to(index+1);
    crate::validation::document(&trial)?;
    let built=trial.rebuild();
    if !built.components.contains_key(&feature.owner) {return Err("unsuppress the operation's component before editing its targets".into());}
    if let Some(error)=built.errors.get(&id) {return Err(error.clone());}
    Ok(Some(kind))
}

pub(super) fn info(s: &Session,id:Id,kind:&FeatureKind,out:&mut J) {
    match kind {
        FeatureKind::Remove(r)=>out["bodies"]=json!(r.bodies),
        FeatureKind::Split(split)=>{
            out["body"]=json!(split.body);out["plane"]=json!(split.plane);
            let applied=s.doc.features.iter().take(s.doc.active()).any(|f|f.id==id && !f.suppressed && s.built.components.contains_key(&f.owner)) && !s.built.errors.contains_key(&id);
            out["pieces"]=json!(s.built.bodies.iter().filter(|b|applied && (b.id==split.body || (b.id>id*1000 && b.id<id*1000+1000))).map(|b|b.id).collect::<Vec<_>>());
        }
        _=>{},
    }
}
