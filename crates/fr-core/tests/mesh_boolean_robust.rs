//! Solid-set and persistence oracles for the Manifold Boolean integration.
use fr_core::{FeatureKind, Session, Unit, api::execute, csg::Bool, io, mesh::Mesh, meshfile, meshops};
use glam::{DMat3,DVec3};
use serde_json::json;

fn cube(lo:DVec3,hi:DVec3)->Mesh {
    let p=vec![DVec3::new(lo.x,lo.y,lo.z),DVec3::new(hi.x,lo.y,lo.z),DVec3::new(hi.x,hi.y,lo.z),DVec3::new(lo.x,hi.y,lo.z),DVec3::new(lo.x,lo.y,hi.z),DVec3::new(hi.x,lo.y,hi.z),DVec3::new(hi.x,hi.y,hi.z),DVec3::new(lo.x,hi.y,hi.z)];
    Mesh::from_indexed(p,vec![[0,2,1],[0,3,2],[4,5,6],[4,6,7],[0,1,5],[0,5,4],[1,2,6],[1,6,5],[2,3,7],[2,7,6],[3,0,4],[3,4,7]],true).unwrap()
}
fn sphere(radius:f64,centre:DVec3)->Mesh {
    let(n,m)=(40usize,80usize);
    let mut p=vec![centre+DVec3::Z*radius,centre-DVec3::Z*radius];
    for r in 1..n {for c in 0..m {let t=r as f64*std::f64::consts::PI/n as f64;let a=c as f64*std::f64::consts::TAU/m as f64;p.push(centre+DVec3::new(t.sin()*a.cos(),t.sin()*a.sin(),t.cos())*radius);}}
    let at=|r:usize,c:usize|(2+(r-1)*m+c%m) as u32;let mut tris=Vec::new();
    for c in 0..m {tris.push([0,at(1,c),at(1,c+1)]);tris.push([1,at(n-1,c+1),at(n-1,c)]);}
    for r in 1..n-1 {for c in 0..m {tris.extend([[at(r,c),at(r+1,c),at(r+1,c+1)],[at(r,c),at(r+1,c+1),at(r,c+1)]]);}}
    let mut out=Mesh::from_indexed(p,tris,true).unwrap();out.snap();out
}
fn closed(mesh:&Mesh) {
    let r=mesh.inspect();assert!(r.watertight,"{r:?}");assert_eq!(r.degenerate_removed,0);assert_eq!(r.duplicates_removed,0);assert_eq!(r.flipped,0);
    let again=meshfile::decode_blob(&meshfile::encode_blob(mesh)).unwrap();
    assert_eq!(again,*mesh);assert!(again.inspect().watertight);
}
fn close(a:f64,b:f64,relative:f64) {assert!((a-b).abs()<=b.abs().max(1.)*relative,"{a} != {b}");}

#[test]
fn curved_lens_booleans_match_independent_analytic_volume_and_set_membership() {
    let(a,b)=(sphere(2.,DVec3::ZERO),sphere(2.,DVec3::X*2.));
    let(union,difference,intersection)=(meshops::boolean(&a,&b,Bool::Union).unwrap(),meshops::boolean(&a,&b,Bool::Subtract).unwrap(),meshops::boolean(&a,&b,Bool::Intersect).unwrap());
    let exact_sphere=4./3.*std::f64::consts::PI*8.;let lens=10.*std::f64::consts::PI/3.;
    for(m,v)in[(&union,2.*exact_sphere-lens),(&difference,exact_sphere-lens),(&intersection,lens)] {closed(m);close(m.volume(),v,0.006);}
    close(union.volume()+intersection.volume(),a.volume()+b.volume(),1e-6);
    close(difference.volume()+intersection.volume(),a.volume(),1e-6);
    // Independent solid-set oracle, avoiding points close to either curved boundary.
    for x in -7..14 {for y in -7..8 {for z in -7..8 {
        let p=DVec3::new(x as f64*0.31,y as f64*0.31,z as f64*0.31);
        let(da,db)=(p.length(),(p-DVec3::X*2.).length());if(da-2.).abs()<0.05||(db-2.).abs()<0.05 {continue;}
        for(m,wanted)in[(&union,da<2.||db<2.),(&difference,da<2.&&db>2.),(&intersection,da<2.&&db<2.)] {assert_eq!(m.bvh().contains(m,p),wanted,"{p}");}
    }}}
}

#[test]
fn torus_cut_preserves_handle_and_half_volume_after_serialization() {
    let(nu,nv)=(80usize,32usize);let mut p=Vec::new();let mut t=Vec::new();
    for u in 0..nu {for v in 0..nv {let a=u as f64*std::f64::consts::TAU/nu as f64;let b=v as f64*std::f64::consts::TAU/nv as f64;p.push(DVec3::new((3.+b.cos())*a.cos(),(3.+b.cos())*a.sin(),b.sin()));}}
    let at=|u:usize,v:usize|((u%nu)*nv+v%nv) as u32;
    for u in 0..nu {for v in 0..nv {t.extend([[at(u,v),at(u+1,v),at(u+1,v+1)],[at(u,v),at(u+1,v+1),at(u,v+1)]]);}}
    let mut torus=Mesh::from_indexed(p,t,true).unwrap();torus.snap();
    let tool=cube(DVec3::new(-5.,-5.,0.),DVec3::splat(5.));
    for op in[Bool::Subtract,Bool::Intersect] {let out=meshops::boolean(&torus,&tool,op).unwrap();closed(&out);close(out.volume(),torus.volume()/2.,1e-6);assert!(!out.bvh().contains(&out,DVec3::new(0.,0.,0.2)));}
}

#[test]
fn nested_cavities_and_disconnected_shells_keep_solid_set_semantics() {
    let outer=cube(DVec3::splat(-3.),DVec3::splat(3.));let inner=cube(DVec3::splat(-2.),DVec3::splat(2.));
    let hollow=meshops::boolean(&outer,&inner,Bool::Subtract).unwrap();closed(&hollow);close(hollow.volume(),152.,1e-6);
    let island=cube(DVec3::splat(-1.),DVec3::splat(1.));
    let union=meshops::boolean(&hollow,&island,Bool::Union).unwrap();closed(&union);close(union.volume(),160.,1e-6);assert_eq!(union.inspect().components,3);
    assert!(meshops::boolean(&hollow,&island,Bool::Intersect).unwrap().is_empty());
    let tool=cube(DVec3::new(0.,-4.,-4.),DVec3::splat(4.));let half=meshops::boolean(&union,&tool,Bool::Subtract).unwrap();closed(&half);close(half.volume(),80.,1e-6);
}

#[test]
fn self_intersecting_closed_operand_uses_arrangement_union_semantics() {
    let a=cube(DVec3::ZERO,DVec3::ONE);let b=cube(DVec3::splat(0.5),DVec3::splat(1.5));
    // Keep the independent overlapping shells in one operand, exercising Auto's
    // self-intersection detection rather than just a two-solid regular Boolean.
    let mut p=a.positions().to_vec();p.extend(b.positions());let mut t=a.indices().to_vec();t.extend(b.indices().iter().map(|v|v.map(|i|i+8)));
    let both=Mesh::from_indexed(p,t,true).unwrap();assert!(both.inspect().watertight);
    let tool=cube(DVec3::splat(-2.),DVec3::splat(3.));
    let out=meshops::boolean(&both,&tool,Bool::Intersect).unwrap();closed(&out);close(out.volume(),1.875,1e-6);assert_eq!(out.inspect().components,1);
}

#[test]
fn repeated_boolean_edit_save_rebuild_and_stl_roundtrip_are_consistent() {
    let mut s=Session::default();let a=sphere(5.,DVec3::ZERO);let b=sphere(3.,DVec3::X*4.);
    let target=s.edit(|d|Ok(d.add_feature(FeatureKind::Import(a)))).unwrap();let tool=s.edit(|d|Ok(d.add_feature(FeatureKind::Import(b)))).unwrap();
    execute(&mut s,&json!({"op":"combine","target":target,"tools":[tool],"operation":"cut"}),None).unwrap();
    let before=s.built.bodies[0].mesh.clone();closed(&before);
    let dir=std::env::temp_dir().join(format!("ferrender-robust-boolean-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();
    let file=dir.join("boolean.ferr");s.save(&file).unwrap();let again=Session::open(&file).unwrap();assert!(again.built.errors.is_empty());assert_eq!(again.built.bodies.len(),1);closed(&again.built.bodies[0].mesh);close(again.built.bodies[0].mesh.volume(),before.volume(),1e-9);
    let mut doc=io::load(&file).unwrap();let rebuilt=doc.rebuild();assert!(rebuilt.errors.is_empty());closed(&rebuilt.bodies[0].mesh);assert_eq!(rebuilt.bodies[0].mesh,before);
    let stl=dir.join("boolean.stl");io::write_stl(rebuilt.bodies.iter(),Unit::Mm,&stl).unwrap();let(imported,report)=io::import_mesh(&stl,Unit::Mm).unwrap();assert!(report.watertight,"{report:?}");close(imported.volume(),before.volume(),1e-9);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn translated_rotated_thin_overlaps_are_not_epsilon_guessed() {
    let rotate=DMat3::from_rotation_z(0.27)*DMat3::from_rotation_y(0.33);
    for offset in[DVec3::ZERO,DVec3::new(1000.,-500.,750.),DVec3::splat(10000.)] {
        let mut a=cube(DVec3::ZERO,DVec3::splat(10.));let mut b=cube(DVec3::new(9.99,0.,0.),DVec3::new(19.99,10.,10.));
        a.map(|p|rotate*p+offset);b.map(|p|rotate*p+offset);
        for(op,volume)in[(Bool::Intersect,1.),(Bool::Subtract,999.),(Bool::Union,1999.)] {let out=meshops::boolean(&a,&b,op).unwrap();closed(&out);close(out.volume(),volume,0.08);}
    }
}

#[test]
#[ignore="read-only local private fixture; set FERRENDER_PRIVATE_RELIEF"]
fn private_relief_rebuilds_and_exports_a_closed_solid() {
    let path=std::env::var("FERRENDER_PRIVATE_RELIEF").expect("set FERRENDER_PRIVATE_RELIEF");let path=std::path::Path::new(&path);let original=std::fs::read(path).unwrap();
    let start=std::time::Instant::now();let mut doc=io::load(path).unwrap();let built=doc.rebuild();println!("private relief: {}ms, {} features, {} bodies, errors {:?}",start.elapsed().as_millis(),doc.features.len(),built.bodies.len(),built.errors);
    assert!(built.errors.is_empty(),"{:?}",built.errors);assert_eq!(built.bodies.len(),1);let mesh=&built.bodies[0].mesh;closed(mesh);println!("{}; volume{}",mesh.inspect().summary(),mesh.volume());
    // Encode/decode through actual STL without putting any personal geometry in Git.
    let dir=std::env::temp_dir().join(format!("ferrender-private-relief-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();let stl=dir.join("roundtrip.stl");io::write_stl(built.bodies.iter(),Unit::Mm,&stl).unwrap();let(_,report)=io::import_mesh(&stl,Unit::Mm).unwrap();std::fs::remove_dir_all(dir).unwrap();assert!(report.watertight,"{report:?}");assert_eq!(std::fs::read(path).unwrap(),original,"the private source must never be changed");
}
