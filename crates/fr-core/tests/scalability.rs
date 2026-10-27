//! Opt-in 5M/12M import and container checks, without expensive edit operations.
//! Set FERRENDER_SCALE_TRIANGLES (5000000 or 12000000) and FERRENDER_SCALE_DIR.
use std::{fs::{self, File}, io::{BufWriter, Write}, path::Path, time::Instant};
use fr_core::{Session, api::execute};
use glam::DVec3;
use serde_json::json;

fn synthetic(path: &Path, triangles: usize) {
    let (rows, cols) = match triangles { 5_000_000 => (1001usize,2500usize), 12_000_000 => (2001,3000), _ => panic!("choose 5000000 or 12000000") };
    let at = |i: usize,j: usize| {
        if i==0 {return DVec3::Z*50.;} if i==rows {return -DVec3::Z*50.;}
        let u=i as f64/rows as f64*std::f64::consts::PI;
        let v=(j%cols) as f64/cols as f64*std::f64::consts::TAU;
        let r=50.+2.*(u*40.).sin()*(v*30.).cos();
        DVec3::new(r*u.sin()*v.cos(),r*u.sin()*v.sin(),r*u.cos())
    };
    let mut out=BufWriter::with_capacity(4<<20,File::create(path).unwrap());
    out.write_all(&[0u8;80]).unwrap();out.write_all(&(triangles as u32).to_le_bytes()).unwrap();
    let mut count=0;
    for i in 0..rows {for j in 0..cols {
        let (a,b,c,d)=(at(i,j),at(i+1,j),at(i+1,j+1),at(i,j+1));
        for (keep,t) in [(i+1<rows,[a,b,c]),(i>0,[a,c,d])] {
            if !keep {continue;}
            let mut record=[0u8;50];
            for (k,x) in t.into_iter().flat_map(|p|p.as_vec3().to_array()).enumerate() {record[12+k*4..16+k*4].copy_from_slice(&x.to_le_bytes());}
            out.write_all(&record).unwrap();count+=1;
        }
    }}
    out.flush().unwrap();assert_eq!(count,triangles);
}

#[test]
#[ignore = "opt-in large mesh test; requires FERRENDER_SCALE_TRIANGLES and FERRENDER_SCALE_DIR"]
fn large_mesh_import_save_reopen_and_pick() {
    let triangles:usize=std::env::var("FERRENDER_SCALE_TRIANGLES").unwrap().parse().unwrap();
    let dir=std::path::PathBuf::from(std::env::var_os("FERRENDER_SCALE_DIR").unwrap());fs::create_dir_all(&dir).unwrap();
    let stl=dir.join(format!("synthetic-{triangles}.stl"));let ferr=dir.join(format!("synthetic-{triangles}.ferr"));
    let t=Instant::now();synthetic(&stl,triangles);let generate_s=t.elapsed().as_secs_f64();println!("generated {triangles} triangles in {generate_s:.3}s");
    let mut session=Session::default();let t=Instant::now();
    execute(&mut session,&json!({"op":"import_mesh","path":stl,"units":"mm"}),None).unwrap();
    let import_s=t.elapsed().as_secs_f64();println!("import {triangles}: {import_s:.3}s");
    assert!(session.built.errors.is_empty());assert_eq!(session.built.bodies.len(),1);
    let mesh=&session.built.bodies[0].mesh;assert_eq!(mesh.len(),triangles);assert!(mesh.is_welded());
    let vertex_count=mesh.vertex_count();let bounds=mesh.bbox().unwrap();let volume=mesh.volume();assert!(volume>0.);
    let t=Instant::now();let _=mesh.bvh();let bvh_s=t.elapsed().as_secs_f64();
    let t=Instant::now();for _ in 0..100 {let hit=mesh.ray(DVec3::Z*150.,-DVec3::Z).unwrap();assert!((100.-hit.0).abs()<1e-6);}let pick_ms=t.elapsed().as_secs_f64()*10.;
    let t=Instant::now();let nearest=mesh.nearest_tri(DVec3::new(60.,1.,2.));let nearest_ms=t.elapsed().as_secs_f64()*1000.;assert!(nearest.is_some());
    println!("BVH {bvh_s:.3}s; mean pick {pick_ms:.5}ms; nearest {nearest_ms:.5}ms");
    let t=Instant::now();let saved=session.save(&ferr).unwrap();let save_s=t.elapsed().as_secs_f64();assert!(saved.container);let bytes=fs::metadata(&ferr).unwrap().len();
    println!("save {save_s:.3}s; container {bytes} bytes");drop(session);
    let t=Instant::now();let reopened=Session::open(&ferr).unwrap();let open_s=t.elapsed().as_secs_f64();
    assert!(reopened.built.errors.is_empty());assert_eq!(reopened.built.bodies.len(),1);
    let mesh=&reopened.built.bodies[0].mesh;assert_eq!(mesh.len(),triangles);assert_eq!(mesh.vertex_count(),vertex_count);assert_eq!(mesh.bbox().unwrap(),bounds);assert!((mesh.volume()-volume).abs()<1e-6);
    println!("reopen {open_s:.3}s; preserved {triangles} triangles, {vertex_count} vertices, bounds and volume");
    let result=json!({"triangles":triangles,"vertices":vertex_count,"generation_s":generate_s,"import_s":import_s,"bvh_s":bvh_s,"pick_ms":pick_ms,"nearest_ms":nearest_ms,"save_s":save_s,"open_s":open_s,"container_bytes":bytes,"stl_bytes":fs::metadata(&stl).unwrap().len(),"volume":volume,"passed":true});
    fs::write(dir.join(format!("result-{triangles}.json")),serde_json::to_vec_pretty(&result).unwrap()).unwrap();
}
