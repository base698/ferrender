//! Analytic regressions from the 0.4 mesh review.
use fr_core::{mesh::Mesh, meshops::{self, Keep, Scheme}, csg::Bool, sketch::Plane};
use glam::DVec3;

fn cube(lo: DVec3, hi: DVec3) -> Mesh {
    let p = vec![DVec3::new(lo.x,lo.y,lo.z),DVec3::new(hi.x,lo.y,lo.z),DVec3::new(hi.x,hi.y,lo.z),DVec3::new(lo.x,hi.y,lo.z),DVec3::new(lo.x,lo.y,hi.z),DVec3::new(hi.x,lo.y,hi.z),DVec3::new(hi.x,hi.y,hi.z),DVec3::new(lo.x,hi.y,hi.z)];
    Mesh::from_indexed(p, vec![[0,2,1],[0,3,2],[4,5,6],[4,6,7],[0,1,5],[0,5,4],[1,2,6],[1,6,5],[2,3,7],[2,7,6],[3,0,4],[3,4,7]], true).unwrap()
}
fn unit() -> Mesh { cube(DVec3::ZERO, DVec3::ONE) }
fn solid(m: &Mesh, volume: f64) {
    assert!((m.volume()-volume).abs()<1e-5,"volume {}, wanted {volume}; {:?}",m.volume(),m.inspect());
    assert!(m.inspect().watertight,"{:?}",m.inspect());
}
#[test]
fn coincident_boolean_identities() {
    let a=unit();
    solid(&meshops::boolean(&a,&a,Bool::Union).unwrap(),1.);
    solid(&meshops::boolean(&a,&a,Bool::Intersect).unwrap(),1.);
    assert!(meshops::boolean(&a,&a,Bool::Subtract).unwrap().is_empty());
}
#[test]
fn coplanar_partial_overlap_boolean_volumes() {
    let a=unit(); let b=cube(DVec3::new(0.5,0.,0.),DVec3::new(1.5,1.,1.));
    solid(&meshops::boolean(&a,&b,Bool::Union).unwrap(),1.5);
    solid(&meshops::boolean(&a,&b,Bool::Intersect).unwrap(),0.5);
    solid(&meshops::boolean(&a,&b,Bool::Subtract).unwrap(),0.5);
}
#[test]
fn face_touching_boxes_have_no_internal_sheet() {
    let a=unit(); let b=cube(DVec3::X,DVec3::new(2.,1.,1.));
    solid(&meshops::boolean(&a,&b,Bool::Union).unwrap(),2.);
    solid(&meshops::boolean(&a,&b,Bool::Subtract).unwrap(),1.);
    assert!(meshops::boolean(&a,&b,Bool::Intersect).unwrap().is_empty());
}
#[test]
fn enclosed_tool_and_disconnected_boolean_components() {
    let a=cube(DVec3::splat(-2.),DVec3::splat(2.)); let b=unit();
    solid(&meshops::boolean(&a,&b,Bool::Union).unwrap(),64.);
    solid(&meshops::boolean(&a,&b,Bool::Intersect).unwrap(),1.);
    solid(&meshops::boolean(&a,&b,Bool::Subtract).unwrap(),63.);
    assert!(meshops::boolean(&b,&a,Bool::Subtract).unwrap().is_empty());
    let mut both=b.clone(); both.append(&cube(DVec3::splat(3.),DVec3::splat(4.)));
    solid(&meshops::boolean(&a,&both,Bool::Union).unwrap(),65.);
    solid(&meshops::boolean(&a,&both,Bool::Intersect).unwrap(),1.);
    solid(&meshops::boolean(&a,&both,Bool::Subtract).unwrap(),63.);
}
#[test]
fn smoothing_zero_strength_is_a_no_op() { let a=unit(); assert_eq!(meshops::smooth(&a,10,0.,None).unwrap(),a); }
#[test]
fn small_mesh_ray_ignores_triangles_behind_origin() { let a=unit(); assert_eq!(a.ray(DVec3::splat(0.5),DVec3::Z).unwrap().0,0.5); assert!(a.ray(DVec3::new(0.5,0.5,2.),DVec3::Z).is_none()); }
#[test]
fn empty_bvh_queries_are_safe() { let m=Mesh::default(); assert!(m.bvh().ray(&m,DVec3::ZERO,DVec3::Z).is_none()); assert!(m.bvh().nearest(&m,DVec3::ZERO).is_none()); assert_eq!(m.bvh().crossings(&m,DVec3::ZERO,DVec3::Z),0); assert!(m.bvh().in_box(DVec3::splat(-1.),DVec3::ONE).is_empty()); }
#[test]
fn subdivision_boundary_uses_boundary_edges_not_all_boundary_vertices() {
    let m=Mesh::from_indexed(vec![DVec3::ZERO,DVec3::X,DVec3::X+DVec3::Y,DVec3::Y],vec![[0,1,2],[0,2,3]],true).unwrap();
    let out=meshops::subdivide(&m,1,Scheme::Loop).unwrap();
    assert!(out.positions().iter().any(|p| p.distance(DVec3::new(0.125,0.125,0.))<1e-6));
    assert!(out.positions().iter().any(|p| p.distance(DVec3::new(0.875,0.875,0.))<1e-6));
}
#[test]
fn invalid_region_mask_returns_error() { assert!(meshops::extrude_region(&unit(),&[true],1.,Some(DVec3::Z)).is_err()); }
#[test]
fn mirror_weld_setting_preserves_separate_halves() {
    let m=cube(DVec3::ZERO,DVec3::ONE); let half=meshops::cut(&m,Plane::XY.offset(0.5),Keep::Positive,false).unwrap().remove(0);
    let separate=meshops::mirror(&half,Plane::XY.offset(0.5),false).unwrap();
    assert_eq!(separate.inspect().components,2); assert!(separate.open_edges()>0);
    solid(&meshops::mirror(&half,Plane::XY.offset(0.5),true).unwrap(),1.);
}
#[test]
fn box_boolean_grid_matches_analytic_volumes() {
    let a=unit();
    for x in [-1.25,-1.,-0.5,0.,0.25,1.,1.25] {
        for y in [-0.5,0.,0.25,1.25] {
            let z=0.25;
            let b=cube(DVec3::new(x,y,z),DVec3::new(x+1.,y+1.,z+1.));
            let overlap=(1.-x.abs()).max(0.)*(1.-y.abs()).max(0.)*(1.-z);
            for (op,expected) in [(Bool::Union,2.-overlap),(Bool::Subtract,1.-overlap),(Bool::Intersect,overlap)] {
                let out=meshops::boolean(&a,&b,op).unwrap();
                if expected==0. {assert!(out.is_empty(),"{x},{y},{z} {op:?}: {:?}",out.inspect());}
                else {println!("grid {x} {y} {z} {op:?}");solid(&out,expected);}
            }
        }
    }
}
#[test]
fn oblique_cut_caps_both_sides_without_losing_volume() {
    for normal in [DVec3::X,DVec3::Y,DVec3::Z,DVec3::new(1.,1.,1.),DVec3::new(0.3,0.7,1.)] {
        let cuts=meshops::cut(&unit(),Plane::from_normal(DVec3::splat(0.5),normal),Keep::Both,true).unwrap();
        for m in &cuts {println!("cut normal {normal:?} bounds {:?}",m.bbox()); solid(m,0.5);}
    }
}
#[test]
fn relief_blur_matches_direct_box_average() {
    use fr_core::reference::Pixels;
    let pixels=Pixels{width:4,height:3,rgba:(0..12).flat_map(|i| [(i*19) as u8,(i*19) as u8,(i*19) as u8,255]).collect()};
    let mut p=meshops::ReliefParams{width:12.,depth:2.,base:0.,resolution:8,invert:false,blur:0,gamma:1.};
    let raw=meshops::from_image(&pixels,&p).unwrap();
    let (cols,rows)=(8usize,6usize);
    let mut h=vec![0.;(cols+1)*(rows+1)];
    for v in raw.positions() {let (c,r)=((v.x/1.5).round() as usize,(v.y/1.5).round() as usize); h[r*(cols+1)+c]=v.z;}
    p.blur=2;
    for _ in 0..3 {
        let old=h.clone();
        for r in 0..=rows {for c in 0..=cols {
            let mut sum=0.;let mut n=0.;
            for rr in r.saturating_sub(2)..=(r+2).min(rows) {for cc in c.saturating_sub(2)..=(c+2).min(cols) {sum+=old[rr*(cols+1)+cc];n+=1.;}}
            h[r*(cols+1)+c]=sum/n;
        }}
    }
    let blurred=meshops::from_image(&pixels,&p).unwrap();
    for v in blurred.positions() {let (c,r)=((v.x/1.5).round() as usize,(v.y/1.5).round() as usize);assert!((v.z-h[r*(cols+1)+c]).abs()<1e-6);}
}
#[test]
fn repair_preserves_cavity_with_many_disconnected_shells() {
    let mut m=cube(DVec3::splat(-2.),DVec3::splat(2.)); let mut hole=unit();hole.flip();m.append(&hole);
    for i in 0..63 {let lo=DVec3::new(10.+i as f64*2.,0.,0.);m.append(&cube(lo,lo+DVec3::ONE));}
    m.weld_exact();m.repair();solid(&m,126.);
}
#[test]
fn ray_crossings_count_shared_edge_hit_once() {
    let m=unit();assert_eq!(m.bvh().crossings(&m,DVec3::splat(0.5),DVec3::X),1);
    assert_eq!(m.bvh().crossings(&m,DVec3::new(-1.,0.5,0.5),DVec3::X),2);
}
#[test]
fn dense_flat_mesh_booleans_preserve_volume_and_topology() {
    let a=meshops::subdivide(&unit(),4,Scheme::Midpoint).unwrap();
    let mut b=a.clone();b.map(|p| p+DVec3::new(0.3,0.,0.));
    assert!(a.len()+b.len()>2048);
    solid(&meshops::boolean(&a,&b,Bool::Union).unwrap(),1.3);
    solid(&meshops::boolean(&a,&b,Bool::Subtract).unwrap(),0.3);
    solid(&meshops::boolean(&a,&b,Bool::Intersect).unwrap(),0.7);
}
#[test]
fn mesh_feature_inherits_target_component_from_root() {
    use fr_core::{Session,api::execute};use serde_json::json;
    let mut s=Session::default();
    execute(&mut s,&json!({"op":"create_component","name":"MovedMesh"}),None).unwrap();
    let component=s.doc.active_component;
    execute(&mut s,&json!({"op":"primitive","type":"box","width":2,"depth":2,"height":2}),None).unwrap();
    let body=s.built.bodies[0].id;
    execute(&mut s,&json!({"op":"move_component","id":component,"translate":[10,20,30],"rotate":[0,0,90]}),None).unwrap();
    execute(&mut s,&json!({"op":"activate_component","id":0}),None).unwrap();
    let before=s.built.body(body).unwrap().mesh.bbox().unwrap();
    execute(&mut s,&json!({"op":"mesh_subdivide","body":body,"levels":1,"scheme":"midpoint"}),None).unwrap();
    assert_eq!(s.doc.features.last().unwrap().owner,component);
    assert_eq!(s.built.body(body).unwrap().component,component);
    let after=s.built.body(body).unwrap().mesh.bbox().unwrap();
    assert!(before.0.distance(after.0)<1e-5 && before.1.distance(after.1)<1e-5);
    assert!(s.built.errors.is_empty());
    execute(&mut s,&json!({"op":"set_visible","id":component,"visible":false}),None).unwrap();
    let scene=execute(&mut s,&json!({"op":"get_scene_info"}),None).unwrap();
    assert_eq!(scene["bodies"][0]["visible"],false);
}
#[test]
fn bvh_queries_match_brute_force_and_invalidate_after_edits() {
    use fr_core::mesh::{ray_tri,dist_tri};
    let mut m=meshops::subdivide(&unit(),2,Scheme::Midpoint).unwrap();
    for pass in 0..2 {
        for i in 0..80 {
            let origin=DVec3::new((i as f64*0.17).sin()*3.+pass as f64*4.,(i as f64*0.31).cos()*3.,2.);
            let dir=(DVec3::new(0.5+pass as f64*4.,0.5,0.5)-origin).normalize();
            let brute=m.tris().filter_map(|t| ray_tri(origin,dir,&t)).filter(|d| *d>=0.).min_by(f64::total_cmp);
            let fast=m.ray(origin,dir).map(|v|v.0);
            assert_eq!(brute.is_some(),fast.is_some());
            if let (Some(a),Some(b))=(brute,fast) {assert!((a-b).abs()<1e-9);}
            let brute=m.tris().map(|t| dist_tri(origin,&t)).min_by(f64::total_cmp).unwrap();
            let fast=m.bvh().nearest(&m,origin).unwrap().0;
            assert!((brute-fast).abs()<1e-9);
        }
        m.map(|p|p+DVec3::X*4.);
    }
}
#[test]
fn boolean_fragment_work_is_bounded_without_recursive_stack() {
    let cutter=Mesh::from_tris((0..7000).map(|i| {let x=i as f64;[DVec3::new(x,0.,0.),DVec3::new(x,1.,0.),DVec3::new(x,0.,1.)]}).collect());
    let err=fr_core::csg::fragments(&cutter,&unit()).unwrap_err();
    assert!(err.contains("too complex"),"{err}");
}
#[test]
fn quadric_decimation_preserves_torus_manifoldness() {
    let (nu,nv)=(32usize,16usize);
    let points=(0..nu).flat_map(|u|(0..nv).map(move|v|{let a=u as f64/nu as f64*std::f64::consts::TAU;let b=v as f64/nv as f64*std::f64::consts::TAU;DVec3::new((3.+b.cos())*a.cos(),(3.+b.cos())*a.sin(),b.sin())})).collect();
    let mut triangles=Vec::new();
    for u in 0..nu {for v in 0..nv {let at=|u:usize,v:usize|((u%nu)*nv+v%nv) as u32;let(a,b,c,d)=(at(u,v),at(u+1,v),at(u+1,v+1),at(u,v+1));triangles.extend([[a,b,c],[a,c,d]]);}}
    let m=Mesh::from_indexed(points,triangles,true).unwrap();
    for target in [512,128,64,32] {let out=meshops::decimate(&m,target,true).unwrap();assert!(out.inspect().watertight,"target{target}: {:?}",out.inspect());assert!(out.volume()>0.);}
}
#[test]
fn cluster_decimation_preserves_requested_open_outline() {
    let n=8usize;let points=(0..=n).flat_map(|y|(0..=n).map(move|x|DVec3::new(x as f64/n as f64,y as f64/n as f64,0.))).collect();
    let mut tris=Vec::new();for y in 0..n {for x in 0..n {let a=(y*(n+1)+x) as u32;let b=a+1;let c=a+(n+1) as u32;tris.extend([[a,b,c+1],[a,c+1,c]]);}}
    let m=Mesh::from_indexed(points,tris,true).unwrap();let adj=m.adjacency();
    let out=meshops::decimate_by(&m,32,meshops::DecimateMethod::Cluster,true).unwrap();
    assert!(out.len()<m.len());
    for v in 0..m.vertex_count() as u32 {if adj.on_boundary(v) {assert!(out.positions().contains(&m.positions()[v as usize]),"lost outline vertex{v}");}}
    assert_eq!(out.open_edges(),m.open_edges());
}

#[test]
fn thickness_is_measured_at_the_requested_point_not_face_centroid() {
    let mut m=cube(DVec3::ZERO,DVec3::new(10.,10.,1.));
    m.map(|p|DVec3::new(p.x,p.y,p.z*(1.+p.x*0.1)));
    let at=DVec3::new(2.,5.,1.2);
    let measured=meshops::measure(&m,Some(at)).thickness_at.unwrap();
    let expected=1.2*(1.01f64).sqrt();
    assert!((measured-expected).abs()<1e-6,"{measured} expected{expected}");
}
#[test]
fn failing_mesh_boolean_preserves_original_design() {
    use fr_core::{Session,FeatureKind,api::execute};use serde_json::json;
    let mut a=meshops::subdivide(&unit(),4,Scheme::Midpoint).unwrap();
    let mut b=a.clone();b.map(|p|p+DVec3::new(0.3,0.,0.));
    let rotation=glam::DMat3::from_rotation_y(0.37)*glam::DMat3::from_rotation_z(0.21);
    a.map(|p|rotation*p);b.map(|p|rotation*p);
    let mut s=Session::default();
    let target=s.edit(|d|Ok(d.add_feature(FeatureKind::Import(a)))).unwrap();
    let tool=s.edit(|d|Ok(d.add_feature(FeatureKind::Import(b)))).unwrap();
    let before=s.doc.clone();
    let error=execute(&mut s,&json!({"op":"combine","target":target,"tools":[tool],"operation":"join"}),None).unwrap_err();
    assert!(error.contains("closed, manifold result"),"{error}");
    assert_eq!(s.doc,before);assert_eq!(s.built.bodies.len(),2);assert!(s.built.errors.is_empty());
}
#[test]
fn mesh_boolean_refuses_open_input_instead_of_guessing_a_volume() {
    let mut open=unit();let mut first=true;open.retain_tris(|_| {let keep=!first;first=false;keep});
    let error=meshops::boolean(&open,&unit(),Bool::Union).unwrap_err();
    assert!(error.contains("closed, manifold inputs"),"{error}");
}
#[test]
#[ignore = "requires FERRENDER_REVIEW_TURTLE pointing to the generated organic test STL"]
fn organic_turtle_edit_pipeline_remains_watertight() {
    use fr_core::{Session,api::execute};use serde_json::json;
    let path=std::env::var("FERRENDER_REVIEW_TURTLE").expect("set FERRENDER_REVIEW_TURTLE");
    let mut s=Session::default();
    execute(&mut s,&json!({"op":"import_mesh","path":path}),None).unwrap();
    let body=s.built.bodies[0].id;
    for command in [
        json!({"op":"mesh_repair","body":body,"fill_holes":12}),
        json!({"op":"mesh_subdivide","body":body,"levels":1,"scheme":"loop"}),
        json!({"op":"mesh_smooth","body":body,"iterations":2,"strength":0.18}),
        json!({"op":"mesh_sculpt","body":body,"brush":"pull","at":[0,0,25],"radius":8,"strength":0.65}),
        json!({"op":"mesh_sculpt","body":body,"brush":"smooth","at":[0,0,25],"radius":9,"strength":0.18}),
        json!({"op":"mesh_decimate","body":body,"target":150000,"method":"quadric","preserve_boundary":true}),
    ] {
        execute(&mut s,&command,None).unwrap();
        let m=&s.built.body(body).unwrap().mesh;let report=m.inspect();
        println!("{}: {}, volume{}",command["op"],report.summary(),m.volume());
        assert!(report.watertight,"after{}: {report:?}",command["op"]);
    }
}
#[test]
fn translated_oblique_cuts_cap_closed_boxes_and_thin_cavities() {
    for translation in [DVec3::ZERO,DVec3::new(1000.,-500.,750.),DVec3::splat(10000.)] {
        for hollow in [false,true] {
            let mut m=cube(DVec3::ZERO,DVec3::splat(10.));
            if hollow {let mut inner=cube(DVec3::splat(0.1),DVec3::splat(9.9));inner.flip();m.append(&inner);m.weld_exact();}
            m=meshops::subdivide(&m,2,Scheme::Midpoint).unwrap();m.map(|p|p+translation);m.snap();
            let volume=m.volume();assert!(m.inspect().watertight);
            for normal in [DVec3::ONE,DVec3::new(0.3,0.7,1.)] {
                let plane=Plane::from_normal(translation+DVec3::splat(5.),normal);
                let cuts=meshops::cut(&m,plane,Keep::Both,true).unwrap();
                for part in &cuts {
                    let r=part.inspect();println!("translation{translation:?}, hollow{hollow}, normal{normal:?}: {:?}, volume{}",r,part.volume());
                    assert!(r.watertight,"translation{translation:?}, hollow{hollow}, normal{normal:?}: {r:?}");
                    assert!((part.volume()-volume*0.5).abs()<volume*0.002+1e-4,"volume{} expected{}",part.volume(),volume*0.5);
                }
            }
        }
    }
}
