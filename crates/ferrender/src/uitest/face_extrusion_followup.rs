//! The two UI routes into Extrude must preserve a rounded face's true curves.
use super::*;
use fr_core::{FeatureKind, Op};

fn rounded_box(h: &mut H) {
    for c in [
        json!({"op":"primitive","type":"box","width":30,"depth":15,"height":20}),
        json!({"op":"fillet_edges","body":1,"edges":[[15,15,0],[15,0,20]],"radius":2}),
    ] {h.state_mut().execute(&c).unwrap();}
    run(h,Action::View("right"));run(h,Action::Fit);h.run_steps(2);
}
fn accept_cut(h: &mut H) {
    let Dialog::Feature(d)=&mut h.state_mut().dialog else {panic!("expected Extrude")};
    assert!(d.face.is_some());d.text="-13.5 mm".into();d.op=Op::Cut;
    h.run_steps(3);
    assert!(h.state().preview.as_ref().is_some_and(|p|p.2.is_none()),"{:?}",h.state().preview.as_ref().map(|p|&p.2));
    h.get_by_label("OK").click();h.run_steps(3);
    assert_eq!(h.state().dialog,Dialog::None);
    let body=h.state().session.built.body(1).unwrap();
    let (lo,hi)=body.mesh.bbox().unwrap();
    assert!((hi.x-lo.x-16.5).abs()<1e-6,"the cut left fillet strips: {lo:?}..{hi:?}");
    assert_eq!(body.mesh.open_edges(),0);
    let area=15.*20.-2.*(4.-std::f64::consts::PI);
    let volume:f64=body.solids.iter().map(|s|s.volume()).sum();
    assert!((volume-area*16.5).abs()<1e-5);
    let (_,sk)=h.state().doc().sketches().last().unwrap();
    assert_eq!(sk.entities.values().filter(|e|matches!(e.geom,Geom::Arc{..})).count(),2);
}
#[test]
fn selected_rounded_face_extrudes_without_polygon_slivers_and_undo_restores() {
    let mut h=state_harness();rounded_box(&mut h);
    let at=crate::view::to_screen(h.state(),DVec3::new(30.,7.,10.));click(&mut h,at);
    assert_eq!(h.state().sel_face.as_ref().map(|f|f.body),Some(1));
    let before=h.state().doc().clone();
    run(&mut h,Action::Extrude);accept_cut(&mut h);
    run(&mut h,Action::Undo);assert_eq!(h.state().doc(),&before);
    run(&mut h,Action::Redo);
    assert!(matches!(h.state().doc().features.last().unwrap().kind,FeatureKind::Extrude(_)));
    assert!((h.state().session.built.body(1).unwrap().mesh.bbox().unwrap().1.x-16.5).abs()<1e-6);
}
#[test]
fn picking_rounded_face_after_opening_extrude_also_preserves_curves() {
    let mut h=state_harness();rounded_box(&mut h);
    h.state_mut().sel_face=None;h.state_mut().sel_body=None;
    run(&mut h,Action::Extrude);
    assert!(matches!(&h.state().dialog,Dialog::Feature(d) if d.face.is_none()));
    let at=crate::view::to_screen(h.state(),DVec3::new(30.,7.,10.));click(&mut h,at);
    accept_cut(&mut h);
}
