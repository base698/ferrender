//! Pipe and emboss: a round section along paths that may leave their plane or lie on a
//! body, and shapes or lettering pressed onto curved faces.

use std::f64::consts::PI;

use fr_core::api::execute;
use fr_core::doc::FeatureKind;
use fr_core::{Id, Session};
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    execute(s, &c, None).unwrap_or_else(|e| panic!("{c} failed: {e}"))
}

fn refused(s: &mut Session, c: J) -> String {
    execute(s, &c, None).expect_err("the command should be refused")
}

fn id(out: &J, key: &str) -> Id { out[key].as_u64().unwrap() as Id }

/// A sketch on `plane` holding `items`. Returns its id.
fn sketch(s: &mut Session, plane: J, items: J) -> Id {
    let sk = id(&cmd(s, json!({"op": "create_sketch", "plane": plane})), "sketch");
    cmd(s, json!({"op": "add_geometry", "sketch": sk, "items": items}));
    sk
}

fn volume(s: &Session) -> f64 {
    s.built.bodies.iter().map(|b| b.solids.iter().map(|x| x.volume()).sum::<f64>()).sum()
}

fn clean(s: &Session) { assert!(s.built.errors.is_empty(), "{:?}", s.built.errors); }

fn near(a: f64, b: f64, rel: f64) { assert!((a - b).abs() <= rel * b.abs(), "{a} is not within {rel} of {b}"); }

fn ball(s: &mut Session, radius: f64) -> Id {
    id(&cmd(s, json!({"op": "primitive", "type": "sphere", "diameter": 2.0 * radius})), "feature")
}

#[test]
fn a_straight_pipe_is_a_cylinder_and_a_bore_makes_it_a_tube() {
    let mut s = Session::default();
    let path = sketch(&mut s, json!("XY"), json!([{"type": "line", "from": [0, 0], "to": [40, 0]}]));
    let out = cmd(&mut s, json!({"op": "pipe", "path": path, "diameter": 10}));
    let pipe = id(&out, "feature");
    clean(&s);
    assert_eq!(s.built.bodies.len(), 1);
    near(volume(&s), PI * 25.0 * 40.0, 1e-4);
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": pipe}));
    assert_eq!(info["type"], "pipe");
    assert_eq!(info["diameter"]["value"], json!(10.0));
    assert_eq!(info["path"][0]["sketch"], json!(path));
    assert_eq!(info["operation"], "new");

    cmd(&mut s, json!({"op": "edit_feature", "feature": pipe, "bore": 6}));
    clean(&s);
    near(volume(&s), PI * (25.0 - 9.0) * 40.0, 1e-4);
    cmd(&mut s, json!({"op": "edit_feature", "feature": pipe, "bore": false, "diameter": 4}));
    clean(&s);
    near(volume(&s), PI * 4.0 * 40.0, 1e-4);
    assert!(refused(&mut s, json!({"op": "edit_feature", "feature": pipe, "bore": 5})).contains("bore"));
}

#[test]
fn corners_get_ball_joints_and_tight_bends_are_refused() {
    let mut s = Session::default();
    let path = sketch(&mut s, json!("XY"), json!([{"type": "rect", "from": [0, 0], "to": [20, 20]}]));
    cmd(&mut s, json!({"op": "pipe", "path": path, "diameter": 2}));
    clean(&s);
    assert_eq!(s.built.bodies.len(), 1);
    assert_eq!(s.built.bodies[0].solids.len(), 1, "one closed frame");
    // Four sides, overlapping inside each corner, where the ball fills the outside of the bend.
    let sides = PI * 1.0 * 80.0;
    let v = volume(&s);
    assert!(v > sides * 0.98 && v < sides + 4.0 * (4.0 / 3.0 * PI) / 4.0 + 1e-6, "{v}");

    let mut s = Session::default();
    let path = sketch(&mut s, json!("XY"), json!([{"type": "arc", "center": [0, 0], "start": [5, 0], "end": [0, 5]}]));
    let e = refused(&mut s, json!({"op": "pipe", "path": path, "diameter": 12}));
    assert!(e.contains("too thick"), "{e}");
}

#[test]
fn four_arcs_on_four_planes_cut_a_tennis_ball_seam() {
    let mut s = Session::default();
    s.doc.units = fr_core::units::Unit::Mm;
    let r = 33.0;
    ball(&mut s, r);
    let before = volume(&s);
    // Circles round the corners of a tetrahedron, 54.7 degrees out: four 120 degree
    // arcs between the edge midpoints, joined smoothly.
    let v = [[1.0, 1.0, 1.0], [1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, -1.0, 1.0]].map(|p| glam::DVec3::from_array(p) / 3f64.sqrt());
    let (a1, a2, b1, b2) = (v[0], v[1], v[2], v[3]);
    let mid = |p: glam::DVec3, q: glam::DVec3| (p + q).normalize() * r;
    let mut runs = Vec::new();
    for (axis, from, to, other) in [(a1, mid(a1, b1), mid(a1, b2), a2), (b2, mid(a1, b2), mid(a2, b2), b1), (a2, mid(a2, b2), mid(a2, b1), a1), (b1, mid(a2, b1), mid(a1, b1), b2)] {
        let origin = axis * r / 3f64.sqrt();
        let x = (from - origin).normalize();
        let y = axis.cross(x);
        let flat = |p: glam::DVec3| json!([(p - origin).dot(x), (p - origin).dot(y)]);
        let away = -(other - axis * other.dot(axis)).normalize();
        let through = origin + away * r * (2.0f64 / 3.0).sqrt();
        runs.push(sketch(&mut s, json!({"origin": origin.to_array(), "normal": axis.to_array(), "x": x.to_array()}), json!([{"type": "arc3", "start": flat(from), "through": flat(through), "end": flat(to)}])));
    }
    let out = cmd(&mut s, json!({"op": "pipe", "path": runs, "diameter": 3, "operation": "cut"}));
    clean(&s);
    assert_eq!(s.built.bodies.len(), 1);
    // The seam is 4 x 120 degrees of circles of radius r * sqrt(2/3); about half the pipe is inside the ball.
    let length = 4.0 * (2.0 * PI / 3.0) * r * (2.0f64 / 3.0).sqrt();
    let removed = before - volume(&s);
    let tube = PI * 1.5 * 1.5 * length;
    assert!(removed > 0.45 * tube && removed < 0.55 * tube, "{removed} of {tube}");
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": id(&out, "feature")}));
    assert_eq!(info["path"].as_array().unwrap().len(), 4);
}

#[test]
fn a_circle_above_a_ball_lays_onto_it_as_a_ring() {
    let mut s = Session::default();
    let b = ball(&mut s, 20.0);
    let before = volume(&s);
    let ring = sketch(&mut s, json!({"origin": [0, 0, 40], "normal": [0, 0, 1], "x": [1, 0, 0]}), json!([{"type": "circle", "center": [0, 0], "radius": 10}]));
    let out = cmd(&mut s, json!({"op": "pipe", "path": [{"sketch": ring, "onto": b}], "diameter": 2, "operation": "join"}));
    clean(&s);
    assert_eq!(s.built.bodies.len(), 1);
    // A circle of radius 10 at the height where the ball is that wide; the half outside the ball is added.
    let tube = PI * 1.0 * (2.0 * PI * 10.0);
    let added = volume(&s) - before;
    assert!(added > 0.4 * tube && added < 0.6 * tube, "{added} of {tube}");
    let top = s.built.bodies[0].mesh.bbox().unwrap().1.z;
    assert!((top - 20.0).abs() < 1e-3, "the ball's top is untouched: {top}");
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": id(&out, "feature")}));
    assert_eq!(info["path"][0]["onto"], json!(b));
    // Laid from below instead, it rings the bottom half.
    cmd(&mut s, json!({"op": "edit_feature", "feature": id(&out, "feature"), "path": [{"sketch": ring, "onto": b, "flip": true}]}));
    clean(&s);
    // A path that misses the body is refused.
    let wide = sketch(&mut s, json!({"origin": [0, 0, 40], "normal": [0, 0, 1], "x": [1, 0, 0]}), json!([{"type": "circle", "center": [0, 0], "radius": 25}]));
    let e = refused(&mut s, json!({"op": "pipe", "path": [{"sketch": wide, "onto": b}], "diameter": 2}));
    assert!(e.contains("misses the body"), "{e}");
}

#[test]
fn lines_and_wavy_curves_laid_onto_bodies_climb_with_the_surface() {
    // A line over a ball climbs and falls with it; the section stays square to the path.
    let mut s = Session::default();
    let b = ball(&mut s, 20.0);
    let before = volume(&s);
    let line = sketch(&mut s, json!({"origin": [0, 0, 40], "normal": [0, 0, 1], "x": [1, 0, 0]}), json!([{"type": "line", "from": [-10, -3], "to": [10, 5]}]));
    cmd(&mut s, json!({"op": "pipe", "path": [{"sketch": line, "onto": b}], "diameter": 2, "operation": "cut"}));
    clean(&s);
    assert!(before - volume(&s) > 20.0, "a groove is cut: {}", before - volume(&s));

    // A spline bowed one way, laid round the side of a can.
    let mut s = Session::default();
    let can = id(&cmd(&mut s, json!({"op": "primitive", "type": "cylinder", "diameter": 20, "height": 30})), "feature");
    let before = volume(&s);
    let bow = sketch(&mut s, json!("XZ"), json!([{"type": "spline", "points": [[-6, 8], [-2, 14], [2, 15], [6, 9]]}]));
    cmd(&mut s, json!({"op": "pipe", "path": [{"sketch": bow, "onto": can}], "diameter": 1.5, "operation": "join"}));
    clean(&s);
    assert_eq!(s.built.bodies.len(), 1);
    let lo = s.built.bodies[0].mesh.bbox().unwrap().0;
    assert!((lo.y + 10.75).abs() < 0.05, "the pipe rides on the side facing -Y: {}", lo.y);
    // About half the tube stands outside the can.
    let length = 2.0 * 14.0;
    let half = PI * 0.75 * 0.75 * length / 2.0;
    let added = volume(&s) - before;
    assert!(added > 0.6 * half && added < 1.3 * half, "{added} of about {half}");

    // A wave that bends both ways defeats the kernel's Booleans against the can. It is
    // refused, never built wrong. (On a fresh can: laid over the bow, it would land on the bow.)
    let mut s = Session::default();
    let can = id(&cmd(&mut s, json!({"op": "primitive", "type": "cylinder", "diameter": 20, "height": 30})), "feature");
    let wave = sketch(&mut s, json!("XZ"), json!([{"type": "spline", "points": [[-6, 10], [-2, 15], [2, 12], [6, 18]]}]));
    let v = volume(&s);
    match execute(&mut s, &json!({"op": "pipe", "path": [{"sketch": wave, "onto": can}], "diameter": 1.5, "operation": "join"}), None) {
        Ok(_) => { let added = volume(&s) - v; assert!(added > 0.6 * half && added < 1.3 * half, "{added}"); }
        Err(e) => assert!(e.contains("could not combine") || e.contains("wrong volume"), "{e}"),
    }
    // A wave whose peaks are tighter than the pipe is refused, naming the bend.
    let sharp = sketch(&mut s, json!("XZ"), json!([{"type": "spline", "points": [[-6, 8], [-2, 18], [2, 10], [6, 22]]}]));
    let e = refused(&mut s, json!({"op": "pipe", "path": [{"sketch": sharp, "onto": can}], "diameter": 4}));
    assert!(e.contains("too thick"), "{e}");
}

#[test]
fn emboss_raises_and_sinks_a_sketch_on_a_cylinder() {
    let mut s = Session::default();
    let can = id(&cmd(&mut s, json!({"op": "primitive", "type": "cylinder", "diameter": 20, "height": 30})), "feature");
    let before = volume(&s);
    // XZ faces -Y, so the shape lands on the side of the can facing -Y.
    let mark = sketch(&mut s, json!("XZ"), json!([{"type": "rect", "from": [-3, 10], "to": [3, 20]}]));
    let out = cmd(&mut s, json!({"op": "emboss", "sketch": mark, "body": can, "depth": 1}));
    let emboss = id(&out, "feature");
    clean(&s);
    assert_eq!(s.built.bodies.len(), 1);
    let (lo, hi) = s.built.bodies[0].mesh.bbox().unwrap();
    assert!((lo.y + 11.0).abs() < 1e-3, "raised 1 off the side facing -Y: {}", lo.y);
    assert!((hi.y - 10.0).abs() < 1e-3, "the far side is untouched: {}", hi.y);
    // The band between radius 10 and 11 under a 6 wide, 10 tall square: a little over 60.
    let added = volume(&s) - before;
    assert!(added > 60.0 && added < 66.0, "{added}");
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": emboss}));
    assert_eq!(info["type"], "emboss");
    assert_eq!(info["body"], json!(can));

    cmd(&mut s, json!({"op": "edit_feature", "feature": emboss, "operation": "cut"}));
    clean(&s);
    let removed = before - volume(&s);
    assert!(removed > 60.0 && removed < 64.0, "{removed}");
    cmd(&mut s, json!({"op": "edit_feature", "feature": emboss, "operation": "join", "flip": true}));
    clean(&s);
    let (lo, hi) = s.built.bodies[0].mesh.bbox().unwrap();
    assert!((hi.y - 11.0).abs() < 1e-3 && (lo.y + 10.0).abs() < 1e-3, "flipped onto the side facing +Y");
    assert!(refused(&mut s, json!({"op": "edit_feature", "feature": emboss, "operation": "intersect"})).contains("emboss"));

    // A shape wider than the can reaches past it.
    let wide = sketch(&mut s, json!("XZ"), json!([{"type": "rect", "from": [-15, 10], "to": [15, 20]}]));
    let e = refused(&mut s, json!({"op": "emboss", "sketch": wide, "body": can, "depth": 1}));
    assert!(e.contains("reach past"), "{e}");
}

#[test]
fn text_raises_on_a_ball_and_engraves_into_it() {
    let mut s = Session::default();
    let b = ball(&mut s, 20.0);
    let slab = id(&cmd(&mut s, json!({"op": "primitive", "type": "box", "width": 30, "depth": 30, "height": 5, "position": [40, 0, 0]})), "feature");
    let ball_volume = |s: &Session| s.built.body(b).unwrap().solids.iter().map(|x| x.volume()).sum::<f64>();
    let before = ball_volume(&s);
    let out = cmd(&mut s, json!({"op": "text", "text": "HI", "operation": "join", "body": b, "face": [0, -20, 0], "height": 8, "depth": 1, "align": "center"}));
    let text = id(&out, "feature");
    clean(&s);
    assert!(ball_volume(&s) > before);
    let lo = s.built.body(b).unwrap().mesh.bbox().unwrap().0;
    assert!(lo.y < -20.5 && lo.y > -21.01, "raised about 1 off the front: {}", lo.y);
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": text}));
    assert_eq!(info["surface"], "curved");
    let FeatureKind::Text(t) = &s.doc.feature(text).unwrap().kind else { panic!("text") };
    assert!(t.curved);

    cmd(&mut s, json!({"op": "edit_feature", "feature": text, "operation": "cut"}));
    clean(&s);
    assert!(ball_volume(&s) < before);
    let e = refused(&mut s, json!({"op": "edit_feature", "feature": text, "height": 60}));
    assert!(e.contains("reach past"), "{e}");
    // Moved to a flat face, it is flat text again.
    cmd(&mut s, json!({"op": "edit_feature", "feature": text, "body": slab, "face": [55, 15, 5]}));
    clean(&s);
    let FeatureKind::Text(t) = &s.doc.feature(text).unwrap().kind else { panic!("text") };
    assert!(!t.curved);
}

#[test]
fn pipes_and_embossing_save_and_open() {
    let mut s = Session::default();
    let b = ball(&mut s, 20.0);
    let ring = sketch(&mut s, json!({"origin": [0, 0, 40], "normal": [0, 0, 1], "x": [1, 0, 0]}), json!([{"type": "circle", "center": [0, 0], "radius": 10}]));
    cmd(&mut s, json!({"op": "pipe", "path": [{"sketch": ring, "onto": b}], "diameter": 2, "operation": "cut"}));
    let mark = sketch(&mut s, json!("XZ"), json!([{"type": "rect", "from": [-3, -3], "to": [3, 3]}]));
    cmd(&mut s, json!({"op": "emboss", "sketch": mark, "body": b, "depth": 1}));
    cmd(&mut s, json!({"op": "text", "text": "A", "operation": "cut", "body": b, "face": [20, 0, 0], "height": 6, "depth": 0.5}));
    clean(&s);
    let v = volume(&s);
    let dir = std::env::temp_dir().join(format!("ferrender-pipe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pipe.ferr");
    cmd(&mut s, json!({"op": "save", "path": path.to_str().unwrap(), "cache": false}));
    assert!(std::fs::read_to_string(&path).unwrap().contains("\"version\": 20"));
    let again = Session::open(&path).unwrap();
    clean(&again);
    near(volume(&again), v, 1e-6);
    assert_eq!(again.doc.features.len(), s.doc.features.len());
}
