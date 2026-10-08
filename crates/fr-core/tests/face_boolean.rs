use fr_core::{api, exact, io, Body, FeatureKind, Geom, Session};
use fr_core::csg::Bool;
use fr_core::face::Face;
use glam::DVec3;
use serde_json::{json, Value};
use std::f64::consts::PI;

fn run(s: &mut Session, command: Value) -> Value {
    api::execute(s, &command, None).unwrap_or_else(|e| panic!("{command}: {e}"))
}
fn id(value: &Value) -> u32 { value["feature"].as_u64().unwrap() as u32 }
fn volume(solids: &[cadrum::Solid]) -> f64 { solids.iter().map(|s| s.volume()).sum() }
fn close(a: f64, b: f64) { assert!((a-b).abs() < 1e-6*b.abs().max(1.), "{a} != {b}"); }
fn rounded_bar(s: &mut Session) -> u32 {
    let body=id(&run(s,json!({"op":"primitive","type":"box","width":30,"depth":15,"height":20})));
    run(s,json!({"op":"fillet_edges","body":body,"edges":[[15,0,20],[15,15,0]],"radius":2}));
    body
}
fn sketch_for_last_extrude(s: &Session) -> &fr_core::Sketch {
    let FeatureKind::Extrude(e)=&s.doc.features.last().unwrap().kind else {panic!()};
    s.doc.sketch(e.sketch).unwrap()
}
fn check_mesh(body: &Body) {
    assert!(body.is_exact());
    assert_eq!(body.mesh.open_edges(),0);
    assert!(body.mesh.volume()>0.);
}

#[test]
fn rounded_face_extrusion_keeps_arcs_instead_of_leaving_chord_slivers() {
    let area=15.*20.-2.*(4.-PI);
    for (distance,operation,length) in [(-13.5,"cut",16.5),(13.5,"join",43.5)] {
        let mut s=Session::default();
        let body=rounded_bar(&mut s);
        run(&mut s,json!({"op":"extrude","face":{"body":body,"point":[30,8,10]},"distance":distance,"operation":operation}));
        let sk=sketch_for_last_extrude(&s);
        assert_eq!(sk.entities.values().filter(|e|matches!(e.geom,Geom::Arc{..})).count(),2);
        assert_eq!(sk.entities.len(),6);
        let b=s.built.body(body).unwrap(); check_mesh(b);
        assert_eq!(b.solids.len(),1);
        let (lo,hi)=b.mesh.bbox().unwrap(); close(lo.x,0.); close(hi.x,length);
        close(volume(&b.solids),area*length);
        let saved=io::validated_json(&s.doc).unwrap();
        let reopened=Session::new(io::from_json(&saved).unwrap());
        assert!(reopened.built.errors.is_empty());
        close(volume(&reopened.built.body(body).unwrap().solids),area*length);
        assert!(s.undo()); close(volume(&s.built.body(body).unwrap().solids),area*30.);
        assert!(s.redo()); close(volume(&s.built.body(body).unwrap().solids),area*length);
    }
}

#[test]
fn opposite_circular_faces_and_inner_holes_keep_their_analytic_boundaries() {
    for (point,distance) in [([0.,0.,0.],-3.),([0.,0.,12.],-3.)] {
        let mut s=Session::default();
        let body=id(&run(&mut s,json!({"op":"primitive","type":"cylinder","diameter":10,"height":12})));
        run(&mut s,json!({"op":"extrude","face":{"body":body,"point":point},"distance":distance,"operation":"cut"}));
        let sk=sketch_for_last_extrude(&s);
        assert_eq!(sk.entities.len(),1); assert!(matches!(sk.entities.values().next().unwrap().geom,Geom::Circle{..}));
        let b=s.built.body(body).unwrap(); check_mesh(b); close(volume(&b.solids),PI*25.*9.);
        let (lo,hi)=b.mesh.bbox().unwrap(); close(hi.z-lo.z,9.);
        close(if point[2]==0. {lo.z} else {12.-hi.z},3.);
    }
    // Unequal circular end faces ensure the plane/trim disambiguation cannot
    // accidentally capture the opposite cap merely because an ID matches.
    for (point,radius) in [([0.,0.,0.],5.),([0.,0.,12.],2.)] {
        let mut s=Session::default();
        let body=id(&run(&mut s,json!({"op":"primitive","type":"cone","bottom_diameter":10,"top_diameter":4,"height":12})));
        let b=s.built.body(body).unwrap();
        let face=Face::near(b,DVec3::from_array(point)).unwrap();
        let other=Face::near(b,DVec3::new(0.,0.,12.-point[2])).unwrap();
        let wrong_id=b.mesh.face_ids[other.tris[0]];
        let edges=exact::face_edges(&b.solids,wrong_id,face.plane.unwrap(),face.at).unwrap();
        assert!(matches!(edges.as_slice(),[fr_core::profile::Seg::Circle(_,r)] if (*r-radius).abs()<1e-7));
        let made=id(&run(&mut s,json!({"op":"extrude","face":{"body":body,"point":point},"distance":3,"operation":"new"})));
        close(volume(&s.built.body(made).unwrap().solids),PI*radius*radius*3.);
    }
    let mut s=Session::default();
    let body=id(&run(&mut s,json!({"op":"primitive","type":"box","width":20,"depth":15,"height":10})));
    run(&mut s,json!({"op":"hole","body":body,"at":[10,7.5,10],"diameter":4,"fit":"plain","through":true}));
    let made=id(&run(&mut s,json!({"op":"extrude","face":{"body":body,"point":[3,3,10]},"distance":3,"operation":"new"})));
    let sk=sketch_for_last_extrude(&s);
    assert_eq!(sk.entities.values().filter(|e|matches!(e.geom,Geom::Circle{..})).count(),1);
    assert_eq!(s.built.bodies.len(),2);
    let b=s.built.body(made).unwrap(); check_mesh(b); close(volume(&b.solids),(300.-4.*PI)*3.);
}

#[test]
fn analytic_face_capture_is_lazy_and_survives_component_placement() {
    let mut s=Session::default();
    let component=run(&mut s,json!({"op":"create_component","name":"Rounded"}))["component"].as_u64().unwrap() as u32;
    let body=rounded_bar(&mut s);
    run(&mut s,json!({"op":"move_component","id":component,"translate":[40,-30,20],"rotate":[20,30,40]}));
    let placement=s.built.component_placement(component);
    let point=placement.transform_point3(DVec3::new(30.,8.,10.));
    let b=s.built.body(body).unwrap();
    let mut face=Face::near(b,point).unwrap();
    assert!(face.exact_edges.is_none(),"hover picking must not sample kernel curves");
    face.prepare_exact(b);
    let moved=face.transformed(placement.inverse());
    assert_eq!(face.exact_edges,moved.exact_edges,"UV coordinates do not change with component placement");
    run(&mut s,json!({"op":"extrude","face":{"body":body,"point":point.to_array()},"distance":-13.5,"operation":"cut"}));
    let b=s.built.body(body).unwrap(); check_mesh(b);
    close(volume(&b.solids),(300.-2.*(4.-PI))*16.5);
    let local=b.local_copy().unwrap(); let (lo,hi)=local.mesh.bbox().unwrap(); close(lo.x,0.); close(hi.x,16.5);
}

#[test]
fn exact_chamfer_edges_are_not_reinterpreted_as_a_circular_corner() {
    let mut s=Session::default();
    let body=id(&run(&mut s,json!({"op":"primitive","type":"box","width":30,"depth":15,"height":20})));
    run(&mut s,json!({"op":"chamfer_edges","body":body,"edges":[[15,0,20],[15,15,0]],"distance":2}));
    run(&mut s,json!({"op":"extrude","face":{"body":body,"point":[30,8,10]},"distance":-13.5,"operation":"cut"}));
    let sk=sketch_for_last_extrude(&s);
    assert_eq!(sk.entities.len(),6);
    assert!(sk.entities.values().all(|e|matches!(e.geom,Geom::Line{..})));
    close(volume(&s.built.body(body).unwrap().solids),296.*16.5);
}

fn check_all_booleans(a: &[cadrum::Solid],b: &[cadrum::Solid]) {
    let (va,vb)=(volume(a),volume(b));
    let mut volumes=Vec::new();
    for (a,b) in [(a,b),(b,a)] {
        let mut result=Vec::new();
        for op in [Bool::Union,Bool::Subtract,Bool::Intersect] {
            let made=exact::boolean(a,b,op).unwrap_or_else(|e|panic!("{op:?}: {e}"));
            assert_eq!(made.len(),1,"{op:?} left extra fragments");
            let v=volume(&made); let mesh=exact::tessellate(&made).unwrap().0;
            assert_eq!(mesh.open_edges(),0,"{op:?} produced an open mesh"); assert!(mesh.volume()>0.);
            assert!((mesh.volume()-v).abs() < v*0.005,"{op:?}: mesh volume {} exact {v}",mesh.volume());
            result.push(v);
        }
        volumes.push(result);
    }
    close(volumes[0][0],volumes[1][0]); close(volumes[0][2],volumes[1][2]);
    // Kernel integration at periodic trims has a few parts-per-million error.
    let tolerance=1e-5*(va+vb);
    assert!((volumes[0][0]+volumes[0][2]-va-vb).abs()<tolerance);
    assert!((volumes[0][1]+volumes[0][2]-va).abs()<tolerance);
    assert!((volumes[1][1]+volumes[1][2]-vb).abs()<tolerance);
    close(volume(a),va); close(volume(b),vb);
}

#[test]
fn torus_booleans_with_plain_and_face_shortened_filleted_boxes_work_in_both_orders() {
    let torus=vec![cadrum::Solid::torus(19.,6.,cadrum::DVec3::Z)];
    let plain=vec![cadrum::Solid::cube(cadrum::DVec3::new(-5.,0.,0.),cadrum::DVec3::new(25.,15.,20.))];
    check_all_booleans(&plain,&torus);
    let mut s=Session::default(); let body=rounded_bar(&mut s);
    run(&mut s,json!({"op":"extrude","face":{"body":body,"point":[30,8,10]},"distance":-13.5,"operation":"cut"}));
    let shortened:Vec<_>=s.built.body(body).unwrap().solids.iter().cloned().map(|s|s.translate(cadrum::DVec3::new(-5.,0.,0.))).collect();
    check_all_booleans(&shortened,&torus);
}

#[test]
fn combine_empty_intersection_is_valid_and_undo_restores_both_inputs() {
    let mut s=Session::default();
    let a=id(&run(&mut s,json!({"op":"primitive","type":"box","width":5,"depth":5,"height":5})));
    let b=id(&run(&mut s,json!({"op":"primitive","type":"torus","major_radius":19,"tube_radius":6,"position":[100,0,0]})));
    run(&mut s,json!({"op":"combine","target":a,"tools":[b],"operation":"intersect"}));
    assert!(s.built.bodies.is_empty()); assert!(s.built.errors.is_empty());
    assert!(s.undo()); assert_eq!(s.built.bodies.len(),2);
    let before=s.doc.clone();
    assert!(api::execute(&mut s,&json!({"op":"primitive","type":"box","width":2,"depth":2,"height":2,"position":[200,0,0],"operation":"intersect"}),None).is_err());
    assert_eq!(s.doc,before);
}

fn trimmed_bar(legacy_chords: bool) -> Vec<cadrum::Solid> {
    let mut s=Session::default(); let body=rounded_bar(&mut s);
    if legacy_chords {
        // Reproduce an old frozen outline from display triangles, without a
        // personal file fixture or automatically rewriting existing sketches.
        let face=Face::near(s.built.body(body).unwrap(),DVec3::new(30.,8.,10.)).unwrap();
        let (sk,profiles)=face.sketch().unwrap();
        s.edit(|d| {
            let sketch=d.add_feature(FeatureKind::Sketch(sk));
            let distance=d.value("-13.5 mm",fr_core::Kind::Length)?;
            d.add_feature(FeatureKind::Extrude(fr_core::doc::Extrude{sketch,profiles,distance,symmetric:false,op:fr_core::Op::Cut,taper:None,through_all:false}));
            Ok(())
        }).unwrap();
    } else {
        run(&mut s,json!({"op":"extrude","face":{"body":body,"point":[30,8,10]},"distance":-13.5,"operation":"cut"}));
    }
    let r=2_f64.sqrt();
    let plane=fr_core::planes::three_points([DVec3::new(16.5,15.,20.),DVec3::ZERO,DVec3::new(16.5,13.+r,2.-r)]).unwrap();
    let mut sk=fr_core::Sketch::new(plane);
    let corners=[[-0.3,3.1],[22.,3.1],[22.,-3.],[ -0.3,-3.]].map(|p|sk.add_point(glam::DVec2::from_array(p)));
    for i in 0..4 {sk.add_line(corners[i],corners[(i+1)%4]);}
    let profiles=fr_core::profile::profiles(&sk).into_iter().map(|p|p.edges).collect();
    s.edit(|d| {
        let sketch=d.add_feature(FeatureKind::Sketch(sk));
        let distance=d.value("-13.5 mm",fr_core::Kind::Length)?;
        d.add_feature(FeatureKind::Extrude(fr_core::doc::Extrude{sketch,profiles,distance,symmetric:false,op:fr_core::Op::Cut,taper:None,through_all:false}));
        Ok(())
    }).unwrap();
    assert!(s.built.errors.is_empty(),"{:?}",s.built.errors);
    s.built.body(body).unwrap().solids.iter().cloned().map(|s|s.translate(cadrum::DVec3::new(-5.,0.,0.))).collect()
}

#[test]
fn trimmed_torus_union_preserves_the_valid_shape_when_cleanup_corrupts_periodic_faces() {
    let bar=trimmed_bar(false);
    let torus=vec![cadrum::Solid::torus(19.,6.,cadrum::DVec3::Z)];
    check_all_booleans(&bar,&torus);
}

#[test]
fn old_chord_slivers_cannot_commit_a_torus_result_with_inconsistent_display_volume() {
    let bar=trimmed_bar(true);
    let torus=vec![cadrum::Solid::torus(19.,6.,cadrum::DVec3::Z)];
    let mut refused=0;
    for (a,b) in [(&bar,&torus),(&torus,&bar)] {
        for op in [Bool::Union,Bool::Subtract,Bool::Intersect] {
            match exact::boolean(a,b,op) {
                Ok(made) => {
                    let mesh=exact::tessellate(&made).unwrap().0;
                    assert!(mesh.volume()>0.); assert_eq!(mesh.open_edges(),0);
                    assert!((mesh.volume()-volume(&made)).abs()<volume(&made)*0.005);
                }
                Err(error) => {assert!(error.contains("kernel"),"{error}"); refused+=1;}
            }
        }
    }
    assert!(refused>0,"this legacy outline should trigger the periodic-trim safeguard");
}
