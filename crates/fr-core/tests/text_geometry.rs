use fr_core::{api, doc::{Text, FeatureKind, Op}, exact, io, Kind, Plane, Session};
use fr_core::text::Align;
use glam::DVec3;
use serde_json::json;

fn block() -> Session {
    let mut s = Session::default();
    for c in [json!({"op":"create_sketch","plane":"XY"}), json!({"op":"add_geometry","items":[{"type":"rect","from":[0,0],"to":[40,30]}]}), json!({"op":"extrude","distance":10})] {
        api::execute(&mut s, &c, None).unwrap();
    }
    s
}
fn lettering(s: &Session, p: DVec3, normal: DVec3, op: Op) -> Text {
    let value = |v, kind| s.doc.value(v, kind).unwrap();
    Text { text: "BO".into(), plane: Plane::from_normal(p,normal), height:value("4 mm",Kind::Length),depth:value("1 mm",Kind::Length),spacing:value("0",Kind::Length),angle:value("0",Kind::Angle),x:value("0",Kind::Length),y:value("0",Kind::Length),align:Align::Left,op,body:Some(2),face:Some(p),frame:s.built.frame(2) }
}
fn add(s: &mut Session, t: Text) -> Result<fr_core::Id,String> {
    s.edit_feature(|d| { let id=d.add_feature(FeatureKind::Text(t)); Ok((id,id)) })
}
fn volume(s: &Session) -> f64 { s.built.body(2).unwrap().solids.iter().map(|s|s.volume()).sum() }

#[test]
fn raised_and_recessed_text_work_on_all_six_face_directions() {
    for (p,n) in [(DVec3::new(15.,10.,10.),DVec3::Z),(DVec3::new(15.,15.,0.),-DVec3::Z),(DVec3::new(40.,10.,3.),DVec3::X),(DVec3::new(0.,15.,3.),-DVec3::X),(DVec3::new(15.,30.,3.),DVec3::Y),(DVec3::new(15.,0.,3.),-DVec3::Y)] {
        let mut s=block();
        let mut t=lettering(&s,p,n,Op::Join);
        // Keep the baseline horizontal on side faces, so cap height fits their 10 mm height.
        if n.z == 0.0 { t.plane.y=DVec3::Z; t.plane.x=t.plane.y.cross(n); }
        let base=volume(&s);
        let id=add(&mut s,t.clone()).unwrap_or_else(|e|panic!("{n}: {e}"));
        let raised=volume(&s)-base;
        assert!(raised>5. && raised<100.);
        assert_eq!(s.built.body(2).unwrap().solids.len(),1);
        s.undo();
        t.op=Op::Cut;
        add(&mut s,t).unwrap();
        assert!((base-volume(&s)-raised).abs()<1e-5, "{n}");
        assert!(s.built.errors.is_empty(), "{id}");
    }
}

#[test]
fn unsupported_lettering_is_transactional_and_lost_face_is_reported() {
    let mut s=block();
    let before=s.doc.clone();
    let baseline=volume(&s);
    let t=lettering(&s,DVec3::new(39.,10.,10.),DVec3::Z,Op::Join);
    assert!(add(&mut s,t).unwrap_err().contains("extends beyond"));
    assert_eq!(s.doc,before);
    assert_eq!(volume(&s),baseline);
    let t=lettering(&s,DVec3::new(10.,10.,50.),DVec3::Z,Op::Cut);
    assert!(add(&mut s,t).unwrap_err().contains("face is no longer"));
    assert_eq!(s.doc,before);
}

#[test]
fn text_follows_face_when_upstream_thickness_changes() {
    let mut s=block();
    let t=lettering(&s,DVec3::new(10.,10.,10.),DVec3::Z,Op::Join);
    let id=add(&mut s,t).unwrap();
    api::execute(&mut s,&json!({"op":"edit_feature","feature":2,"distance":15}),None).unwrap();
    assert!(s.built.errors.is_empty(),"{:?}",s.built.errors);
    let (_,hi)=exact::bounds(&s.built.body(2).unwrap().solids).unwrap();
    assert!((hi.z-16.).abs()<1e-5);
    let encoded=io::to_json(&s.doc);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&encoded).unwrap()["version"],2);
    let mut loaded=io::from_json(&encoded).unwrap();
    assert!(loaded.rebuild().errors.is_empty());
    assert!(matches!(loaded.feature(id).unwrap().kind,FeatureKind::Text(_)));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&io::to_json(&block().doc)).unwrap()["version"],1);
}

#[test]
fn font_counters_remain_open_in_standalone_solid() {
    let mut s=Session::default();
    let mut t=lettering(&s,DVec3::ZERO,DVec3::Z,Op::New);
    t.body=None;t.face=None;t.frame=None;
    let profiles=fr_core::text::profiles("BO",4.,0.,Align::Left).unwrap();
    let expected:f64=profiles.iter().map(|p| fr_core::profile::signed_area(&p.outer).abs()-p.holes.iter().map(|h|fr_core::profile::signed_area(h).abs()).sum::<f64>()).sum();
    let id=add(&mut s,t).unwrap();
    let body=s.built.body(id).unwrap();
    let actual:f64=body.solids.iter().map(|s|s.volume()).sum();
    assert!((actual-expected).abs()<1e-5);
    assert_eq!(body.solids.len(),2);
    assert_eq!(body.mesh.open_edges(),0);
}

#[test]
fn multiple_labels_follow_a_base_thickness_change() {
    let mut s=block();
    let first=lettering(&s,DVec3::new(5.,5.,10.),DVec3::Z,Op::Join);
    add(&mut s,first).unwrap();
    let second=lettering(&s,DVec3::new(5.,18.,10.),DVec3::Z,Op::Cut);
    add(&mut s,second).unwrap();
    api::execute(&mut s,&json!({"op":"edit_feature","feature":2,"distance":15}),None).unwrap();
    assert!(s.built.errors.is_empty(), "{:?}",s.built.errors);
}
