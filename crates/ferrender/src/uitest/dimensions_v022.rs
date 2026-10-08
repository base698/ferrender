use super::*;
use crate::app::EditTarget;

fn setup() -> (H<'static>, u32, u32) {
    let mut h = state_harness();
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().cam.scale = 10.0;
    let mut line = 0;
    h.state_mut().sketch_edit(|sk, _| {
        let a = sk.add_point(DVec2::new(-20.0, 10.0));
        let b = sk.add_point(DVec2::new(-8.0, 14.0));
        line = sk.add_line(a, b);
        Ok(())
    });
    h.run_steps(2);
    let sid = h.state().sketch().unwrap().0;
    (h, sid, line)
}

#[test]
fn direct_line_click_and_preselection_edit_one_persistent_dimension() {
    let (mut h, sid, line) = setup();
    run(&mut h, Action::Tool(Tool::Dimension));
    let midpoint = at(&h, -14.0, 12.0);
    click(&mut h, midpoint);
    assert!(matches!(h.state().value_edit.as_ref().unwrap().target, EditTarget::New(CKind::Distance, _)));
    h.event(Event::Text("10 mm".into()));
    h.step();
    assert_eq!(h.state().value_edit.as_ref().unwrap().text, "10 mm", "typing replaces the selected current value");
    key(&mut h, Key::Enter);
    assert!(h.state().value_edit.is_none());
    let sk = h.state().doc().sketch(sid).unwrap();
    assert!((sk.measure(CKind::Distance, &[line])-10.0).abs()<1e-6);
    let cid = *sk.constraints.keys().next().unwrap();
    run(&mut h, Action::Tool(Tool::Select));
    h.state_mut().sel = vec![line];
    key(&mut h, Key::D);
    assert!(matches!(h.state().value_edit.as_ref().unwrap().target, EditTarget::Existing(id) if id == cid));
    h.event(Event::Text("15 mm".into()));
    h.step();
    key(&mut h, Key::Enter);
    assert_eq!(h.state().doc().sketch(sid).unwrap().constraints.len(), 1);
    assert!((h.state().doc().sketch(sid).unwrap().measure(CKind::Distance, &[line])-15.0).abs()<1e-6);
    run(&mut h, Action::Undo);
    assert!((h.state().doc().sketch(sid).unwrap().measure(CKind::Distance, &[line])-10.0).abs()<1e-6);
    run(&mut h, Action::Redo);
    assert!((h.state().doc().sketch(sid).unwrap().measure(CKind::Distance, &[line])-15.0).abs()<1e-6);
}

#[test]
fn curves_reopen_existing_sizes_and_two_selected_items_dimension_between_them() {
    let (mut h, _, line) = setup();
    let (mut circle, mut arc, mut radius, mut a, mut b, mut parallel, mut slanted) = (0,0,0,0,0,0,0);
    h.state_mut().sketch_edit(|sk, d| {
        let c = sk.add_point(DVec2::new(20.0, 10.0));
        circle = sk.add(Geom::Circle { c, r: 5.0 }, false);
        radius = sk.add_constraint(CKind::Radius, &[circle], Some(d.enter("5 mm", fr_core::Kind::Length)?))?;
        a = sk.add_point(DVec2::new(20.0,-10.0));
        b = sk.add_point(DVec2::new(30.0,-10.0));
        let through = sk.add_point(DVec2::new(25.0,-5.0));
        arc = sk.add_arc3(a,through,b,false)?;
        let p = sk.add_point(DVec2::new(-20.0,-10.0));
        let q = sk.add_point(DVec2::new(-8.0,-6.0));
        parallel = sk.add_line(p,q);
        let end = sk.add_point(DVec2::new(-16.0,2.0));
        slanted = sk.add_line(p,end);
        Ok(())
    });
    h.run_steps(2);
    for (refs, expected) in [(vec![circle],CKind::Radius),(vec![arc],CKind::Radius),(vec![line,parallel],CKind::Distance),(vec![line,slanted],CKind::Angle),(vec![a,b],CKind::Distance),(vec![a,line],CKind::Distance)] {
        run(&mut h,Action::Tool(Tool::Select));
        h.state_mut().sel=refs.clone();
        run(&mut h,Action::Tool(Tool::Dimension));
        match &h.state().value_edit.as_ref().unwrap().target {
            EditTarget::New(kind, actual) => { assert_eq!(*kind,expected); assert_eq!(actual.len(),refs.len()); },
            EditTarget::Existing(id) => assert_eq!(*id,radius),
            _=>panic!("unexpected edit"),
        }
    }
    // Unsupported mixed geometry/constraint selection must not index the wrong entity.
    h.state_mut().dimension_selection(vec![radius,circle],Pos2::new(500.0,500.0));
    assert!(h.state().toast.as_ref().unwrap().0.contains("geometry"));
}

#[test]
fn invalid_dimension_remains_visible_and_never_changes_geometry() {
    let (mut h, _, line) = setup();
    h.state_mut().sel=vec![line];
    run(&mut h,Action::Tool(Tool::Dimension));
    let before=h.state().doc().clone();
    h.state_mut().value_edit.as_mut().unwrap().text="1 / 0".into();
    assert!(!h.state_mut().commit_value());
    let blank=at(&h,30.0,-20.0);
    click(&mut h,blank);
    assert!(h.state().value_edit.as_ref().unwrap().error.is_some());
    assert_eq!(h.state().doc(),&before);
    h.state_mut().value_edit.as_mut().unwrap().text="1e1 mm".into();
    assert!(h.state_mut().commit_value());
    assert!((h.state().sketch().unwrap().1.measure(CKind::Distance,&[line])-10.0).abs()<1e-6);
}
