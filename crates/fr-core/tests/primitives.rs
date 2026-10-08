use fr_core::{api, io, Body, FeatureKind, Session, Unit};
use fr_core::primitives::PrimitiveShape;
use glam::DVec3;
use serde_json::{json, Value};
use std::f64::consts::PI;

fn run(s: &mut Session, c: Value) -> Value { api::execute(s,&c,None).unwrap_or_else(|e|panic!("{c}: {e}")) }
fn id(v: &Value) -> u32 { v["feature"].as_u64().unwrap() as u32 }
fn primitive(s: &mut Session, mut c: Value) -> u32 { c["op"] = json!("primitive"); id(&run(s,c)) }
fn volume(b: &Body) -> f64 { b.solids.iter().map(|s|s.volume()).sum() }
fn near(a: f64,b: f64) { assert!((a-b).abs() < b.abs().max(1.)*1e-6,"{a} != {b}"); }
fn bounds(b: &Body,lo: [f64;3],hi: [f64;3]) {
    let (a,z)=b.mesh.bbox().unwrap();
    // Display vertices approximate curved extrema to the 0.02 mm deflection;
    // kernel bounding boxes add a further tolerance and are not exact extrema.
    assert!((a-DVec3::from_array(lo)).abs().max_element()<0.025 && (z-DVec3::from_array(hi)).abs().max_element()<0.025,"{a:?}..{z:?} != {lo:?}..{hi:?}");
}

#[test]
fn primitives_have_expected_exact_volume_bounds_and_topology() {
    for (command,expected,lo,hi,faces) in [
        (json!({"type":"box","width":10,"depth":20,"height":30}),6000.,[0.,0.,0.],[10.,20.,30.],6),
        (json!({"type":"cylinder","diameter":10,"height":12}),300.*PI,[-5.,-5.,0.],[5.,5.,12.],3),
        (json!({"type":"sphere","diameter":10}),500.*PI/3.,[-5.,-5.,-5.],[5.,5.,5.],1),
        (json!({"type":"cone","bottom_diameter":10,"height":12}),100.*PI,[-5.,-5.,0.],[5.,5.,12.],2),
        (json!({"type":"cone","bottom_diameter":0,"top_diameter":10,"height":12}),100.*PI,[-5.,-5.,0.],[5.,5.,12.],2),
        (json!({"type":"cone","bottom_diameter":10,"top_diameter":4,"height":12}),156.*PI,[-5.,-5.,0.],[5.,5.,12.],3),
        (json!({"type":"cone","bottom_diameter":10,"top_diameter":10,"height":12}),300.*PI,[-5.,-5.,0.],[5.,5.,12.],3),
        (json!({"type":"torus","major_radius":10,"tube_radius":2}),80.*PI*PI,[-12.,-12.,-2.],[12.,12.,2.],1),
        // Exactly the allowed gap must survive binary floating-point rounding.
        (json!({"type":"torus","major_radius":10,"tube_radius":9.999}),20.*PI*PI*9.999_f64.powi(2),[-19.999,-19.999,-9.999],[19.999,19.999,9.999],1),
    ] {
        let mut s=Session::default();
        let body=primitive(&mut s,command.clone());
        assert_eq!(s.doc.features.len(),1,"a primitive must not create hidden sketches");
        let b=s.built.body(body).unwrap();
        assert!(b.is_exact()); near(volume(b),expected); bounds(b,lo,hi);
        assert_eq!(b.solids.iter().map(|s|s.iter_face().count()).sum::<usize>(),faces);
        assert_eq!(b.mesh.open_edges(),0,"unclosed mesh for {command}");
        let info=run(&mut s,json!({"op":"get_object_info","id":body}));
        assert_eq!(info["type"],"primitive"); assert_eq!(info["shape"],command["type"]);
        assert_eq!(info["topology"]["faces"].as_array().unwrap().len(),faces);
        assert!(s.doc.feature(body).unwrap().name.ends_with('1'));
    }
}

#[test]
fn primitive_rotation_order_parameters_editing_and_undo_keep_local_history() {
    let mut s=Session::default();
    run(&mut s,json!({"op":"set_parameter","name":"width","expr":"2 mm"}));
    run(&mut s,json!({"op":"set_parameter","name":"offset","expr":"10 mm"}));
    let body=primitive(&mut s,json!({"type":"box","width":"$width","depth":3,"height":4,"position":["$offset",20,30],"rotate":[90,0,90]}));
    // Rx sends (x,y,z) to (x,-z,y); Rz then sends it to (z,x,y).
    bounds(s.built.body(body).unwrap(),[10.,20.,30.],[14.,22.,33.]);
    run(&mut s,json!({"op":"set_parameter","name":"width","expr":"5 mm"}));
    bounds(s.built.body(body).unwrap(),[10.,20.,30.],[14.,25.,33.]);
    let before=s.doc.clone();
    run(&mut s,json!({"op":"edit_feature","feature":body,"position":[-10,0,0],"rotate":[0,0,0],"height":8}));
    assert_eq!(s.doc.features.len(),1);
    bounds(s.built.body(body).unwrap(),[-10.,0.,0.],[-5.,3.,8.]);
    assert!(s.undo()); assert_eq!(s.doc,before);
    bounds(s.built.body(body).unwrap(),[10.,20.,30.],[14.,25.,33.]);
    assert!(s.redo()); bounds(s.built.body(body).unwrap(),[-10.,0.,0.],[-5.,3.,8.]);
    run(&mut s,json!({"op":"set_units","units":"in"}));
    let info=run(&mut s,json!({"op":"get_object_info","id":body}));
    near(info["height"]["value"].as_f64().unwrap(),0.315);
    near(info["position"][0]["value"].as_f64().unwrap(),-0.3937);
    bounds(s.built.body(body).unwrap(),[-10.,0.,0.],[-5.,3.,8.]);
}

#[test]
fn invalid_primitives_are_rejected_atomically_even_when_the_feature_is_unbuilt() {
    let mut s=Session::default();
    let body=primitive(&mut s,json!({"type":"box","width":10,"depth":10,"height":10}));
    for command in [
        json!({"type":"box","width":0,"depth":10,"height":10}),
        json!({"type":"box","width":-1,"depth":10,"height":10}),
        json!({"type":"box","width":10001,"depth":10,"height":10}),
        json!({"type":"box","width":0.000001,"depth":10,"height":10}),
        json!({"type":"sphere","diameter":"1/0"}),
        json!({"type":"sphere","diameter":10,"position":[0,0]}),
        json!({"type":"sphere","diameter":10,"position":[0,0,1000001]}),
        json!({"type":"sphere","diameter":10,"rotate":[0,0,"1/0"]}),
        json!({"type":"sphere","diameter":10,"width":3}),
        json!({"type":"cylinder","diameter":10,"height":0}),
        json!({"type":"cone","bottom_diameter":0,"top_diameter":0,"height":10}),
        json!({"type":"cone","bottom_diameter":10,"top_diameter":-1,"height":10}),
        json!({"type":"cone","bottom_diameter":10,"top_diameter":10.00000001,"height":10}),
        json!({"type":"torus","major_radius":10,"tube_radius":10}),
        json!({"type":"torus","major_radius":10,"tube_radius":11}),
        json!({"type":"torus","major_radius":10,"tube_radius":9.99999}),
    ] {
        let mut command=command; command["op"]=json!("primitive");
        let before=s.doc.clone();
        assert!(api::execute(&mut s,&command,None).is_err(),"accepted {command}");
        assert_eq!(s.doc,before); assert_eq!(s.built.bodies.len(),1);
    }
    for state in ["suppressed","rolled_back"] {
        if state=="suppressed" { run(&mut s,json!({"op":"edit_feature","feature":body,"suppressed":true})); }
        else { run(&mut s,json!({"op":"rollback","to":"start"})); }
        let before=s.doc.clone();
        assert!(api::execute(&mut s,&json!({"op":"edit_feature","feature":body,"width":0,"name":"Should not survive"}),None).is_err());
        assert_eq!(s.doc,before);
    }
}

#[test]
fn primitive_operations_and_patterns_stay_in_the_source_component() {
    let mut s=Session::default();
    let root=primitive(&mut s,json!({"type":"box","width":20,"depth":20,"height":10}));
    let owner=run(&mut s,json!({"op":"create_component","name":"Plate"}))["component"].as_u64().unwrap() as u32;
    let plate=primitive(&mut s,json!({"type":"box","width":20,"depth":20,"height":10,"operation":"join"}));
    assert_eq!(s.built.bodies.len(),2);
    let hole=primitive(&mut s,json!({"type":"cylinder","diameter":2,"height":12,"position":[5,5,-1],"operation":"cut"}));
    run(&mut s,json!({"op":"activate_component","id":0}));
    let pattern=id(&run(&mut s,json!({"op":"pattern","feature":hole,"type":"linear","axis":"x","count":2,"spacing":10,"axis2":"y","count2":2,"spacing2":10})));
    assert_eq!(s.doc.feature(pattern).unwrap().owner,owner);
    near(volume(s.built.body(root).unwrap()),4000.);
    near(volume(s.built.body(plate).unwrap()),4000.-40.*PI);
    run(&mut s,json!({"op":"activate_component","id":owner}));
    let post=primitive(&mut s,json!({"type":"sphere","diameter":2,"position":[1,2,20]}));
    let copies=id(&run(&mut s,json!({"op":"pattern","feature":post,"type":"linear","axis":"x","count":2,"spacing":8})));
    run(&mut s,json!({"op":"move_component","id":owner,"translate":[100,200,300],"rotate":[0,0,90]}));
    bounds(s.built.body(post).unwrap(),[97.,200.,319.],[99.,202.,321.]);
    bounds(s.built.body(copies*1000+1).unwrap(),[97.,208.,319.],[99.,210.,321.]);
    near(volume(s.built.body(root).unwrap()),4000.);
    assert!(s.built.errors.is_empty());
}

#[test]
fn primitives_support_face_sketches_planes_holes_and_exact_modifiers() {
    let mut s=Session::default();
    let body=primitive(&mut s,json!({"type":"box","width":20,"depth":20,"height":10}));
    let plane=id(&run(&mut s,json!({"op":"create_plane","kind":"offset","base":{"face":{"body":body,"point":[10,10,10]}},"distance":2})));
    let sketch=run(&mut s,json!({"op":"create_sketch","plane":{"id":plane}}))["sketch"].as_u64().unwrap() as u32;
    assert_eq!(s.doc.sketch(sketch).unwrap().on,Some(plane));
    run(&mut s,json!({"op":"edit_feature","feature":body,"height":15}));
    near(s.built.planes[&plane].plane.origin.z,17.);
    run(&mut s,json!({"op":"chamfer_edges","body":body,"edges":[[10,0,15]],"distance":1}));
    assert!(s.built.body(body).unwrap().is_exact());
    run(&mut s,json!({"op":"hole","body":body,"at":[10,10,15],"diameter":2,"fit":"plain","through":true}));
    assert!(volume(s.built.body(body).unwrap()) < 6000.-15.*PI);
    let rod=primitive(&mut s,json!({"type":"cylinder","diameter":6,"height":10,"position":[40,0,0]}));
    run(&mut s,json!({"op":"thread","body":rod,"face":[43,0,5],"thread":"M6","allowance":0.2}));
    assert_eq!(s.built.body(rod).unwrap().threads.len(),1);
    assert!(s.built.errors.is_empty());
}

#[test]
fn native_primitive_validation_schema_roundtrip_and_exports() {
    let mut s=Session::default();
    assert_eq!(serde_json::from_str::<Value>(&io::to_json(&s.doc)).unwrap()["version"],1);
    let body=primitive(&mut s,json!({"type":"torus","major_radius":8,"tube_radius":2,"position":[10,20,30]}));
    let native=io::validated_json(&s.doc).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&native).unwrap()["version"],7);
    let reopened=Session::new(io::from_json(&native).unwrap());
    assert_eq!(reopened.doc,s.doc); near(volume(reopened.built.body(body).unwrap()),64.*PI*PI);
    for problem in ["negative","expr","position"] {
        let mut bad=s.doc.clone();
        let FeatureKind::Primitive(p)=&mut bad.feature_mut(body).unwrap().kind else {panic!()};
        match problem {
            "negative" => {let PrimitiveShape::Torus {tube_radius,..}=&mut p.shape else {panic!()}; tube_radius.v=-1.;},
            "expr" => p.rotate[0].expr="0".repeat(4097),
            "position" => p.position[0].v=f64::INFINITY,
            _=>unreachable!(),
        }
        assert!(io::validated_json(&bad).is_err());
        assert!(io::from_json(&io::to_json(&bad)).is_err());
    }
    let dir=std::env::temp_dir().join(format!("ferrender-primitive-export-{}",std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let stl=dir.join("torus.stl"); let step=dir.join("torus.step");
    run(&mut s,json!({"op":"export_stl","path":stl}));
    let exported=io::parse_stl(&std::fs::read(&stl).unwrap(),Unit::Mm).unwrap();
    assert_eq!(exported.open_edges(),0);
    assert!((exported.volume().abs()-64.*PI*PI).abs() < 64.*PI*PI*0.02);
    let result=run(&mut s,json!({"op":"export_step","path":step}));
    assert_eq!(result["solids"],1);
    assert!(std::fs::metadata(&step).unwrap().len()>1000);
    std::fs::remove_dir_all(&dir).unwrap();
}
