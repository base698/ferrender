//! Ellipses in sketches: the entity, its solver behaviour, the regions it bounds and the
//! solids made from it.

use std::f64::consts::PI;

use fr_core::api::execute;
use fr_core::profile::profiles;
use fr_core::sketch::{CKind, Geom};
use fr_core::{Id, Session, solver};
use glam::DVec2;
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    execute(s, &c, None).unwrap_or_else(|e| panic!("{c} failed: {e}"))
}

fn refused(s: &mut Session, c: J) -> String {
    execute(s, &c, None).expect_err("the command should be refused")
}

/// A sketch on `plane` lifted by `offset` holding `items`. Returns the sketch id and the reply.
fn sketch(s: &mut Session, plane: &str, offset: f64, items: J) -> (Id, J) {
    cmd(s, json!({"op": "create_sketch", "plane": plane, "offset": offset}));
    let id = s.doc.sketches().last().unwrap().0.id;
    let out = cmd(s, json!({"op": "add_geometry", "sketch": id, "items": items}));
    (id, out)
}

fn volume(s: &Session) -> f64 {
    s.built.bodies.iter().map(|b| b.solids.iter().map(|x| x.volume()).sum::<f64>()).sum()
}

fn within(a: f64, b: f64, part: f64) -> bool { (a - b).abs() <= part * b.abs() }

#[test]
fn an_ellipse_is_a_centre_and_two_axis_points_held_square() {
    let mut s = Session::default();
    let (sid, out) = sketch(&mut s, "XY", 0.0, json!([{"type": "ellipse", "center": [5, 0], "radii": [30, 10], "rotation": 30}]));
    let (ent, pts) = (out["items"][0]["entities"][0].as_u64().unwrap() as Id, out["items"][0]["points"].as_array().unwrap().clone());
    let [c, a, b] = [0, 1, 2].map(|i| pts[i].as_u64().unwrap() as Id);
    let sk = s.doc.sketch(sid).unwrap().clone();
    assert_eq!(sk.entities[&ent].geom, Geom::Ellipse { c, a, b });
    let dir = DVec2::from_angle(30f64.to_radians());
    assert!((sk.pos(a) - (DVec2::new(5.0, 0.0) + dir * 30.0)).length() < 1e-9);
    assert!((sk.pos(b) - (DVec2::new(5.0, 0.0) + dir.perp() * 10.0)).length() < 1e-9, "the second axis is a quarter turn on");
    // Five ways to move: where it is, which way it points, and its two radii.
    assert_eq!(solver::solve(&mut sk.clone(), &[]).dof, 5);

    // Its description gives the same numbers back.
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": sid}));
    let e = info["sketch"]["entities"].as_array().unwrap().iter().find(|e| e["type"] == "ellipse").expect("listed").clone();
    assert_eq!(e["center"], json!(c));
    assert_eq!(e["axis_points"], json!([a, b]));
    assert_eq!(e["radii"], json!([30.0, 10.0]));
    assert_eq!(e["rotation"], json!(30.0));

    // Dragging the end of one axis turns and stretches it; the other stays square to it.
    let mut dragged = sk.clone();
    let target = DVec2::new(5.0, 40.0);
    assert!(solver::solve(&mut dragged, &[(a, target)]).ok);
    let (u, w) = (dragged.pos(a) - dragged.pos(c), dragged.pos(b) - dragged.pos(c));
    assert!((dragged.pos(a) - target).length() < 1e-4, "{:?}", dragged.pos(a));
    assert!(u.dot(w).abs() < 1e-6 * u.length() * w.length(), "the axes stay perpendicular");
    assert!((w.length() - 10.0).abs() < 0.5, "and the other radius is about what it was: {}", w.length());

    // The radii are ordinary distances between its points.
    cmd(&mut s, json!({"op": "add_constraint", "sketch": sid, "kind": "distance", "refs": [c, a], "value": "40 mm"}));
    cmd(&mut s, json!({"op": "add_constraint", "sketch": sid, "kind": "distance", "refs": [c, b], "value": "15 mm"}));
    cmd(&mut s, json!({"op": "add_constraint", "sketch": sid, "kind": "horizontal", "refs": [c, a]}));
    let sk = s.doc.sketch(sid).unwrap().clone();
    let (_, u, v) = sk.ellipse(ent).unwrap();
    assert!((u.length() - 40.0).abs() < 1e-6 && (v.length() - 15.0).abs() < 1e-6 && u.y.abs() < 1e-6, "{u:?} {v:?}");
    assert_eq!(solver::solve(&mut sk.clone(), &[]).dof, 2, "only its position is left free");

    // A point can be held on the curve.
    let p = cmd(&mut s, json!({"op": "add_geometry", "sketch": sid, "items": [{"type": "point", "at": [20, 30]}]}))["items"][0]["points"][0].as_u64().unwrap() as Id;
    cmd(&mut s, json!({"op": "add_constraint", "sketch": sid, "kind": "coincident", "refs": [p, ent]}));
    let sk = s.doc.sketch(sid).unwrap().clone();
    let (centre, u, v) = sk.ellipse(ent).unwrap();
    let d = sk.pos(p) - centre;
    let k = (d.dot(u) / u.length_squared()).powi(2) + (d.dot(v) / v.length_squared()).powi(2);
    assert!((k - 1.0).abs() < 1e-6, "the point lies on the ellipse: {k}");
    // What an ellipse does not take is said plainly.
    assert!(refused(&mut s, json!({"op": "add_constraint", "sketch": sid, "kind": "radius", "refs": [ent], "value": "5 mm"})).contains("radius needs"));
    assert!(refused(&mut s, json!({"op": "offset", "sketch": sid, "ids": [ent], "distance": 2})).contains("ellipse"));
    assert!(refused(&mut s, json!({"op": "add_geometry", "sketch": sid, "items": [{"type": "ellipse", "center": [0, 0], "radii": [10, 0]}]})).contains("positive"));
    assert!(s.doc.sketch(sid).unwrap().constraints.values().any(|c| c.kind == CKind::Coincident));
}

#[test]
fn an_ellipse_bounds_a_region_and_extrudes_to_its_exact_volume() {
    let mut s = Session::default();
    let (sid, _) = sketch(&mut s, "XY", 0.0, json!([{"type": "ellipse", "center": [0, 0], "radii": [30, 10], "rotation": 20}]));
    let regions = profiles(s.doc.sketch(sid).unwrap());
    assert_eq!(regions.len(), 1);
    // The drawn outline is a 72-gon; the solid is made from the curve itself.
    assert!(within(fr_core::profile::signed_area(&regions[0].outer), PI * 300.0, 2e-3));
    cmd(&mut s, json!({"op": "extrude", "sketch": sid, "distance": 5}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let exact = PI * 30.0 * 10.0 * 5.0;
    assert!((volume(&s) - exact).abs() < 1e-8, "{} is not {exact}: the edge is not an exact ellipse", volume(&s));
    let b = &s.built.bodies[0];
    assert!(b.is_exact() && b.mesh.open_edges() == 0);
    // Its side is one face named after the ellipse, so a fillet finds its rim.
    let body = b.id;
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": body}));
    assert_eq!(info["topology"]["faces"].as_array().unwrap().len(), 3, "two caps and one side: {}", info["topology"]["faces"]);
    cmd(&mut s, json!({"op": "fillet_edges", "body": body, "edges": "all", "radius": 1}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);

    // A circle inside an ellipse is a hole; an ellipse inside a rectangle is one too.
    let (sid, _) = sketch(&mut s, "XY", 50.0, json!([
        {"type": "ellipse", "center": [0, 0], "radii": [30, 20]},
        {"type": "circle", "center": [0, 0], "radius": 5},
    ]));
    cmd(&mut s, json!({"op": "extrude", "sketch": sid, "distance": 2, "operation": "new"}));
    let ring = s.built.bodies.last().unwrap().solids.iter().map(|x| x.volume()).sum::<f64>();
    assert!((ring - (PI * 600.0 - PI * 25.0) * 2.0).abs() < 1e-8, "{ring}");
}

#[test]
fn an_ellipse_revolves_lofts_and_sweeps() {
    // Revolved about the sketch's Y axis, 40 mm away: a ring of volume 2 pi R A.
    let mut s = Session::default();
    let (sid, _) = sketch(&mut s, "XZ", 0.0, json!([{"type": "ellipse", "center": [40, 0], "radii": [10, 5]}]));
    cmd(&mut s, json!({"op": "revolve", "sketch": sid, "axis": "y", "angle": 360}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let ring = 2.0 * PI * 40.0 * PI * 50.0;
    assert!(within(volume(&s), ring, 1e-9), "{} is not {ring}", volume(&s));

    // A circle lofted to an ellipse above it is a straight-walled funnel however the ellipse
    // is turned: each point of the circle is joined to the one in the same direction, so a
    // slice at height t is an ellipse of radii 10 and 10 + 10 t. Joined a quarter turn out
    // it would hold a third less.
    let funnel = 30.0 * PI * 10.0 * 15.0;
    for turned in [0.0, 30.0, 90.0, 135.0] {
        let mut s = Session::default();
        let (low, _) = sketch(&mut s, "XY", 0.0, json!([{"type": "circle", "center": [0, 0], "radius": 10}]));
        let (high, _) = sketch(&mut s, "XY", 30.0, json!([{"type": "ellipse", "center": [0, 0], "radii": [20, 10], "rotation": turned}]));
        cmd(&mut s, json!({"op": "loft", "sections": [low, high], "ruled": true}));
        assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
        assert!(within(volume(&s), funnel, 1e-6), "turned {turned}: {} is not {funnel}", volume(&s));
        // The same with the ellipse first.
        cmd(&mut s, json!({"op": "undo"}));
        cmd(&mut s, json!({"op": "loft", "sections": [high, low], "ruled": true}));
        assert!(within(volume(&s), funnel, 1e-6), "ellipse first, turned {turned}: {}", volume(&s));
    }
    let mut s = Session::default();
    let (low, _) = sketch(&mut s, "XY", 0.0, json!([{"type": "circle", "center": [0, 0], "radius": 10}]));
    let (high, _) = sketch(&mut s, "XY", 30.0, json!([{"type": "ellipse", "center": [0, 0], "radii": [20, 10], "rotation": 90}]));
    cmd(&mut s, json!({"op": "loft", "sections": [low, high], "ruled": true}));
    let (lo, hi) = s.built.bodies[0].mesh.bbox().unwrap();
    // Measured on the display triangles, which sit a little inside the curve.
    assert!((hi.y - 20.0).abs() < 5e-3 && (lo.y + 20.0).abs() < 5e-3 && (hi.x - 10.0).abs() < 5e-3, "it reaches the ellipse, turned a quarter: {lo:?} {hi:?}");

    // An ellipse comes to a point as a cone does.
    cmd(&mut s, json!({"op": "undo"}));
    let (apex, out) = sketch(&mut s, "XY", 60.0, json!([{"type": "point", "at": [0, 0]}]));
    let tip = out["items"][0]["points"][0].as_u64().unwrap();
    cmd(&mut s, json!({"op": "loft", "sections": [high, {"sketch": apex, "point": tip}], "ruled": true}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    // The tip is a flat a thousandth of a millimetre in radius, which adds pi h (a + b) r / 6.
    let cone = PI * 20.0 * 10.0 * 30.0 / 3.0 + PI * 30.0 * 30.0 * 1e-3 / 6.0;
    assert!(within(volume(&s), cone, 1e-6), "{} is not {cone}", volume(&s));

    // An elliptical profile swept along a line.
    let mut s = Session::default();
    let (path, _) = sketch(&mut s, "XY", 0.0, json!([{"type": "line", "from": [0, 0], "to": [50, 0]}]));
    let (profile, _) = sketch(&mut s, "YZ", 0.0, json!([{"type": "ellipse", "center": [0, 0], "radii": [6, 3]}]));
    cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(within(volume(&s), PI * 18.0 * 50.0, 1e-9), "{}", volume(&s));
}

#[test]
fn an_ellipse_can_be_a_sweep_path() {
    // A 2 mm circle carried once round a 30 by 20 ellipse: its area times the ellipse's perimeter.
    let mut s = Session::default();
    let (path, _) = sketch(&mut s, "XY", 0.0, json!([{"type": "ellipse", "center": [0, 0], "radii": [30, 20]}]));
    let (profile, _) = sketch(&mut s, "XZ", 0.0, json!([{"type": "circle", "center": [30, 0], "radius": 2}]));
    cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let perimeter = PI * (3.0 * 50.0 - ((3.0 * 30.0 + 20.0) * (30.0 + 3.0 * 20.0) as f64).sqrt());
    assert!(within(volume(&s), PI * 4.0 * perimeter, 5e-3), "{} is not near {}", volume(&s), PI * 4.0 * perimeter);
}

#[test]
fn a_design_with_an_ellipse_is_format_16_and_reopens() {
    let mut s = Session::default();
    let (sid, _) = sketch(&mut s, "XY", 0.0, json!([{"type": "ellipse", "center": [0, 0], "radii": [12, 7], "rotation": -15}]));
    cmd(&mut s, json!({"op": "extrude", "sketch": sid, "distance": 3}));
    let dir = std::env::temp_dir().join(format!("ferrender-ellipse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ellipse.ferr");
    cmd(&mut s, json!({"op": "save", "path": path.to_str().unwrap(), "cache": false}));
    assert!(std::fs::read_to_string(&path).unwrap().contains("\"version\": 16"), "an ellipse needs format 16");
    let again = Session::open(&path).unwrap();
    assert!(again.built.errors.is_empty(), "{:?}", again.built.errors);
    assert!((volume(&again) - volume(&s)).abs() < 1e-9);
    assert_eq!(again.doc.sketch(sid), s.doc.sketch(sid));
    // Without the ellipse the design goes back to an older format.
    let mut plain = Session::default();
    let (sid, _) = sketch(&mut plain, "XY", 0.0, json!([{"type": "circle", "center": [0, 0], "radius": 5}]));
    cmd(&mut plain, json!({"op": "extrude", "sketch": sid, "distance": 3}));
    let path = dir.join("circle.ferr");
    cmd(&mut plain, json!({"op": "save", "path": path.to_str().unwrap(), "cache": false}));
    assert!(!std::fs::read_to_string(&path).unwrap().contains("\"version\": 16"));
}
