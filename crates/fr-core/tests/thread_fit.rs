use fr_core::{Session, api};
use serde_json::{Value, json};
use glam::DVec3;

fn run(s: &mut Session, c: Value) -> Value { api::execute(s, &c, None).unwrap_or_else(|e| panic!("{c}: {e}")) }
fn screw(size: &str, chamfer: bool, offset: f64) -> Session {
    let t = fr_core::threads::find(size).unwrap();
    let mut s = Session::default();
    run(&mut s, json!({"op":"create_sketch"}));
    run(&mut s, json!({"op":"add_geometry","items":[{"type":"rect","from":[-t.major,-t.major],"to":[t.major,t.major]}]}));
    run(&mut s, json!({"op":"extrude","distance":2}));
    run(&mut s, json!({"op":"create_sketch","offset":2}));
    run(&mut s, json!({"op":"add_geometry","items":[{"type":"circle","center":[0,0],"radius":t.major/2.0}]}));
    run(&mut s, json!({"op":"extrude","distance":12,"operation":"join"}));
    if chamfer { run(&mut s, json!({"op":"chamfer_edges","body":2,"edges":[[t.major/2.0,0,14]],"distance":0.6*t.pitch})); }
    run(&mut s, json!({"op":"thread","body":2,"face":[t.major/2.0,0,8],"thread":size,"allowance":0.2,"offset":offset}));
    assert!(s.built.errors.is_empty(), "{:?}",s.built.errors);
    s
}

fn radial_diameter(s: &Session, z: f64, angle: f64) -> f64 {
    let d = DVec3::new(angle.cos(), angle.sin(), 0.0);
    let origin = d*30.0 + DVec3::Z*z;
    2.0*(30.0-s.built.body(2).unwrap().mesh.ray(origin,-d).unwrap().0)
}

#[test]
fn chamfer_before_thread_does_not_leave_a_blocking_collar() {
    for size in ["M3","M6"] {
        let t=fr_core::threads::find(size).unwrap();
        let s=screw(size,true,0.0);
        let root=t.minor()-0.2;
        let z=14.0-0.6*t.pitch+1e-4;
        for k in 0..24 {
            let diameter=radial_diameter(&s,z,k as f64*std::f64::consts::TAU/24.0);
            assert!(diameter <= root+1e-5,"{size}: unthreaded collar {diameter} exceeds root {root}");
        }
        let b=s.built.body(2).unwrap();
        let (lo,hi)=b.mesh.bbox().unwrap();
        assert!((hi.z-14.0).abs()<1e-6,"keep overall screw length");
        assert!((hi.x-t.major).abs()<1e-6 && (lo.x+t.major).abs()<1e-6,"keep screw head");
        assert_eq!(b.threads[0].open_edges(),0,"thread lead stays watertight");
        let bytes=fr_core::io::stl_bytes([b],fr_core::Unit::Mm);
        let imported=fr_core::io::parse_stl(&bytes,fr_core::Unit::Mm).unwrap();
        assert!(imported.tris().flatten().all(|v|v.is_finite()));
        println!("{size}: collar diameter {:.4}, thread root {root:.4}; head and length preserved",radial_diameter(&s,z,0.0));
    }
}

#[test]
fn a_free_thread_tip_has_a_lead_but_an_offset_thread_leaves_the_tip_alone() {
    for size in ["M3","M6"] {
        let t=fr_core::threads::find(size).unwrap();
        let s=screw(size,false,0.0);
        let mesh=&s.built.body(2).unwrap().threads[0];
        for v in mesh.tris().flatten().filter(|v|(v.z-14.).abs()<1e-8) {
            assert!(v.truncate().length() <= (t.minor()-0.2)/2.0+1e-6);
        }
        assert_eq!(mesh.open_edges(),0);
        let s=screw(size,true,1.0);
        let diameter=radial_diameter(&s,14.0-0.6*t.pitch+1e-4,0.0);
        assert!(diameter>t.major-0.01,"partial thread must not trim an unrelated end");
    }
}
