//! Persistent construction references, resolved against history in order.
use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};
use crate::{Body, Built, Document, Feature, FeatureKind, Id, Plane, Sketch, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OriginPlane { XY, XZ, YZ }
impl OriginPlane {
    pub fn plane(self) -> Plane { match self { Self::XY => Plane::XY, Self::XZ => Plane::XZ, Self::YZ => Plane::YZ } }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaneRef {
    Origin(OriginPlane),
    Face { body: Id, at: DVec3, frame: Option<[DVec3; 2]>, #[serde(default, skip_serializing_if = "Option::is_none")] tag: Option<crate::tag::Tag> },
    Plane(Id),
    /// A plane given outright, in the owner component's frame.
    Free(Plane),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointRef {
    World(DVec3),
    Vertex { body: Id, at: DVec3, frame: Option<[DVec3; 2]> },
    SketchPoint { sketch: Id, point: Id },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaneKind {
    Offset { base: PlaneRef, distance: Value },
    Midplane { a: PlaneRef, b: PlaneRef, flip: bool },
    ThreePoint { points: [PointRef; 3] },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstructionPlane {
    pub kind: PlaneKind,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub visibility_pinned: bool,
}
fn yes() -> bool { true }
impl ConstructionPlane {
    pub fn new(kind: PlaneKind) -> Self { Self { kind, visible: true, visibility_pinned: false } }
}
#[derive(Clone, Debug)]
pub struct ResolvedPlane { pub component:Id, pub plane: Plane, pub corners: [DVec3; 4] }

pub fn three_points(p: [DVec3; 3]) -> Result<Plane, String> {
    if p.iter().any(|p| !p.is_finite()) { return Err("the three points must be finite".into()); }
    let (a,b) = (p[1]-p[0], p[2]-p[0]);
    let n = a.cross(b);
    if n.length() <= 1e-9 * (a.length()*b.length()).max(1.0) {
        return Err("the three points are in a line; move one".into());
    }
    let x = a.normalize();
    Ok(Plane { origin: p[0], x, y: n.normalize().cross(x) })
}

pub fn midplane(a: Plane, b: Plane, centre: DVec3, flip: bool) -> Result<Plane, String> {
    let (na,nb) = (a.normal(),b.normal());
    let dot = na.dot(nb);
    if dot.abs() > 1.0 - 1e-6 {
        let gap = (b.origin-a.origin).dot(na);
        if gap.abs() < 1e-7 { return Err("choose two different faces; these lie in the same plane".into()); }
        let origin = a.origin + na*gap*0.5;
        let origin = centre - na*(centre-origin).dot(na);
        return Ok(Plane { origin, ..a });
    }
    let axis = na.cross(nb);
    // Intersection of n_a·p=d_a and n_b·p=d_b, then slide along it to the centre.
    let p = (a.origin.dot(na)*nb.cross(axis) + b.origin.dot(nb)*axis.cross(na))/axis.length_squared();
    let x = axis.normalize();
    let origin = p + x*(centre-p).dot(x);
    let n = if flip { na+nb } else { na-nb }.normalize();
    Ok(Plane { origin, x, y: n.cross(x) })
}

pub fn rectangle(plane: Plane, points: &[DVec3], fallback: f64) -> [DVec3; 4] {
    let (mut lo,mut hi) = (DVec2::splat(f64::INFINITY),DVec2::splat(f64::NEG_INFINITY));
    for p in points { let p=plane.to_local(*p); lo=lo.min(p); hi=hi.max(p); }
    if points.is_empty() { lo=DVec2::splat(-fallback/2.0); hi=-lo; }
    let centre=(lo+hi)*0.5;
    let half=((hi-lo)*0.575).max(DVec2::splat(5.0));
    let (lo,hi)=(centre-half,centre+half);
    [lo,DVec2::new(hi.x,lo.y),hi,DVec2::new(lo.x,hi.y)].map(|p|plane.to_world(p))
}

pub fn bounds_points(body: &Body) -> Vec<DVec3> {
    body.mesh.bbox().map(|(lo,hi)| (0..8).map(|i|DVec3::new(
        if i&1==0 {lo.x} else {hi.x}, if i&2==0 {lo.y} else {hi.y}, if i&4==0 {lo.z} else {hi.z})).collect()).unwrap_or_default()
}

fn candidates(body: &Body, at: DVec3, frame: Option<[DVec3;2]>) -> [DVec3;2] {
    // Prefer the bounds-mapped point when an earlier feature resized the body.
    match (frame,body.mesh.bbox()) {
        (Some([lo,hi]),Some((now_lo,now_hi))) => [now_lo+(at-lo)/(hi-lo).max(DVec3::splat(1e-9))*(now_hi-now_lo),at],
        _ => [at,at],
    }
}

pub fn face(body: &Body, at: DVec3, frame: Option<[DVec3;2]>) -> Result<crate::face::Face,String> {
    face_tagged(body,at,frame,None).map(|f|f.0)
}

/// [`face`] for a reference that may carry a tag: the tagged face is tried first, wherever it is
/// now. Returns the face, the tag of the face found, and how it was found.
pub fn face_tagged(body: &Body, at: DVec3, frame: Option<[DVec3;2]>, tag: Option<&crate::tag::Tag>) -> Result<(crate::face::Face,Option<crate::tag::Tag>,crate::tag::Level),String> {
    use crate::tag::Level;
    let mut tries: Vec<(DVec3,Level)> = Vec::new();
    if let Some(tag)=tag {
        let faces=crate::exact::faces_tagged(&body.solids,&body.tags);
        let exact_hit=faces.iter().find(|f|f.tag.as_ref()==Some(tag));
        let family=exact_hit.or_else(||faces.iter().filter(|f|f.tag.as_ref().is_some_and(|t|t.family()==tag.family())).min_by(|a,b|a.at.distance(at).total_cmp(&b.at.distance(at))));
        if let Some(f)=family {
            let level=if exact_hit.is_some() {Level::Tag} else {Level::Origin};
            for p in candidates(body,at,frame) { tries.push((p-f.normal*(p-f.at).dot(f.normal),level)); }
            tries.push((f.at,level));
        }
    }
    tries.extend(candidates(body,at,frame).into_iter().map(|p|(p,Level::Position)));
    for (p,level) in tries {
        let Some(f)=crate::face::Face::near(body,p) else { continue };
        let Some(plane)=f.plane else { continue };
        if (p-plane.origin).dot(plane.normal()).abs() > 1e-5 { continue; }
        let q=plane.to_local(p);
        if f.loops.iter().filter(|ring| crate::profile::inside(ring,q)).count()%2==1 {
            let found=crate::exact::face_tag_at(&body.solids,&body.tags,p);
            return Ok((f,found,level));
        }
    }
    Err("the face is no longer flat, or is gone; edit the plane and pick it again".into())
}

impl Document {
    pub fn plane_reference(&self, reference: &PlaneRef, built: &Built, owner:Id) -> Result<(Plane,Vec<DVec3>),String> {
        let mut copy=reference.clone();
        self.plane_reference_mut(&mut copy,built,owner).map(|r|(r.0,r.1))
    }

    /// [`plane_reference`](Self::plane_reference) that lets a face reference learn the tag of the face it found.
    pub fn plane_reference_mut(&self, reference: &mut PlaneRef, built: &Built, owner:Id) -> Result<(Plane,Vec<DVec3>,Option<crate::tag::Level>),String> {
        match reference {
            PlaneRef::Origin(origin) => {
                let mut points=Vec::new();
                for body in &built.bodies {
                    let t=built.component_placement(owner).inverse()*if built.placements_applied {glam::DAffine3::IDENTITY} else {built.component_placement(body.component)};
                    points.extend(bounds_points(body).into_iter().map(|p|t.transform_point3(p)));
                }
                Ok((origin.plane(),points,None))
            }
            PlaneRef::Free(plane) => { Sketch::new(*plane).validate()?; Ok((*plane,Vec::new(),None)) }
            PlaneRef::Plane(id) => built.planes.get(id).map(|p|{
                let t=built.component_placement(owner).inverse()*if built.placements_applied {glam::DAffine3::IDENTITY} else {built.component_placement(p.component)};
                (p.plane.transformed(t),p.corners.map(|p|t.transform_point3(p)).to_vec(),None)
            })
                .ok_or_else(||format!("plane {id} is missing, rolled back, suppressed, or comes later in the timeline")),
            PlaneRef::Face {body,at,frame,tag} => {
                let body=built.body(*body).ok_or("a body it used no longer exists")?;
                let local=if built.placements_applied {std::borrow::Cow::Owned(body.local_copy()?)} else {std::borrow::Cow::Borrowed(body)};
                let (f,found,level)=face_tagged(&local,*at,*frame,tag.as_ref())?;
                if tag.is_none() { *tag=found; }
                let t=built.component_placement(owner).inverse()*built.component_placement(body.component);
                Ok((f.plane.unwrap().transformed(t),f.outline.into_iter().flatten().map(|p|t.transform_point3(p)).collect(),Some(level)))
            }
        }
    }

    pub fn point_reference(&self, reference: &PointRef, built: &Built, before: Id, owner:Id) -> Result<DVec3,String> {
        match reference {
            PointRef::World(p) => Ok(*p),
            PointRef::SketchPoint {sketch,point} => {
                let at=self.features.iter().position(|f|f.id==before).unwrap_or(self.active());
                let feature=self.features.iter().take(at).find(|f|f.id==*sketch && !f.suppressed).ok_or("the sketch point's sketch is missing or comes later")?;
                if let Some(e)=built.errors.get(sketch) { return Err(format!("the sketch point could not be placed: {e}")); }
                let FeatureKind::Sketch(s)=&feature.kind else { return Err("the point reference is not a sketch".into()) };
                if !built.components.contains_key(&feature.owner) {return Err("the sketch point's component is suppressed or unavailable".into());}
                let t=built.component_placement(owner).inverse()*built.component_placement(feature.owner);
                Ok(t.transform_point3(s.plane.to_world(*s.points.get(point).ok_or("the sketch point was deleted")?)))
            }
            PointRef::Vertex {body,at,frame} => {
                let body=built.body(*body).ok_or("a body it used no longer exists")?;
                let local=if built.placements_applied {std::borrow::Cow::Owned(body.local_copy()?)} else {std::borrow::Cow::Borrowed(body)};
                let vertices:Vec<_>=local.edges.iter().flat_map(|e|e.first().into_iter().chain(e.last())).copied().collect();
                for p in candidates(&local,*at,*frame) {
                    if let Some(v)=vertices.iter().min_by(|a,b|a.distance_squared(p).total_cmp(&b.distance_squared(p)))
                        && v.distance(p)<1e-5 { return Ok((built.component_placement(owner).inverse()*built.component_placement(body.component)).transform_point3(*v)); }
                }
                Err("the vertex is no longer there; edit the plane and pick it again".into())
            }
        }
    }

    /// Resolves a construction plane. `f` is the feature's own copy: face references learn
    /// the tags of the faces they found. Also returns how its face picks resolved, if it has any.
    pub fn resolve_plane(&self, f: &mut Feature, built: &Built) -> Result<(ResolvedPlane,Option<crate::tag::Level>),String> {
        let (id,owner)=(f.id,f.owner);
        let FeatureKind::Plane(p)=&mut f.kind else { return Err("not a construction plane".into()) };
        let mut levels=Vec::new();
        let (plane,points)=match &mut p.kind {
            PlaneKind::Offset {base,distance} => { let (base,points,level)=self.plane_reference_mut(base,built,owner)?; levels.extend(level); (base.offset(distance.v),points) }
            PlaneKind::Midplane {a,b,flip} => {
                if a==b { return Err("choose two different faces".into()); }
                let (a,mut points,la)=self.plane_reference_mut(a,built,owner)?;
                let (b,other,lb)=self.plane_reference_mut(b,built,owner)?;
                levels.extend(la); levels.extend(lb);
                points.extend(other);
                let centre=if points.is_empty() {(a.origin+b.origin)*0.5} else {points.iter().sum::<DVec3>()/points.len() as f64};
                (midplane(a,b,centre,*flip)?,points)
            }
            PlaneKind::ThreePoint {points} => {
                let points=points.iter().map(|p|self.point_reference(p,built,id,owner)).collect::<Result<Vec<_>,_>>()?;
                (three_points([points[0],points[1],points[2]])?,points)
            }
        };
        Sketch::new(plane).validate()?;
        Ok((ResolvedPlane { component:owner, plane, corners: rectangle(plane,&points,50.0) },crate::tag::weakest(levels)))
    }
}
