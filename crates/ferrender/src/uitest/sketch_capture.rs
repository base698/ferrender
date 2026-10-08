//! Real pointer placement with sticky, screen-space sketch geometry capture.
use super::*;
use fr_core::{Id, Sketch};

fn setup<'a>() -> (H<'a>,Id,Id,[Id;2]) {
    let mut h=state_harness();
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().cam.scale=10.0;
    h.state_mut().opts.snap_grid=true;
    let mut points=[0;2]; let mut line=0;
    assert!(h.state_mut().sketch_edit(|sk,_| {
        points=[sk.add_point(DVec2::new(-20.0,0.37)),sk.add_point(DVec2::new(20.0,0.37))];
        sk.fixed.extend(points);
        line=sk.add_line(points[0],points[1]); Ok(())
    }));
    let sid=h.state().sketch().unwrap().0;
    run(&mut h,Action::Tool(Tool::Point));
    (h,sid,line,points)
}

fn hover(h:&mut H,x:f64,y:f64) {
    let p=at(h,x,y); h.hover_at(p); h.step();
}
fn click_modifiers(h:&mut H,p:Pos2,modifiers:Modifiers) {
    h.event(Event::ModifiersChanged(modifiers)); h.hover_at(p); h.step();
    for pressed in [true,false] { h.event(Event::PointerButton {pos:p,button:PointerButton::Primary,pressed,modifiers}); h.step(); }
}
fn attached_point(sk:&Sketch,line:Id)->Id {
    sk.constraints.values().find(|c|c.kind==CKind::Coincident && c.refs.contains(&line)).and_then(|c|c.refs.iter().find(|id|sk.points.contains_key(id))).copied().expect("placed point should carry a saved point-on-line constraint")
}

#[test]
fn point_capture_tracks_a_line_beyond_acquisition_and_releases_away_without_grid_stealing() {
    let (mut h,_,line,_)=setup();
    hover(&mut h,3.0,0.87);
    let first=h.state().sketch_capture.current().unwrap();
    assert_eq!(first.on,Some(line));
    assert!(first.p.distance(DVec2::new(3.0,0.37))<1e-5,"capture must beat the nearby integer grid");
    for (x,y) in [(5.0,1.17),(8.0,1.47),(11.0,1.77)] { hover(&mut h,x,y); }
    let tracked=h.state().sketch_capture.current().unwrap();
    assert_eq!(tracked.on,Some(line));
    assert!(tracked.p.distance(DVec2::new(11.0,0.37))<1e-5,"the captured point should slide along the edge");
    hover(&mut h,11.0,2.57);
    assert!(h.state().sketch_capture.current().is_none(),"22 px away must release");
    hover(&mut h,11.0,1.77);
    assert!(h.state().sketch_capture.current().is_none(),"14 px must not acquire a fresh edge");
    hover(&mut h,11.0,0.87);
    assert_eq!(h.state().sketch_capture.current().unwrap().on,Some(line));
}

#[test]
fn placed_capture_is_a_saved_constraint_that_follows_the_line_and_undoes() {
    let (mut h,sid,line,endpoints)=setup();
    let p=at(&h,6.0,0.87); click(&mut h,p);
    let point=attached_point(h.state().doc().sketch(sid).unwrap(),line);
    assert!((h.state().doc().sketch(sid).unwrap().pos(point).y-0.37).abs()<1e-6);
    let placed=h.state().doc().clone();
    run(&mut h,Action::Undo);
    assert!(!h.state().doc().sketch(sid).unwrap().points.contains_key(&point));
    run(&mut h,Action::Redo);
    assert_eq!(h.state().doc(),&placed);
    h.state_mut().session.edit(|doc| {
        let sk=doc.sketch_mut(sid).unwrap();
        for endpoint in endpoints {sk.points.get_mut(&endpoint).unwrap().y+=5.0;}
        Ok(())
    }).unwrap();
    h.state_mut().refresh(); h.step();
    assert!((h.state().doc().sketch(sid).unwrap().pos(point).y-5.37).abs()<1e-5,"a saved attachment must follow the fixed source line after an edit");
    let saved=fr_core::io::to_json(h.state().doc());
    let restored=fr_core::Session::new(fr_core::io::from_json(&saved).unwrap());
    assert_eq!(attached_point(restored.doc.sketch(sid).unwrap(),line),point);
    assert!((restored.doc.sketch(sid).unwrap().pos(point).y-5.37).abs()<1e-5);
}

#[test]
fn endpoints_win_alt_bypasses_and_tool_or_sketch_changes_drop_capture() {
    let (mut h,sid,line,endpoints)=setup();
    hover(&mut h,19.6,0.87);
    assert_eq!(h.state().sketch_capture.current().unwrap().point,Some(endpoints[1]));
    let count=h.state().doc().sketch(sid).unwrap().points.len();
    let p=at(&h,19.6,0.87); click(&mut h,p);
    assert_eq!(h.state().doc().sketch(sid).unwrap().points.len(),count,"endpoint capture must reuse the endpoint");
    hover(&mut h,6.0,0.87);
    assert_eq!(h.state().sketch_capture.current().unwrap().on,Some(line));
    let p=at(&h,6.0,0.87); click_modifiers(&mut h,p,Modifiers::ALT);
    let sk=h.state().doc().sketch(sid).unwrap();
    assert!(sk.points.values().any(|p|p.distance(DVec2::new(6.0,0.87))<1e-5),"Alt places freely even while grid snap is enabled");
    assert!(!sk.constraints.values().any(|c|c.kind==CKind::Coincident && c.refs.contains(&line)));
    h.event(Event::ModifiersChanged(Modifiers::NONE));h.step();
    for (x,y) in [(8.0,0.87),(9.0,1.17),(10.0,1.47),(11.0,1.77)] {hover(&mut h,x,y);}
    assert_eq!(h.state().sketch_capture.current().unwrap().on,Some(line));
    run(&mut h,Action::Tool(Tool::Line));
    assert!(h.state().sketch_capture.current().is_none(),"a different tool cannot inherit the wider release band");
    h.state_mut().finish_sketch();
    assert!(h.state().sketch_capture.current().is_none());
    h.state_mut().create_sketch(Plane::XY); h.step();
    run(&mut h,Action::Tool(Tool::Point));
    hover(&mut h,6.0,0.87);
    assert!(h.state().sketch_capture.current().is_none(),"capture cannot leak into a different sketch");
}

#[test]
fn circles_and_visible_arc_extents_capture_real_points() {
    let mut h=state_harness();h.state_mut().create_sketch(Plane::XY);h.state_mut().cam.scale=10.0;
    let (mut circle,mut arc)=(0,0);
    assert!(h.state_mut().sketch_edit(|sk,_| {
        let c=sk.add_point(DVec2::new(-12.0,10.0));circle=sk.add(Geom::Circle {c,r:6.0},false);
        let c=sk.add_point(DVec2::new(12.0,10.0));let s=sk.add_point(DVec2::new(18.0,10.0));let e=sk.add_point(DVec2::new(12.0,16.0));
        sk.fixed.extend([c,s,e]);arc=sk.add(Geom::Arc {c,s,e},false);Ok(())
    }));
    run(&mut h,Action::Tool(Tool::Point));
    hover(&mut h,-18.5,10.0);
    let snap=h.state().sketch_capture.current().unwrap();assert_eq!(snap.on,Some(circle));
    assert!((snap.p.distance(DVec2::new(-12.0,10.0))-6.0).abs()<1e-7);
    hover(&mut h,12.0,3.5);
    assert!(h.state().sketch_capture.current().is_none(),"the missing three quarters of an arc are not snappable");
    let off=DVec2::new(12.0,10.0)+DVec2::splat(6.5/std::f64::consts::SQRT_2);
    hover(&mut h,off.x,off.y);
    let snap=h.state().sketch_capture.current().unwrap();assert_eq!(snap.on,Some(arc));
    assert!((snap.p.distance(DVec2::new(12.0,10.0))-6.0).abs()<1e-7);
    let p=at(&h,off.x,off.y);click(&mut h,p);
    assert!(h.state().sketch().unwrap().1.constraints.values().any(|c|c.kind==CKind::Coincident && c.refs.contains(&arc)));
}

#[test]
fn shift_angle_lock_remains_authoritative_when_a_line_endpoint_passes_a_captured_edge() {
    let (mut h,_,source,_)=setup();h.state_mut().opts.snap_grid=false;
    run(&mut h,Action::Tool(Tool::Line));
    let start=at(&h,-12.0,-10.0);click(&mut h,start);
    hover(&mut h,-2.0,-5.0);
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));h.step();
    let locked=h.state().typed.as_ref().unwrap().locked.unwrap();
    hover(&mut h,12.0,0.87);
    let end=at(&h,12.0,0.87);click_modifiers(&mut h,end,Modifiers::SHIFT);
    let sk=h.state().sketch().unwrap().1;
    let line=*sk.entities.keys().find(|id|**id!=source).unwrap();
    assert!(sk.constraints.values().any(|c|c.kind==CKind::Angle && c.refs==[line]));
    assert!((sk.measure(CKind::Angle,&[line])-locked).abs()<1e-5);
    assert!(!sk.constraints.values().any(|c|c.kind==CKind::Coincident && c.refs.contains(&source)),"an angle-adjusted endpoint must not save a false edge attachment");
}

#[test]
fn captured_edge_point_and_release_hint_render_in_both_themes() {
    let mut h=harness();h.state_mut().create_sketch(Plane::XY);h.state_mut().cam.scale=12.0;
    let mut line=0;
    assert!(h.state_mut().sketch_edit(|sk,_| {
        let a=sk.add_point(DVec2::new(-20.0,8.0));let b=sk.add_point(DVec2::new(20.0,8.0));
        sk.fixed.extend([a,b]);line=sk.add_line(a,b);Ok(())
    }));
    run(&mut h,Action::Tool(Tool::Point));hover(&mut h,3.0,8.6);
    assert_eq!(h.state().sketch_capture.current().unwrap().on,Some(line));
    save(&mut h,"sticky-sketch-capture.png");
    h.state_mut().set_appearance(Appearance::Dark);
    save(&mut h,"sticky-sketch-capture-dark.png");
}
