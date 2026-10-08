use super::*;
use crate::primitives::{Primitive, PrimitiveShape};

pub(super) fn create(d: &Document, c: &J) -> R<Primitive> {
    let dimension = |key: &str| d.value(&text_of(&c[key]).map_err(|_|format!("a primitive needs {key}"))?,Kind::Length);
    let shape = match c["type"].as_str() {
        Some("box") => PrimitiveShape::Box {width:dimension("width")?,depth:dimension("depth")?,height:dimension("height")?},
        Some("cylinder") => PrimitiveShape::Cylinder {diameter:dimension("diameter")?,height:dimension("height")?},
        Some("sphere") => PrimitiveShape::Sphere {diameter:dimension("diameter")?},
        Some("cone") => PrimitiveShape::Cone {bottom_diameter:dimension("bottom_diameter")?,top_diameter:if c["top_diameter"].is_null() {d.value("0 mm",Kind::Length)?} else {dimension("top_diameter")?},height:dimension("height")?},
        Some("torus") => PrimitiveShape::Torus {major_radius:dimension("major_radius")?,tube_radius:dimension("tube_radius")?},
        _ => return Err("primitive type must be box, cylinder, sphere, cone or torus".into()),
    };
    let mut primitive = Primitive {shape,position:std::array::from_fn(|_|Value {expr:"0 mm".into(),v:0.}),rotate:std::array::from_fn(|_|Value {expr:"0 deg".into(),v:0.}),op:Op::New};
    update(d,c,&mut primitive)?;
    Ok(primitive)
}

pub(super) fn update(d: &Document, c: &J, p: &mut Primitive) -> R<()> {
    if !c["type"].is_null() && c["type"].as_str() != Some(p.shape.name()) { return Err("the primitive type cannot be changed; create a new primitive instead".into()); }
    for key in ["width","depth","height","diameter","bottom_diameter","top_diameter","major_radius","tube_radius"] {
        if !c[key].is_null() && !p.shape.dimensions().iter().any(|(name,_)|*name == key) { return Err(format!("{key} is not a dimension of a {}",p.shape.name())); }
    }
    for (key,value) in p.shape.dimensions_mut() {
        if !c[key].is_null() { *value = d.value(&text_of(&c[key])?,Kind::Length)?; }
    }
    for (key,values,kind) in [("position",&mut p.position,Kind::Length),("rotate",&mut p.rotate,Kind::Angle)] {
        if c[key].is_null() { continue; }
        let input = c[key].as_array().filter(|a|a.len()==3).ok_or(format!("{key} needs exactly three values"))?;
        for (value,input) in values.iter_mut().zip(input) { *value = d.value(&text_of(input)?,kind)?; }
    }
    p.op = op_of(c,p.op)?;
    p.validate()
}

pub(super) fn info(s: &Session, p: &Primitive, out: &mut J) {
    out["type"] = json!("primitive");
    out["shape"] = json!(p.shape.name());
    out["operation"] = json!(p.op.name());
    for (key,value) in p.shape.dimensions() { out[key] = json!({"expr":value.expr,"value":len_out(&s.doc,value.v)}); }
    out["position"] = json!(p.position.each_ref().map(|v|json!({"expr":v.expr,"value":len_out(&s.doc,v.v)})));
    out["rotate"] = json!(p.rotate.each_ref().map(|v|json!({"expr":v.expr,"value":v.v})));
}
