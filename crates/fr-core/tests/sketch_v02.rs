use fr_core::{CKind, Document, Geom, Kind, Plane, Session, Sketch, Unit, Value, api::execute, exact, io, profile::profiles, sketch::{arc3_center, spline_polyline, tangent_arc_center}, solver};
use glam::DVec2;
use serde_json::{Value as J, json};

fn run(s: &mut Session, c: J) -> J { execute(s,&c,None).unwrap_or_else(|e| panic!("{c}: {e}")) }
fn near(a: f64,b: f64) { assert!((a-b).abs() < 1e-6 * b.abs().max(1.0),"{a} != {b}"); }
fn val(v: f64)->Value { Value { expr:format!("{v} mm"), v } }

#[test]
fn scientific_literals_keep_units_and_api_numbers() {
    let d = Document::new(Unit::Mm);
    for (text,want) in [("1e-12 mm",1e-12),("-2.5E+1 mm",-25.0),(".25e2 cm",250.0),("1.e2 + 2",102.0),("1e3mm / 2",500.0)] { near(d.eval(text,Kind::Length).unwrap(),want); }
    for text in ["1e", "1e+", "1e--2", "1e999", "1.2.3e2", "1e2e3"] { assert!(d.eval(text,Kind::Length).is_err(),"accepted {text}"); }
    let mut s = Session::default();
    run(&mut s,json!({"op":"create_sketch"}));
    let r = run(&mut s,json!({"op":"add_geometry","items":[{"type":"point","at":[1.234567e-12,3.0]}]}));
    let id = r["items"][0]["points"][0].as_u64().unwrap() as u32;
    let x = s.doc.sketch(1).unwrap().pos(id).x;
    assert!((x - 1.234567e-12).abs() < 1e-22);
}

#[test]
fn coordinate_dimensions_remain_parametric_and_atomic() {
    let mut s = Session::default();
    run(&mut s,json!({"op":"create_sketch","plane":"XZ"}));
    run(&mut s,json!({"op":"set_parameter","name":"height","expr":"30 mm"}));
    let p=run(&mut s,json!({"op":"point_coordinates","x":"-2 mm","y":"$height"}))["point"].as_u64().unwrap() as u32;
    assert_eq!(s.doc.sketch(1).unwrap().pos(p),DVec2::new(-2.0,30.0));
    run(&mut s,json!({"op":"set_parameter","name":"height","expr":"36 mm"}));
    near(s.doc.sketch(1).unwrap().pos(p).y,36.0);
    run(&mut s,json!({"op":"set_units","units":"in"}));
    near(s.doc.sketch(1).unwrap().pos(p).x,-2.0);
    let original=io::to_json(&s.doc);
    assert_eq!(serde_json::from_str::<J>(&original).unwrap()["version"],3);
    assert_eq!(io::from_json(&original).unwrap(),s.doc);
    run(&mut s,json!({"op":"point_coordinates","point":p,"x":"3 mm","y":"$height"}));
    near(s.doc.sketch(1).unwrap().pos(p).x,3.0);
    assert_eq!(s.doc.sketch(1).unwrap().constraints.len(),2);
    assert!(s.undo());
    assert_eq!(io::to_json(&s.doc),original);
    assert!(execute(&mut s,&json!({"op":"point_coordinates","point":0,"x":2,"y":3}),None).is_err());
    assert_eq!(io::to_json(&s.doc),original);
    assert!(s.redo(),"rejected coordinate command lost redo");
    run(&mut s,json!({"op":"add_constraint","kind":"fix","refs":[p]}));
    let before=io::to_json(&s.doc);
    assert!(execute(&mut s,&json!({"op":"point_coordinates","point":p,"x":4,"y":5}),None).is_err());
    assert_eq!(io::to_json(&s.doc),before);
}

#[test]
fn coordinate_conflict_rolls_back_other_point_motion() {
    let mut s=Session::default();
    run(&mut s,json!({"op":"create_sketch"}));
    let a=run(&mut s,json!({"op":"point_coordinates","x":2,"y":3}))["point"].as_u64().unwrap();
    let b=run(&mut s,json!({"op":"point_coordinates","x":8,"y":3}))["point"].as_u64().unwrap();
    run(&mut s,json!({"op":"add_constraint","kind":"horizontal","refs":[a,b]}));
    let before=io::to_json(&s.doc);
    assert!(execute(&mut s,&json!({"op":"point_coordinates","point":b,"x":8,"y":9}),None).is_err());
    assert_eq!(io::to_json(&s.doc),before);
}

#[test]
fn three_point_arcs_choose_the_requested_side_and_keep_through_point() {
    for (through,end,want_sweep) in [((0.0,1.0),(-1.0,0.0),std::f64::consts::PI),((0.0,-1.0),(-1.0,0.0),std::f64::consts::PI),((-1.0,0.0),(0.0,-1.0),std::f64::consts::PI*1.5)] {
        let mut sk=Sketch::new(Plane::XY);
        let a=sk.add_point(DVec2::X); let m=sk.add_point(DVec2::from(through)); let b=sk.add_point(DVec2::from(end));
        let arc=sk.add_arc3(a,m,b,false).unwrap();
        near(sk.arc_angles(arc).unwrap().1,want_sweep);
        assert!(sk.dist_to(arc,sk.pos(m)) < 0.002);
        assert_eq!(sk.arc_guides.get(&arc),Some(&fr_core::sketch::ArcGuide::Through { point:m }));
        assert!(solver::solve(&mut sk,&[(m,sk_pos(through)*1.1)]).ok);
        let (centre,r)=sk.curve(arc).unwrap(); near(centre.distance(sk.pos(m)),r);
    }
    assert!(arc3_center(DVec2::ZERO,DVec2::X,DVec2::X*2.0).is_err());
    assert!(arc3_center(DVec2::ZERO,DVec2::ZERO,DVec2::Y).is_err());
}
fn sk_pos(p:(f64,f64))->DVec2 { DVec2::from(p) }

#[test]
fn tangent_arcs_extend_both_sides_and_survive_solver_edits() {
    for endpoint_first in [false,true] {
        for sign in [-1.0,1.0] {
            let mut sk=Sketch::new(Plane::XY);
            let a=sk.add_point(DVec2::new(-2.0,0.0)); let b=sk.add_point(DVec2::ZERO);
            let line=if endpoint_first { sk.add_line(b,a) } else { sk.add_line(a,b) };
            let e=sk.add_point(DVec2::new(2.0,sign*2.0));
            let arc=sk.add_tangent_arc(line,b,e,false).unwrap();
            let incoming=sk.endpoint_tangent(line,b).unwrap(); let outgoing=-sk.endpoint_tangent(arc,b).unwrap();
            near(incoming.dot(outgoing),1.0);
            assert!(solver::solve(&mut sk,&[(e,DVec2::new(3.0,sign*2.0))]).ok);
            near(sk.endpoint_tangent(line,b).unwrap().dot(-sk.endpoint_tangent(arc,b).unwrap()),1.0);
        }
    }
    assert!(tangent_arc_center(DVec2::ZERO,DVec2::X,DVec2::X*2.0).is_err());
}

#[test]
fn splines_are_native_curves_and_support_closed_exact_solids() {
    let mut sk=Sketch::new(Plane::XY);
    let points=[DVec2::new(0.0,0.0),DVec2::new(3.0,3.0),DVec2::new(7.0,3.0),DVec2::new(10.0,0.0)];
    let ids=points.map(|p| sk.add_point(p));
    let spline=sk.add_spline(ids,false).unwrap();
    let poly=spline_polyline(points);
    assert!(poly.len()>8);
    assert!(poly[0].distance(points[0])<1e-7 && poly.last().unwrap().distance(points[3])<1e-7);
    for p in points { assert!(sk.dist_to(spline,p)<0.01); }
    assert_eq!(sk.open_endpoints(),vec![ids[0],ids[3]]);
    sk.add_line(ids[3],ids[0]);
    assert!(sk.open_endpoints().is_empty());
    let ps=profiles(&sk); assert_eq!(ps.len(),1);
    let solids=exact::extrude(&[&ps[0]],&sk.plane,0.0,2.0).unwrap();
    assert!(!solids.is_empty());
    let step=String::from_utf8(exact::step(solids.iter()).unwrap()).unwrap();
    assert!(step.contains("B_SPLINE_CURVE"),"STEP lost the native spline");
    let (mesh,_)=exact::tessellate(&solids).unwrap();
    assert_eq!(mesh.open_edges(),0);
    let volume=solids.iter().map(|s|s.volume()).sum::<f64>(); assert!(volume>30.0 && volume<70.0);
    assert!(sk.add_constraint(CKind::Radius,&[spline],Some(val(2.0))).is_err());
    assert!(sk.add_constraint(CKind::Coincident,&[ids[1],spline],None).is_err());
    let old=sk.polyline(spline); sk.points.insert(ids[1],DVec2::new(3.0,4.0));
    assert_ne!(sk.polyline(spline),old);
    assert!(!profiles(&sk).is_empty());
}

#[test]
fn spline_copy_mirror_revolve_and_native_roundtrip() {
    let mut s=Session::default();
    run(&mut s,json!({"op":"create_sketch","plane":"XZ"}));
    let r=run(&mut s,json!({"op":"add_geometry","items":[{"type":"spline","points":[[1,0],[3,3],[2,7],[1,10]]},{"type":"polyline","points":[[1,10],[0,10],[0,0],[1,0]]}]}));
    let spline=r["items"][0]["entities"][0].as_u64().unwrap() as u32;
    let id=run(&mut s,json!({"op":"revolve","axis":"y","operation":"new"}))["feature"].as_u64().unwrap() as u32;
    assert!(s.built.errors.is_empty()); assert!(!s.built.body(id).unwrap().solids.is_empty());
    let original=io::to_json(&s.doc); let mut doc=io::from_json(&original).unwrap();
    let built=doc.rebuild(); assert!(built.errors.is_empty()); assert!(!built.body(id).unwrap().solids.is_empty());
    let sk=s.doc.sketch(1).unwrap(); let clip=sk.copy(&[spline]); clip.validate().unwrap();
    let mut other=Sketch::new(Plane::XY); let pasted=other.paste(&clip,DVec2::new(5.0,0.0));
    let Geom::Spline { a,.. }=other.entities[&pasted[0]].geom else { panic!() }; near(other.pos(a).x,6.0);
    let axis_end=other.add_point(DVec2::Y*20.0); let axis=other.add_line(0,axis_end);
    let mirrored=other.mirror(&pasted,axis).unwrap();
    let Geom::Spline { a,.. }=other.entities[&mirrored[0]].geom else { panic!() }; near(other.pos(a).x,-6.0);
    assert!(solver::solve(&mut other,&[]).ok);
}

#[test]
fn gaps_match_profiles_with_coincident_points_and_ignore_guides() {
    let mut sk=Sketch::new(Plane::XY);
    let a=sk.add_point(DVec2::new(1.0,1.0));let b=sk.add_point(DVec2::new(3.0,1.0));let b2=sk.add_point(DVec2::new(3.0,1.0));let c=sk.add_point(DVec2::new(3.0,3.0));let a2=sk.add_point(DVec2::new(1.0,1.0+2e-6));
    sk.add_line(a,b);sk.add_line(b2,c);sk.add_line(c,a2);
    assert_eq!(sk.open_endpoints(),vec![a,a2]); assert!(profiles(&sk).is_empty());
    sk.add_constraint(CKind::Coincident,&[a,a2],None).unwrap();assert!(solver::solve(&mut sk,&[]).ok);
    let guide=sk.add_point(DVec2::new(10.0,10.0));sk.add(Geom::Line { a:0,b:guide },true);sk.add(Geom::Circle {c:guide,r:2.0},false);
    assert!(sk.open_endpoints().is_empty()); assert_eq!(profiles(&sk).len(),2);
}

#[test]
fn malformed_new_geometry_is_atomic_and_ordinary_files_stay_v1() {
    let mut s=Session::default();run(&mut s,json!({"op":"create_sketch"}));
    let before=io::to_json(&s.doc);assert_eq!(serde_json::from_str::<J>(&before).unwrap()["version"],1);
    for item in [json!({"type":"arc3","start":[0,0],"through":[1,0],"end":[2,0]}),json!({"type":"spline","points":[[0,0],[0,0],[2,0],[3,1]]}),json!({"type":"spline","points":[[0,0],[1,2]]}),json!({"type":"tangent_arc","source":999,"start":0,"end":[3,4]})] {
        assert!(execute(&mut s,&json!({"op":"add_geometry","items":[item]}),None).is_err());assert_eq!(io::to_json(&s.doc),before);
    }
}

#[test]
fn coordinate_dimensions_follow_copy_and_move_without_losing_expressions() {
    let mut sk=Sketch::new(Plane::XY);
    let p=sk.add_point(DVec2::new(2.0,3.0)); sk.set_point_coordinates(p,val(2.0),val(3.0)).unwrap();
    let clip=sk.copy(&[p]); let pasted=sk.paste(&clip,DVec2::new(5.0,-6.0));
    assert!(solver::solve(&mut sk,&[]).ok); assert_eq!(sk.pos(pasted[0]),DVec2::new(7.0,-3.0));
    sk.translate(&[p],DVec2::new(-3.0,7.0)); assert!(solver::solve(&mut sk,&[]).ok); assert_eq!(sk.pos(p),DVec2::new(-1.0,10.0));
    for c in sk.constraints.values() { near(Document::new(Unit::Mm).eval(&c.value.as_ref().unwrap().expr,Kind::Length).unwrap(),c.value.as_ref().unwrap().v); }
}

#[test]
fn spline_trim_rejection_preserves_geometry_and_closed_spline_extrudes() {
    let mut sk=Sketch::new(Plane::XY);
    let p=[DVec2::new(1.0,0.0),DVec2::new(4.0,4.0),DVec2::new(-2.0,4.0)].map(|p| sk.add_point(p));
    let curve=sk.add_spline([p[0],p[1],p[2],p[0]],false).unwrap();
    assert!(sk.open_endpoints().is_empty());
    let ps=profiles(&sk); assert_eq!(ps.len(),1);
    let solid=exact::extrude(&[&ps[0]],&sk.plane,0.0,1.0).unwrap(); assert!(!solid.is_empty());
    let end=sk.add_point(DVec2::new(8.0,8.0)); let line=sk.add_line(0,end);
    let before=sk.clone(); assert!(sk.trim(curve,DVec2::new(4.0,4.0)).is_err()); assert_eq!(sk,before);
    assert!(sk.trim(line,DVec2::new(2.0,2.0)).is_err()); assert_eq!(sk,before);
}

#[test]
fn arc_through_point_crosses_chord_and_survives_copy_save_and_delete() {
    let mut s=Session::default();run(&mut s,json!({"op":"create_sketch"}));
    let r=run(&mut s,json!({"op":"add_geometry","items":[{"type":"arc3","start":[0,0],"through":[5,5],"end":[10,0]}]}));
    let arc=r["items"][0]["entities"][0].as_u64().unwrap() as u32;
    let middle=r["items"][0]["points"][1].as_u64().unwrap() as u32;
    let end=r["items"][0]["points"][2].as_u64().unwrap() as u32;
    run(&mut s,json!({"op":"point_coordinates","point":end,"x":10,"y":0}));
    for y in [-5.0,5.0,-8.0,2.0] {
        run(&mut s,json!({"op":"point_coordinates","point":middle,"x":5,"y":y}));
        let sk=s.doc.sketch(1).unwrap();
        assert!(sk.dist_to(arc,sk.pos(middle))<0.02,"middle point left drawn arc at y={y}");
        assert!(sk.polyline(arc).iter().all(|p| p.y*y >= -1e-6));
    }
    let serialized=io::to_json(&s.doc);assert_eq!(serde_json::from_str::<J>(&serialized).unwrap()["version"],3);
    let mut restored=io::from_json(&serialized).unwrap();restored.rebuild();let sk=restored.sketch(1).unwrap();
    assert!(sk.dist_to(arc,sk.pos(middle))<0.02);
    let clip=sk.copy(&[arc]);clip.validate().unwrap();assert!(clip.points.iter().any(|p|p.0==middle));
    let mut pasted=Sketch::new(Plane::XY);let newarc=pasted.paste(&clip,DVec2::new(20.0,10.0))[0];
    let fr_core::sketch::ArcGuide::Through {point:newmiddle}=pasted.arc_guides[&newarc] else {panic!()};
    assert!(solver::solve(&mut pasted,&[]).ok);assert!(pasted.dist_to(newarc,pasted.pos(newmiddle))<0.02);
    pasted.remove(&[newarc]);assert!(!pasted.points.contains_key(&newmiddle));assert!(pasted.arc_guides.is_empty());
}

#[test]
fn guided_arc_minor_major_edits_and_tangent_end_crossing_keep_intent() {
    let mut sk=Sketch::new(Plane::XY);
    let p=[DVec2::X,DVec2::NEG_X,DVec2::NEG_Y].map(|p|sk.add_point(p));let arc=sk.add_arc3(p[0],p[1],p[2],false).unwrap();
    sk.set_point_coordinates(p[0],val(1.0),val(0.0)).unwrap();sk.set_point_coordinates(p[2],val(0.0),val(-1.0)).unwrap();
    near(sk.arc_angles(arc).unwrap().1,std::f64::consts::PI*1.5);
    sk.set_point_coordinates(p[1],val(0.7),val(-0.1)).unwrap();assert!(solver::solve(&mut sk,&[]).ok);
    assert!(sk.dist_to(arc,sk.pos(p[1]))<0.01);assert!(sk.arc_angles(arc).unwrap().1<std::f64::consts::PI);
    let mut sk=Sketch::new(Plane::XY);let a=sk.add_point(DVec2::new(-2.0,0.0));let line=sk.add_line(a,0);sk.add_constraint(CKind::Fix,&[line],None).unwrap();
    let end=sk.add_point(DVec2::new(2.0,2.0));let arc=sk.add_tangent_arc(line,0,end,false).unwrap();
    for y in [-2.0,3.0,-4.0] {
        sk.set_point_coordinates(end,val(2.0),val(y)).unwrap();assert!(solver::solve(&mut sk,&[]).ok);
        near(sk.endpoint_tangent(line,0).unwrap().dot(-sk.endpoint_tangent(arc,0).unwrap()),1.0);
        assert!(sk.polyline(arc).iter().all(|p|p.y*y>=-1e-5));
    }
    let clip=sk.copy(&[line,arc]);clip.validate().unwrap();let mut other=Sketch::new(Plane::XY);let ids=other.paste(&clip,DVec2::new(10.0,20.0));assert!(solver::solve(&mut other,&[]).ok);other.validate().unwrap();assert!(other.arc_guides.contains_key(&ids[1]));
    let clip=sk.copy(&[arc]);assert!(clip.arc_guides.is_empty(),"copying without the tangent source must give an ordinary arc");
    let constraint=sk.constraints.iter().find(|(_,c)|c.kind==CKind::Tangent).map(|(id,_)|*id).unwrap();sk.remove(&[constraint]);assert!(!sk.arc_guides.contains_key(&arc));sk.validate().unwrap();
}

#[test]
fn invalid_arc_guides_are_rejected_and_tangent_source_move_keeps_shared_point() {
    use fr_core::sketch::ArcGuide;
    let mut sk=Sketch::new(Plane::XY);let a=sk.add_point(DVec2::new(-2.0,0.0));let line=sk.add_line(a,0);let end=sk.add_point(DVec2::new(2.0,2.0));let arc=sk.add_tangent_arc(line,0,end,false).unwrap();
    sk.translate(&[line,arc],DVec2::new(10.0,20.0));sk.validate().unwrap();
    let ArcGuide::Tangent {source,start}=sk.arc_guides[&arc] else {panic!()};assert_eq!(source,line);assert_ne!(start,0);assert!(sk.ent_points(line).contains(&start));assert!(solver::solve(&mut sk,&[]).ok);
    sk.remove(&[line]);assert!(!sk.arc_guides.contains_key(&arc));sk.validate().unwrap();
    let c1=sk.add_point(DVec2::ZERO);let c2=sk.add_point(DVec2::new(2.0,0.0));let joint=sk.add_point(DVec2::X);let e1=sk.add_point(DVec2::Y);let e2=sk.add_point(DVec2::new(2.0,-1.0));
    let first=sk.add(Geom::Arc {c:c1,s:joint,e:e1},false);let second=sk.add(Geom::Arc {c:c2,s:joint,e:e2},false);sk.add_constraint(CKind::Tangent,&[first,second],None).unwrap();
    sk.arc_guides.insert(first,ArcGuide::Tangent {source:second,start:joint});sk.arc_guides.insert(second,ArcGuide::Tangent {source:first,start:joint});
    assert!(sk.validate().unwrap_err().contains("cycle"));
    sk.arc_guides.remove(&second);sk.arc_guides.insert(first,ArcGuide::Through {point:99999});assert!(sk.validate().is_err());
}
