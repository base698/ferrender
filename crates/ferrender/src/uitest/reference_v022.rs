//! Real Select-tool pointer interactions for reference-image placement.
use super::*;
use fr_core::{Id, reference::ReferenceImage};

fn setup(rotation: f64) -> (H<'static>, Id) {
    let mut h = harness();
    h.state_mut().create_sketch(Plane::XY);
    let Mode::Sketch(sid) = h.state().mode else { panic!("missing sketch"); };
    // A public, generated tracing grid: no personal photograph is embedded.
    let pixels = image::RgbaImage::from_fn(120, 60, |x, y| {
        if x % 20 == 0 || y % 20 == 0 { image::Rgba([20, 55, 85, 255]) }
        else if x < 60 { image::Rgba([40, 165, 215, 255]) }
        else { image::Rgba([245, 175, 60, 255]) }
    });
    let mut png = std::io::Cursor::new(Vec::new());
    pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let mut image = ReferenceImage::from_bytes("tracing-grid.png", png.get_ref(), 60.0).unwrap();
    image.origin = DVec2::new(-40.0, -20.0);
    image.rotation = rotation;
    image.opacity = 0.75;
    assert!(h.state_mut().sketch_edit(|sk, _| { sk.reference = Some(image); Ok(()) }));
    h.state_mut().cam.target = DVec3::ZERO;
    h.state_mut().cam.scale = 5.0;
    h.run_steps(3);
    (h, sid)
}

fn stored(h: &H, sid: Id) -> ReferenceImage { h.state().doc().sketch(sid).unwrap().reference.clone().unwrap() }
fn position(h: &H, p: DVec2) -> Pos2 { at(h, p.x, p.y) }
fn close(a: DVec2, b: DVec2) { assert!(a.distance(b) < 1e-4, "{a:?} != {b:?}"); }

fn begin_drag(h: &mut H, start: Pos2, to: Pos2) {
    h.hover_at(start);
    h.step();
    button(h, start, true);
    h.step();
    h.hover_at(to);
    h.step();
}

#[test]
fn reference_move_previews_then_commits_one_undo_step() {
    let (mut h, sid) = setup(0.0);
    let before = stored(&h, sid);
    let revision = h.state().session.rev;
    let center = before.corners()[0].lerp(before.corners()[2], 0.5);
    let start = position(&h, center);
    let mid = position(&h, center + DVec2::new(4.0, 2.0));
    let end = position(&h, center + DVec2::new(8.0, 4.0));
    begin_drag(&mut h, start, mid);
    assert!(h.state().reference_drag.is_dragging());
    h.hover_at(end);
    h.step();
    assert_eq!(stored(&h, sid), before, "dragging must not edit the document");
    assert_eq!(h.state().session.rev, revision, "dragging must not rebuild");
    close(h.state().reference_drag.preview(sid).unwrap().origin, before.origin + DVec2::new(8.0, 4.0));
    button(&h, end, false);
    h.step();
    assert_eq!(h.state().session.rev, revision + 1);
    let moved = stored(&h, sid);
    close(moved.origin, before.origin + DVec2::new(8.0, 4.0));
    run(&mut h, Action::Undo);
    assert_eq!(stored(&h, sid), before);
    run(&mut h, Action::Redo);
    assert_eq!(stored(&h, sid), moved);
}

#[test]
fn rotated_reference_corner_scales_and_survives_save_with_visible_handles() {
    let (mut h, sid) = setup(27.0);
    let before = stored(&h, sid);
    let center = position(&h, before.corners()[0].lerp(before.corners()[2], 0.5));
    click(&mut h, center);
    assert!(h.state().reference_drag.selected(sid));
    let anchor = before.corners()[0];
    let corner = before.corners()[2];
    let start = position(&h, corner);
    let end = position(&h, anchor + (corner - anchor) * 1.35);
    drag(&mut h, &[start, end]);
    let scaled = stored(&h, sid);
    assert!((scaled.width - 81.0).abs() < 1e-4);
    assert_eq!(scaled.rotation, before.rotation);
    assert_eq!(scaled.height() / scaled.width, 0.5);
    close(scaled.corners()[0], anchor);
    let restored = fr_core::io::from_json(&fr_core::io::to_json(h.state().doc())).unwrap();
    assert_eq!(restored.sketch(sid).unwrap().reference.as_ref(), Some(&scaled));
    let path = out_dir().join("reference-select-0.2.2.ferr");
    h.state_mut().session.save(&path).unwrap();
    save(&mut h, "reference-select-light-0.2.2.png");
    h.state_mut().set_appearance(Appearance::Dark);
    save(&mut h, "reference-select-dark-0.2.2.png");
    run(&mut h, Action::Undo);
    assert_eq!(stored(&h, sid), before);
}

#[test]
fn reference_escape_focus_loss_and_release_outside_canvas_are_transactional() {
    let (mut h, sid) = setup(0.0);
    let before = stored(&h, sid);
    let center = before.corners()[0].lerp(before.corners()[2], 0.5);
    let start = position(&h, center);
    let end = start + egui::vec2(35.0, 20.0);
    let revision = h.state().session.rev;
    begin_drag(&mut h, start, end);
    key(&mut h, Key::Escape);
    assert!(!h.state().reference_drag.is_dragging());
    assert!(h.state().reference_drag.selected(sid));
    assert_eq!(stored(&h, sid), before);
    button(&h, end, false);
    h.step();
    key(&mut h, Key::Escape);
    assert!(!h.state().reference_drag.selected(sid));
    assert_eq!(h.state().session.rev, revision);

    begin_drag(&mut h, start, end);
    h.event(Event::WindowFocused(false));
    h.step();
    assert!(!h.state().reference_drag.is_dragging());
    assert_eq!(stored(&h, sid), before);
    h.event(Event::WindowFocused(true));
    button(&h, end, false);
    h.step();

    // Outside the viewport over the browser panel still releases the original drag.
    let outside = egui::pos2(h.state().vp.left() - 30.0, h.state().vp.center().y);
    let expected = before.origin + DVec2::new((outside.x - start.x) as f64 / 5.0, -(outside.y - start.y) as f64 / 5.0);
    begin_drag(&mut h, start, end);
    h.hover_at(outside);
    h.step();
    button(&h, outside, false);
    h.step();
    assert!(!h.state().reference_drag.is_dragging());
    close(stored(&h, sid).origin, expected);
    assert_eq!(h.state().session.rev, revision + 1);
    run(&mut h, Action::Undo);
    assert_eq!(stored(&h, sid), before);
}

#[test]
fn reference_respects_geometry_hidden_images_and_tool_changes() {
    let (mut h, sid) = setup(0.0);
    let before = stored(&h, sid);
    let (mut line, mut first) = (0, 0);
    h.state_mut().sketch_edit(|sk, _| {
        first = sk.add_point(DVec2::new(-30.0, -10.0));
        let second = sk.add_point(DVec2::new(-10.0, -10.0));
        line = sk.add_line(first, second);
        Ok(())
    });
    h.run_steps(2);
    let edge = at(&h, -20.0, -10.0);
    click(&mut h, edge);
    assert_eq!(h.state().sel, [line], "geometry must be selectable through the image");
    assert!(!h.state().reference_drag.selected(sid));
    let point = at(&h, -30.0, -10.0);
    click(&mut h, point);
    assert_eq!(h.state().sel, [first]);
    let empty_image = at(&h, -20.0, 5.0);
    click(&mut h, empty_image);
    assert!(h.state().reference_drag.selected(sid));
    let moved = empty_image + egui::vec2(35.0, 20.0);
    begin_drag(&mut h, empty_image, moved);
    run(&mut h, Action::Tool(Tool::Line));
    assert!(!h.state().reference_drag.is_dragging());
    assert!(!h.state().reference_drag.selected(sid));
    assert_eq!(stored(&h, sid), before);
    button(&h, moved, false);
    h.step();
    run(&mut h, Action::Tool(Tool::Select));
    h.state_mut().sketch_edit(|sk, _| { sk.reference.as_mut().unwrap().visible = false; Ok(()) });
    h.run_steps(2);
    click(&mut h, empty_image);
    assert!(!h.state().reference_drag.selected(sid));
    assert!(!h.state().reference_drag.is_dragging());
}
