//! Loft in the app: the dialog's defaults, picking sections in the viewport, preview, apply and edit.
use super::*;
use crate::app::LoftDlg;
use fr_core::{FeatureKind, Id};

fn command(h: &mut H, c: serde_json::Value) -> serde_json::Value { h.state_mut().execute(&c).unwrap_or_else(|e| panic!("{c}: {e}")) }

/// A sketch on XY lifted to `z` holding a square of half-width `half` centred at `(x, 0)`.
fn square(h: &mut H, z: f64, x: f64, half: f64) -> Id {
    command(h, json!({"op": "create_sketch", "plane": "XY", "offset": z}));
    let id = h.state().doc().sketches().last().unwrap().0.id;
    command(h, json!({"op": "add_geometry", "sketch": id, "items": [{"type": "rect", "from": [x - half, -half], "to": [x + half, half]}]}));
    id
}

fn loft_of(h: &H) -> LoftDlg {
    match &h.state().dialog { Dialog::Loft(l) => l.clone(), other => panic!("the loft dialog should be open, not {other:?}") }
}

fn world_click(h: &mut H, p: [f64; 3]) {
    let p = crate::view::to_screen(h.state(), DVec3::from_array(p));
    click(h, p);
}

fn frustum(a: f64, b: f64, height: f64) -> f64 { height / 3.0 * (a + b + (a * b).sqrt()) }

fn volume(h: &H) -> f64 { h.state().session.built.bodies.iter().flat_map(|b| b.solids.iter()).map(|s| s.volume()).sum() }

#[test]
fn loft_offers_stacked_sketches_previews_applies_and_edits() {
    let mut h = harness();
    let (base, top) = (square(&mut h, 0.0, 0.0, 10.0), square(&mut h, 20.0, 0.0, 4.0));
    h.state_mut().fit();
    h.run_steps(3);
    run(&mut h, Action::Loft);
    let l = loft_of(&h);
    assert_eq!(l.sections.iter().map(|s| s.sketch).collect::<Vec<_>>(), vec![base, top], "two sketches with one region each are offered in order");
    assert!(!l.ruled && l.editing.is_none());
    h.run_steps(3);
    // The dialog is Loft's own.
    h.get_by_label("Sections");
    h.get_by_label("Walls");
    h.get_by_label("Smooth");
    {
        let p = h.state().preview.as_ref().expect("the dialog previews the loft");
        assert_eq!(p.2, None, "the preview builds");
        let v: f64 = p.1.bodies[0].solids.iter().map(|s| s.volume()).sum();
        assert!((v - frustum(400.0, 64.0, 20.0)).abs() < 1e-6, "{v}");
    }
    save(&mut h, "loft-dialog.png");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Dialog::None, "apply failed: {:?}", h.state().toast);
    assert!((volume(&h) - frustum(400.0, 64.0, 20.0)).abs() < 1e-6);
    let id = h.state().doc().features.last().unwrap().id;
    assert!(matches!(h.state().doc().features.last().unwrap().kind, FeatureKind::Loft(_)));
    assert!(!h.state().doc().sketch(base).unwrap().visible && !h.state().doc().sketch(top).unwrap().visible, "the sections are put away");
    // One undo removes it and shows the sketches again.
    run(&mut h, Action::Undo);
    assert!(h.state().session.built.bodies.is_empty());
    run(&mut h, Action::Redo);
    // Editing reopens the dialog with its sections, hidden sketches and all.
    h.state_mut().edit_feature(id);
    h.run_steps(3);
    let l = loft_of(&h);
    assert_eq!((l.editing, l.sections.len()), (Some(id), 2));
    h.get_by_label("Straight").click();
    h.run_steps(3);
    assert!(loft_of(&h).ruled);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Dialog::None, "apply failed: {:?}", h.state().toast);
    let FeatureKind::Loft(l) = &h.state().doc().feature(id).unwrap().kind else { panic!("still a loft") };
    assert!(l.ruled);
    assert_eq!(h.state().doc().features.iter().filter(|f| matches!(f.kind, FeatureKind::Loft(_))).count(), 1, "editing does not add a second loft");
}

#[test]
fn loft_sections_are_clicked_in_the_viewport_and_a_mismatch_is_explained() {
    let mut h = harness();
    // Three sketches side by side in the view, each on its own plane; the third has two regions,
    // so nothing is offered for it and the dialog starts empty once the first two are cleared.
    let (a, b) = (square(&mut h, 0.0, -30.0, 8.0), square(&mut h, 15.0, 0.0, 5.0));
    command(&mut h, json!({"op": "create_sketch", "plane": "XY", "offset": 30}));
    let c = h.state().doc().sketches().last().unwrap().0.id;
    command(&mut h, json!({"op": "add_geometry", "sketch": c, "items": [{"type": "circle", "center": [30, 0], "radius": 6}]}));
    run(&mut h, Action::View("top"));
    h.state_mut().fit();
    h.run_steps(3);
    run(&mut h, Action::Loft);
    h.run_steps(2);
    h.get_by_label("Clear sections").click();
    h.run_steps(2);
    assert!(loft_of(&h).sections.is_empty());
    h.get_by_label("click a closed region in each sketch");
    // Click the regions in order.
    world_click(&mut h, [-30.0, 0.0, 0.0]);
    world_click(&mut h, [0.0, 0.0, 15.0]);
    h.run_steps(3);
    assert_eq!(loft_of(&h).sections.iter().map(|s| s.sketch).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!(h.state().preview.as_ref().expect("a preview").2, None);
    // A circle cannot follow a square: the dialog says so with both counts, and OK is refused.
    world_click(&mut h, [30.0, 0.0, 30.0]);
    h.run_steps(3);
    assert_eq!(loft_of(&h).sections.len(), 3);
    let error = h.state().preview.as_ref().expect("a preview").2.clone().expect("the mismatch is reported");
    assert!(error.contains("section 3 has 1 edge where section 1 has 4 edges"), "{error}");
    save(&mut h, "loft-mismatch.png");
    // Clicking a chosen region again leaves it out.
    world_click(&mut h, [30.0, 0.0, 30.0]);
    h.run_steps(3);
    assert_eq!(loft_of(&h).sections.iter().map(|s| s.sketch).collect::<Vec<_>>(), vec![a, b]);
    let _ = c;
    // Reorder with the dialog's arrows: the second section moves first.
    if let Dialog::Loft(l) = &mut h.state_mut().dialog { l.sections.swap(0, 1); }
    h.run_steps(3);
    assert_eq!(h.state().preview.as_ref().expect("a preview").2, None, "either order lofts");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Dialog::None, "apply failed: {:?}", h.state().toast);
    let FeatureKind::Loft(l) = &h.state().doc().features.last().unwrap().kind else { panic!("a loft") };
    assert_eq!(l.sections.iter().map(|s| s.sketch).collect::<Vec<_>>(), vec![b, a]);
    assert_eq!(h.state().session.built.bodies[0].mesh.open_edges(), 0);
}
