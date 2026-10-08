//! A face of a body, as picked by a click or named by a nearby point.

use glam::{DVec2, DVec3};

use crate::doc::Body;
use crate::sketch::{Id, Plane, Sketch};

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub body: Id,
    /// Indices into the body's triangles, sorted.
    pub tris: Vec<usize>,
    /// The face's plane, facing outward, if it is flat.
    pub plane: Option<Plane>,
    /// Its outlines in space, outer boundary and holes alike.
    pub outline: Vec<Vec<DVec3>>,
    /// The outlines in the plane's coordinates, for flat faces.
    pub loops: Vec<Vec<DVec2>>,
    pub area: f64,
    /// A point on the face, which names it in features that refer to it.
    pub at: DVec3,
}

impl Face {
    /// Rigidly change the coordinate frame while keeping sketch UVs and topology.
    pub fn transformed(&self,t:glam::DAffine3)->Self {
        let mut face=self.clone();
        face.plane=face.plane.map(|p|p.transformed(t));
        face.at=t.transform_point3(face.at);
        for p in face.outline.iter_mut().flatten() {*p=t.transform_point3(*p);}
        face
    }
    /// The face of `body` that contains triangle `tri`.
    pub fn pick(body: &Body, tri: usize) -> Face {
        let tris = body.mesh.face(tri);
        // Sketch coordinates stay lined up with the model's by putting the plane's origin nearest the world's.
        let plane = body.mesh.face_plane(&tris).map(|(p, n)| Plane::from_normal(n * n.dot(p), n));
        let outline = body.mesh.face_loops(&tris);
        let loops = plane.map_or(Vec::new(), |pl| outline.iter().map(|l| l.iter().map(|p| pl.to_local(*p)).collect()).collect());
        let at = body.mesh.tris[tri].iter().sum::<DVec3>() / 3.0;
        Face { body: body.id, area: body.mesh.face_area(&tris), tris, plane, outline, loops, at }
    }

    /// The face of `body` nearest to a point in space.
    pub fn near(body: &Body, p: DVec3) -> Option<Face> {
        body.mesh.nearest_tri(p).map(|t| Face::pick(body, t))
    }

    /// A hidden sketch tracing the face, and the profiles in it to extrude.
    pub fn sketch(&self) -> Result<(Sketch, Vec<Vec<Id>>), String> {
        let mut sk = Sketch::new(self.plane.ok_or("Only flat faces can be extruded.")?);
        sk.project(&self.outline);
        sk.visible = false;
        let profiles: Vec<Vec<Id>> = crate::profile::profiles(&sk).iter().filter(|p| p.depth % 2 == 0).map(|p| p.edges.clone()).collect();
        if profiles.is_empty() { Err("That face's outline could not be traced.".into()) } else { Ok((sk, profiles)) }
    }
}
