use fr_core::{api,io,Body,FeatureKind,Session};
use glam::DVec3;
use serde_json::{json,Value};

fn run(s:&mut Session,c:Value)->Value {api::execute(s,&c,None).unwrap_or_else(|e|panic!("{c}: {e}"))}
fn id(v:&Value)->u32 {v["feature"].as_u64().unwrap() as u32}
fn block(s:&mut Session,position:[f64;3],size:[f64;3])->u32 {
    id(&run(s,json!({"op":"primitive","type":"box","width":size[0],"depth":size[1],"height":size[2],"position":position})))
}
fn plane(s:&mut Session,distance:f64)->u32 {id(&run(s,json!({"op":"create_plane","kind":"offset","base":"XY","distance":distance})))}
fn vol(b:&Body)->f64 {b.solids.iter().map(|s|s.volume()).sum()}
fn near(a:f64,b:f64) {assert!((a-b).abs()<1e-6*b.abs().max(1.),"{a} != {b}");}
fn bounds(b:&Body,lo:[f64;3],hi:[f64;3]) {
    let (a,z)=b.mesh.bbox().unwrap();assert!(a.distance(DVec3::from_array(lo))<1e-5 && z.distance(DVec3::from_array(hi))<1e-5,"{a:?}..{z:?} != {lo:?}..{hi:?}");
}

#[test]
fn removing_the_source_cone_preserves_its_mirror_and_history() {
    let mut s=Session::default();
    let source=id(&run(&mut s,json!({"op":"primitive","type":"cone","bottom_diameter":10,"height":15,"position":[20,0,0]})));
    let pattern=id(&run(&mut s,json!({"op":"pattern","feature":source,"type":"mirror","normal":"x"})));
    let copy=pattern*1000+1;
    let expected=vol(s.built.body(copy).unwrap());
    let before=s.doc.clone();
    let removed=run(&mut s,json!({"op":"remove_body","body":source}));
    let remove=id(&removed);
    assert_eq!(removed["removed_bodies"],json!([source]));
    assert!(s.doc.feature(source).is_some() && s.doc.feature(pattern).is_some());
    assert!(s.built.body(source).is_none());near(vol(s.built.body(copy).unwrap()),expected);
    assert_eq!(s.built.bodies.len(),1);
    assert_eq!(serde_json::from_str::<Value>(&io::validated_json(&s.doc).unwrap()).unwrap()["version"],8);
    let reopened=Session::new(io::from_json(&io::to_json(&s.doc)).unwrap());
    assert!(reopened.built.body(source).is_none());near(vol(reopened.built.body(copy).unwrap()),expected);
    assert!(s.undo());assert_eq!(s.doc,before);assert_eq!(s.built.bodies.len(),2);
    assert!(s.redo());assert_eq!(s.built.bodies.len(),1);
    run(&mut s,json!({"op":"edit_feature","feature":remove,"suppressed":true}));assert_eq!(s.built.bodies.len(),2);
    run(&mut s,json!({"op":"edit_feature","feature":remove,"suppressed":false}));assert_eq!(s.built.bodies.len(),1);
    run(&mut s,json!({"op":"rollback","to":pattern}));assert_eq!(s.built.bodies.len(),2);
    run(&mut s,json!({"op":"rollback","to":"end"}));assert_eq!(s.built.bodies.len(),1);
    run(&mut s,json!({"op":"delete_feature","feature":remove}));assert_eq!(s.built.bodies.len(),2);
}

#[test]
fn removing_multiple_component_bodies_is_all_or_nothing_when_a_source_disappears() {
    let mut s=Session::default();let root=block(&mut s,[0.;3],[10.;3]);
    let owner=run(&mut s,json!({"op":"create_component","name":"Other"}))["component"].as_u64().unwrap() as u32;
    let other=block(&mut s,[20.,0.,0.],[5.;3]);
    let remove=id(&run(&mut s,json!({"op":"remove_body","bodies":[root,other]})));
    assert_eq!(s.doc.feature(remove).unwrap().owner,0,"Remove follows its first explicit target, regardless of active component");
    assert!(s.built.bodies.is_empty());
    run(&mut s,json!({"op":"edit_feature","feature":owner,"suppressed":true}));
    assert!(s.built.errors.contains_key(&remove));
    near(vol(s.built.body(root).unwrap()),1000.);
    assert_eq!(s.built.bodies.len(),1,"one missing target must not partially remove the other component's body");
    run(&mut s,json!({"op":"edit_feature","feature":owner,"suppressed":false}));
    assert!(!s.built.errors.contains_key(&remove));assert!(s.built.bodies.is_empty());
    run(&mut s,json!({"op":"edit_feature","feature":remove,"suppressed":true}));
    assert_eq!(s.built.bodies.len(),2);
}

#[test]
fn split_pieces_can_move_remove_and_join_back_with_stable_ids() {
    let mut s=Session::default();let target=block(&mut s,[-5.,-4.,-3.],[10.,8.,6.]);
    let split=id(&run(&mut s,json!({"op":"split_body","body":target,"plane":"XY"})));
    let copy=split*1000+1;
    assert_eq!(s.built.bodies.len(),2);
    bounds(s.built.body(target).unwrap(),[-5.,-4.,-3.],[5.,4.,0.]);
    bounds(s.built.body(copy).unwrap(),[-5.,-4.,0.],[5.,4.,3.]);
    for b in &s.built.bodies {near(vol(b),240.);assert!(b.is_exact());assert_eq!(b.mesh.open_edges(),0);}
    let info=run(&mut s,json!({"op":"get_object_info","id":split}));assert_eq!(info["pieces"],json!([target,copy]));
    run(&mut s,json!({"op":"edit_feature","feature":split,"suppressed":true}));
    assert_eq!(run(&mut s,json!({"op":"get_object_info","id":split}))["pieces"],json!([]));
    run(&mut s,json!({"op":"edit_feature","feature":split,"suppressed":false}));
    run(&mut s,json!({"op":"rollback","to":target}));
    assert_eq!(run(&mut s,json!({"op":"get_object_info","id":split}))["pieces"],json!([]));
    run(&mut s,json!({"op":"rollback","to":"end"}));
    let info=run(&mut s,json!({"op":"get_object_info","id":copy}));assert_eq!(info["type"],"body");
    run(&mut s,json!({"op":"transform","body":copy,"translate":[20,0,0]}));
    bounds(s.built.body(copy).unwrap(),[15.,-4.,0.],[25.,4.,3.]);
    run(&mut s,json!({"op":"remove_body","bodies":[copy]}));assert!(s.built.body(copy).is_none());
    assert!(s.undo());assert!(s.undo());
    run(&mut s,json!({"op":"combine","target":target,"tools":[copy],"operation":"join"}));
    assert_eq!(s.built.bodies.len(),1);near(vol(s.built.body(target).unwrap()),480.);
    bounds(s.built.body(target).unwrap(),[-5.,-4.,-3.],[5.,4.,3.]);
    assert!(s.undo());assert_eq!(s.built.bodies.len(),2);
    let reopened=Session::new(io::from_json(&io::validated_json(&s.doc).unwrap()).unwrap());
    assert_eq!(reopened.built.bodies.iter().map(|b|b.id).collect::<Vec<_>>(),vec![target,copy]);
    assert_eq!(reopened.doc,s.doc);
}

#[test]
fn construction_plane_edits_move_the_cut_and_split_pieces_are_valid_plane_references() {
    let mut s=Session::default();let target=block(&mut s,[0.;3],[10.;3]);let cut=plane(&mut s,3.);
    let split=id(&run(&mut s,json!({"op":"split_body","body":target,"plane":{"plane":cut}})));
    let copy=split*1000+1;
    near(vol(s.built.body(target).unwrap()),300.);near(vol(s.built.body(copy).unwrap()),700.);
    run(&mut s,json!({"op":"edit_feature","feature":cut,"distance":4}));
    near(vol(s.built.body(target).unwrap()),400.);near(vol(s.built.body(copy).unwrap()),600.);
    let attached=id(&run(&mut s,json!({"op":"create_plane","kind":"offset","base":{"face":{"body":copy,"point":[5,5,4]}},"distance":1})));
    // The lower face points down, so its positive offset is below the cut.
    near(s.built.planes[&attached].plane.origin.z,3.);
    run(&mut s,json!({"op":"edit_feature","feature":cut,"distance":6}));
    near(s.built.planes[&attached].plane.origin.z,5.);
    io::validated_json(&s.doc).unwrap();
    run(&mut s,json!({"op":"delete_feature","feature":cut}));
    assert!(s.built.errors.contains_key(&split));assert_eq!(s.built.bodies.len(),1);
    assert_eq!(run(&mut s,json!({"op":"get_object_info","id":split}))["pieces"],json!([]));
    near(vol(s.built.body(target).unwrap()),1000.);
    run(&mut s,json!({"op":"edit_feature","feature":split,"suppressed":true}));
    assert!(!s.built.errors.contains_key(&split));
}

#[test]
fn a_planar_face_is_used_as_an_infinite_splitting_plane() {
    let mut s=Session::default();let target=block(&mut s,[0.;3],[10.;3]);
    let cutter=block(&mut s,[20.,20.,5.],[2.,2.,1.]);
    let split=id(&run(&mut s,json!({"op":"split_body","body":target,"plane":{"face":{"body":cutter,"point":[21,21,5]}}})));
    assert_eq!(s.built.bodies.len(),3);
    near(vol(s.built.body(target).unwrap()),500.);near(vol(s.built.body(split*1000+1).unwrap()),500.);
    near(vol(s.built.body(cutter).unwrap()),4.);
    bounds(s.built.body(target).unwrap(),[0.,0.,5.],[10.,10.,10.]);
}

#[test]
fn split_uses_target_local_origin_planes_and_cross_component_construction_planes() {
    let mut s=Session::default();
    let owner=run(&mut s,json!({"op":"create_component","name":"Target"}))["component"].as_u64().unwrap() as u32;
    let target=block(&mut s,[-5.,-5.,-5.],[10.;3]);
    run(&mut s,json!({"op":"move_component","id":owner,"translate":[100,200,300],"rotate":[0,90,0]}));
    run(&mut s,json!({"op":"activate_component","id":0}));
    let split=id(&run(&mut s,json!({"op":"split_body","body":target,"plane":"XY"})));
    assert_eq!(s.doc.feature(split).unwrap().owner,owner);
    bounds(s.built.body(target).unwrap(),[95.,195.,295.],[100.,205.,305.]);
    bounds(s.built.body(split*1000+1).unwrap(),[100.,195.,295.],[105.,205.,305.]);
    assert!(s.undo());
    let other=run(&mut s,json!({"op":"create_component","name":"Cutter"}))["component"].as_u64().unwrap() as u32;
    run(&mut s,json!({"op":"move_component","id":other,"translate":[101,200,300],"rotate":[0,90,0]}));
    let cut=plane(&mut s,0.);
    let split=id(&run(&mut s,json!({"op":"split_body","body":target,"plane":{"plane":cut}})));
    near(vol(s.built.body(target).unwrap()),600.);near(vol(s.built.body(split*1000+1).unwrap()),400.);
    assert_eq!(s.doc.feature(split).unwrap().owner,owner);
    run(&mut s,json!({"op":"move_component","id":other,"translate":[102,200,300]}));
    near(vol(s.built.body(target).unwrap()),700.);near(vol(s.built.body(split*1000+1).unwrap()),300.);
    assert!(s.built.bodies.iter().all(|b|b.component==owner));
    let remove=id(&run(&mut s,json!({"op":"remove_body","body":split*1000+1})));
    assert_eq!(s.doc.feature(remove).unwrap().owner,owner);
}

#[test]
fn splitting_a_multi_solid_body_produces_independent_pieces_and_reserves_ids() {
    let mut s=Session::default();let a=block(&mut s,[0.,0.,-2.],[2.,2.,4.]);let b=block(&mut s,[10.,0.,-2.],[2.,2.,4.]);
    run(&mut s,json!({"op":"combine","target":a,"tools":[b],"operation":"join"}));
    assert_eq!(s.built.body(a).unwrap().solids.len(),2);
    let split=id(&run(&mut s,json!({"op":"split_body","body":a,"plane":"XY"})));
    assert_eq!(s.built.bodies.len(),4);
    for body in &s.built.bodies {near(vol(body),8.);assert_eq!(body.solids.len(),1);}
    let copy=split*1000+1;
    run(&mut s,json!({"op":"set_visible","id":copy,"visible":false}));
    // Slot zero is not a synthetic body and remains available to real features.
    s.doc.next_id=split*1000;
    let unrelated=block(&mut s,[40.,0.,0.],[1.;3]);assert_eq!(unrelated,split*1000);
    run(&mut s,json!({"op":"set_visible","id":unrelated,"visible":false}));
    s.doc.next_id=copy;
    let later=block(&mut s,[30.,0.,0.],[1.;3]);assert!(later>=split*1000+1000);
    assert!(s.built.body(copy).is_some());assert_eq!(s.doc.body_owner(copy),Some(0));
    let mut bad=s.doc.clone();bad.feature_mut(later).unwrap().id=split*1000+900;
    assert!(io::validated_json(&bad).is_err());
    run(&mut s,json!({"op":"delete_feature","feature":split}));
    assert_eq!(s.built.body(a).unwrap().solids.len(),2);
    assert!(s.built.body(copy).is_none());assert!(!s.doc.hidden_bodies.contains(&copy));
    assert!(s.built.body(unrelated).is_some() && s.doc.hidden_bodies.contains(&unrelated),"deleting a split must not unhide an ordinary body with ID split*1000");
}

#[test]
fn invalid_remove_split_and_geometry_edits_are_atomic() {
    let mut s=Session::default();let target=block(&mut s,[0.;3],[10.;3]);let mid=plane(&mut s,5.);let far=plane(&mut s,20.);
    for command in [
        json!({"op":"remove_body","bodies":[]}),json!({"op":"remove_body","bodies":[target,target]}),
        json!({"op":"remove_body","bodies":[target,999999]}),json!({"op":"remove_body","body":target,"bodies":[target]}),
        json!({"op":"split_body","body":target,"plane":"XY"}),
        json!({"op":"split_body","body":target,"plane":{"plane":far}}),
        json!({"op":"split_body","body":target,"plane":{"plane":999999}}),
    ] {
        let before=s.doc.clone();assert!(api::execute(&mut s,&command,None).is_err(),"accepted {command}");assert_eq!(s.doc,before);
        assert_eq!(s.built.bodies.len(),1);near(vol(s.built.body(target).unwrap()),1000.);
    }
    let split=id(&run(&mut s,json!({"op":"split_body","body":target,"plane":{"plane":mid}})));
    for unbuilt in [false,true] {
        if unbuilt {run(&mut s,json!({"op":"rollback","to":far}));}
        let before=s.doc.clone();
        assert!(api::execute(&mut s,&json!({"op":"edit_feature","feature":split,"plane":"XY","name":"Wrong"}),None).is_err());
        assert_eq!(s.doc,before);
    }
    run(&mut s,json!({"op":"rollback","to":"end"}));
    run(&mut s,json!({"op":"edit_feature","feature":split,"plane":{"plane":mid}}));
    let remove=id(&run(&mut s,json!({"op":"remove_body","body":target})));
    run(&mut s,json!({"op":"edit_feature","feature":remove,"body":split*1000+1}));
    assert!(s.built.body(target).is_some());assert!(s.built.body(split*1000+1).is_none());
}

#[test]
fn split_rejects_mesh_threads_curved_faces_and_planes_in_empty_gaps() {
    let mut s=Session::default();let a=block(&mut s,[0.,0.,-10.],[2.,2.,5.]);let b=block(&mut s,[0.,0.,5.],[2.,2.,5.]);
    run(&mut s,json!({"op":"combine","target":a,"tools":[b],"operation":"join"}));
    let before=s.doc.clone();assert!(api::execute(&mut s,&json!({"op":"split_body","body":a,"plane":"XY"}),None).is_err());assert_eq!(s.doc,before);
    let mesh=s.built.body(a).unwrap().mesh.clone();
    let mesh_id=s.edit(|d|Ok(d.add_feature(FeatureKind::Import(mesh)))).unwrap();
    assert!(api::execute(&mut s,&json!({"op":"split_body","body":mesh_id,"plane":"XY"}),None).unwrap_err().contains("mesh"));
    let rod=id(&run(&mut s,json!({"op":"primitive","type":"cylinder","diameter":6,"height":10,"position":[20,0,-5]})));
    let before=s.doc.clone();
    assert!(api::execute(&mut s,&json!({"op":"split_body","body":a,"plane":{"face":{"body":rod,"point":[23,0,0]}}}),None).is_err());assert_eq!(s.doc,before);
    run(&mut s,json!({"op":"thread","body":rod,"face":[23,0,0],"thread":"M6","allowance":0.2}));
    let before=s.doc.clone();assert!(api::execute(&mut s,&json!({"op":"split_body","body":rod,"plane":"XY"}),None).unwrap_err().contains("threads"));assert_eq!(s.doc,before);
}
