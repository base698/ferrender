//! Component ownership and rigid frames. Features build in their owner's frame.
use glam::{DAffine3,DQuat,DVec3};
use serde::{Deserialize,Serialize};
use crate::{Built,Document,FeatureKind,Id,Kind,Plane,Value};

#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
pub struct Placement { pub translate:[Value;3], pub rotate:[Value;3] }
impl Default for Placement {
    fn default()->Self {
        Self {translate:std::array::from_fn(|_|Value {expr:"0 mm".into(),v:0.}),rotate:std::array::from_fn(|_|Value {expr:"0 deg".into(),v:0.})}
    }
}
impl Placement {
    pub fn affine(&self)->DAffine3 {
        let r=self.rotate.each_ref().map(|v|(v.v%360.).to_radians());
        DAffine3::from_rotation_translation(DQuat::from_rotation_z(r[2])*DQuat::from_rotation_y(r[1])*DQuat::from_rotation_x(r[0]),DVec3::from_array(self.translate.each_ref().map(|v|v.v)))
    }
}
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
pub struct Component {
    #[serde(default)] pub placement:Placement,
    #[serde(default="yes")] pub visible:bool,
}
fn yes()->bool {true}
impl Default for Component {fn default()->Self {Self {placement:Placement::default(),visible:true}}}
#[derive(Clone,Debug)]
pub struct BuiltComponent {pub placement:DAffine3,pub visible:bool}

pub fn steps(transform:DAffine3)->Vec<crate::exact::Place> {
    let (axis,angle)=DQuat::from_mat3(&transform.matrix3).normalize().to_axis_angle();
    let mut steps=Vec::new();
    if angle.abs()>1e-12 {steps.push(crate::exact::Place::Turn {origin:DVec3::ZERO,axis,angle});}
    if transform.translation.length_squared()>1e-24 {steps.push(crate::exact::Place::Shift(transform.translation));}
    steps
}
impl Plane {
    pub fn transformed(self,t:DAffine3)->Self {Self {origin:t.transform_point3(self.origin),x:t.transform_vector3(self.x),y:t.transform_vector3(self.y)}}
}
impl Built {
    pub fn component_placement(&self,id:Id)->DAffine3 {self.components.get(&id).map_or(DAffine3::IDENTITY,|c|c.placement)}
    pub fn component_visible(&self,id:Id)->bool {self.components.get(&id).is_some_and(|c|c.visible)}
    pub fn sketch_plane(&self,doc:&Document,id:Id)->Option<Plane> {
        let f=doc.feature(id)?;
        let FeatureKind::Sketch(sk)=&f.kind else {return None};
        if self.errors.contains_key(&id) || f.suppressed || !self.components.contains_key(&f.owner)
            || !doc.features.iter().take(doc.active()).any(|f|f.id==id) {return None;}
        Some(sk.plane.transformed(self.component_placement(f.owner)))
    }
}
impl Document {
    pub fn component_contains(&self,root:Id,mut id:Id)->bool {
        for _ in 0..=32 {
            if id==root {return true;}
            if id==0 {return false;}
            let Some(f)=self.feature(id).filter(|f|matches!(f.kind,FeatureKind::Component(_))) else {return false};
            id=f.owner;
        }
        false
    }
    pub fn component_name(&self,id:Id)->String {if id==0 {"Root".into()} else {self.feature(id).map_or_else(||"Missing component".into(),|f|f.name.clone())}}
    pub fn component_available(&self,mut id:Id)->bool {
        for _ in 0..=32 {
            if id==0 {return true;}
            let Some(f)=self.features.iter().take(self.active()).find(|f|f.id==id && !f.suppressed && matches!(f.kind,FeatureKind::Component(_))) else {return false};
            id=f.owner;
        }
        false
    }
    pub fn activate_component(&mut self,id:Id)->Result<(),String> {
        if !self.component_available(id) {return Err("that component is missing, suppressed, or after the history marker".into());}
        self.active_component=id; Ok(())
    }
    pub fn create_component(&mut self,name:Option<String>,parent:Id,activate:bool)->Result<Id,String> {
        if !self.component_available(parent) {return Err("the parent component is not available here".into());}
        let id=self.add_feature_to(parent,FeatureKind::Component(Component::default()))?;
        if let Some(name)=name.filter(|n|!n.trim().is_empty()) {self.feature_mut(id).unwrap().name=name;}
        if activate {self.active_component=id;}
        crate::validation::document(self)?;
        Ok(id)
    }
    pub fn move_component(&mut self,id:Id,placement:Placement)->Result<(),String> {
        if id==0 {return Err("the root component cannot be moved".into());}
        for v in &placement.translate {self.value(&v.expr,Kind::Length)?;}
        for v in &placement.rotate {self.value(&v.expr,Kind::Angle)?;}
        let f=self.feature_mut(id).ok_or("the component does not exist")?;
        let FeatureKind::Component(c)=&mut f.kind else {return Err("select a component to move".into())};
        c.placement=placement; Ok(())
    }
    pub fn add_feature_to(&mut self,owner:Id,kind:FeatureKind)->Result<Id,String> {
        if !self.component_available(owner) {return Err("the component is not available at this history position".into());}
        let id=self.add_feature(kind);
        self.feature_mut(id).unwrap().owner=owner;
        Ok(id)
    }
    pub fn body_owner(&self,id:Id)->Option<Id> {
        self.feature(id).or_else(||self.feature(id/1000).filter(|f|matches!(f.kind,FeatureKind::Pattern(_)|FeatureKind::Split(_)))).map(|f|f.owner)
    }
    pub fn delete_feature(&mut self,id:Id)->Result<Vec<Id>,String> {
        let f=self.feature(id).ok_or("the feature does not exist")?;
        let subtree=matches!(f.kind,FeatureKind::Component(_));
        let removed:Vec<_>=self.features.iter().filter(|f|f.id==id || (subtree && self.component_contains(id,f.owner))).map(|f|f.id).collect();
        if let Some(at)=self.rollback {self.rollback=Some(at-self.features.iter().take(at).filter(|f|removed.contains(&f.id)).count());}
        let removed_patterns:Vec<_>=self.features.iter().filter(|f|removed.contains(&f.id) && matches!(f.kind,FeatureKind::Pattern(_)|FeatureKind::Split(_))).map(|f|f.id).collect();
        self.features.retain(|f|!removed.contains(&f.id));
        self.hidden_bodies.retain(|b|!removed.contains(b) && (*b%1000==0 || !removed_patterns.contains(&(b/1000))));
        if removed.contains(&self.active_component) {self.active_component=0;}
        Ok(removed)
    }
}
