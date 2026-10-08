//! Headless end-to-end tests: the real app, driven with synthetic input and
//! rendered offscreen on the GPU. Frames are written to `target/uitest/`
//! for eyeballing.

mod dimensions_v022;
mod desktop_open;
mod v030;

use std::path::PathBuf;

use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use fr_core::{CKind, Geom, Plane};
use glam::{DVec2, DVec3};
use serde_json::json;

use crate::app::{Action, App, Dialog, Mode, Tool};
use crate::config::Appearance;
use crate::recovery::Recovery;

mod reference_v022;

type H<'a> = Harness<'a, App>;

mod drawing_v022;

fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/uitest");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness<'a>() -> H<'a> {
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_eframe(|cc| App::new(cc, None));
    // Existing pixel checks exercise Light, independently of the test host's desktop.
    h.state_mut().set_appearance(Appearance::Light);
    h.run_steps(3);
    h
}

fn button(h: &H, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
}

fn click(h: &mut H, pos: Pos2) {
    h.hover_at(pos);
    h.step();
    button(h, pos, true);
    h.step();
    button(h, pos, false);
    h.step();
}

fn drag(h: &mut H, pts: &[Pos2]) {
    h.hover_at(pts[0]);
    h.step();
    button(h, pts[0], true);
    h.step();
    for p in &pts[1..] {
        h.hover_at(*p);
        h.step();
    }
    button(h, *pts.last().unwrap(), false);
    h.step();
}

fn key(h: &mut H, key: Key) {
    h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.step();
}

/// Screen position of a point on the XY plane, in millimetres.
fn at(h: &H, x: f64, y: f64) -> Pos2 {
    crate::view::to_screen(h.state(), DVec3::new(x, y, 0.0))
}

fn save(h: &mut H, name: &str) -> image::RgbaImage {
    h.run_steps(2);
    let img = h.render().expect("the app should render");
    img.save(out_dir().join(name)).unwrap();
    img
}

fn run(h: &mut H, a: Action) {
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, a);
    h.step();
}

#[test]
fn sketch_dimension_copy_paste_and_extrude() {
    let mut h = harness();
    assert!(h.state().gpu, "the test harness should give the app a GPU viewport");
    run(&mut h, Action::NewSketch);
    assert_eq!(h.state().dialog, Dialog::PickPlane);
    h.state_mut().create_sketch(Plane::XY);
    h.run_steps(2);
    let Mode::Sketch(sid) = h.state().mode else { panic!("should be editing the new sketch") };
    let sketch = |h: &H| h.state().session.doc.sketch(sid).unwrap().clone();

    // A rectangle from the origin: the first corner snaps onto the origin point.
    run(&mut h, Action::Tool(Tool::Rect));
    let (a, b) = (at(&h, 0.0, 0.0), at(&h, 50.0, 30.0));
    click(&mut h, a);
    click(&mut h, b);
    let sk = sketch(&h);
    assert_eq!(sk.entities.len(), 4);
    assert_eq!(sk.constraints.len(), 4, "a rectangle gets horizontal and vertical constraints");
    assert_eq!(h.state().report.dof, 2);

    // Dimension the bottom edge directly by clicking it and typing.
    run(&mut h, Action::Tool(Tool::Dimension));
    let edge = at(&h, 25.0, 0.0);
    click(&mut h, edge);
    assert!(h.state().value_edit.is_some(), "the dimension box should open");
    h.state_mut().value_edit.as_mut().unwrap().text = "w = 60 mm".into();
    assert!(h.state_mut().commit_value());
    h.step();
    let sk = sketch(&h);
    assert_eq!(sk.constraints.values().filter(|c| c.kind == CKind::Distance).count(), 1);
    assert_eq!(h.state().doc().params[0].name, "w", "typing `w = 60 mm` defines a parameter");
    let widest = sk.points.values().map(|p| p.x).fold(0.0, f64::max);
    assert!((widest - 60.0).abs() < 1e-6, "the rectangle should now be 60 wide, got {widest}");

    // Typing into the real box works too: the left edge, in inches.
    let edge = at(&h, 0.0, 15.0);
    click(&mut h, edge);
    h.step();
    h.event(Event::Text("1 in".into()));
    h.step();
    key(&mut h, Key::Enter);
    h.run_steps(2);
    assert!(h.state().value_edit.is_none(), "Enter should confirm the box");
    let tallest = sketch(&h).points.values().map(|p| p.y).fold(0.0, f64::max);
    assert!((tallest - 25.4).abs() < 1e-6, "1 in should be 25.4 mm, got {tallest}");
    assert_eq!(h.state().report.dof, 0, "two dimensions fully constrain a rectangle on the origin");

    // A circle inside it, then copy and paste it under the pointer.
    run(&mut h, Action::Tool(Tool::Circle));
    let (c, r) = (at(&h, 15.0, 12.0), at(&h, 20.0, 12.0));
    click(&mut h, c);
    click(&mut h, r);
    assert_eq!(sketch(&h).entities.len(), 5);
    run(&mut h, Action::Tool(Tool::Select));
    click(&mut h, r);
    assert_eq!(h.state().sel.len(), 1, "clicking the circle selects it");
    h.event(Event::Copy);
    h.step();
    let target = at(&h, 45.0, 12.0);
    h.hover_at(target);
    h.step();
    h.event(Event::Paste(String::new()));
    h.run_steps(2);
    let sk = sketch(&h);
    assert_eq!(sk.entities.len(), 6, "paste adds a second circle");
    let pasted = sk.curve(h.state().sel[0]).unwrap();
    assert!((pasted.0.x - 45.0).abs() < 0.5 && (pasted.0.y - 12.0).abs() < 0.5 && (pasted.1 - 5.0).abs() < 0.1, "{pasted:?}");

    // Dragging a point of a fully constrained rectangle goes nowhere; undo steps back through the paste.
    let corner = at(&h, 60.0, 25.4);
    drag(&mut h, &[corner, corner + egui::vec2(30.0, -20.0), corner + egui::vec2(60.0, -40.0)]);
    let widest = sketch(&h).points.values().map(|p| p.x).fold(0.0, f64::max);
    assert!((widest - 60.0).abs() < 1e-6);
    run(&mut h, Action::Undo);
    assert_eq!(sketch(&h).entities.len(), 5);
    run(&mut h, Action::Redo);
    assert_eq!(sketch(&h).entities.len(), 6);
    save(&mut h, "sketch.png");

    // Extrude straight from the sketch: the plate with both holes is preselected.
    run(&mut h, Action::Extrude);
    let Dialog::Feature(mut f) = h.state().dialog.clone() else { panic!("the extrude dialog should open") };
    assert_eq!(f.profiles.len(), 1, "the outer region is preselected; the circles are its holes");
    f.text = "t = 8 mm".into();
    h.state_mut().dialog = Dialog::Feature(f);
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none() && p.1.bodies.len() == 1), "the dialog previews the body");
    save(&mut h, "extrude-dialog.png");
    h.state_mut().apply_dialog();
    h.state_mut().fit();
    let body = &h.state().session.built.bodies[0];
    let expect = (60.0 * 25.4 - 2.0 * std::f64::consts::PI * 25.0) * 8.0;
    assert!((body.mesh.volume() - expect).abs() < expect * 0.003, "{} vs {expect}", body.mesh.volume());

    // The body is drawn by the GPU: its centre is no longer background.
    let img = save(&mut h, "model.png");
    let centre = crate::view::to_screen(h.state(), DVec3::new(30.0, 20.0, 8.0));
    let px = img.get_pixel(centre.x as u32, centre.y as u32).0;
    assert!(px[0] < 225 && px[2] > px[0], "expected the shaded body at the centre, got {px:?}");

    // Parameters drive the model.
    h.state_mut().execute(&json!({"op": "set_parameter", "name": "t", "expr": "0.5 in"})).unwrap();
    let top = h.state().session.built.bodies[0].mesh.bbox().unwrap().1.z;
    assert!((top - 12.7).abs() < 1e-6);
}

#[test]
fn line_tool_constraints_and_revolve() {
    let mut h = harness();
    h.state_mut().create_sketch(Plane::XY);
    h.run_steps(2);
    let Mode::Sketch(sid) = h.state().mode else { panic!() };
    let sketch = |h: &H| h.state().session.doc.sketch(sid).unwrap().clone();

    // A closed outline drawn with the line tool, a little off square.
    run(&mut h, Action::Tool(Tool::Line));
    for (x, y) in [(10.0, 0.0), (30.0, 0.3), (31.0, 40.0), (10.0, 38.0), (10.0, 0.0)] {
        let p = at(&h, x, y);
        click(&mut h, p);
    }
    let sk = sketch(&h);
    assert_eq!(sk.entities.len(), 4, "the outline closes on its first point");
    assert!(h.state().clicks.is_empty(), "closing the outline ends the line");
    assert_eq!(fr_core::profile::profiles(&sk).len(), 1);
    assert!(sk.constraints.values().any(|c| c.kind == CKind::Horizontal), "a nearly horizontal segment snaps horizontal");

    // Select two opposite sides and make them parallel from the toolbar action.
    run(&mut h, Action::Tool(Tool::Select));
    let ids: Vec<u32> = sk.entities.keys().copied().collect();
    h.state_mut().sel = vec![ids[0], ids[2]];
    run(&mut h, Action::Constrain(CKind::Parallel));
    let sk = sketch(&h);
    let (a, b) = (sk.line(ids[0]).unwrap(), sk.line(ids[2]).unwrap());
    assert!((a.1 - a.0).perp_dot(b.1 - b.0).abs() < 1e-6, "the sides should be parallel");
    // Asking for something impossible is refused and leaves the sketch alone.
    h.state_mut().sel = vec![ids[0], ids[2]];
    run(&mut h, Action::Constrain(CKind::Perpendicular));
    assert_eq!(sketch(&h), sk);
    assert!(h.state().toast.is_some());
    save(&mut h, "lines.png");

    // Lathe it around the sketch's Y axis.
    run(&mut h, Action::Revolve);
    let Dialog::Feature(f) = h.state().dialog.clone() else { panic!("the revolve dialog should open") };
    assert!(f.revolve && f.profiles.len() == 1);
    h.run_steps(2);
    h.state_mut().toast = None;
    h.state_mut().apply_dialog();
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.clone()), None, "the revolve should apply cleanly");
    h.state_mut().fit();
    let body = &h.state().session.built.bodies[0];
    assert_eq!(body.mesh.open_edges(), 0);
    assert!(body.mesh.volume() > 50_000.0);
    save(&mut h, "revolve.png");
}

fn drag_with(h: &mut H, b: PointerButton, from: Pos2, by: egui::Vec2) {
    h.hover_at(from);
    h.step();
    h.event(Event::PointerButton { pos: from, button: b, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for i in 1..=4 {
        h.hover_at(from + by * (i as f32 / 4.0));
        h.step();
    }
    h.event(Event::PointerButton { pos: from + by, button: b, pressed: false, modifiers: Modifiers::NONE });
    h.step();
}

#[test]
fn mouse_navigation() {
    let mut h = harness();
    let c = h.state().vp.center();
    let before = h.state().cam;
    drag_with(&mut h, PointerButton::Primary, c, egui::vec2(80.0, 30.0));
    let orbited = h.state().cam;
    assert!(orbited.yaw != before.yaw && orbited.pitch != before.pitch, "dragging in the model view should orbit");
    assert_eq!(orbited.target, before.target);
    drag_with(&mut h, PointerButton::Secondary, c, egui::vec2(-40.0, 10.0));
    assert!(h.state().cam.yaw != orbited.yaw, "right-drag should orbit too");
    let turned = h.state().cam;
    drag_with(&mut h, PointerButton::Middle, c, egui::vec2(80.0, 30.0));
    assert!(h.state().cam.target != turned.target, "middle-drag should pan");
    assert_eq!((h.state().cam.yaw, h.state().cam.pitch), (turned.yaw, turned.pitch));

    // Space-drag pans.
    let panned = h.state().cam;
    h.event(Event::PointerMoved(c));
    h.key_down(Key::Space);
    h.step();
    drag_with(&mut h, PointerButton::Primary, c, egui::vec2(50.0, 0.0));
    h.key_up(Key::Space);
    h.step();
    assert!(h.state().cam.target != panned.target && h.state().cam.yaw == panned.yaw, "Space-drag should pan");

    // In a sketch, the left button draws and selects, so the view stays put; the right button still orbits.
    h.state_mut().create_sketch(Plane::XY);
    h.run_steps(2);
    let c = h.state().vp.center();
    let facing = h.state().cam;
    drag_with(&mut h, PointerButton::Primary, c + egui::vec2(200.0, 200.0), egui::vec2(60.0, 40.0));
    assert_eq!(h.state().cam, facing);
    drag_with(&mut h, PointerButton::Secondary, c, egui::vec2(60.0, 40.0));
    assert!(h.state().cam.yaw != facing.yaw);
}

/// A 40 x 20 x 10 plate with its corner on the origin.
fn plate(h: &mut H) {
    for cmd in [json!({"op": "create_sketch", "plane": "XY"}), json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [40, 20]}]}), json!({"op": "extrude", "distance": 10})] {
        h.state_mut().execute(&cmd).unwrap();
    }
    h.state_mut().fit();
    h.run_steps(2);
}

fn volume(h: &H) -> f64 {
    h.state().session.built.bodies.iter().map(|b| b.mesh.volume()).sum()
}

#[test]
fn faces_select_and_extrude() {
    let mut h = harness();
    plate(&mut h);
    let top = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 10.0));
    let before = save(&mut h, "face-before.png");
    click(&mut h, top);
    let face = h.state().sel_face.clone().expect("clicking a body selects the face under the pointer");
    assert!(h.state().sel_body.is_none(), "a click picks the face, not the whole body");
    assert!((face.area - 800.0).abs() < 1e-6, "the top face is 40 x 20, got {}", face.area);
    let plane = face.plane.expect("the top face is flat");
    assert!((plane.normal() - DVec3::Z).length() < 1e-9 && (plane.origin.z - 10.0).abs() < 1e-9);
    assert_eq!(face.loops.len(), 1);
    assert_eq!(face.loops[0].len(), 4, "the outline is the four corners");
    assert_eq!(h.state().target_body(), Some(face.body), "Move acts on the body a selected face belongs to");

    // The selected face is lit; the front face is not.
    let after = save(&mut h, "face-selected.png");
    let px = |img: &image::RgbaImage, p: Pos2| img.get_pixel(p.x as u32, p.y as u32).0;
    let front = crate::view::to_screen(h.state(), DVec3::new(20.0, 0.0, 5.0));
    assert_ne!(px(&before, top), px(&after, top), "the selected face should change colour");
    assert_eq!(px(&before, front), px(&after, front), "the other faces should not");

    // Extrude with the face selected: it pulls the face out and joins.
    run(&mut h, Action::Extrude);
    let Dialog::Feature(mut f) = h.state().dialog.clone() else { panic!("the extrude dialog should open") };
    assert_eq!(f.face.as_ref().map(|f| f.body), Some(face.body), "the selected face is what gets extruded");
    f.text = "5 mm".into();
    h.state_mut().dialog = Dialog::Feature(f);
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "{:?}", h.state().preview.as_ref().map(|p| &p.2));
    h.state_mut().apply_dialog();
    assert_eq!(h.state().session.built.bodies.len(), 1);
    assert!((volume(&h) - 40.0 * 20.0 * 15.0).abs() < 1.0, "{}", volume(&h));
    assert_eq!(h.state().session.built.bodies[0].mesh.open_edges(), 0);

    // Picking a face inside the dialog, and pushing it in, cuts.
    h.state_mut().fit();
    h.run_steps(2);
    run(&mut h, Action::Extrude);
    let side = crate::view::to_screen(h.state(), DVec3::new(20.0, 0.0, 7.0));
    click(&mut h, side);
    let Dialog::Feature(mut f) = h.state().dialog.clone() else { panic!() };
    let picked = f.face.clone().expect("clicking a face in the dialog picks it");
    assert!((picked.plane.unwrap().normal() + DVec3::Y).length() < 1e-9, "the front face looks along -Y");
    f.text = "-4 mm".into();
    h.state_mut().dialog = Dialog::Feature(f);
    h.run_steps(2);
    save(&mut h, "face-extrude-dialog.png");
    h.state_mut().apply_dialog();
    assert!((volume(&h) - 40.0 * 16.0 * 15.0).abs() < 1.0, "pushing the front face in 4 leaves 40 x 16 x 15, got {}", volume(&h));
    assert!(h.state().session.built.errors.is_empty());

    // New Sketch with a face selected starts on that face.
    h.state_mut().fit();
    h.run_steps(2);
    let top = crate::view::to_screen(h.state(), DVec3::new(20.0, 12.0, 15.0));
    click(&mut h, top);
    run(&mut h, Action::NewSketch);
    let Mode::Sketch(sid) = h.state().mode else { panic!("should be sketching on the face") };
    assert!((h.state().session.doc.sketch(sid).unwrap().plane.origin.z - 15.0).abs() < 1e-9);
}

#[test]
fn move_dialog_types_and_drags() {
    let mut h = harness();
    plate(&mut h);
    let lo = |h: &H| h.state().session.built.bodies[0].mesh.bbox().unwrap().0;

    // Select a face, press Move, type a distance, press Enter in the box.
    let top = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 10.0));
    click(&mut h, top);
    run(&mut h, Action::Transform);
    let Dialog::Transform(mut t) = h.state().dialog.clone() else { panic!("Move should open for the body the face belongs to") };
    t.translate[0] = "1 in".into();
    t.translate[2] = "5".into();
    h.state_mut().dialog = Dialog::Transform(t);
    h.run_steps(2);
    let shown = h.state().shown().bodies[0].mesh.bbox().unwrap().0;
    assert!((shown - DVec3::new(25.4, 0.0, 5.0)).length() < 1e-9, "the preview shows the body moved, got {shown}");
    h.state_mut().apply_dialog();
    assert_eq!(h.state().dialog, Dialog::None);
    assert!((lo(&h) - DVec3::new(25.4, 0.0, 5.0)).length() < 1e-9, "{}", lo(&h));

    // Dragging the body in the Move dialog slides it across the screen.
    run(&mut h, Action::View("top"));
    h.state_mut().fit();
    h.run_steps(2);
    let grab = crate::view::to_screen(h.state(), DVec3::new(45.0, 10.0, 15.0));
    click(&mut h, grab);
    run(&mut h, Action::Transform);
    let (cam, scale) = (h.state().cam, h.state().cam.scale as f32);
    drag_with(&mut h, PointerButton::Primary, grab, egui::vec2(10.0 * scale, -6.0 * scale));
    assert_eq!(h.state().cam, cam, "dragging the body should not turn the view");
    let Dialog::Transform(t) = h.state().dialog.clone() else { panic!() };
    let moved = [h.state().doc().eval(&t.translate[0], fr_core::Kind::Length).unwrap(), h.state().doc().eval(&t.translate[1], fr_core::Kind::Length).unwrap()];
    assert!((moved[0] - 10.0).abs() < 0.3 && (moved[1] - 6.0).abs() < 0.3, "dragged 10 right and 6 up in the top view, got {moved:?}");
    save(&mut h, "move-dialog.png");
    h.state_mut().apply_dialog();
    assert!((lo(&h).x - 35.4).abs() < 0.3 && (lo(&h).y - 6.0).abs() < 0.3, "{}", lo(&h));
}

#[test]
fn sketch_toolkit_in_the_app() {
    let mut h = harness();
    plate(&mut h);
    // Sketch on the plate's top face, found by clicking it.
    let top = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 10.0));
    click(&mut h, top);
    run(&mut h, Action::NewSketch);
    h.run_steps(2);
    let Mode::Sketch(sid) = h.state().mode else { panic!("should be sketching on the face") };
    let sketch = |h: &H| h.state().session.doc.sketch(sid).unwrap().clone();
    let on = |h: &H, x: f64, y: f64| crate::view::to_screen(h.state(), sketch(h).plane.to_world(glam::DVec2::new(x, y)));

    // Project: clicking the face brings its four edges in, fixed.
    run(&mut h, Action::Tool(Tool::Project));
    let p = on(&h, 20.0, 10.0);
    click(&mut h, p);
    let sk = sketch(&h);
    assert_eq!(sk.entities.len(), 4, "the face's outline is projected");
    assert_eq!(h.state().report.dof, 0, "projected geometry is fixed");

    // Polygon: an octagon in the middle.
    h.state_mut().opts.sides = 8;
    run(&mut h, Action::Tool(Tool::Polygon));
    let (c, r) = (on(&h, 20.0, 10.0), on(&h, 26.0, 10.0));
    click(&mut h, c);
    click(&mut h, r);
    let sk = sketch(&h);
    assert_eq!(sk.entities.values().filter(|e| !e.construction).count(), 12, "eight sides added to the four edges");
    save(&mut h, "polygon-on-face.png");

    // Fillet a corner of a fresh rectangle through the value box.
    run(&mut h, Action::FinishSketch);
    h.state_mut().create_sketch(Plane::XY.offset(30.0));
    h.run_steps(2);
    let Mode::Sketch(sid) = h.state().mode else { panic!() };
    let sketch = |h: &H| h.state().session.doc.sketch(sid).unwrap().clone();
    let on = |h: &H, x: f64, y: f64| crate::view::to_screen(h.state(), sketch(h).plane.to_world(glam::DVec2::new(x, y)));
    run(&mut h, Action::Tool(Tool::Rect));
    let (a, b) = (on(&h, 5.0, 5.0), on(&h, 45.0, 30.0));
    click(&mut h, a);
    click(&mut h, b);
    run(&mut h, Action::Tool(Tool::Select));
    let corner = on(&h, 45.0, 30.0);
    click(&mut h, corner);
    run(&mut h, Action::Fillet);
    assert!(h.state().value_edit.is_some(), "Fillet asks for a radius");
    h.state_mut().value_edit.as_mut().unwrap().text = "6 mm".into();
    assert!(h.state_mut().commit_value());
    h.step();
    let sk = sketch(&h);
    let arc = sk.entities.iter().find(|(_, e)| matches!(e.geom, fr_core::Geom::Arc { .. })).map(|(id, _)| *id).expect("a fillet arc");
    assert!((sk.curve(arc).unwrap().1 - 6.0).abs() < 1e-6);

    // Offset the whole outline inward, then trim and mirror.
    run(&mut h, Action::SelectAll);
    let lines: Vec<u32> = sk.entities.keys().copied().filter(|e| sk.line(*e).is_some()).collect();
    h.state_mut().sel = lines.clone();
    run(&mut h, Action::Offset);
    h.state_mut().value_edit.as_mut().unwrap().text = "3 mm".into();
    assert!(h.state_mut().commit_value(), "{:?}", h.state().value_edit.as_ref().and_then(|v| v.error.clone()));
    h.step();
    assert_eq!(sketch(&h).entities.len(), sk.entities.len() + lines.len(), "one offset line per line");

    // A line across the rectangle, trimmed where it sticks out on the left.
    run(&mut h, Action::Tool(Tool::Line));
    let (a, b) = (on(&h, -5.0, 15.0), on(&h, 20.0, 15.0));
    click(&mut h, a);
    click(&mut h, b);
    run(&mut h, Action::Cancel);
    let before = sketch(&h);
    run(&mut h, Action::Tool(Tool::Trim));
    let stub = on(&h, -3.0, 15.0);
    click(&mut h, stub);
    let after = sketch(&h);
    let cross: Vec<_> = after.entities.keys().filter_map(|e| after.line(*e)).filter(|l| (l.0.y - 15.0).abs() < 1e-6 && (l.1.y - 15.0).abs() < 1e-6).collect();
    assert_eq!(cross.len(), 1);
    // The nearest crossing is the offset outline, 3 outside the rectangle.
    assert!((cross[0].0.x.min(cross[0].1.x) - 2.0).abs() < 1e-3, "the stub is cut back to the offset outline: {cross:?}");
    assert_eq!(after.entities.len(), before.entities.len());
    save(&mut h, "sketch-tools.png");
}

#[test]
fn patterns_extents_and_section() {
    let mut h = harness();
    plate(&mut h);
    let vol = |h: &H| h.state().session.built.bodies.iter().map(|b| b.mesh.volume()).sum::<f64>();

    // A hole sketched above the plate and cut through all of it from the dialog.
    for cmd in [json!({"op": "create_sketch", "plane": "XY", "offset": 20}), json!({"op": "add_geometry", "items": [{"type": "circle", "center": [6, 10], "radius": 2}]})] {
        h.state_mut().execute(&cmd).unwrap();
    }
    h.run_steps(2);
    run(&mut h, Action::Extrude);
    let Dialog::Feature(mut f) = h.state().dialog.clone() else { panic!() };
    assert_eq!(f.profiles.len(), 1, "the only visible profile is preselected");
    f.text = "-1 mm".into();
    f.through_all = true;
    f.op = fr_core::Op::Cut;
    h.state_mut().dialog = Dialog::Feature(f);
    h.run_steps(2);
    h.state_mut().apply_dialog();
    let one = 8000.0 - vol(&h);
    assert!(one > 120.0 && one < 127.0, "a 2 mm hole through 10 mm removes about 125, got {one}");

    // Repeat the hole four times along X from the Pattern dialog.
    run(&mut h, Action::Pattern);
    let Dialog::Pattern(mut p) = h.state().dialog.clone() else { panic!("Pattern should open on the last feature") };
    (p.kind, p.axis, p.count, p.text) = (1, 0, 4, "9 mm".into());
    h.state_mut().dialog = Dialog::Pattern(p);
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "{:?}", h.state().preview.as_ref().map(|p| &p.2));
    h.state_mut().apply_dialog();
    assert!((8000.0 - vol(&h) - one * 4.0).abs() < 1.0, "{}", 8000.0 - vol(&h));
    h.state_mut().fit();
    let whole = save(&mut h, "pattern.png");

    // Section through the holes: the near half disappears and the cut face is hatched.
    h.state_mut().section = crate::app::Section { on: true, axis: 1, offset: 10.0, flip: false };
    let cut = save(&mut h, "section.png");
    let px = |img: &image::RgbaImage, p: Pos2| img.get_pixel(p.x as u32, p.y as u32).0;
    let near_edge = crate::view::to_screen(h.state(), DVec3::new(20.0, 0.0, 5.0));
    // Between two holes, where the plane passes through solid.
    let cut_face = crate::view::to_screen(h.state(), DVec3::new(19.5, 10.0, 5.0));
    assert_ne!(px(&whole, near_edge), px(&cut, near_edge), "the front of the plate is cut away");
    let c = px(&cut, cut_face);
    assert!(c[0] > c[2] + 40, "the cut face is drawn in the hatch colour, got {c:?}");
}

#[test]
fn extrude_arrow_drags_the_distance() {
    let mut h = harness();
    for cmd in [json!({"op": "create_sketch", "plane": "XY"}), json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [40, 20]}]})] {
        h.state_mut().execute(&cmd).unwrap();
    }
    h.state_mut().fit();
    // Leave room above and below the flat sketch for the arrow.
    h.state_mut().cam.scale /= 4.0;
    h.run_steps(2);
    run(&mut h, Action::Extrude);
    let dist = |h: &H| match &h.state().dialog {
        Dialog::Feature(f) => h.state().doc().eval(&f.text, fr_core::Kind::Length).unwrap(),
        _ => panic!("the extrude dialog should be open"),
    };
    assert_eq!(dist(&h), 10.0);

    // The arrow stands on the middle of the profile and reaches the current distance.
    let (cam, scale) = (h.state().cam, h.state().cam.scale as f32);
    let tip = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 10.0));
    let up = (crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 11.0)) - tip).normalized();
    let per_mm = (crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 11.0)) - tip).length();
    drag_with(&mut h, PointerButton::Primary, tip, up * per_mm * 15.0);
    assert_eq!(h.state().cam, cam, "pulling the arrow must not turn the view");
    assert!((dist(&h) - 25.0).abs() < 0.6, "pulled 15 further along the arrow, got {}", dist(&h));
    h.run_steps(2);
    let shown = h.state().shown().bodies[0].mesh.bbox().unwrap().1.z;
    assert!((shown - dist(&h)).abs() < 1e-6, "the preview follows the arrow");
    save(&mut h, "extrude-arrow.png");

    // Pull it back through the sketch plane: the extrude goes the other way.
    let tip = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, dist(&h)));
    drag_with(&mut h, PointerButton::Primary, tip, -up * per_mm * 40.0);
    assert!(dist(&h) < -10.0, "{}", dist(&h));
    let _ = scale;

    // Dragging anywhere else still orbits.
    let corner = h.state().vp.left_bottom() + egui::vec2(60.0, -120.0);
    drag_with(&mut h, PointerButton::Primary, corner, egui::vec2(50.0, 20.0));
    assert!(h.state().cam.yaw != cam.yaw);

    // On a face, the arrow stands on the face and pushing it in cuts.
    h.state_mut().apply_dialog();
    run(&mut h, Action::View("iso"));
    h.state_mut().fit();
    h.run_steps(2);
    let lo = h.state().session.built.bodies[0].mesh.bbox().unwrap().0.z;
    let face = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 0.0));
    click(&mut h, face);
    assert!(h.state().sel_face.is_some());
    run(&mut h, Action::Extrude);
    let Dialog::Feature(f) = h.state().dialog.clone() else { panic!() };
    assert!(f.face.is_some(), "the selected face is extruded");
    assert!(lo < -10.0);
}

#[test]
fn fillet_shell_and_timeline_rollback() {
    let mut h = harness();
    plate(&mut h);
    let exact = |h: &H| h.state().session.built.bodies[0].solids.iter().map(|s| s.volume()).sum::<f64>();
    assert!(h.state().session.built.bodies[0].is_exact(), "bodies made from sketches are exact solids");
    assert!((exact(&h) - 8000.0).abs() < 1e-9);

    // Fillet: click two edges of the top face, type a radius.
    run(&mut h, Action::Blend(false));
    for at in [DVec3::new(20.0, 0.0, 10.0), DVec3::new(40.0, 10.0, 10.0)] {
        let p = crate::view::to_screen(h.state(), at);
        click(&mut h, p);
    }
    let Dialog::Blend(mut b) = h.state().dialog.clone() else { panic!("the fillet dialog should be open") };
    assert_eq!(b.edges.len(), 2, "each click on an edge picks it");
    b.text = "3 mm".into();
    h.state_mut().dialog = Dialog::Blend(b);
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "{:?}", h.state().preview.as_ref().map(|p| &p.2));
    save(&mut h, "fillet-dialog.png");
    h.state_mut().apply_dialog();
    let q = 9.0 * (1.0 - std::f64::consts::PI / 4.0);
    let lost = 8000.0 - exact(&h);
    assert!(lost > q * 55.0 && lost < q * 60.0, "two r=3 fillets, 40 and 20 long, meeting at a corner: {lost}");

    // A radius that cannot fit is refused in the preview, and OK stays off.
    run(&mut h, Action::Blend(false));
    let p = crate::view::to_screen(h.state(), DVec3::new(0.0, 10.0, 10.0));
    click(&mut h, p);
    let Dialog::Blend(mut b) = h.state().dialog.clone() else { panic!() };
    b.text = "30 mm".into();
    h.state_mut().dialog = Dialog::Blend(b);
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_some()), "an oversized fillet should show an error");
    run(&mut h, Action::Cancel);

    // Shell: the bottom face is clicked from below.
    run(&mut h, Action::View("bottom"));
    h.state_mut().fit();
    h.run_steps(2);
    run(&mut h, Action::Shell);
    let p = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 0.0));
    click(&mut h, p);
    let Dialog::Shell(sh) = h.state().dialog.clone() else { panic!("the shell dialog should be open") };
    assert_eq!(sh.faces.len(), 1);
    h.run_steps(2);
    let before = exact(&h);
    h.state_mut().apply_dialog();
    assert!(exact(&h) < before * 0.6, "hollowed to a 2 mm wall: {} of {before}", exact(&h));
    assert!(h.state().session.built.errors.is_empty(), "{:?}", h.state().session.built.errors);
    run(&mut h, Action::View("iso"));
    h.state_mut().fit();
    save(&mut h, "shelled.png");

    // Roll the timeline back to before the fillet, then forward again.
    assert_eq!(h.state().doc().features.len(), 4);
    h.state_mut().roll_to(2);
    h.run_steps(2);
    assert!((exact(&h) - 8000.0).abs() < 1e-9, "rolled back, the plain plate is shown");
    save(&mut h, "rolled-back.png");
    // Dragging the marker along the chips moves it.
    let chips = h.state().chips.clone();
    assert_eq!(chips.len(), 4);
    let from = egui::pos2((chips[1].right() + chips[2].left()) / 2.0, chips[1].center().y);
    let to = egui::pos2(chips[2].right() + 4.0, chips[1].center().y);
    drag(&mut h, &[from, from + (to - from) * 0.5, to]);
    assert_eq!(h.state().doc().active(), 3, "dragged past the fillet's chip");
    assert!(exact(&h) < 8000.0 && exact(&h) > 7000.0, "the fillet is back, the shell is not");
    h.state_mut().roll_to(4);
    assert!(exact(&h) < 4500.0);
}

/// The marker centre at a given boundary, after layout has caught up.
fn timeline_at(h: &H, count: usize) -> Pos2 {
    let chips = &h.state().chips;
    if count == 0 { egui::pos2(chips[0].left() - 12.0, chips[0].center().y) }
    else if count == chips.len() { egui::pos2(chips[count - 1].right() + 12.0, chips[0].center().y) }
    else { egui::pos2((chips[count - 1].right() + chips[count].left()) / 2.0, chips[0].center().y) }
}

#[test]
fn timeline_drag_builds_once_on_drop_and_waits_for_the_gpu() {
    let mut h = harness();
    plate(&mut h);
    for _ in 0..2 {
        h.state_mut().execute(&json!({"op": "transform", "body": 2, "translate": [1, 0, 0]})).unwrap();
    }
    h.run_steps(2);
    h.render().unwrap();
    let (rev, edits, dirty) = (h.state().session.rev, h.state().session.edits, h.state().session.dirty);
    let from = timeline_at(&h, 4);
    h.hover_at(from);
    h.step();
    button(&h, from, true);
    h.step();
    for count in [0, 2, 1, 3, 2] {
        let to = timeline_at(&h, count);
        h.hover_at(to);
        h.step();
        assert_eq!(h.state().timeline.preview, Some((count, rev)));
        assert_eq!(h.state().doc().active(), 4, "holding the marker never changes the model");
        assert_eq!((h.state().session.rev, h.state().session.edits, h.state().session.dirty), (rev, edits, dirty));
        h.render().unwrap();
    }
    let to = timeline_at(&h, 2);
    button(&h, to, false);
    h.step();
    assert_eq!(h.state().doc().active(), 2);
    assert_eq!(h.state().session.rev, rev + 1, "exactly one rebuild for the whole gesture");
    assert!(h.state().timeline.busy(), "cannot re-grab before rendering the new model");
    h.step();
    assert!(h.state().timeline.busy(), "UI frames alone do not acknowledge a GPU render");

    // A premature press must not become a drag as soon as the GPU catches up.
    let from = timeline_at(&h, 2);
    h.hover_at(from);
    button(&h, from, true);
    h.step();
    h.render().unwrap();
    h.step(); // registers the fence after the new frame has been submitted
    h.render().unwrap(); // polls the GPU queue
    h.run_steps(2);
    assert!(h.state().timeline.busy(), "keep a press made while busy locked until release");
    assert!(h.state().timeline.preview.is_none());
    let to = timeline_at(&h, 0);
    h.hover_at(to);
    h.step();
    button(&h, to, false);
    h.step();
    assert!(!h.state().timeline.busy());
    assert_eq!(h.state().doc().active(), 2);
    assert_eq!(h.state().session.rev, rev + 1);

    // A fresh drag works; rolling to an empty scene must not wait for a body draw.
    let from = timeline_at(&h, 2);
    let to = timeline_at(&h, 0);
    drag(&mut h, &[from, to]);
    assert_eq!(h.state().doc().active(), 0);
    h.step();
    assert!(!h.state().timeline.busy());
    run(&mut h, Action::Undo);
    assert_eq!(h.state().doc().active(), 2);
    run(&mut h, Action::Undo);
    assert_eq!(h.state().doc().active(), 4, "intermediate drag positions created no undo steps");
}

#[test]
fn timeline_cancel_noop_and_document_replacement_leave_no_pending_drag() {
    let mut h = state_harness();
    plate(&mut h);
    let rev = h.state().session.rev;
    let from = timeline_at(&h, 2);
    let to = timeline_at(&h, 0);
    drag(&mut h, &[from, to, from]);
    assert_eq!(h.state().session.rev, rev, "dropping at the original boundary is a no-op");
    assert!(!h.state().timeline.busy());

    h.hover_at(from);
    h.step();
    button(&h, from, true);
    h.step();
    h.hover_at(to);
    h.step();
    assert!(h.state().timeline.preview.is_some());
    key(&mut h, Key::Escape);
    button(&h, to, false);
    h.step();
    assert_eq!(h.state().session.rev, rev);
    assert_eq!(h.state().doc().active(), 2);
    assert!(h.state().timeline.preview.is_none());

    drag(&mut h, &[from, to]);
    assert_eq!(h.state().doc().active(), 0);
    h.step();
    assert!(!h.state().timeline.busy(), "software/empty view unlocks too");
    h.state_mut().timeline.begin_update();
    h.state_mut().execute(&json!({"op": "new", "discard_unsaved": true})).unwrap();
    assert!(!h.state().timeline.busy(), "replacing the document discards the old render fence");
}

#[test]
fn corners_are_not_clipped_when_the_view_is_centred_off_the_body() {
    let mut h = harness();
    plate(&mut h);
    // Centre the view on one bottom corner and look straight down the long diagonal, so the
    // opposite top corner is the point of the body nearest the eye.
    let (near, far) = (DVec3::new(0.0, 20.0, 10.0), DVec3::new(40.0, 0.0, 0.0));
    let toward = (near - far).normalize();
    let cam = &mut h.state_mut().cam;
    (cam.target, cam.yaw, cam.pitch, cam.scale) = (far, toward.y.atan2(toward.x), toward.z.asin(), 12.0);
    let img = save(&mut h, "corner-depth.png");
    // Just inside each of the three faces that meet at that corner, the outside of that face
    // must be what is drawn. With the corner clipped away, the inside of the body shows instead.
    let cam = h.state().cam;
    for (inward, normal) in [(DVec3::new(2.0, -2.0, 0.0), DVec3::Z), (DVec3::new(2.0, 0.0, -2.0), DVec3::Y), (DVec3::new(0.0, -2.0, -2.0), -DVec3::X)] {
        let p = crate::view::to_screen(h.state(), near + inward);
        let px = img.get_pixel(p.x as u32, p.y as u32).0;
        let want = fr_core::render::BODY.map(|c| (c as f64 * fr_core::render::shade(normal, &cam)) as i32);
        assert!((0..3).all(|i| (px[i] as i32 - want[i]).abs() <= 3), "the face toward {normal} should be drawn at the corner nearest the eye: got {px:?}, want {want:?}");
    }
}

#[test]
fn holes_and_threads_from_the_catalog() {
    let mut h = harness();
    plate(&mut h);
    let pi = std::f64::consts::PI;
    let exact = |h: &H| h.state().session.built.bodies.iter().flat_map(|b| b.solids.iter()).map(|x| x.volume()).sum::<f64>();

    // Hole: each click on the top face puts one there, and clicking it again takes it away.
    run(&mut h, Action::Hole);
    for at in [DVec3::new(10.0, 10.0, 10.0), DVec3::new(30.0, 10.0, 10.0), DVec3::new(20.0, 5.0, 10.0)] {
        let p = crate::view::to_screen(h.state(), at);
        click(&mut h, p);
    }
    let p = crate::view::to_screen(h.state(), DVec3::new(20.0, 5.0, 10.0));
    click(&mut h, p);
    let Dialog::Hole(mut d) = h.state().dialog.clone() else { panic!("the hole dialog should be open") };
    assert_eq!(d.at.len(), 2, "three placed, one taken away again");
    assert!(d.dir.distance(-DVec3::Z) < 1e-9, "the drill goes into the face that was clicked");
    assert!(d.at.iter().all(|p| (p.z - 10.0).abs() < 1e-6) && (d.at[0].x - 10.0).abs() < 0.2);
    // As it opens: a normal M3 clearance hole, straight through.
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "{:?}", h.state().preview.as_ref().map(|p| &p.2));
    save(&mut h, "hole-dialog.png");
    // Countersunk for a flat head.
    d.shape = fr_core::doc::HoleShape::Countersink;
    h.state_mut().dialog = Dialog::Hole(d);
    h.run_steps(2);
    h.state_mut().apply_dialog();
    let cone = |a: f64, b: f64| pi * (a - b) / 3.0 * (a * a + a * b + b * b) - pi * b * b * (a - b);
    let lost = 8000.0 - exact(&h);
    assert!((lost - 2.0 * (pi * 1.7 * 1.7 * 10.0 + cone(3.15, 1.7))).abs() < 1e-6, "two countersunk 3.4 mm holes: {lost}");

    // A modeled M5 tapped hole, blind: the thread is there to see and the body is still exact.
    run(&mut h, Action::Hole);
    let p = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 10.0));
    click(&mut h, p);
    let Dialog::Hole(mut d) = h.state().dialog.clone() else { panic!() };
    (d.fit, d.thread, d.modeled, d.through, d.depth) = (fr_core::doc::HoleFit::Tapped, "M5x0.8".into(), true, false, "6 mm".into());
    h.state_mut().dialog = Dialog::Hole(d);
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "{:?}", h.state().preview.as_ref().map(|p| &p.2));
    save(&mut h, "tapped-hole-dialog.png");
    h.state_mut().apply_dialog();
    let body = &h.state().session.built.bodies[0];
    assert_eq!((body.threads.len(), body.is_exact(), body.mesh.open_edges()), (1, true, 0));
    let (lo, hi) = body.threads[0].bbox().unwrap();
    assert!((lo.z - 4.0).abs() < 1e-6 && (hi.z - 10.0).abs() < 1e-6, "the thread runs the 6 mm of the hole: {lo} {hi}");

    // A thread that has no size to go by says so in the dialog instead of applying.
    run(&mut h, Action::Hole);
    let p = crate::view::to_screen(h.state(), DVec3::new(35.0, 4.0, 10.0));
    click(&mut h, p);
    let Dialog::Hole(mut d) = h.state().dialog.clone() else { panic!() };
    (d.fit, d.diameter, d.shape, d.head_diameter) = (fr_core::doc::HoleFit::Plain, "4 mm".into(), fr_core::doc::HoleShape::Counterbore, "3 mm".into());
    h.state_mut().dialog = Dialog::Hole(d);
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.as_ref().is_some_and(|e| e.contains("wider"))), "{:?}", h.state().preview.as_ref().map(|p| &p.2));
    run(&mut h, Action::Cancel);

    // Thread: a 6 mm rod standing on the plate, clicked on its side, is offered M6.
    for cmd in [json!({"op": "create_sketch", "plane": "XY", "offset": 10}), json!({"op": "add_geometry", "items": [{"type": "circle", "center": [5, 15], "radius": 3}]}), json!({"op": "extrude", "distance": 12, "operation": "join"})] {
        h.state_mut().execute(&cmd).unwrap();
    }
    h.state_mut().fit();
    h.run_steps(2);
    run(&mut h, Action::Thread);
    let side = DVec3::new(5.0, 15.0, 16.0) + DVec3::new(1.0, -1.0, 0.0).normalize() * 3.0;
    let p = crate::view::to_screen(h.state(), side);
    click(&mut h, p);
    let Dialog::Thread(mut t) = h.state().dialog.clone() else { panic!("the thread dialog should be open") };
    assert_eq!((t.thread.as_str(), t.found.map(|f| ((f.0 * 1e6).round() / 1e6, f.1))), ("M6x1", Some((6.0, false))));
    (t.full, t.offset, t.length) = (false, "0 mm".into(), "8 mm".into());
    h.state_mut().dialog = Dialog::Thread(t);
    h.run_steps(2);
    assert!(h.state().preview.as_ref().is_some_and(|p| p.2.is_none()), "{:?}", h.state().preview.as_ref().map(|p| &p.2));
    h.state_mut().apply_dialog();
    let body = &h.state().session.built.bodies[0];
    assert_eq!((h.state().session.built.bodies.len(), body.threads.len(), body.mesh.open_edges()), (1, 2, 0));
    let (lo, hi) = body.threads[1].bbox().unwrap();
    assert!((lo.z - 14.0).abs() < 1e-6 && (hi.z - 22.0).abs() < 1e-6, "8 mm of thread from the rod's tip: {lo} {hi}");
    assert!(h.state().session.built.errors.is_empty(), "{:?}", h.state().session.built.errors);
    run(&mut h, Action::View("iso"));
    h.state_mut().fit();
    save(&mut h, "holes-and-threads.png");

    // It all comes back from a saved file.
    let path = out_dir().join("holes.ferr");
    h.state_mut().session.save(&path).unwrap();
    let again = fr_core::Session::open(&path).unwrap();
    assert!(again.built.errors.is_empty(), "{:?}", again.built.errors);
    assert_eq!((again.built.bodies[0].threads.len(), again.built.bodies[0].mesh.tris.len()), (2, h.state().session.built.bodies[0].mesh.tris.len()));
}

#[test]
fn unsaved_work_comes_back_after_a_crash() {
    let dir = std::env::temp_dir().join(format!("ferrender-uitest-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let boxed = json!({"op": "batch", "commands": [
        {"op": "create_sketch", "plane": "XY"},
        {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "rect", "from": [0, 0], "to": [20, 10]}]},
        {"op": "extrude", "sketch": "$last_sketch", "distance": 5}
    ]});

    // An app with unsaved work that goes away without closing.
    let mut h = harness();
    h.state_mut().recovery = Some(Recovery::start(dir.clone()));
    h.state_mut().execute(&boxed).unwrap();
    h.run_steps(3);
    assert!(h.state().session.dirty);
    h.state().recovery.as_ref().unwrap().wait();
    assert!(h.state().recovery.as_ref().unwrap().found().is_empty(), "the copy is not offered while its app runs");
    // Dropped without closing: the lock goes, the copy stays, as after a crash.
    drop(h.state_mut().recovery.take());
    drop(h);

    // The next app finds it and offers it.
    let mut h = harness();
    let r = Recovery::start(dir.clone());
    h.state_mut().recover = r.found();
    h.state_mut().recovery = Some(r);
    assert_eq!(h.state().recover.len(), 1);
    h.run_steps(3);
    save(&mut h, "recover.png");
    h.get_by_label("Recover").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.session.built.bodies.len(), 1, "the body is back");
    assert!(app.session.dirty && app.session.path.is_none(), "recovered work is still unsaved");
    assert!(app.recover.is_empty());
    app.recovery.as_ref().unwrap().wait();
    assert!(app.recovery.as_ref().unwrap().found().is_empty(), "the old copy is gone");
    assert_eq!(std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "ferr-recovery")).count(), 1, "this app now holds the only copy");

    // Closing normally leaves nothing behind.
    h.state_mut().recovery.as_mut().unwrap().close();
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_point_tool_places_a_point() {
    let mut h = harness();
    h.state_mut().create_sketch(Plane::XY);
    h.run_steps(2);
    let Mode::Sketch(sid) = h.state().mode else { panic!("should be editing the new sketch") };
    let points = |h: &H| h.state().session.doc.sketch(sid).unwrap().points.len();
    // A point takes one click; placing it must not look for a second.
    run(&mut h, Action::Tool(Tool::Point));
    let (before, spot) = (points(&h), at(&h, 12.0, 7.0));
    click(&mut h, spot);
    assert_eq!(points(&h), before + 1);
    assert!(h.state().clicks.is_empty());
}

#[test]
fn typed_sizes_while_drawing_and_measure() {
    let mut h = harness();
    h.state_mut().create_sketch(Plane::XY);
    h.run_steps(2);
    let Mode::Sketch(sid) = h.state().mode else { panic!("should be editing the new sketch") };
    let sketch = |h: &H| h.state().session.doc.sketch(sid).unwrap().clone();
    let typed = |h: &mut H, text: &str| {
        h.event(Event::Text(text.into()));
        h.run_steps(2);
    };
    let sizes = |h: &H, kind: CKind| {
        let mut v: Vec<f64> = sketch(h).constraints.values().filter(|c| c.kind == kind).map(|c| c.value.as_ref().unwrap().v).collect();
        v.sort_by(f64::total_cmp);
        v
    };

    // Rectangle: first corner, type the width, Tab, type the height, Enter. The pointer only picks the side.
    run(&mut h, Action::Tool(Tool::Rect));
    let (a, b) = (at(&h, 0.0, 0.0), at(&h, 17.0, 9.0));
    click(&mut h, a);
    h.hover_at(b);
    h.run_steps(3);
    assert_eq!(h.state().typed.as_ref().map(|t| (t.fields.len(), t.active)), Some((2, 0)), "the size boxes open with the first corner");
    typed(&mut h, "30");
    key(&mut h, Key::Tab);
    h.run_steps(2);
    assert_eq!(h.state().typed.as_ref().map(|t| (t.fields[0].as_str(), t.active)), Some(("30", 1)), "Tab moves to the height");
    typed(&mut h, "t = 0.5 in");
    save(&mut h, "typed-rect.png");
    key(&mut h, Key::Enter);
    h.run_steps(2);
    let sk = sketch(&h);
    assert_eq!(sk.entities.len(), 4, "Enter places the rectangle");
    assert_eq!(sizes(&h, CKind::Distance), vec![12.7, 30.0], "both typed sizes became dimensions");
    let (lo, hi) = sk.bbox().unwrap();
    assert!((hi.x - lo.x - 30.0).abs() < 1e-6 && (hi.y - lo.y - 12.7).abs() < 1e-6 && lo.distance(DVec2::ZERO) < 1e-6, "{lo} {hi}");
    assert_eq!(h.state().doc().params[0].name, "t", "`t = 0.5 in` in a box defines the parameter");
    assert_eq!(h.state().report.dof, 0, "typed sizes on a rectangle from the origin leave nothing free");
    assert!(h.state().typed.is_none() && h.state().clicks.is_empty());

    // One size typed, the other left to the click.
    let (a, b) = (at(&h, 40.0, 0.0), at(&h, 47.0, -21.0));
    click(&mut h, a);
    h.hover_at(b);
    h.run_steps(3);
    typed(&mut h, "10");
    click(&mut h, b);
    let sk = sketch(&h);
    let far = sk.points.values().find(|p| (p.y + 21.0).abs() < 0.5 && p.x > 45.0).copied().expect("the corner the click chose");
    assert!((far.x - 50.0).abs() < 1e-6, "the width held at 10 while the click set the height: {far}");
    assert_eq!(sizes(&h, CKind::Distance), vec![10.0, 12.7, 30.0]);

    // Circle: a typed diameter. Line: a typed length along the way the pointer points.
    run(&mut h, Action::Tool(Tool::Circle));
    let (a, b) = (at(&h, 15.0, 30.0), at(&h, 16.0, 31.0));
    click(&mut h, a);
    h.hover_at(b);
    h.run_steps(3);
    typed(&mut h, "8");
    key(&mut h, Key::Enter);
    h.run_steps(2);
    assert_eq!(sizes(&h, CKind::Diameter), vec![8.0]);
    assert!(sketch(&h).entities.values().any(|e| matches!(e.geom, Geom::Circle { r, .. } if (r - 4.0).abs() < 1e-9)));
    run(&mut h, Action::Tool(Tool::Line));
    let (a, b) = (at(&h, -20.0, 20.0), at(&h, -17.0, 24.0));
    click(&mut h, a);
    h.hover_at(b);
    h.run_steps(3);
    typed(&mut h, "25");
    key(&mut h, Key::Enter);
    h.run_steps(2);
    let sk = sketch(&h);
    let end = sk.points.values().find(|p| p.distance(DVec2::new(-5.0, 40.0)) < 1e-6);
    assert!(end.is_some(), "25 long along the 3-4-5 direction the pointer gave: {:?}", sk.points.values().collect::<Vec<_>>());
    // The line carries on from there; Enter with nothing typed ends it, as before.
    assert_eq!(h.state().clicks.len(), 1);
    h.run_steps(2);
    key(&mut h, Key::Enter);
    h.run_steps(2);
    assert!(h.state().clicks.is_empty() && h.state().typed.is_none());
    // Something that is not a size is refused and the shape is not placed.
    run(&mut h, Action::Tool(Tool::Circle));
    let (a, b) = (at(&h, 60.0, 30.0), at(&h, 62.0, 30.0));
    click(&mut h, a);
    h.hover_at(b);
    h.run_steps(3);
    typed(&mut h, "big");
    let before = sketch(&h).entities.len();
    key(&mut h, Key::Enter);
    h.run_steps(2);
    assert_eq!((sketch(&h).entities.len(), h.state().clicks.len()), (before, 1));
    key(&mut h, Key::Escape);
    h.run_steps(2);
    assert!(h.state().clicks.is_empty());

    // Measure, on a 40 x 20 x 10 plate: face to face, edge to edge, corner to corner.
    let mut h = harness();
    plate(&mut h);
    run(&mut h, Action::Measure);
    let pick = |h: &mut H, p: DVec3| {
        let p = crate::view::to_screen(h.state(), p);
        click(h, p);
    };
    let result = |h: &H| match &h.state().dialog {
        Dialog::Measure(m) => (m.picks.len(), m.result),
        other => panic!("the measure dialog should be open, not {other:?}"),
    };
    pick(&mut h, DVec3::new(20.0, 10.0, 10.0));
    pick(&mut h, DVec3::new(40.0, 10.0, 5.0));
    let (n, r) = result(&h);
    let r = r.expect("two picks give a measurement");
    assert!(n == 2 && r.distance < 1e-9 && r.apart.is_none() && (r.angle.unwrap() - 90.0).abs() < 1e-9, "the top and an end meet at a right angle: {r:?}");
    save(&mut h, "measure.png");
    // A third pick starts again: two parallel edges of the top, 20 apart.
    pick(&mut h, DVec3::new(20.0, 0.0, 10.0));
    assert_eq!(result(&h).0, 1);
    pick(&mut h, DVec3::new(20.0, 20.0, 10.0));
    let r = result(&h).1.unwrap();
    assert!((r.apart.unwrap() - 20.0).abs() < 1e-9 && r.angle == Some(0.0), "{r:?}");
    // Opposite corners of the top.
    pick(&mut h, DVec3::new(0.0, 0.0, 10.0));
    pick(&mut h, DVec3::new(40.0, 20.0, 10.0));
    let r = result(&h).1.unwrap();
    assert!((r.distance - 2000f64.sqrt()).abs() < 1e-9 && (r.to - r.from).abs().distance(DVec3::new(40.0, 20.0, 0.0)) < 1e-9, "{r:?}");
    save(&mut h, "measure-corners.png");
    key(&mut h, Key::Escape);
    assert_eq!(h.state().dialog, Dialog::None);
}


/// These command-boundary regressions need no GPU or native window.
fn state_harness<'a>() -> H<'a> {
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_eframe(|cc| App::new(cc, None));
    h.state_mut().set_appearance(Appearance::Light);
    h.step();
    h
}

#[test]
fn appearance_follows_live_system_events_and_manual_overrides() {
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_eframe(|cc| App::new(cc, None));
    assert_eq!(h.state().config.appearance, Appearance::System, "new installations follow the OS");
    // kittest forces its own theme after App::new; restore the app's selection.
    h.state_mut().set_appearance(Appearance::System);
    h.input_mut().system_theme = None;
    h.run_steps(2);
    assert_eq!(h.ctx.theme(), egui::Theme::Light, "a desktop that cannot report its theme uses the Light fallback");

    for theme in [egui::Theme::Dark, egui::Theme::Light, egui::Theme::Dark] {
        h.input_mut().system_theme = Some(theme);
        h.run_steps(2);
        assert_eq!(h.ctx.theme() == egui::Theme::Dark, theme == egui::Theme::Dark, "System follows each OS change without restarting");
        assert_eq!(h.state().config.appearance, Appearance::System);
    }

    for (appearance, dark) in [(Appearance::Light, false), (Appearance::Dark, true)] {
        h.state_mut().set_appearance(appearance);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            h.input_mut().system_theme = Some(theme);
            h.run_steps(2);
            assert_eq!(h.ctx.theme() == egui::Theme::Dark, dark, "a manual appearance overrides OS events");
            assert_eq!(h.state().config.appearance, appearance);
        }
    }

    h.input_mut().system_theme = Some(egui::Theme::Light);
    h.step();
    h.state_mut().set_appearance(Appearance::System);
    h.run_steps(2);
    assert_eq!(h.ctx.theme(), egui::Theme::Light, "returning to System immediately uses the current OS appearance");
}

#[test]
fn appearance_menu_changes_theme_without_editing_or_rebuilding_the_design() {
    let mut h = state_harness();
    plate(&mut h);
    h.state_mut().session.dirty = false;
    let document = h.state().doc().clone();
    let session = &h.state().session;
    let original = (session.rev, session.edits, session.dirty, volume(&h), h.state().cam);

    for (label, appearance, dark) in [("Dark", Appearance::Dark, true), ("Light", Appearance::Light, false), ("System", Appearance::System, true)] {
        h.input_mut().system_theme = Some(egui::Theme::Dark);
        h.get_by_label("View").click();
        h.run_steps(2);
        h.get_by_label_contains("Appearance").click();
        h.run_steps(2);
        h.get_by_label(label).click();
        h.run_steps(3);
        assert_eq!(h.state().config.appearance, appearance);
        assert_eq!(h.ctx.theme() == egui::Theme::Dark, dark);
        assert_eq!(h.state().doc(), &document, "appearance is an app setting, not a document edit");
        let session = &h.state().session;
        assert_eq!((session.rev, session.edits, session.dirty, volume(&h), h.state().cam), original);
    }
}

#[test]
fn appearance_renders_sketch_dialog_and_model_in_both_themes() {
    let mut h = harness();
    h.state_mut().opts.grid = false;
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().execute(&json!({"op":"add_geometry", "items":[
        {"type":"rect", "from":[0,0], "to":[40,20]},
        {"type":"circle", "center":[10,10], "radius":4}
    ]})).unwrap();
    h.state_mut().fit();
    h.run_steps(3);
    run(&mut h, Action::PointCoordinates);
    for (name, appearance, dark) in [("light", Appearance::Light, false), ("dark", Appearance::Dark, true)] {
        h.state_mut().set_appearance(appearance);
        let img = save(&mut h, &format!("appearance-{name}-sketch-dialog.png"));
        h.get_by_label("Place Point");
        assert_eq!(h.ctx.theme() == egui::Theme::Dark, dark);
        let sample = h.state().vp.left_top() + egui::vec2(25.0, 100.0);
        let bg = img.get_pixel(sample.x as u32, sample.y as u32).0;
        let expected = crate::theme::Palette::from_ctx(&h.ctx).background.to_array();
        assert_eq!(bg, expected, "{name} viewport must use the current palette");
        let edge = at(&h, 20.0, 0.0);
        let visible = (-3..=3).any(|dy| (-3..=3).any(|dx| {
            let p = img.get_pixel((edge.x as i32 + dx) as u32, (edge.y as i32 + dy) as u32).0;
            (0..3).any(|i| p[i].abs_diff(bg[i]) > 60)
        }));
        assert!(visible, "sketch edges must contrast with the {name} background");
    }

    let mut h = harness();
    h.state_mut().opts.grid = false;
    plate(&mut h);
    for gpu in [true, false] {
        h.state_mut().gpu = gpu;
        for (name, appearance) in [("light", Appearance::Light), ("dark", Appearance::Dark)] {
            h.state_mut().set_appearance(appearance);
            let backend = if gpu { "gpu" } else { "cpu" };
            let img = save(&mut h, &format!("appearance-{name}-{backend}-model.png"));
            let point = crate::view::to_screen(h.state(), DVec3::new(20.0, 10.0, 10.0));
            let body = img.get_pixel(point.x as u32, point.y as u32).0;
            let bg = crate::theme::Palette::from_ctx(&h.ctx).background.to_array();
            assert!((0..3).any(|i| body[i].abs_diff(bg[i]) > 60), "the shaded solid must remain visible in {name}/{backend}: {body:?} vs {bg:?}");
            let sample = h.state().vp.left_top() + egui::vec2(25.0, 100.0);
            assert_eq!(img.get_pixel(sample.x as u32, sample.y as u32).0, bg, "{backend} must leave the current viewport background visible");
        }
    }
}

#[test]
fn editing_extrusions_preserves_taper_and_through_all() {
    let mut h = state_harness();
    let app = h.state_mut();
    for cmd in [
        json!({"op":"create_sketch", "plane":"XY"}),
        json!({"op":"add_geometry", "items":[{"type":"rect","from":[0,0],"to":[20,10]}]}),
        json!({"op":"extrude","distance":10,"taper":5}),
    ] {
        app.execute(&cmd).unwrap();
    }
    let original = app.doc().features.last().unwrap().clone();
    let volume = app.session.built.bodies[0].mesh.volume();
    app.edit_feature(original.id);
    app.apply_dialog();
    assert_eq!(app.doc().features.last().unwrap(), &original);
    assert!((app.session.built.bodies[0].mesh.volume() - volume).abs() < 1e-8);

    app.execute(&json!({"op":"new", "discard_unsaved":true})).unwrap();
    for cmd in [
        json!({"op":"create_sketch", "plane":"XY"}),
        json!({"op":"add_geometry", "items":[{"type":"rect","from":[0,0],"to":[40,20]}]}),
        json!({"op":"extrude","distance":10}),
        json!({"op":"create_sketch","plane":"XY","offset":10}),
        json!({"op":"add_geometry","items":[{"type":"rect","from":[25,5],"to":[35,15]}]}),
        json!({"op":"extrude","distance":-1,"extent":"all","operation":"cut"}),
    ] {
        app.execute(&cmd).unwrap();
    }
    let original = app.doc().features.last().unwrap().clone();
    let volume = app.session.built.bodies[0].mesh.volume();
    app.edit_feature(original.id);
    app.apply_dialog();
    assert_eq!(app.doc().features.last().unwrap(), &original);
    assert!((app.session.built.bodies[0].mesh.volume() - volume).abs() < 1e-8);
}

#[test]
fn automation_cannot_discard_unsaved_work_inside_nested_batches() {
    let mut h = state_harness();
    let app = h.state_mut();
    app.execute(&json!({"op":"set_parameter", "name":"keep", "expr":"5 mm"})).unwrap();
    let original = app.doc().clone();
    for cmd in [
        json!({"op":"new"}),
        json!({"op":"open", "path":"not-opened.ferr"}),
        json!({"op":"batch", "commands":[{"op":"batch", "commands":[{"op":"new"}]}]}),
    ] {
        assert!(app.execute(&cmd).unwrap_err().contains("unsaved changes"));
        assert_eq!(app.doc(), &original);
    }
    app.dialog = Dialog::PickPlane;
    app.sel_feature = Some(99);
    app.execute(&json!({"op":"batch", "commands":[{"op":"batch", "commands":[{"op":"new", "discard_unsaved":true}]}]})).unwrap();
    assert!(app.doc().params.is_empty());
    assert_eq!(app.dialog, Dialog::None);
    assert!(app.sel_feature.is_none() && app.fit_pending);
    assert!(!app.session.dirty);

    // A batch starting clean becomes protected as soon as one child changes the design.
    let error = app.execute(&json!({"op":"batch", "commands":[
        {"op":"set_parameter", "name":"new_work", "expr":"10 mm"},
        {"op":"batch", "commands":[{"op":"new"}]}
    ]})).unwrap_err();
    assert!(error.contains("unsaved changes"));
    assert_eq!(app.doc().params[0].name, "new_work");
    assert!(app.session.dirty);
}

#[test]
fn automation_bounds_nested_batches_before_applying_commands() {
    let mut h = state_harness();
    let mut nested = json!({"op":"new", "discard_unsaved":true});
    for _ in 0..18 {
        nested = json!({"op":"batch", "commands":[nested]});
    }
    let app = h.state_mut();
    let before = app.doc().clone();
    assert!(app.execute(&nested).unwrap_err().contains("nested batches"));
    assert_eq!(app.doc(), &before);
}


#[test]
fn malformed_and_oversized_clipboard_geometry_is_refused() {
    let mut h = state_harness();
    h.state_mut().create_sketch(Plane::XY);
    h.run_steps(3);
    let original = h.state().doc().clone();
    let malformed = json!({
        "ferrender_clip": 1,
        "points": [[1, [0, 0]]],
        "entities": [[2, {"type":"line", "a":1, "b":999}]],
        "constraints": []
    });
    h.event(Event::Paste(malformed.to_string()));
    h.run_steps(2);
    assert_eq!(h.state().doc(), &original);
    assert!(h.state().toast.as_ref().unwrap().0.contains("Could not paste"));

    h.event(Event::Paste("x".repeat(4 * 1024 * 1024 + 1)));
    h.run_steps(2);
    assert_eq!(h.state().doc(), &original);
    assert!(h.state().toast.as_ref().unwrap().0.contains("4 MiB"));
}

#[test]
fn failed_file_open_and_import_remain_visible_and_keep_the_design() {
    let mut h = state_harness();
    h.state_mut().execute(&json!({"op":"set_parameter", "name":"keep", "expr":"5 mm"})).unwrap();
    let original = h.state().doc().clone();
    let revision = h.state().session.rev;
    let dir = std::env::temp_dir().join(format!("ferrender-file-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let broken = dir.join("broken.ferr");
    std::fs::write(&broken, "not a Ferrender design").unwrap();
    // Native file pickers block inside a frame; the next frame can arrive long
    // after the notification deadline calculated from that frame's old time.
    h.state_mut().now = 0.0;
    h.state_mut().open_path(&broken);
    h.input_mut().time = Some(60.0);
    h.run_steps(3);
    assert_eq!(h.state().doc(), &original);
    assert_eq!(h.state().session.rev, revision);
    assert!(h.state().session.dirty);
    assert_eq!(h.state().file_error.as_ref().unwrap().path, broken);
    h.get_by_label("Could not open design");
    h.get_by_label("Your current design has been kept.");
    h.get_by_label("Dismiss").click();
    h.run_steps(2);
    assert!(h.state().file_error.is_none());

    let broken_stl = dir.join("broken.stl");
    std::fs::write(&broken_stl, "not an STL mesh").unwrap();
    h.state_mut().import_stl(&broken_stl, fr_core::Unit::Mm);
    h.input_mut().time = Some(180.0);
    h.run_steps(3);
    assert_eq!(h.state().doc(), &original);
    assert_eq!(h.state().session.rev, revision);
    h.get_by_label("Could not import STL");
    assert!(h.state().file_error.as_ref().is_some_and(|e| !e.message.is_empty()));
    key(&mut h, Key::Escape);
    assert!(h.state().file_error.is_none());
    // An existing save path avoids the native picker while exercising its real
    // failure handler. A directory cannot be replaced with a design file.
    h.state_mut().session.path = Some(dir.clone());
    run(&mut h, Action::Save);
    h.input_mut().time = Some(240.0);
    h.run_steps(3);
    h.get_by_label("Could not save design");
    assert!(h.state().session.dirty);
    assert_eq!(h.state().doc(), &original);
    key(&mut h, Key::Escape);
    h.state_mut().export_stl_path(&dir, fr_core::Unit::Mm);
    h.input_mut().time = Some(300.0);
    h.run_steps(3);
    h.get_by_label("Could not export STL");
    assert_eq!(h.state().doc(), &original);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn text_dialog_preview_planes_and_timeline_edit_round_trip() {
    let mut h = state_harness();
    for plane in [Plane::XY, Plane::XZ, Plane::YZ] {
        run(&mut h, Action::Text);
        let Dialog::Text(mut t) = h.state().dialog.clone() else { panic!("Text dialog should open") };
        t.text = "AB".into();
        t.plane = plane;
        t.height = "4 mm".into();
        t.depth = "0.8 mm".into();
        t.spacing = "0.3 mm".into();
        t.angle = "17 deg".into();
        t.x = "2 mm".into();
        t.y = "3 mm".into();
        t.align = fr_core::text::Align::Right;
        h.state_mut().dialog = Dialog::Text(t);
        h.state_mut().update_preview();
        let (_, built, error) = h.state().preview.as_ref().unwrap();
        assert!(error.is_none(), "{error:?}");
        assert!(!built.bodies.is_empty());
        assert!(h.state().doc().features.is_empty(), "preview must not commit the text");
        h.state_mut().apply_dialog();
        assert_eq!(h.state().dialog, Dialog::None);
        let original = h.state().doc().clone();
        let feature = original.features.last().unwrap();
        h.state_mut().edit_feature(feature.id);
        let Dialog::Text(t) = &h.state().dialog else { panic!("timeline should reopen the Text dialog") };
        assert_eq!(t.text, "AB");
        assert_eq!(t.plane, plane);
        assert_eq!(t.editing, Some(feature.id));
        assert_eq!(t.align, fr_core::text::Align::Right);
        assert_eq!([t.height.as_str(), t.depth.as_str(), t.spacing.as_str(), t.angle.as_str(), t.x.as_str(), t.y.as_str()], ["4 mm", "0.8 mm", "0.3 mm", "17 deg", "2 mm", "3 mm"]);
        h.state_mut().apply_dialog();
        assert_eq!(h.state().doc(), &original, "editing without changes must preserve all text settings");
        let path = std::env::temp_dir().join(format!("ferrender-text-roundtrip-{}.ferr", std::process::id()));
        h.state_mut().session.save(&path).unwrap();
        h.state_mut().open_path(&path);
        assert!(h.state().file_error.is_none());
        h.state_mut().edit_feature(feature.id);
        h.state_mut().apply_dialog();
        assert_eq!(h.state().doc(), &original, "native reopening must keep text editable");
        std::fs::remove_file(path).unwrap();
        run(&mut h, Action::Pattern);
        assert!(matches!(&h.state().dialog, Dialog::Pattern(p) if p.source == Some(feature.id)), "standalone text can be selected for a pattern");
        h.state_mut().execute(&json!({"op":"new", "discard_unsaved":true})).unwrap();
    }
}

#[test]
fn text_face_click_raises_and_timeline_edit_engraves() {
    let mut h = state_harness();
    plate(&mut h);
    let original_volume = volume(&h);
    run(&mut h, Action::View("top"));
    run(&mut h, Action::Fit);
    run(&mut h, Action::Text);
    let Dialog::Text(mut t) = h.state().dialog.clone() else { panic!() };
    t.text = "A".into();
    t.height = "4 mm".into();
    h.state_mut().dialog = Dialog::Text(t);
    let anchor = DVec3::new(8.0, 7.0, 10.0);
    let pos = crate::view::to_screen(h.state(), anchor);
    click(&mut h, pos);
    let Dialog::Text(t) = &h.state().dialog else { panic!() };
    assert_eq!(t.op, fr_core::Op::Join);
    assert!((t.plane.origin - anchor).length() < 1e-4, "use the actual click, not the triangle center");
    assert_eq!(t.face, Some(t.plane.origin));
    assert!(t.body.is_some() && t.frame.is_some());
    h.state_mut().update_preview();
    assert!(h.state().preview.as_ref().unwrap().2.is_none(), "{:?}", h.state().preview.as_ref().unwrap().2);
    h.state_mut().apply_dialog();
    assert!(volume(&h) > original_volume);
    let id = h.state().doc().features.last().unwrap().id;
    let original = h.state().doc().feature(id).unwrap().clone();
    h.state_mut().edit_feature(id);
    h.state_mut().apply_dialog();
    assert_eq!(h.state().doc().feature(id).unwrap(), &original, "attached text edit preserves face and frame");
    h.state_mut().edit_feature(id);
    let Dialog::Text(mut t) = h.state().dialog.clone() else { panic!() };
    t.op = fr_core::Op::Cut;
    h.state_mut().dialog = Dialog::Text(t);
    h.state_mut().apply_dialog();
    assert_eq!(h.state().dialog, Dialog::None);
    assert!(volume(&h) < original_volume, "editing Raised to Engraved removes material");
}

#[test]
fn text_rejects_curved_and_mesh_faces_without_changing_placement() {
    let mut h = state_harness();
    for cmd in [
        json!({"op":"create_sketch", "plane":"XY"}),
        json!({"op":"add_geometry", "items":[{"type":"circle", "center":[0,0], "radius":10}]}),
        json!({"op":"extrude", "distance":10}),
    ] { h.state_mut().execute(&cmd).unwrap(); }
    run(&mut h, Action::Text);
    let original = h.state().dialog.clone();
    let body = &h.state().session.built.bodies[0];
    let curved = fr_core::face::Face::near(body, DVec3::new(10.0, 0.0, 5.0)).unwrap();
    assert!(curved.plane.is_none());
    assert!(h.state_mut().text_on_face(curved).unwrap_err().contains("curved"));
    assert_eq!(h.state().dialog, original);
    let body = &h.state().session.built.bodies[0];
    let mesh = body.mesh.clone();
    let mut flat = fr_core::face::Face::near(body, DVec3::new(0.0, 0.0, 10.0)).unwrap();
    let id = h.state_mut().session.edit(|d| Ok(d.add_feature(fr_core::FeatureKind::Import(mesh)))).unwrap();
    flat.body = id;
    assert!(h.state_mut().text_on_face(flat).unwrap_err().contains("mesh"));
    assert_eq!(h.state().dialog, original);
}

/// Optional visual check of the text editor and its live solid preview.
#[test]
#[ignore = "writes a GPU screenshot for visual review"]
fn text_emboss_dialog_screenshot() {
    let mut h = harness();
    plate(&mut h);
    run(&mut h, Action::Text);
    let body = &h.state().session.built.bodies[0];
    let mut face = fr_core::face::Face::near(body, DVec3::new(5.0, 7.0, 10.0)).unwrap();
    face.at = DVec3::new(5.0, 7.0, 10.0);
    h.state_mut().text_on_face(face).unwrap();
    let Dialog::Text(mut t) = h.state().dialog.clone() else { panic!() };
    t.text = "FERR".into();
    t.height = "5 mm".into();
    h.state_mut().dialog = Dialog::Text(t);
    h.run_steps(3);
    assert!(h.state().preview.as_ref().unwrap().2.is_none(), "{:?}", h.state().preview.as_ref().unwrap().2);
    save(&mut h, "text-emboss-preview.png");
    h.state_mut().open_path(&out_dir().join("missing-design.ferr"));
    save(&mut h, "file-error-dialog.png");
}

#[test]
fn text_editing_uses_its_place_in_the_timeline_and_resolves_the_baseline() {
    let mut h = state_harness();
    plate(&mut h);
    let body_id = h.state().session.built.bodies[0].id;
    run(&mut h, Action::Text);
    let body = h.state().session.built.body(body_id).unwrap();
    let mut face = fr_core::face::Face::near(body, DVec3::new(8.0, 7.0, 10.0)).unwrap();
    face.at = DVec3::new(8.0, 7.0, 10.0);
    h.state_mut().text_on_face(face).unwrap();
    let Dialog::Text(mut t) = h.state().dialog.clone() else { panic!() };
    t.text = "A".into();
    t.height = "4 mm".into();
    h.state_mut().dialog = Dialog::Text(t);
    h.state_mut().apply_dialog();
    let text_id = h.state().doc().features.last().unwrap().id;
    h.state_mut().execute(&json!({"op":"transform", "body":body_id, "translate":[2,0,0]})).unwrap();
    let original = h.state().doc().clone();
    assert!((h.state().session.built.body(body_id).unwrap().mesh.bbox().unwrap().1.x - 42.0).abs() < 1e-5);
    run(&mut h, Action::View("top"));
    run(&mut h, Action::Fit);
    h.state_mut().edit_feature(text_id);
    h.run_steps(2);
    assert_eq!(h.state().doc(), &original, "preview must not roll the real timeline back");
    let source = h.state().text_source().body(body_id).unwrap();
    assert!((source.mesh.bbox().unwrap().1 - DVec3::new(40.0, 20.0, 10.0)).length() < 1e-5, "picking must exclude this text and later Move");
    assert!((h.state().shown().body(body_id).unwrap().mesh.bbox().unwrap().1.x - 40.0).abs() < 1e-5, "preview must exclude the later Move too");
    let anchor = DVec3::new(20.0, 7.0, 10.0);
    let pos = crate::view::to_screen(h.state(), anchor);
    click(&mut h, pos);
    let Dialog::Text(mut t) = h.state().dialog.clone() else { panic!() };
    assert!((t.face.unwrap() - anchor).length() < 1e-4);
    assert!((t.frame.unwrap()[1] - DVec3::new(40.0, 20.0, 10.0)).length() < 1e-5);
    t.x = "3 mm".into();
    t.y = "2 mm".into();
    t.angle = "45 deg".into();
    h.state_mut().dialog = Dialog::Text(t);
    h.state_mut().update_preview();
    assert!((h.state().text_baseline().unwrap() - DVec3::new(23.0, 9.0, 10.0)).length() < 1e-4, "marker includes X/Y offsets before angle rotation");
    assert!(h.state().preview.as_ref().unwrap().2.is_none());
    h.state_mut().apply_dialog();
    assert_eq!(h.state().dialog, Dialog::None);
    assert_eq!(h.state().doc().active(), original.active(), "later features return after applying");
    assert!((h.state().session.built.body(body_id).unwrap().mesh.bbox().unwrap().1.x - 42.0).abs() < 1e-5);
    h.state_mut().execute(&json!({"op":"transform", "body":body_id, "translate":[1000,0,0]})).unwrap();
    run(&mut h, Action::Fit);
    assert!(h.state().cam.target.x > 1000.0);
    h.state_mut().edit_feature(text_id);
    h.run_steps(2);
    assert!(h.state().cam.target.x < 40.0, "editing must fit the earlier geometry, even after a distant move");
    run(&mut h, Action::Cancel);
    h.run_steps(2);
    assert!(h.state().cam.target.x > 1000.0, "Cancel returns to the completed geometry");
}

#[test]
fn file_error_modal_blocks_viewport_and_shortcuts_until_dismissed() {
    let mut h = state_harness();
    plate(&mut h);
    run(&mut h, Action::View("top"));
    run(&mut h, Action::Fit);
    let pos = crate::view::to_screen(h.state(), DVec3::new(2.0, 2.0, 10.0));
    h.state_mut().open_path(&out_dir().join("missing-modal-blocking-design.ferr"));
    h.run_steps(3);
    assert!(h.state().file_error.is_some());
    click(&mut h, pos);
    assert!(h.state().sel_face.is_none(), "modal backdrop must block picking faces");
    key(&mut h, Key::E);
    assert_eq!(h.state().dialog, Dialog::None, "modal must block model keyboard shortcuts");
    assert!(h.state().file_error.is_some(), "clicking outside does not discard an error");
    h.get_by_label("Dismiss").click();
    h.run_steps(3);
    assert!(h.state().file_error.is_none());
    click(&mut h, pos);
    assert!(h.state().sel_face.is_some(), "picking must work again after Dismiss");
}

#[test]
fn point_coordinate_dialog_edits_dimensions_and_rejects_conflicts() {
    let mut h = state_harness();
    h.state_mut().create_sketch(Plane::XY);
    run(&mut h, Action::PointCoordinates);
    let Dialog::PointCoordinates(mut d) = h.state().dialog.clone() else { panic!("coordinate dialog") };
    d.x = "width = 12 mm".into();
    d.y = "-2.5e0 mm".into();
    h.state_mut().dialog = Dialog::PointCoordinates(d);
    h.run_steps(2);
    h.get_by_label("Place Point").click();
    h.run_steps(2);
    let p = h.state().sel[0];
    let (_, sk) = h.state().sketch().unwrap();
    assert!(sk.pos(p).distance(DVec2::new(12.0, -2.5)) < 1e-6);
    assert_eq!(sk.constraints.len(), 2);
    assert!(sk.constraints.values().any(|c| c.value.as_ref().is_some_and(|v| v.expr.contains("width"))));
    h.state_mut().open_point_coordinates(Some(p));
    let Dialog::PointCoordinates(mut d) = h.state().dialog.clone() else { unreachable!() };
    d.x = "$width / 2".into();
    d.y = "1 in".into();
    h.state_mut().dialog = Dialog::PointCoordinates(d);
    h.run_steps(2);
    h.get_by_label("Apply Coordinates").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Dialog::None);
    assert!(h.state().sketch().unwrap().1.pos(p).distance(DVec2::new(6.0,25.4)) < 1e-6);
    assert_eq!(h.state().sketch().unwrap().1.constraints.len(), 2, "updates existing dimensions");
    run(&mut h, Action::Undo);
    assert!(h.state().sketch().unwrap().1.pos(p).distance(DVec2::new(12.0,-2.5)) < 1e-6);
    run(&mut h, Action::Redo);
    let before = h.state().doc().clone();
    h.state_mut().open_point_coordinates(Some(p));
    let Dialog::PointCoordinates(mut d) = h.state().dialog.clone() else { unreachable!() };
    d.x = "1e999".into();
    assert!(!h.state_mut().apply_point_coordinates(&d));
    assert_eq!(h.state().doc(), &before, "bad numbers leave document untouched");
}

#[test]
fn new_curve_tools_close_profiles_and_show_editable_points() {
    let mut h = harness();
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().cam.scale = 12.0;
    h.state_mut().opts.dimensions = false;
    h.run_steps(2);
    run(&mut h, Action::Tool(Tool::Arc3));
    for (x,y) in [(-25.0,0.0),(-15.0,0.0),(-20.0,8.0)] { let pos=at(&h,x,y); click(&mut h,pos); }
    let (_, sk) = h.state().sketch().unwrap();
    assert_eq!(sk.entities.len(),1);
    assert_eq!(sk.open_endpoints().len(),2);
    assert_eq!(sk.arc_guides.len(),1,"the through-point relationship remains editable");
    run(&mut h, Action::Tool(Tool::Line));
    for (x,y) in [(-15.0,0.0),(-25.0,0.0)] { let pos=at(&h,x,y); click(&mut h,pos); }
    assert!(h.state().sketch().unwrap().1.open_endpoints().is_empty());
    assert_eq!(fr_core::profile::profiles(h.state().sketch().unwrap().1).len(),1);
    run(&mut h, Action::Tool(Tool::Spline));
    for (x,y) in [(0.0,0.0),(5.0,10.0),(10.0,5.0),(15.0,0.0)] { let pos=at(&h,x,y); click(&mut h,pos); }
    let (_, sk) = h.state().sketch().unwrap();
    let (spline, fit) = sk.entities.iter().find_map(|(id,e)| if let Geom::Spline {a,b,c,d}=e.geom {Some((*id,[a,b,c,d]))} else {None}).unwrap();
    let original = sk.pos(fit[1]);
    run(&mut h, Action::Tool(Tool::Select));
    h.state_mut().sel=vec![spline];
    let (from,to)=(at(&h,original.x,original.y),at(&h,original.x,original.y+3.0));
    drag(&mut h,&[from,from+egui::vec2(0.0,-8.0),to]);
    assert!(h.state().sketch().unwrap().1.pos(fit[1]).distance(original)>2.0);
    run(&mut h, Action::Tool(Tool::Line));
    for (x,y) in [(15.0,0.0),(0.0,0.0)] { let pos=at(&h,x,y); click(&mut h,pos); }
    run(&mut h, Action::Tool(Tool::Select));
    h.state_mut().sel=vec![spline];
    h.run_steps(2);
    assert!(h.state().sketch().unwrap().1.open_endpoints().is_empty());
    assert_eq!(fr_core::profile::profiles(h.state().sketch().unwrap().1).len(),2);
    save(&mut h,"sketch-curves-02.png");
}

#[test]
fn tangent_arc_clicks_preserve_the_selected_source_and_undo() {
    let mut h=state_harness();
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().cam.scale=10.0;
    h.run_steps(2);
    run(&mut h, Action::Tool(Tool::Line));
    for (x,y) in [(-20.0,-10.0),(-10.0,-10.0)] { let p=at(&h,x,y); click(&mut h,p); }
    run(&mut h, Action::Tool(Tool::Select));
    let line=*h.state().sketch().unwrap().1.entities.keys().next().unwrap();
    h.state_mut().sel=vec![line];
    run(&mut h, Action::Tool(Tool::TangentArc));
    assert_eq!(h.state().sel,vec![line]);
    for (x,y) in [(-10.0,-10.0),(0.0,0.0)] { let p=at(&h,x,y); click(&mut h,p); }
    let (_,sk)=h.state().sketch().unwrap();
    assert_eq!(sk.entities.len(),2);
    assert!(sk.constraints.values().any(|c| c.kind==CKind::Tangent));
    assert!(h.state().report.ok);
    run(&mut h,Action::Undo);
    assert_eq!(h.state().sketch().unwrap().1.entities.len(),1);
    run(&mut h,Action::Redo);
    assert_eq!(h.state().sketch().unwrap().1.entities.len(),2);
}

#[test]
fn reference_overlay_calibration_clicks_undo_and_portable_save() {
    let mut h=harness();
    h.state_mut().create_sketch(Plane::XY);
    let mut png=std::io::Cursor::new(Vec::new());
    let pixels=image::RgbaImage::from_fn(16,8,|x,y| {
        if x<8 && y<4 { image::Rgba([220,20,30,255]) }
        else if x>=8 && y<4 { image::Rgba([30,200,40,255]) }
        else if x<8 { image::Rgba([30,60,220,255]) }
        else { image::Rgba([230,200,30,255]) }
    });
    pixels.write_to(&mut png,image::ImageFormat::Png).unwrap();
    let mut reference=fr_core::reference::ReferenceImage::from_bytes("tracing-test.png",png.get_ref(),20.0).unwrap();
    reference.origin=DVec2::new(-20.0,-20.0);
    reference.opacity=0.8;
    assert!(h.state_mut().sketch_edit(|sk,_|{sk.reference=Some(reference);Ok(())}));
    h.state_mut().fit();
    h.run_steps(2);
    let image=save(&mut h,"reference-overlay-02.png");
    // Top-left source quadrant must remain red (the image must not be vertically flipped).
    let red=at(&h,-15.25,-12.6);
    let pixel=image.get_pixel(red.x.round() as u32,red.y.round() as u32);
    assert!(i16::from(pixel[0])>i16::from(pixel[1])+50 && i16::from(pixel[0])>i16::from(pixel[2])+50,"top-left pixel {pixel:?}");
    run(&mut h,Action::ReferenceImage);
    h.get_by_label("Calibrate from Two Points…").click();
    h.run_steps(2);
    assert!(h.state().reference_editor.is_calibrating());
    let a=at(&h,-20.0,-20.0); click(&mut h,a);
    let b=at(&h,-10.0,-20.0); click(&mut h,b);
    h.run_steps(2);
    assert!(!h.state().reference_editor.is_calibrating());
    h.get_by_label("Calibrate and Apply").click();
    h.run_steps(2);
    let (_,sk)=h.state().sketch().unwrap();
    let calibrated=sk.reference.as_ref().unwrap();
    assert!((calibrated.width-60.0).abs()<1e-5);
    assert!(calibrated.origin.distance(DVec2::new(-20.0,-20.0))<1e-5);
    let native=fr_core::io::to_json(h.state().doc());
    assert!(native.contains("tracing-test.png"));
    let restored=fr_core::io::from_json(&native).unwrap();
    assert_eq!(restored,h.state().doc().clone());
    run(&mut h,Action::Undo);
    assert!((h.state().sketch().unwrap().1.reference.as_ref().unwrap().width-20.0).abs()<1e-5);
    run(&mut h,Action::Redo);
    assert!((h.state().sketch().unwrap().1.reference.as_ref().unwrap().width-60.0).abs()<1e-5);
    run(&mut h,Action::ReferenceImage);
    h.get_by_label("Remove Image").click();
    h.run_steps(2);
    assert!(h.state().sketch().unwrap().1.reference.is_none());
    run(&mut h,Action::Undo);
    assert!(h.state().sketch().unwrap().1.reference.is_some());
}

#[test]
fn position_labels_accept_signed_zero_and_keep_zero_valued_parameters() {
    let mut h=state_harness();
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().execute(&json!({"op":"set_parameter","name":"offset","expr":"0 mm"})).unwrap();
    h.state_mut().open_point_coordinates(None);
    let Dialog::PointCoordinates(mut d)=h.state().dialog.clone() else {unreachable!()};
    d.x="$offset".into(); d.y="0 mm".into();
    assert!(h.state_mut().apply_point_coordinates(&d));
    let p=h.state().sel[0];
    assert_ne!(p,fr_core::ORIGIN,"zero-valued parameter cannot be dropped by reusing fixed origin");
    h.state_mut().execute(&json!({"op":"set_parameter","name":"offset","expr":"7 mm"})).unwrap();
    assert!((h.state().sketch().unwrap().1.pos(p).x-7.0).abs()<1e-6);
    let cid=*h.state().sketch().unwrap().1.constraints.iter().find(|(_,c)|c.kind==CKind::PositionX).unwrap().0;
    for (text,expected) in [("-3 mm",-3.0),("0 mm",0.0)] {
        h.state_mut().edit_dimension(cid,Pos2::new(500.0,500.0));
        h.state_mut().value_edit.as_mut().unwrap().text=text.into();
        assert!(h.state_mut().commit_value());
        assert!((h.state().sketch().unwrap().1.pos(p).x-expected).abs()<1e-6);
    }
    h.state_mut().dialog=Dialog::None;
    h.state_mut().sel=vec![p];
    run(&mut h,Action::ReferenceImage);
    key(&mut h,Key::Delete);
    assert!(h.state().sketch().unwrap().1.points.contains_key(&p),"image editor must not forward Delete into the selected sketch");
    key(&mut h,Key::Escape);
    assert!(h.state().reference_editor.sketch_id().is_none());
}
