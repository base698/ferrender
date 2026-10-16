//! Outlines depend on local geometry, not its distance from the world origin.
use fr_core::{Document, FeatureKind, Session, Unit, mesh::Mesh, render::{self, Camera, Image}};
use glam::DVec3;
fn session(mesh: Mesh) -> Session {
    let mut d = Document::new(Unit::Mm); d.add_feature(FeatureKind::Import(mesh)); Session::new(d)
}
fn render(s: &Session, cam: Camera) -> Image {
    let mut img = Image {w:320,h:320,rgba:vec![255;320*320*4]};
    render::draw_bodies(&mut img, s.visible_bodies(), &cam, None); img
}
fn ellipsoid() -> Mesh {
    let (rings, cols) = (300, 400);
    let at = |r:usize,c:usize| {
        let (u,v) = (r as f64/rings as f64*std::f64::consts::PI,c as f64/cols as f64*std::f64::consts::TAU);
        DVec3::new(30.0*u.sin()*v.cos(),20.0*u.sin()*v.sin(),40.0*u.cos())
    };
    let mut tris = Vec::new();
    for r in 0..rings { for c in 0..cols {
        let (a,b,c,d)=(at(r,c),at(r+1,c),at(r+1,c+1),at(r,c+1));
        if r>0 {tris.push([a,b,d]);}
        if r+1<rings {tris.push([b,c,d]);}
    }}
    Mesh::from_tris(tris)
}
#[test]
fn smooth_mesh_outlines_are_translation_invariant() {
    let m=ellipsoid();
    let a=session(m.clone());
    let cam=Camera{scale:3.0,..Camera::iso()};
    let first=render(&a,cam);
    let shift=DVec3::new(1000.0,-500.0,750.0);
    let mut moved=m; moved.map(|p|p+shift);
    let b=session(moved);
    let second=render(&b,Camera{target:shift,..cam});
    let changed=first.rgba.chunks_exact(4).zip(second.rgba.chunks_exact(4)).filter(|(a,b)|(0..3).any(|i|a[i].abs_diff(b[i])>5)).count();
    assert!(changed<20,"translating both the mesh and camera must not create false outlines: {changed} changed pixels");
    let interior=(120..200).flat_map(|y|(120..200).map(move|x|y*320+x));
    let dark=interior.filter(|p|first.rgba[p*4]<80).count();
    assert!(dark<20,"a smooth interior should be shaded, not stippled: {dark} dark pixels");
}
#[test]
fn parallel_depth_steps_keep_their_outline() {
    let mut tris=Vec::new();
    for (x0,x1,z) in [(-20.0,0.0,0.0),(0.0,20.0,5.0)] {
        let [a,b,c,d]=[DVec3::new(x0,-20.0,z),DVec3::new(x1,-20.0,z),DVec3::new(x1,20.0,z),DVec3::new(x0,20.0,z)];
        tris.extend([[a,b,c],[a,c,d]]);
    }
    let s=session(Mesh::from_tris(tris));
    let (yaw,pitch)=Camera::named("top").unwrap();
    let img=render(&s,Camera{yaw,pitch,scale:5.0,..Camera::iso()});
    assert!(img.rgba[(130*320+159)*4]<80,"parallel surfaces at different depths still have a visible step");
}
