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
