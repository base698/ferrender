//! Mesh booleans with BSP trees, after Evan Wallace's csg.js. The trees
//! leave T-junctions along the cuts, which are stitched closed afterwards.

use glam::DVec3;

use crate::mesh::Mesh;

const EPS: f64 = 1e-6;

#[derive(Clone)]
struct Poly {
    v: Vec<DVec3>,
    n: DVec3,
    w: f64,
}

impl Poly {
    fn new(v: Vec<DVec3>) -> Option<Poly> {
        let n = (v[1] - v[0]).cross(v[2] - v[0]);
        (n.length_squared() > 1e-24).then(|| {
            let n = n.normalize();
            Poly { w: n.dot(v[0]), n, v }
        })
    }

    fn flip(&mut self) {
        self.v.reverse();
        self.n = -self.n;
        self.w = -self.w;
    }
}

#[derive(Default)]
struct Node {
    plane: Option<(DVec3, f64)>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polys: Vec<Poly>,
}

/// Sorts `p` against a plane into coplanar-front, coplanar-back, front and back parts.
fn split(plane: (DVec3, f64), p: Poly, cf: &mut Vec<Poly>, cb: &mut Vec<Poly>, f: &mut Vec<Poly>, b: &mut Vec<Poly>) {
    let (n, w) = plane;
    let side: Vec<i8> = p.v.iter().map(|v| {
        let t = n.dot(*v) - w;
        if t < -EPS { -1 } else if t > EPS { 1 } else { 0 }
    }).collect();
    let (any_front, any_back) = (side.contains(&1), side.contains(&-1));
    match (any_front, any_back) {
        (false, false) => if n.dot(p.n) > 0.0 { cf.push(p) } else { cb.push(p) },
        (true, false) => f.push(p),
        (false, true) => b.push(p),
        (true, true) => {
            let (mut fv, mut bv) = (Vec::new(), Vec::new());
            for i in 0..p.v.len() {
                let j = (i + 1) % p.v.len();
                let (vi, vj) = (p.v[i], p.v[j]);
                if side[i] >= 0 {
                    fv.push(vi);
                }
                if side[i] <= 0 {
                    bv.push(vi);
                }
                if side[i] * side[j] < 0 {
                    let t = (w - n.dot(vi)) / n.dot(vj - vi);
                    let x = vi.lerp(vj, t);
                    fv.push(x);
                    bv.push(x);
                }
            }
            let part = |v: Vec<DVec3>| (v.len() >= 3).then(|| Poly { v, n: p.n, w: p.w });
            f.extend(part(fv));
            b.extend(part(bv));
        }
    }
}

impl Node {
    fn build(&mut self, polys: Vec<Poly>) {
        if polys.is_empty() {
            return;
        }
        let plane = *self.plane.get_or_insert((polys[0].n, polys[0].w));
        let (mut f, mut b, mut same) = (Vec::new(), Vec::new(), Vec::new());
        let mut same_back = Vec::new();
        for p in polys {
            split(plane, p, &mut same, &mut same_back, &mut f, &mut b);
        }
        self.polys.append(&mut same);
        self.polys.append(&mut same_back);
        if !f.is_empty() {
            self.front.get_or_insert_default().build(f);
        }
        if !b.is_empty() {
            self.back.get_or_insert_default().build(b);
        }
    }

    fn invert(&mut self) {
        self.polys.iter_mut().for_each(Poly::flip);
        if let Some((n, w)) = &mut self.plane {
            *n = -*n;
            *w = -*w;
        }
        if let Some(f) = &mut self.front {
            f.invert();
        }
        if let Some(b) = &mut self.back {
            b.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }

    /// Removes the parts of `polys` that are inside this tree's solid.
    fn clip(&self, polys: Vec<Poly>) -> Vec<Poly> {
        let Some(plane) = self.plane else { return polys };
        let (mut f, mut b) = (Vec::new(), Vec::new());
        let (mut cf, mut cb) = (Vec::new(), Vec::new());
        for p in polys {
            split(plane, p, &mut cf, &mut cb, &mut f, &mut b);
        }
        f.append(&mut cf);
        b.append(&mut cb);
        let mut f = match &self.front {
            Some(n) => n.clip(f),
            None => f,
        };
        if let Some(n) = &self.back {
            f.extend(n.clip(b));
        }
        f
    }

    fn clip_to(&mut self, other: &Node) {
        self.polys = other.clip(std::mem::take(&mut self.polys));
        if let Some(f) = &mut self.front {
            f.clip_to(other);
        }
        if let Some(b) = &mut self.back {
            b.clip_to(other);
        }
    }

    fn all(&self, out: &mut Vec<Poly>) {
        out.extend(self.polys.iter().cloned());
        if let Some(f) = &self.front {
            f.all(out);
        }
        if let Some(b) = &self.back {
            b.all(out);
        }
    }
}

fn tree(m: &Mesh) -> Node {
    let mut n = Node::default();
    n.build(m.tris.iter().filter_map(|t| Poly::new(t.to_vec())).collect());
    n
}

fn mesh(n: &Node) -> Mesh {
    let mut polys = Vec::new();
    n.all(&mut polys);
    let mut m = Mesh::default();
    for p in polys {
        for i in 1..p.v.len() - 1 {
            let t = [p.v[0], p.v[i], p.v[i + 1]];
            if (t[1] - t[0]).cross(t[2] - t[0]).length_squared() > 1e-20 {
                m.tris.push(t);
            }
        }
    }
    m
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bool {
    Union,
    Subtract,
    Intersect,
}

/// Booleans above this many triangles take long enough to be worth refusing.
pub const MAX_TRIS: usize = 400_000;

pub fn boolean(a: &Mesh, b: &Mesh, op: Bool) -> Result<Mesh, String> {
    if a.tris.len() + b.tris.len() > MAX_TRIS {
        return Err(format!("the meshes have {} triangles; booleans are limited to {MAX_TRIS}", a.tris.len() + b.tris.len()));
    }
    // The trees recurse as deep as the mesh is convex, so give them room.
    let (a, b) = (a.clone(), b.clone());
    std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(move || {
            let (mut a, mut b) = (tree(&a), tree(&b));
            let mut rest = Vec::new();
            match op {
                Bool::Union => {
                    a.clip_to(&b);
                    b.clip_to(&a);
                    b.invert();
                    b.clip_to(&a);
                    b.invert();
                    b.all(&mut rest);
                    a.build(rest);
                }
                Bool::Subtract => {
                    a.invert();
                    a.clip_to(&b);
                    b.clip_to(&a);
                    b.invert();
                    b.clip_to(&a);
                    b.invert();
                    b.all(&mut rest);
                    a.build(rest);
                    a.invert();
                }
                Bool::Intersect => {
                    a.invert();
                    b.clip_to(&a);
                    b.invert();
                    a.clip_to(&b);
                    b.clip_to(&a);
                    b.all(&mut rest);
                    a.build(rest);
                    a.invert();
                }
            }
            let mut out = mesh(&a);
            out.stitch();
            out
        })
        .and_then(|h| h.join().map_err(|_| std::io::Error::other("boolean failed")))
        .map_err(|e| e.to_string())
}
