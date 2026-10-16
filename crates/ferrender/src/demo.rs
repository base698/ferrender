//! Renders the README screenshots by driving the real app offscreen.
//!
//! ```sh
//! cargo test --release -p ferrender readme_ -- --ignored
//! ```
//!
//! Frames are written to `target/demo/` as PNG; `scripts/readme-images.sh` runs this and
//! turns them into the JPEGs under `docs/images/`.

use std::path::PathBuf;

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use fr_core::doc::{HoleFit, HoleShape};
use glam::DVec3;
use serde_json::{Value, json};

use crate::app::{Action, App, Dialog, Tool};

type H = Harness<'static, App>;

fn launch() -> H {
    let mut h = Harness::builder().with_size(vec2(1400.0, 880.0)).with_pixels_per_point(2.0).wgpu().build_eframe(|cc| App::new(cc, None));
    h.run_steps(4);
    h
}

fn shot(h: &mut H, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/demo");
    std::fs::create_dir_all(&dir).unwrap();
    // No pointer in the picture.
    h.event(Event::PointerGone);
    h.run_steps(4);
    h.render().expect("the app should render").save(dir.join(name)).unwrap();
}

fn exec(h: &mut H, cmd: Value) -> Value {
    h.state_mut().execute(&cmd).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn run(h: &mut H, a: Action) {
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, a);
    h.run_steps(2);
}

fn click(h: &mut H, pos: Pos2) {
    let button = |h: &H, pressed| h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.hover_at(pos);
    h.step();
    button(h, true);
    h.step();
    button(h, false);
    h.step();
}

/// Clicks the point of the model at `p`, in millimetres.
fn click_at(h: &mut H, p: [f64; 3]) {
    let pos = crate::view::to_screen(h.state(), DVec3::from(p));
    click(h, pos);
}

/// Looks from a named view with everything in frame, then moves in by `zoom` on `target`.
fn look(h: &mut H, view: &'static str, zoom: f64, target: Option<[f64; 3]>) {
    run(h, Action::View(view));
    h.state_mut().fit();
    let cam = &mut h.state_mut().cam;
    cam.scale *= zoom;
    if let Some(t) = target {
        cam.target = DVec3::from(t);
    }
    h.run_steps(2);
}

fn previewed(h: &H) {
    let p = h.state().preview.as_ref().expect("the dialog should be previewing");
    assert!(p.2.is_none(), "{:?}", p.2);
}

/// The plate and boss of the bearing block, from one constrained sketch driven by parameters.
fn block(h: &mut H) {
    for (name, expr) in [("w", "60 mm"), ("d", "40 mm"), ("t", "8 mm"), ("boss", "26 mm")] {
        exec(h, json!({"op": "set_parameter", "name": name, "expr": expr}));
    }
    exec(h, json!({"op": "create_sketch", "plane": "XY"}));
    let made = exec(h, json!({"op": "add_geometry", "sketch": "$last_sketch", "items": [
        {"type": "rect", "from": [0, 0], "to": [55, 35]},
        {"type": "circle", "center": [25, 15], "radius": 11}
    ]}));
    let (sides, circle, centre) = (&made["items"][0]["entities"], &made["items"][1]["entities"][0], &made["items"][1]["points"][0]);
    for (kind, refs, value) in [
        ("distance", json!([sides[0]]), "$w"),
        ("distance", json!([sides[1]]), "$d"),
        ("diameter", json!([circle]), "$boss"),
        ("distance", json!([centre, sides[3]]), "$w / 2"),
        ("distance", json!([centre, sides[0]]), "$d / 2"),
    ] {
        exec(h, json!({"op": "add_constraint", "sketch": "$last_sketch", "kind": kind, "refs": refs, "value": value}));
    }
}

#[test]
#[ignore]
fn readme_part() {
    let mut h = launch();
    block(&mut h);

    // The sketch, fully constrained, with the parameters that drive it.
    h.state_mut().edit_sketch(1);
    h.state_mut().show_params = true;
    h.run_steps(3);
    h.state_mut().cam.scale *= 0.72;
    h.state_mut().cam.target = DVec3::new(19.0, 27.0, 0.0);
    assert_eq!(h.state().report.dof, 0, "the block's sketch should be fully constrained");
    shot(&mut h, "sketch.png");
    h.state_mut().show_params = false;
    run(&mut h, Action::FinishSketch);

    // Extrude, with the arrow standing on the profile.
    look(&mut h, "iso", 0.55, None);
    run(&mut h, Action::Extrude);
    let Dialog::Feature(mut f) = h.state().dialog.clone() else { panic!("the extrude dialog should be open") };
    f.text = "$t".into();
    h.state_mut().dialog = Dialog::Feature(f);
    h.run_steps(2);
    click_at(&mut h, [6.0, 6.0, 0.0]);
    click_at(&mut h, [30.0, 20.0, 0.0]);
    h.run_steps(3);
    shot(&mut h, "extrude.png");
    run(&mut h, Action::Cancel);
    exec(&mut h, json!({"op": "extrude", "sketch": 1, "distance": "$t", "profiles": "all"}));
    exec(&mut h, json!({"op": "extrude", "sketch": 1, "distance": "$t + 14 mm", "profiles": [0], "operation": "join"}));
    let body = exec(&mut h, json!({"op": "get_scene_info"}))["bodies"][0]["id"].clone();
    exec(&mut h, json!({"op": "fillet_edges", "body": body, "edges": [[0, 0, 4], [60, 0, 4], [0, 40, 4], [60, 40, 4]], "radius": 6}));

    // Fillet: the edge where the boss meets the plate, picked by clicking it.
    look(&mut h, "iso", 0.8, None);
    run(&mut h, Action::Blend(false));
    let picked = |h: &H| match &h.state().dialog {
        Dialog::Blend(b) => b.edges.len(),
        _ => panic!("the fillet dialog should be open"),
    };
    for deg in (0..360).step_by(15) {
        let a = (deg as f64).to_radians();
        click_at(&mut h, [30.0 + 13.0 * a.cos(), 20.0 + 13.0 * a.sin(), 8.0]);
        if picked(&h) == 1 {
            break;
        }
    }
    assert_eq!(picked(&h), 1, "the boss's foot should be clickable from this side");
    let Dialog::Blend(mut b) = h.state().dialog.clone() else { unreachable!() };
    b.text = "3 mm".into();
    h.state_mut().dialog = Dialog::Blend(b);
    h.run_steps(3);
    previewed(&h);
    shot(&mut h, "fillet.png");
    h.state_mut().apply_dialog();

    // Hole: four counterbored M4 clearance holes, each a click on the top of the plate.
    run(&mut h, Action::Hole);
    for at in [[8.0, 8.0, 8.0], [52.0, 8.0, 8.0], [52.0, 32.0, 8.0], [8.0, 32.0, 8.0]] {
        click_at(&mut h, at);
    }
    let Dialog::Hole(mut d) = h.state().dialog.clone() else { panic!("the hole dialog should be open") };
    assert_eq!(d.at.len(), 4, "{:?}", d.at);
    (d.shape, d.thread) = (HoleShape::Counterbore, "M4x0.7".into());
    h.state_mut().dialog = Dialog::Hole(d);
    h.run_steps(3);
    previewed(&h);
    shot(&mut h, "hole.png");
    h.state_mut().apply_dialog();

    // A tapped M12 bore through the boss, with the real thread.
    run(&mut h, Action::Hole);
    click_at(&mut h, [30.0, 20.0, 22.0]);
    let Dialog::Hole(mut d) = h.state().dialog.clone() else { panic!() };
    assert_eq!(d.at.len(), 1);
    (d.fit, d.thread, d.modeled) = (HoleFit::Tapped, "M12x1.75".into(), true);
    h.state_mut().dialog = Dialog::Hole(d);
    h.run_steps(3);
    previewed(&h);
    shot(&mut h, "tapped.png");
    h.state_mut().apply_dialog();

    // The bolt that goes in it: a hex head and a rod, then Thread on the rod.
    for cmd in [
        json!({"op": "create_sketch", "plane": "XY"}),
        json!({"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "ngon", "center": [88, 20], "radius": 10.39, "sides": 6}]}),
        json!({"op": "extrude", "sketch": "$last_sketch", "distance": 7.5}),
        json!({"op": "create_sketch", "plane": "XY", "offset": 7.5}),
        json!({"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "circle", "center": [88, 20], "radius": 6}]}),
        json!({"op": "extrude", "sketch": "$last_sketch", "distance": 30, "operation": "join"}),
    ] {
        exec(&mut h, cmd);
    }
    look(&mut h, "iso", 1.35, Some([56.0, 26.0, 8.0]));
    run(&mut h, Action::Thread);
    for deg in (0..360).step_by(15) {
        let a = (deg as f64).to_radians();
        click_at(&mut h, [88.0 + 6.0 * a.cos(), 20.0 + 6.0 * a.sin(), 24.0]);
        if matches!(&h.state().dialog, Dialog::Thread(t) if t.found.is_some_and(|f| !f.1)) {
            break;
        }
    }
    let Dialog::Thread(mut t) = h.state().dialog.clone() else { panic!("the thread dialog should be open") };
    assert_eq!(t.thread, "M12x1.75", "{t:?}");
    (t.full, t.offset, t.length) = (false, "0 mm".into(), "22 mm".into());
    h.state_mut().dialog = Dialog::Thread(t);
    h.run_steps(3);
    previewed(&h);
    shot(&mut h, "thread.png");
    h.state_mut().apply_dialog();
    assert!(h.state().session.built.errors.is_empty(), "{:?}", h.state().session.built.errors);

    look(&mut h, "iso", 1.5, Some([50.0, 20.0, 10.0]));
    shot(&mut h, "hero.png");

    // Close on the threads.
    look(&mut h, "iso", 2.4, Some([62.0, 20.0, 22.0]));
    shot(&mut h, "threads.png");

    // Measure: the top of the boss to the top of the plate.
    look(&mut h, "iso", 1.35, Some([56.0, 26.0, 8.0]));
    // A click near an edge takes the edge, so try spots until both land on the faces.
    let spots = [[2.0, 9.0], [-2.0, 9.0], [9.0, 2.0], [9.0, -2.0], [-9.0, 2.0], [-9.0, -2.0], [2.0, -9.0], [-2.0, -9.0]];
    let apart = |h: &H| match &h.state().dialog {
        Dialog::Measure(m) => m.result.and_then(|r| r.apart).map(|a| (a * 1e6).round() / 1e6),
        _ => None,
    };
    for [x, y] in spots {
        run(&mut h, Action::Cancel);
        run(&mut h, Action::Measure);
        click_at(&mut h, [30.0 + x, 20.0 + y, 22.0]);
        click_at(&mut h, [30.0 + x * 2.2, 20.0 + y * 1.9, 8.0]);
        if apart(&h) == Some(14.0) {
            break;
        }
    }
    assert_eq!(apart(&h), Some(14.0), "the boss stands 14 mm above the plate");
    shot(&mut h, "measure.png");
    run(&mut h, Action::Cancel);

    // Section Analysis through the bore.
    h.state_mut().section = crate::app::Section { on: true, axis: 1, plane: None, offset: 20.0, flip: false };
    h.state_mut().show_section = true;
    look(&mut h, "iso", 1.5, Some([44.0, 14.0, 6.0]));
    shot(&mut h, "section.png");
}

/// Sizes typed while drawing: the width is held, the height is being typed.
#[test]
#[ignore]
fn readme_typed_sizes() {
    let mut h = launch();
    h.state_mut().create_sketch(fr_core::Plane::XY);
    h.run_steps(2);
    h.state_mut().cam.scale *= 2.6;
    h.state_mut().cam.target = DVec3::new(22.0, 10.0, 0.0);
    h.run_steps(2);
    run(&mut h, Action::Tool(Tool::Rect));
    click_at(&mut h, [0.0, 0.0, 0.0]);
    let far = crate::view::to_screen(h.state(), DVec3::new(41.0, 22.0, 0.0));
    h.hover_at(far);
    h.run_steps(3);
    for text in ["w = 48", "\t", "0.75 in"] {
        if text == "\t" {
            for pressed in [true, false] {
                h.event(Event::Key { key: egui::Key::Tab, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
                h.step();
            }
        } else {
            h.event(Event::Text(text.into()));
            h.run_steps(2);
        }
    }
    shot(&mut h, "typed.png");
}
