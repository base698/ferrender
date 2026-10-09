use super::*;
use crate::planes::{OriginPlane, PlaneRef, PointRef, PlaneKind};

fn face_ref(s: &Session, v: &J) -> R<PlaneRef> {
    let body=id_of(v,"body")?;
    let at=xyz(&v["point"])?*s.doc.units.mm();
    let b=s.built.body(body).ok_or("the selected body does not exist")?;
    let face=crate::planes::face(b,at,None)?;
    Ok(PlaneRef::Face { body, at: b.to_local(face.at), frame: s.built.frame(body), tag: None })
}

pub(super) fn base(s: &Session, v: &J) -> R<PlaneRef> {
    if let Some(word)=v.as_str() { return match word.to_ascii_uppercase().as_str() {
        "XY"=>Ok(PlaneRef::Origin(OriginPlane::XY)), "XZ"=>Ok(PlaneRef::Origin(OriginPlane::XZ)), "YZ"=>Ok(PlaneRef::Origin(OriginPlane::YZ)),
        _=>Err("the base should be XY, XZ, YZ, a face, or a construction plane".into())
    }; }
    if !v["face"].is_null() { return face_ref(s,&v["face"]); }
    let id=id_of(v,"plane")?;
    if !s.built.planes.contains_key(&id) { return Err("the construction plane is not available at this history position".into()); }
    Ok(PlaneRef::Plane(id))
}

pub(super) fn update(s: &Session, c: &J, old: Option<&PlaneKind>) -> R<(PlaneKind,Vec<crate::doc::Param>)> {
    let mut probe=s.doc.clone();
    let name=c["kind"].as_str().or_else(||old.map(|p|match p {PlaneKind::Offset {..}=>"offset",PlaneKind::Midplane {..}=>"midplane",PlaneKind::ThreePoint {..}=>"three_point"})).ok_or("choose offset, midplane, or three_point")?;
    let kind=match name {
        "offset"=> {
            let previous=match old {Some(PlaneKind::Offset {base,distance})=>Some((base,distance)),_=>None};
            let base=if c["base"].is_null() {previous.map(|p|p.0.clone()).ok_or("an offset plane needs a base")?} else {base(s,&c["base"])?};
            let distance=if c["distance"].is_null() {previous.map(|p|p.1.clone()).ok_or("an offset plane needs a distance")?} else {probe.enter(&text_of(&c["distance"])?,Kind::Length)?};
            PlaneKind::Offset {base,distance}
        }
        "midplane"=> {
            let previous=match old {Some(PlaneKind::Midplane {a,b,flip})=>Some((a,b,*flip)),_=>None};
            let (a,b)=if c["faces"].is_null() {
                let p=previous.ok_or("a midplane needs two faces")?; (p.0.clone(),p.1.clone())
            } else {
                let faces=c["faces"].as_array().filter(|a|a.len()==2).ok_or("a midplane needs exactly two faces")?;
                (face_ref(s,&faces[0])?,face_ref(s,&faces[1])?)
            };
            PlaneKind::Midplane {a,b,flip:c["flip"].as_bool().unwrap_or(previous.is_some_and(|p|p.2))}
        }
        "three_point"=> {
            if c["points"].is_null() && let Some(PlaneKind::ThreePoint {points})=old { return Ok((PlaneKind::ThreePoint {points:points.clone()},probe.params)); }
            let list=c["points"].as_array().filter(|p|p.len()==3).ok_or("a plane needs exactly three points")?;
            let points=list.iter().map(|p| -> R<PointRef> {
                if let Some(a)=p.as_array().filter(|a|a.len()==3) { return Ok(PointRef::World(DVec3::new(mm(&probe,&a[0])?,mm(&probe,&a[1])?,mm(&probe,&a[2])?))); }
                if !p["sketch"].is_null() {
                    let (sketch,point)=(id_of(p,"sketch")?,id_of(p,"point")?);
                    if !s.doc.sketch(sketch).is_some_and(|s|s.points.contains_key(&point)) {return Err("the sketch point does not exist".into());}
                    return Ok(PointRef::SketchPoint {sketch,point});
                }
                let body=id_of(p,"body")?;
                let at=xyz(&p["point"])?*s.doc.units.mm();
                if s.built.body(body).is_none() {return Err("the vertex's body does not exist".into());}
                Ok(PointRef::Vertex {body,at:s.built.body(body).unwrap().to_local(at),frame:s.built.frame(body)})
            }).collect::<R<Vec<_>>>()?;
            PlaneKind::ThreePoint {points:points.try_into().unwrap()}
        }
        _=>return Err("choose offset, midplane, or three_point".into()),
    };
    Ok((kind,probe.params))
}

pub(super) fn info(s: &Session, id: Id, o: &mut J) {
    if let Some(FeatureKind::Plane(p))=s.doc.feature(id).map(|f|&f.kind) {
        o["inputs"]=json!(p.kind);
        o["visible"]=json!(p.visible);
        o["kind"]=json!(match p.kind {PlaneKind::Offset {..}=>"offset",PlaneKind::Midplane {..}=>"midplane",PlaneKind::ThreePoint {..}=>"three_point"});
    }
    if let Some(p)=s.built.planes.get(&id) {
        o["origin"]=json!((p.plane.origin/s.doc.units.mm()).to_array());
        o["x"]=json!(p.plane.x.to_array()); o["y"]=json!(p.plane.y.to_array()); o["normal"]=json!(p.plane.normal().to_array());
    }
}
