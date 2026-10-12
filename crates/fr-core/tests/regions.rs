//! Regions of a sketch where lines meet along each other or cross without a shared point.

use fr_core::api::execute;
use fr_core::profile::profiles;
use fr_core::{Id, Session};
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    execute(s, &c, None).unwrap_or_else(|e| panic!("{c} failed: {e}"))
}

fn sketch(s: &mut Session, items: J) -> Id {
    cmd(s, json!({"op": "create_sketch", "plane": "XY"}));
    let id = s.doc.sketches().last().unwrap().0.id;
    cmd(s, json!({"op": "add_geometry", "sketch": id, "items": items}));
    id
}

fn areas(s: &Session, sid: Id) -> Vec<f64> {
    let mut a: Vec<f64> = profiles(s.doc.sketch(sid).unwrap()).iter().map(|p| (fr_core::profile::signed_area(&p.outer) - p.holes.iter().map(|h| fr_core::profile::signed_area(h).abs()).sum::<f64>()).round()).collect();
    a.sort_by(|x, y| x.total_cmp(y));
    a
}

#[test]
fn a_table_top_with_four_corner_squares_is_five_regions() {
    // A 210 by 120 rectangle with a 10 mm square in each corner, each square sharing the
    // corner point and lying along two edges: the squares and the rest of the top.
    let mut s = Session::default();
    let sid = sketch(&mut s, json!([
        {"type": "rect", "from": [-105, -60], "to": [105, 60]},
        {"type": "rect", "from": [-105, 50], "to": [-95, 60]},
        {"type": "rect", "from": [95, 50], "to": [105, 60]},
        {"type": "rect", "from": [95, -60], "to": [105, -50]},
        {"type": "rect", "from": [-105, -60], "to": [-95, -50]},
    ]));
    assert_eq!(areas(&s, sid), vec![100.0, 100.0, 100.0, 100.0, 210.0 * 120.0 - 400.0]);
    // Extruding every region makes one slab of the whole top.
    cmd(&mut s, json!({"op": "extrude", "sketch": sid, "distance": 13, "profiles": "all"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let v: f64 = s.built.bodies.iter().map(|b| b.solids.iter().map(|x| x.volume()).sum::<f64>()).sum();
    assert!((v - 210.0 * 120.0 * 13.0).abs() < 1e-3, "{v}");
    // A square's region is told from the top's by its edges: both carry the shared edges.
    let regions = profiles(s.doc.sketch(sid).unwrap());
    let small: Vec<_> = regions.iter().filter(|p| fr_core::profile::signed_area(&p.outer).round() == 100.0).collect();
    assert_eq!(small.len(), 4);
    assert!(small.iter().all(|p| p.edges.len() == 6), "a corner square lies on two of the top's edges: {:?}", small.iter().map(|p| p.edges.len()).collect::<Vec<_>>());
}

#[test]
fn lines_drawn_across_a_shape_split_it_where_they_cross() {
    // A cross through a square with no shared points: four regions.
    let mut s = Session::default();
    let sid = sketch(&mut s, json!([
        {"type": "rect", "from": [0, 0], "to": [20, 20]},
        {"type": "line", "from": [-5, 10], "to": [25, 10]},
        {"type": "line", "from": [10, -5], "to": [10, 25]},
    ]));
    assert_eq!(areas(&s, sid), vec![100.0, 100.0, 100.0, 100.0]);
    // A line from one edge's middle to another's, ending on the edges without their points: two regions.
    let sid = sketch(&mut s, json!([
        {"type": "rect", "from": [0, 0], "to": [20, 20]},
        {"type": "line", "from": [0, 5], "to": [20, 15]},
    ]));
    assert_eq!(areas(&s, sid), vec![200.0, 200.0]);
    // Two overlapping rectangles: three regions, and the lens between them is one.
    let sid = sketch(&mut s, json!([
        {"type": "rect", "from": [0, 0], "to": [20, 20]},
        {"type": "rect", "from": [10, 10], "to": [30, 30]},
    ]));
    assert_eq!(areas(&s, sid), vec![100.0, 300.0, 300.0]);
    // A lone rectangle is still one region, and a circle inside it is still a hole.
    let sid = sketch(&mut s, json!([
        {"type": "rect", "from": [0, 0], "to": [20, 20]},
        {"type": "circle", "center": [10, 10], "radius": 2},
    ]));
    let regions = profiles(s.doc.sketch(sid).unwrap());
    assert_eq!(regions.iter().filter(|p| p.depth == 0).count(), 1);
    assert_eq!(regions.iter().find(|p| p.depth == 0).unwrap().holes.len(), 1);
}

use fr_core::profile::Seg;
use std::f64::consts::PI;

/// The exact area of each region: what one unit of extrusion of it alone holds.
fn exact_areas(s: &mut Session, sid: Id) -> Vec<f64> {
    let count = profiles(s.doc.sketch(sid).unwrap()).len();
    let mut out: Vec<f64> = (0..count).map(|i| {
        cmd(s, json!({"op": "extrude", "sketch": sid, "distance": 1, "profiles": [i], "operation": "new"}));
        assert!(s.built.errors.is_empty(), "region {i}: {:?}", s.built.errors);
        let v = s.built.bodies.last().unwrap().solids.iter().map(|x| x.volume()).sum::<f64>();
        cmd(s, json!({"op": "undo"}));
        v
    }).collect();
    out.sort_by(|x, y| x.total_cmp(y));
    out
}

fn near(a: &[f64], b: &[f64], tol: f64) -> bool { a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol) }

#[test]
fn a_circle_is_cut_where_geometry_ends_on_it_and_not_otherwise() {
    // A chord whose ends are on the circle makes two regions, each bounded by the chord
    // and its own part of the circle, and they are told apart.
    let mut s = Session::default();
    let sid = sketch(&mut s, json!([
        {"type": "circle", "center": [0, 0], "radius": 10},
        {"type": "line", "from": [-6, 8], "to": [6, 8]},
    ]));
    let cap = 100.0 * (0.8f64).acos() - 48.0;
    let got = exact_areas(&mut s, sid);
    assert!(near(&got, &[cap, 100.0 * PI - cap], 1e-7), "{got:?}");
    let regions = profiles(s.doc.sketch(sid).unwrap());
    assert_ne!(regions[0].edges, regions[1].edges, "two regions with the same entities around them are still told apart");
    assert!(s.doc.sketch(sid).unwrap().open_endpoints().is_empty(), "an end on a curve is not an open end");
    cmd(&mut s, json!({"op": "extrude", "sketch": sid, "distance": 2, "profiles": "all"}));
    let whole: f64 = s.built.bodies.iter().map(|b| b.solids.iter().map(|x| x.volume()).sum::<f64>()).sum();
    assert!((whole - 200.0 * PI).abs() < 1e-7, "{whole}");

    // A point on the circle, or a line that only starts there and leads nowhere, leaves it one circle.
    for stray in [json!({"type": "point", "at": [10, 0]}), json!({"type": "line", "from": [0, 10], "to": [0, 25]})] {
        let mut s = Session::default();
        let sid = sketch(&mut s, json!([{"type": "circle", "center": [0, 0], "radius": 10}, {"type": "point", "at": [-10, 0]}, stray]));
        let regions = profiles(s.doc.sketch(sid).unwrap());
        assert_eq!(regions.len(), 1);
        assert!(matches!(regions[0].path[..], [Seg::Circle(..)]), "still one whole circle: {:?}", regions[0].path);
    }

    // Two circles that share a point at each crossing: a lens and two crescents.
    let mut s = Session::default();
    let sid = sketch(&mut s, json!([
        {"type": "circle", "center": [0, 0], "radius": 5},
        {"type": "circle", "center": [6, 0], "radius": 5},
        {"type": "point", "at": [3, 4]},
        {"type": "point", "at": [3, -4]},
    ]));
    let lens = 2.0 * (25.0 * (0.6f64).acos() - 12.0);
    let got = exact_areas(&mut s, sid);
    assert!(near(&got, &[lens, 25.0 * PI - lens, 25.0 * PI - lens], 1e-7), "{got:?}");
    let regions = profiles(s.doc.sketch(sid).unwrap());
    let mut ids: Vec<&Vec<Id>> = regions.iter().map(|p| &p.edges).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "all three are told apart");
}

#[test]
fn an_arc_is_cut_where_a_line_ends_on_it() {
    // A half disc, then a line from the middle of its straight side up to its arc: two quarters.
    let mut s = Session::default();
    let sid = sketch(&mut s, json!([
        {"type": "arc", "center": [0, 0], "start": [10, 0], "end": [-10, 0]},
        {"type": "line", "from": [-10, 0], "to": [10, 0]},
    ]));
    assert!(near(&exact_areas(&mut s, sid), &[50.0 * PI], 1e-7));
    cmd(&mut s, json!({"op": "add_geometry", "sketch": sid, "items": [{"type": "line", "from": [0, 0], "to": [0, 10]}]}));
    let got = exact_areas(&mut s, sid);
    assert!(near(&got, &[25.0 * PI, 25.0 * PI], 1e-7), "{got:?}");
    assert!(s.doc.sketch(sid).unwrap().open_endpoints().is_empty());
}

#[test]
fn shapes_drawn_up_to_an_ellipse_close_against_it() {
    let (a, b) = (33.583f64, 22.084f64);
    let on = |deg: f64| [a * deg.to_radians().cos(), b * deg.to_radians().sin()];
    // An arc standing on top of the ellipse, as drawn by hand: its two ends are on the curve.
    let mut s = Session::default();
    let sid = sketch(&mut s, json!([
        {"type": "ellipse", "center": [0, 0], "radii": [a, b]},
        {"type": "arc3", "start": on(72.0), "through": [6.0, 27.8], "end": on(108.0)},
    ]));
    let sk = s.doc.sketch(sid).unwrap().clone();
    assert!(sk.open_endpoints().is_empty(), "the arc's ends are on the ellipse, not open: {:?}", sk.open_endpoints());
    let regions = profiles(&sk);
    assert_eq!(regions.len(), 2, "the ellipse and the cap on it");
    let got = exact_areas(&mut s, sid);
    assert!((got[1] - PI * a * b).abs() < 1e-6, "the ellipse itself, to its exact area: {} against {}", got[1], PI * a * b);
    assert!(got[0] > 20.0 && got[0] < 200.0, "the cap: {}", got[0]);
    // Extruded together they are one body holding both.
    cmd(&mut s, json!({"op": "extrude", "sketch": sid, "distance": 5, "profiles": "all"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies.len(), 1);
    let v = s.built.bodies[0].solids.iter().map(|x| x.volume()).sum::<f64>();
    assert!((v - 5.0 * (got[0] + got[1])).abs() < 1e-5, "{v} against {}", 5.0 * (got[0] + got[1]));
    assert_eq!(s.built.bodies[0].mesh.open_edges(), 0);

    // A spline across the inside of the ellipse, end to end on the curve: two regions with
    // the same two entities around them, which make up the whole ellipse.
    let mut s = Session::default();
    let sid = sketch(&mut s, json!([
        {"type": "ellipse", "center": [0, 0], "radii": [a, b], "rotation": 15},
    ]));
    let sk = s.doc.sketch(sid).unwrap().clone();
    let ellipse = *sk.entities.keys().next().unwrap();
    let (c, u, v) = sk.ellipse(ellipse).unwrap();
    let at = |deg: f64| { let p = c + u * deg.to_radians().cos() + v * deg.to_radians().sin(); [p.x, p.y] };
    cmd(&mut s, json!({"op": "add_geometry", "sketch": sid, "items": [{"type": "spline", "points": [at(150.0), [-8.0, 4.0], [6.0, 9.0], at(40.0)]}]}));
    let regions = profiles(s.doc.sketch(sid).unwrap());
    assert_eq!(regions.len(), 2);
    assert_ne!(regions[0].edges, regions[1].edges);
    let got = exact_areas(&mut s, sid);
    assert!((got[0] + got[1] - PI * a * b).abs() < 1e-5, "{got:?} should make up {}", PI * a * b);
}
