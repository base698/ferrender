use super::*;
use crate::components::Placement;

pub(super) fn node(s:&Session,id:Id)->J {
    let component=s.doc.feature(id).and_then(|f|if let FeatureKind::Component(c)=&f.kind {Some(c)} else {None});
    let children=s.doc.features.iter().filter(|f|f.owner==id && matches!(f.kind,FeatureKind::Component(_))).map(|f|node(s,f.id)).collect::<Vec<_>>();
    let default=Placement::default();
    let placement=component.map_or(&default,|c|&c.placement);
    json!({"id":id,"name":s.doc.component_name(id),"type":"component",
        "visible":s.built.component_visible(id),"own_visible":component.is_none_or(|c|c.visible),
        "available":s.built.components.contains_key(&id),
        "placement":{"translate":placement.translate.each_ref().map(|v|v.expr.clone()),"rotate":placement.rotate.each_ref().map(|v|v.expr.clone()),"world":s.built.component_placement(id).to_cols_array()},
        "children":children,
        "features":s.doc.features.iter().filter(|f|f.owner==id).map(|f|f.id).collect::<Vec<_>>(),
        "bodies":s.built.bodies.iter().filter(|b|b.component==id).filter_map(|b|body_info(s,b.id)).collect::<Vec<_>>()})
}

pub(super) fn move_values(d:&Document,c:&J,id:Id)->R<Placement> {
    if id==0 {return Err("the root component cannot be moved".into());}
    let Some(FeatureKind::Component(component))=d.feature(id).map(|f|&f.kind) else {return Err("the component does not exist".into())};
    let mut placement=component.placement.clone();
    for (key,values,kind) in [("translate",&mut placement.translate,Kind::Length),("rotate",&mut placement.rotate,Kind::Angle)] {
        if c[key].is_null() {continue;}
        let input=c[key].as_array().filter(|v|v.len()==3).ok_or(format!("{key} needs exactly three values"))?;
        for (v,input) in values.iter_mut().zip(input) {*v=d.value(&text_of(input)?,kind)?;}
    }
    Ok(placement)
}

pub(super) fn export_component(s:&Session,c:&J)->R<Option<Id>> {
    if c["component"].is_null() {return Ok(None);}
    let id=id_of(c,"component")?;
    if id!=0 && !matches!(s.doc.feature(id).map(|f|&f.kind),Some(FeatureKind::Component(_))) {return Err("the export component does not exist".into());}
    Ok(Some(id))
}
