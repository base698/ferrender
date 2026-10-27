use fr_core::{api,io,Session,FeatureKind};
use glam::DVec3;
use serde_json::{json,Value};
fn run(s:&mut Session,c:Value)->Value { api::execute(s,&c,None).unwrap_or_else(|e|panic!("{c}: {e}")) }
fn id(v:&Value,key:&str)->u32 {v[key].as_u64().unwrap() as u32}
fn box_part(s:&mut Session)->u32 {
    run(s,json!({"op":"create_sketch","plane":"XY"}));
    run(s,json!({"op":"add_geometry","items":[{"type":"rect","from":[0,0],"to":[40,20]}]}));
    id(&run(s,json!({"op":"extrude","distance":10,"operation":"new"})),"feature")
}
fn post(s:&mut Session,plane:u32)->u32 {
    run(s,json!({"op":"create_sketch","plane":{"id":plane}}));
    run(s,json!({"op":"add_geometry","items":[{"type":"circle","center":[20,10],"radius":3}]}));
    id(&run(s,json!({"op":"extrude","distance":3,"operation":"new"})),"feature")
}
fn close(a:f64,b:f64) {assert!((a-b).abs()<1e-5,"{a} != {b}");}

#[test]
fn offsets_keep_axes_compose_and_follow_parameters() {
    let mut s=Session::default();
    for (base,normal) in [("XY",DVec3::Z),("XZ",-DVec3::Y),("YZ",DVec3::X)] {
        let p=id(&run(&mut s,json!({"op":"create_plane","kind":"offset","base":base,"distance":-5})),"feature");
        assert!(s.built.planes[&p].plane.origin.distance(normal*-5.)<1e-9);
        let q=id(&run(&mut s,json!({"op":"create_plane","kind":"offset","base":{"plane":p},"distance":8})),"feature");
        assert!(s.built.planes[&q].plane.origin.distance(normal*3.)<1e-9);
    }
    run(&mut s,json!({"op":"set_parameter","name":"h","expr":"12 mm"}));
    let p=id(&run(&mut s,json!({"op":"create_plane","kind":"offset","base":"XY","distance":"$h"})),"feature");
    let body=post(&mut s,p);
    close(s.built.body(body).unwrap().mesh.bbox().unwrap().0.z,12.);
    run(&mut s,json!({"op":"set_parameter","name":"h","expr":"19 mm"}));
    close(s.built.body(body).unwrap().mesh.bbox().unwrap().0.z,19.);
    assert!(s.built.errors.is_empty());
}

#[test]
fn face_plane_follows_growth_and_broken_references_never_build_stale_geometry() {
    let mut s=Session::default(); let b=box_part(&mut s);
    let p=id(&run(&mut s,json!({"op":"create_plane","kind":"offset","base":{"face":{"body":b,"point":[20,10,10]}},"distance":5})),"feature");
    let body=post(&mut s,p);
    close(s.built.body(body).unwrap().mesh.bbox().unwrap().0.z,15.);
    run(&mut s,json!({"op":"edit_feature","feature":b,"distance":20}));
    close(s.built.planes[&p].plane.origin.z,25.);
    close(s.built.body(body).unwrap().mesh.bbox().unwrap().1.z,28.);
    run(&mut s,json!({"op":"delete_feature","feature":b}));
    assert!(s.built.errors.contains_key(&p));
    assert!(s.built.errors[&body].contains("sketch could not be placed"));
    assert!(s.built.body(body).is_none());
    let reopened=Session::new(io::from_json(&io::to_json(&s.doc)).unwrap());
    assert!(reopened.built.body(body).is_none());
    s.undo(); assert!(s.built.errors.is_empty());
    run(&mut s,json!({"op":"edit_feature","feature":p,"suppressed":true}));
    assert!(s.built.body(body).is_none());
    s.undo(); run(&mut s,json!({"op":"rollback","to":b}));
    assert!(s.built.planes.is_empty()); assert_eq!(s.built.bodies.len(),1);
    run(&mut s,json!({"op":"rollback","to":"end"})); assert!(s.built.errors.is_empty());
}

#[test]
fn midplanes_parallel_and_angled_and_flip() {
    let mut s=Session::default(); let b=box_part(&mut s);
    let p=id(&run(&mut s,json!({"op":"create_plane","kind":"midplane","faces":[{"body":b,"point":[20,10,10]},{"body":b,"point":[20,10,0]}]})),"feature");
    close(s.built.planes[&p].plane.origin.z,5.);
    assert!(s.built.planes[&p].plane.x.distance(DVec3::X)<1e-8);
    let q=id(&run(&mut s,json!({"op":"create_plane","kind":"midplane","faces":[{"body":b,"point":[20,10,10]},{"body":b,"point":[40,10,5]}]})),"feature");
    let a=s.built.planes[&q].plane;
    close(a.normal().dot(DVec3::Z).abs(),std::f64::consts::FRAC_1_SQRT_2);
    close((DVec3::new(40.,0.,10.)-a.origin).dot(a.normal()),0.);
    run(&mut s,json!({"op":"edit_feature","feature":q,"flip":true}));
    close(a.normal().dot(s.built.planes[&q].plane.normal()),0.);
    let before=s.doc.clone();
    assert!(api::execute(&mut s,&json!({"op":"create_plane","kind":"midplane","faces":[{"body":b,"point":[20,10,10]},{"body":b,"point":[25,10,10]}]}),None).is_err());
    assert_eq!(s.doc,before);
}

#[test]
fn three_points_follow_vertices_and_editable_sketch_points() {
    let mut s=Session::default(); let b=box_part(&mut s);
    let p=id(&run(&mut s,json!({"op":"create_plane","kind":"three_point","points":[{"body":b,"point":[0,0,0]},{"body":b,"point":[40,0,0]},{"body":b,"point":[0,20,10]}]})),"feature");
    let plane=s.built.planes[&p].plane;
    close((DVec3::new(0.,20.,10.)-plane.origin).dot(plane.normal()),0.);
    let sk=id(&run(&mut s,json!({"op":"create_sketch","plane":"XY"})),"sketch");
    let point=id(&run(&mut s,json!({"op":"point_coordinates","sketch":sk,"x":5,"y":5})),"point");
    let p=id(&run(&mut s,json!({"op":"create_plane","kind":"three_point","points":[[0,0,0],[0,0,10],{"sketch":sk,"point":point}]})),"feature");
    run(&mut s,json!({"op":"point_coordinates","sketch":sk,"point":point,"x":10,"y":5}));
    close(DVec3::new(10.,5.,0.).dot(s.built.planes[&p].plane.normal()),0.);
    let before=s.doc.clone();
    assert!(api::execute(&mut s,&json!({"op":"create_plane","kind":"three_point","points":[[0,0,0],[1,1,1],[2,2,2]]}),None).is_err());
    assert_eq!(s.doc,before);
}

#[test]
fn plane_file_roundtrip_validates_order_and_keeps_legacy_schema() {
    let mut s=Session::default(); box_part(&mut s);
    assert_eq!(serde_json::from_str::<Value>(&io::to_json(&s.doc)).unwrap()["version"],1);
    let p=id(&run(&mut s,json!({"op":"create_plane","kind":"offset","base":"XY","distance":"gap = 3 mm"})),"feature");
    let body=post(&mut s,p);
    run(&mut s,json!({"op":"set_visible","id":p,"visible":false}));
    let encoded=io::to_json(&s.doc);
    assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap()["version"],5);
    assert_eq!(s.doc,io::from_json(&encoded).unwrap());
    let mut bad=s.doc.clone();
    let FeatureKind::Plane(plane)=&mut bad.feature_mut(p).unwrap().kind else {panic!()};
    if let fr_core::planes::PlaneKind::Offset {base,..}=&mut plane.kind { *base=fr_core::planes::PlaneRef::Face {tag:None,body,at:DVec3::ZERO,frame:None}; }
    assert!(io::from_json(&io::to_json(&bad)).unwrap_err().contains("earlier"));
    let mut dangling=s.doc.clone(); dangling.features.retain(|f|f.id!=p);
    let reopened=Session::new(io::from_json(&io::to_json(&dangling)).unwrap());
    assert!(reopened.built.body(body).is_none());
}

#[test]
fn invalid_plane_edits_are_rejected_even_when_the_feature_is_not_currently_built() {
    let mut s = Session::default();
    let base = box_part(&mut s);
    let plane = id(&run(&mut s, json!({"op":"create_plane", "kind":"offset", "base":"XY", "distance":5})), "feature");
    run(&mut s, json!({"op":"rollback", "to":base}));
    for command in [
        json!({"op":"edit_feature", "feature":plane, "kind":"three_point", "points":[[0,0,0],[1,1,1],[2,2,2]]}),
        json!({"op":"edit_feature", "feature":plane, "kind":"offset", "base":{"plane":999999}, "distance":"uncommitted_gap = 7 mm"}),
    ] {
        let before = s.doc.clone();
        assert!(api::execute(&mut s, &command, None).is_err(), "invalid rolled-back plane edit succeeded: {command}");
        assert_eq!(s.doc, before, "rejected plane edit changed geometry, parameters or history marker");
    }
    run(&mut s, json!({"op":"rollback", "to":"end"}));
    run(&mut s, json!({"op":"edit_feature", "feature":plane, "suppressed":true}));
    let before = s.doc.clone();
    assert!(api::execute(&mut s, &json!({"op":"edit_feature", "feature":plane, "kind":"three_point", "points":[[0,0,0],[1,1,1],[2,2,2]]}), None).is_err());
    assert_eq!(s.doc, before, "invalid suppressed plane edit must not be deferred until unsuppression");
    run(&mut s, json!({"op":"edit_feature", "feature":plane, "suppressed":false}));
    close(s.built.planes[&plane].plane.origin.z, 5.);
}
