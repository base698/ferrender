//! A face of a body, as picked by a click or named by a nearby point.

use glam::{DVec2, DVec3};

use crate::doc::Body;
use crate::sketch::{Id, Plane, Sketch};
use crate::profile::Seg;

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
    /// Analytic edges of an exact flat face, in the same coordinates as `loops`.
    /// Display polygons are deliberately separate: using their chords for a cut
    /// leaves thin strips behind along circular fillets.
    pub exact_edges: Option<Vec<Seg>>,
    /// The picked face's kernel identity, retained for lazy outline extraction.
    exact_id: Option<u64>,
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
        let at = body.mesh.tri(tri).iter().sum::<DVec3>() / 3.0;
        let exact_id = body.mesh.face_ids.get(tri).copied();
        Face { body: body.id, area: body.mesh.face_area(&tris), tris, plane, outline, loops, exact_edges: None, exact_id, at }
    }

    /// The face of `body` nearest to a point in space.
    pub fn near(body: &Body, p: DVec3) -> Option<Face> {
        body.mesh.nearest_tri(p).map(|t| Face::pick(body, t))
    }
    /// Capture analytic boundaries when committing a face choice, not while
    /// hovering. Call before transforming this face away from `body`'s frame;
    /// captured UV coordinates remain valid through subsequent rigid placement.
    pub fn prepare_exact(&mut self, body: &Body) {
        if self.body==body.id {
            self.exact_edges=self.plane.and_then(|plane|self.exact_id.and_then(|id|crate::exact::face_edges(&body.solids,id,plane,self.at)));
        }
    }

    /// A hidden sketch tracing the face, and the profiles in it to extrude.
    pub fn sketch(&self) -> Result<(Sketch, Vec<Vec<Id>>), String> {
        let mut sk = Sketch::new(self.plane.ok_or("Only flat faces can be extruded.")?);
        if let Some(edges) = &self.exact_edges {
            use crate::sketch::Geom;
            for edge in edges {
                let id = match *edge {
                    Seg::Line(a,b) => { let a=sk.point_at(a,1e-7); let b=sk.point_at(b,1e-7); if a==b {continue;} sk.add_line(a,b) },
                    Seg::Arc(a,m,b) => {
                        let a=sk.point_at(a,1e-7); let m=sk.point_at(m,1e-7); let b=sk.point_at(b,1e-7);
                        sk.add_arc3(a,m,b,false)?
                    }
                    Seg::Circle(centre,r) => { let c=sk.point_at(centre,1e-7); sk.add(Geom::Circle{c,r},false) },
                    Seg::Spline(points) => { let ids=points.map(|p|sk.point_at(p,1e-7)); sk.add_spline(ids,false)? },
                };
                sk.fixed.extend(sk.ent_points(id));
                if matches!(sk.entities[&id].geom,Geom::Circle{..}) {sk.fixed.insert(id);}
            }
            sk.fixed.extend(sk.points.keys().copied());
        } else {
            sk.project(&self.outline);
        }
        sk.visible = false;
        let profiles: Vec<Vec<Id>> = crate::profile::profiles(&sk).iter().filter(|p| p.depth % 2 == 0).map(|p| p.edges.clone()).collect();
        if profiles.is_empty() { Err("That face's outline could not be traced.".into()) } else { Ok((sk, profiles)) }
    }
}
