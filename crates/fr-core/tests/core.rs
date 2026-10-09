use fr_core::api::execute;
use fr_core::expr::Kind;
use fr_core::profile::profiles;
use fr_core::sketch::Geom;
use fr_core::{CKind, Document, Plane, Session, Sketch, Unit, io, solver};
use glam::DVec2;
use serde_json::{Value as J, json};

fn run(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3 * b.abs().max(1.0)
}

#[test]
fn expressions_units_and_parameters() {
    let mut d = Document::new(Unit::Mm);
    assert_eq!(d.eval("10", Kind::Length).unwrap(), 10.0);
    assert_eq!(d.eval("2 in", Kind::Length).unwrap(), 50.8);
    assert_eq!(d.eval("1cm + 5", Kind::Length).unwrap(), 15.0);
    assert_eq!(d.eval("1\"", Kind::Length).unwrap(), 25.4);
    assert!(d.eval("10 mm", Kind::Angle).is_err());
    assert!(d.eval("2 mm * 3 mm", Kind::Length).is_err());
    assert!(d.eval("$nope", Kind::Length).is_err());

    // `d = 10mm` in a box defines the parameter and uses it.
    let v = d.enter("d=10mm", Kind::Length).unwrap();
    assert_eq!((v.expr.as_str(), v.v), ("$d", 10.0));
    assert_eq!(d.eval("$d * 2 + 1 in", Kind::Length).unwrap(), 45.4);
    assert_eq!(d.eval("d / 4", Kind::Length).unwrap(), 2.5);
    assert_eq!(d.eval("$d / 5mm", Kind::Scalar).unwrap(), 2.0);
    assert!(d.set_param("mm", "3").is_err());
    assert!(d.set_param("d", "$d + 1").is_err(), "a parameter cannot refer to itself");
    assert_eq!(d.eval("$d", Kind::Length).unwrap(), 10.0);

    // Bare numbers are pinned to the units they were typed in.
    d.units = Unit::In;
    let v = d.value("2", Kind::Length).unwrap();
    assert_eq!((v.expr.as_str(), v.v), ("2 in", 50.8));
    d.units = Unit::Mm;
    assert_eq!(d.eval(&v.expr, Kind::Length).unwrap(), 50.8);
}

#[test]
fn solver_holds_constraints_and_counts_freedom() {
    let mut sk = Sketch::new(Plane::XY);
    let (a, c) = (sk.point_at(DVec2::ZERO, 1e-6), sk.add_point(DVec2::new(37.0, 21.0)));
    let l = sk.add_rect(a, c, false);
    assert_eq!(solver::solve(&mut sk, &[]).dof, 2, "a rectangle on the origin can still change width and height");

    let val = |v: f64| Some(fr_core::Value { expr: format!("{v} mm"), v });
    sk.add_constraint(CKind::Distance, &[l[0]], val(50.0)).unwrap();
    sk.add_constraint(CKind::Distance, &[l[1]], val(30.0)).unwrap();
    let r = solver::solve(&mut sk, &[]);
    assert!(r.ok && r.dof == 0, "{r:?}");
    assert!(close(sk.pos(c).x, 50.0) && close(sk.pos(c).y, 30.0), "{:?}", sk.pos(c));

    // A circle tangent to the top and right sides, with a set radius.
    let cc = sk.add_point(DVec2::new(20.0, 10.0));
    let circle = sk.add(Geom::Circle { c: cc, r: 4.0 }, false);
    sk.add_constraint(CKind::Tangent, &[l[1], circle], None).unwrap();
    sk.add_constraint(CKind::Tangent, &[l[2], circle], None).unwrap();
    sk.add_constraint(CKind::Radius, &[circle], val(6.0)).unwrap();
    let r = solver::solve(&mut sk, &[]);
    assert!(r.ok && r.dof == 0, "{r:?}");
    assert!(close(sk.pos(cc).x, 44.0) && close(sk.pos(cc).y, 24.0), "{:?}", sk.pos(cc));

    // A conflicting dimension is reported, not silently bent.
    sk.add_constraint(CKind::Distance, &[l[2]], val(80.0)).unwrap();
    assert!(!solver::solve(&mut sk, &[]).ok);
}

#[test]
fn every_constraint_kind_solves() {
    let mut sk = Sketch::new(Plane::XY);
    let p: Vec<_> = [(1.0, 2.0), (30.0, 6.0), (4.0, 20.0), (33.0, 31.0), (15.0, 9.0), (50.0, 2.0), (60.0, 12.0)].iter().map(|(x, y)| sk.add_point(DVec2::new(*x, *y))).collect();
    let l1 = sk.add_line(p[0], p[1]);
    let l2 = sk.add_line(p[2], p[3]);
    let l3 = sk.add_line(p[5], p[6]);
    let c1 = sk.add(Geom::Circle { c: p[4], r: 3.0 }, false);
    let c2 = sk.add(Geom::Circle { c: p[5], r: 5.0 }, false);
    let val = |v: f64| Some(fr_core::Value { expr: String::new(), v });
    sk.add_constraint(CKind::Horizontal, &[l1], None).unwrap();
    sk.add_constraint(CKind::Parallel, &[l1, l2], None).unwrap();
    sk.add_constraint(CKind::Equal, &[l1, l2], None).unwrap();
    sk.add_constraint(CKind::Distance, &[l1, l2], val(12.0)).unwrap();
    sk.add_constraint(CKind::Midpoint, &[p[4], l1], None).unwrap();
    sk.add_constraint(CKind::Equal, &[c1, c2], None).unwrap();
    sk.add_constraint(CKind::Angle, &[l1, l3], val(45.0)).unwrap();
    sk.add_constraint(CKind::Coincident, &[p[6], l2], None).unwrap();
    sk.add_constraint(CKind::Vertical, &[p[0], p[2]], None).unwrap();
    sk.add_constraint(CKind::Fix, &[p[0]], None).unwrap();
    let r = solver::solve(&mut sk, &[]);
    assert!(r.ok, "{r:?}");
    let (a, b) = sk.line(l1).unwrap();
    let (c, d) = sk.line(l2).unwrap();
    assert!(close(a.y, b.y) && close(c.y, d.y) && close((c.y - a.y).abs(), 12.0));
    assert!(close(a.distance(b), c.distance(d)) && close(a.x, c.x));
    assert!(close(sk.pos(p[4]).x, (a.x + b.x) / 2.0));
    assert_eq!(sk.pos(p[0]), DVec2::new(1.0, 2.0), "fixed points do not move");
    assert!(close(sk.measure(CKind::Angle, &[l1, l3]), 45.0));
    assert!(close(sk.curve(c1).unwrap().1, sk.curve(c2).unwrap().1));
}

#[test]
fn dragging_respects_constraints() {
    let mut sk = Sketch::new(Plane::XY);
    let (a, c) = (sk.point_at(DVec2::ZERO, 1e-6), sk.add_point(DVec2::new(40.0, 20.0)));
    let l = sk.add_rect(a, c, false);
    sk.add_constraint(CKind::Distance, &[l[0]], Some(fr_core::Value { expr: String::new(), v: 40.0 })).unwrap();
    let r = solver::solve(&mut sk, &[(c, DVec2::new(70.0, 35.0))]);
    assert!(r.ok);
    // Width is dimensioned, so only the height follows the pointer.
    assert!(close(sk.pos(c).x, 40.0) && close(sk.pos(c).y, 35.0), "{:?}", sk.pos(c));
}

#[test]
fn profiles_nest_and_split() {
    let mut s = Session::default();
    let sk = run(&mut s, json!({"op": "create_sketch", "plane": "XY"}))["sketch"].as_u64().unwrap() as u32;
    run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [
        {"type": "rect", "from": [0, 0], "to": [40, 20]},
        {"type": "line", "from": [20, 0], "to": [20, 20]},
        {"type": "circle", "center": [10, 10], "radius": 4},
        {"type": "line", "from": [50, 0], "to": [60, 5]},
    ]}));
    // The divider's ends lie on the rectangle's sides but do not split them,
    // so the rectangle is one region with the circle nested in it.
    let p = profiles(s.doc.sketch(sk).unwrap());
    assert_eq!(p.len(), 2, "{p:?}");
    let rect = p.iter().find(|p| p.depth == 0).unwrap();
    assert!(close(rect.area(), 800.0 - std::f64::consts::PI * 16.0), "{}", rect.area());
    assert_eq!(rect.holes.len(), 1);
    assert!(p.iter().any(|p| p.depth == 1 && close(p.area(), 50.2)), "the circle is its own region");

    // Drawn so the divider shares corner points, it does split.
    let sk2 = run(&mut s, json!({"op": "create_sketch", "plane": "XY"}))["sketch"].as_u64().unwrap() as u32;
    run(&mut s, json!({"op": "add_geometry", "sketch": sk2, "items": [
        {"type": "polyline", "closed": true, "points": [[0, 0], [20, 0], [40, 0], [40, 20], [20, 20], [0, 20]]},
        {"type": "line", "from": [20, 0], "to": [20, 20]},
    ]}));
    let p = profiles(s.doc.sketch(sk2).unwrap());
    assert_eq!(p.len(), 2);
    assert!(p.iter().all(|p| close(p.area(), 400.0)));
}

#[test]
fn extrude_revolve_cut_and_stl() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "set_parameter", "name": "h", "expr": "10 mm"}));
    run(&mut s, json!({"op": "create_sketch", "plane": "XY"}));
    run(&mut s, json!({"op": "add_geometry", "items": [
        {"type": "rect", "from": [0, 0], "to": [40, 20]},
        {"type": "circle", "center": [10, 10], "radius": 4},
    ]}));
    let r = run(&mut s, json!({"op": "extrude", "distance": "$h"}));
    let body = &s.built.bodies[0];
    let plate = (800.0 - std::f64::consts::PI * 16.0) * 10.0;
    assert!((body.mesh.volume() - plate).abs() < plate * 0.002, "{} vs {plate}", body.mesh.volume());
    assert_eq!(body.mesh.open_edges(), 0, "extrusions are watertight");
    assert_eq!(r["changed_bodies"][0]["size"], json!([40.0, 20.0, 10.0]));

    // The parameter drives the height.
    run(&mut s, json!({"op": "set_parameter", "name": "h", "expr": "1 in"}));
    assert!(close(s.built.bodies[0].mesh.bbox().unwrap().1.z, 25.4));
    run(&mut s, json!({"op": "undo"}));
    assert!(close(s.built.bodies[0].mesh.bbox().unwrap().1.z, 10.0));

    // Cut a slot through it from a second sketch.
    let sk2 = run(&mut s, json!({"op": "create_sketch", "plane": "XY", "offset": 10}))["sketch"].clone();
    run(&mut s, json!({"op": "add_geometry", "sketch": sk2, "items": [{"type": "rect", "from": [25, 5], "to": [35, 15]}]}));
    run(&mut s, json!({"op": "extrude", "sketch": sk2, "distance": -4, "operation": "cut"}));
    assert_eq!(s.built.bodies.len(), 1);
    let v = s.built.bodies[0].mesh.volume();
    assert!((v - (plate - 400.0)).abs() < plate * 0.002, "{v}");
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies[0].mesh.open_edges(), 0, "a cut leaves the body watertight");

    // Lathe: a rectangle turned around the sketch's y axis is a tube.
    let sk3 = run(&mut s, json!({"op": "create_sketch", "plane": "XZ"}))["sketch"].clone();
    run(&mut s, json!({"op": "add_geometry", "sketch": sk3, "items": [{"type": "rect", "from": [60, 0], "to": [70, 30]}]}));
    run(&mut s, json!({"op": "revolve", "sketch": sk3, "axis": "y", "operation": "new"}));
    let tube = &s.built.bodies[1].mesh;
    let exact = std::f64::consts::PI * (70f64.powi(2) - 60f64.powi(2)) * 30.0;
    assert!((tube.volume() - exact).abs() < exact * 0.01, "{} vs {exact}", tube.volume());
    assert_eq!(tube.open_edges(), 0);
    let fid = s.doc.features.last().unwrap().id;
    run(&mut s, json!({"op": "edit_feature", "feature": fid, "angle": 90}));
    let quarter = &s.built.bodies[1].mesh;
    assert!((quarter.volume() - exact / 4.0).abs() < exact * 0.01);
    assert_eq!(quarter.open_edges(), 0, "a partial revolve is capped");

    // STL in each unit, and back in again.
    let dir = std::env::temp_dir().join(format!("ferrender-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("part.stl");
    let tube_id = s.built.bodies[1].id;
    for (unit, per_mm) in [("mm", 1.0), ("cm", 0.1), ("in", 1.0 / 25.4)] {
        run(&mut s, json!({"op": "export_stl", "path": path, "units": unit, "bodies": [tube_id]}));
        let m = io::read_stl(&path, Unit::Mm).unwrap();
        assert!(close(m.bbox().unwrap().1.z, 30.0 * per_mm), "{unit}");
    }
    let before = s.built.bodies.len();
    let imp = run(&mut s, json!({"op": "import_stl", "path": path, "units": "in"}));
    assert_eq!(s.built.bodies.len(), before + 1);
    assert!(close(imp["body"]["size"][2].as_f64().unwrap(), 30.0));
    let id = imp["feature"].clone();
    run(&mut s, json!({"op": "transform", "body": id, "translate": [0, 0, "2 cm"], "scale": 2}));
    assert!(close(s.built.body(id.as_u64().unwrap() as u32).unwrap().mesh.bbox().unwrap().1.z, 80.0));

    // The document survives a round trip through its file.
    let file = dir.join("part.ferr");
    run(&mut s, json!({"op": "save", "path": file}));
    let again = Session::open(&file).unwrap();
    assert_eq!(again.doc, s.doc);
    assert_eq!(again.built.bodies.len(), s.built.bodies.len());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn api_rejects_bad_input_without_changing_the_document() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "create_sketch"}));
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [10, 10]}]}));
    let before = s.doc.clone();
    for bad in [
        json!({"op": "extrude", "distance": 0}),
        json!({"op": "extrude", "distance": "5 deg"}),
        json!({"op": "extrude", "distance": 5, "operation": "cut"}),
        json!({"op": "add_constraint", "kind": "parallel", "refs": [1]}),
        json!({"op": "add_geometry", "items": [{"type": "blob"}]}),
        json!({"op": "nonsense"}),
    ] {
        assert!(execute(&mut s, &bad, None).is_err(), "{bad} should fail");
        assert_eq!(s.doc, before, "{bad} should leave the document alone");
    }
    let shot = run(&mut s, json!({"op": "get_viewport_screenshot", "width": 320, "height": 240}));
    assert!(shot["png_base64"].as_str().unwrap().len() > 500);
    let info = run(&mut s, json!({"op": "get_object_info", "id": 1}));
    assert_eq!(info["sketch"]["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(info["sketch"]["degrees_of_freedom"], 2);
}

#[test]
fn copy_and_paste_keeps_shape_and_constraints() {
    let mut sk = Sketch::new(Plane::XY);
    let (a, c) = (sk.add_point(DVec2::new(5.0, 5.0)), sk.add_point(DVec2::new(25.0, 15.0)));
    let lines = sk.add_rect(a, c, false);
    sk.add_constraint(CKind::Distance, &[lines[0]], Some(fr_core::Value { expr: "20 mm".into(), v: 20.0 })).unwrap();
    let (np, ne, nc) = (sk.points.len(), sk.entities.len(), sk.constraints.len());
    let clip = sk.copy(&lines);
    let pasted = sk.paste(&clip, DVec2::new(100.0, 0.0));
    assert_eq!(pasted.len(), 4);
    assert_eq!((sk.points.len(), sk.entities.len(), sk.constraints.len()), (np + 4, ne + 4, nc * 2));
    assert!(solver::solve(&mut sk, &[]).ok);
    assert_eq!(profiles(&sk).len(), 2);
    assert!(close(sk.line(pasted[0]).unwrap().0.x, 105.0));

    // A lone point and a lone line copy too.
    let clip = sk.copy(&[a]);
    assert_eq!(sk.paste(&clip, DVec2::new(0.0, 50.0)).len(), 1);
    let clip = sk.copy(&[lines[1]]);
    let one = sk.paste(&clip, DVec2::new(0.0, 50.0));
    assert_eq!(one.len(), 1);
    assert!(sk.line(one[0]).is_some());
}

fn id(v: &J) -> u32 {
    v.as_u64().unwrap_or_else(|| panic!("{v} is not an id")) as u32
}

fn area(s: &Session, sk: u32) -> f64 {
    profiles(s.doc.sketch(sk).unwrap()).iter().filter(|p| p.depth == 0).map(|p| p.area()).sum()
}

#[test]
fn sketch_tools_rework_geometry() {
    let mut s = Session::default();
    let sk = id(&run(&mut s, json!({"op": "create_sketch"}))["sketch"]);

    // A hexagon on the origin: only its size and turn are free.
    let hex = run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [{"type": "ngon", "center": [0, 0], "radius": 10, "sides": 6}]}));
    assert_eq!(hex["degrees_of_freedom"], 2);
    assert!(close(area(&s, sk), 1.5 * 3f64.sqrt() * 100.0), "{}", area(&s, sk));
    let side = id(&hex["items"][0]["entities"][0]);
    run(&mut s, json!({"op": "add_constraint", "sketch": sk, "kind": "distance", "refs": [side], "value": 6}));
    assert!(close(area(&s, sk), 1.5 * 3f64.sqrt() * 36.0), "a hexagon's side equals its radius");

    // Fillet one corner of a rectangle and chamfer another.
    let sk = id(&run(&mut s, json!({"op": "create_sketch"}))["sketch"]);
    let r = run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [{"type": "rect", "from": [5, 5], "to": [45, 25]}]}));
    let corners: Vec<u32> = r["items"][0]["points"].as_array().unwrap().iter().map(id).collect();
    run(&mut s, json!({"op": "fillet", "sketch": sk, "point": corners[1], "radius": 5}));
    let cut = 25.0 - std::f64::consts::PI * 25.0 / 4.0;
    assert!((area(&s, sk) - (800.0 - cut)).abs() < 0.05, "{}", area(&s, sk));
    run(&mut s, json!({"op": "chamfer", "sketch": sk, "point": corners[3], "distance": 4}));
    assert!((area(&s, sk) - (800.0 - cut - 8.0)).abs() < 0.05, "{}", area(&s, sk));
    assert!(execute(&mut s, &json!({"op": "fillet", "sketch": sk, "point": corners[0], "radius": 50}), None).is_err(), "too big a fillet is refused");
    let info = run(&mut s, json!({"op": "get_object_info", "id": sk}));
    assert!(info["sketch"]["constraints"].as_array().unwrap().iter().any(|c| c["kind"] == "radius" && c["value"] == 5.0), "the fillet carries its radius as a dimension");

    // Offset a rectangle outward by 2: a 44 x 24 rectangle that follows the original.
    let sk = id(&run(&mut s, json!({"op": "create_sketch"}))["sketch"]);
    let r = run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [{"type": "rect", "from": [0, 0], "to": [40, 20]}, {"type": "circle", "center": [60, 10], "radius": 5}]}));
    let lines = r["items"][0]["entities"].clone();
    let o = run(&mut s, json!({"op": "offset", "sketch": sk, "ids": lines, "distance": "2 mm"}));
    assert_eq!(o["new"].as_array().unwrap().len(), 4);
    let outer = profiles(s.doc.sketch(sk).unwrap()).iter().map(|p| signed(&p.outer)).fold(0.0, f64::max);
    assert!(close(outer, 44.0 * 24.0), "{outer}");
    let ring = run(&mut s, json!({"op": "offset", "sketch": sk, "ids": [r["items"][1]["entities"][0]], "distance": -2}));
    assert!(close(s.doc.sketch(sk).unwrap().curve(id(&ring["new"][0])).unwrap().1, 3.0));

    // Mirror a circle across a line; the copy follows when the original is resized.
    let sk = id(&run(&mut s, json!({"op": "create_sketch"}))["sketch"]);
    let r = run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [{"type": "circle", "center": [10, 5], "radius": 2}, {"type": "line", "from": [20, -10], "to": [20, 30]}]}));
    let (circle, axis) = (id(&r["items"][0]["entities"][0]), id(&r["items"][1]["entities"][0]));
    run(&mut s, json!({"op": "add_constraint", "sketch": sk, "kind": "fix", "refs": [axis]}));
    let m = id(&run(&mut s, json!({"op": "mirror", "sketch": sk, "ids": [circle], "axis": axis}))["new"][0]);
    let image = s.doc.sketch(sk).unwrap().curve(m).unwrap();
    assert!(close(image.0.x, 30.0) && close(image.0.y, 5.0) && close(image.1, 2.0), "{image:?}");
    run(&mut s, json!({"op": "add_constraint", "sketch": sk, "kind": "radius", "refs": [circle], "value": 4}));
    assert!(close(s.doc.sketch(sk).unwrap().curve(m).unwrap().1, 4.0));

    // Trim: a line through a circle loses its middle; the circle then loses its top.
    let sk = id(&run(&mut s, json!({"op": "create_sketch"}))["sketch"]);
    let r = run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [{"type": "line", "from": [-10, 1], "to": [10, 1]}, {"type": "circle", "center": [0, 0], "radius": 5}]}));
    let (line, circle) = (id(&r["items"][0]["entities"][0]), id(&r["items"][1]["entities"][0]));
    run(&mut s, json!({"op": "trim", "sketch": sk, "entity": line, "near": [0, 1]}));
    let sketch = s.doc.sketch(sk).unwrap();
    let lines: Vec<_> = sketch.entities.keys().filter_map(|e| sketch.line(*e)).collect();
    assert_eq!(lines.len(), 2, "the line is now two stubs outside the circle");
    assert!(lines.iter().all(|l| l.0.x.abs() >= 4.89 && l.1.x.abs() >= 4.89), "{lines:?}");
    run(&mut s, json!({"op": "trim", "sketch": sk, "entity": circle, "near": [0, 5]}));
    let sketch = s.doc.sketch(sk).unwrap();
    assert!(matches!(sketch.entities[&circle].geom, Geom::Arc { .. }), "the circle became an arc");
    let (_, sweep) = sketch.arc_angles(circle).unwrap();
    assert!(sweep > std::f64::consts::PI && sweep < std::f64::consts::TAU, "the arc keeps the lower part: {sweep}");
    assert_eq!(profiles(sketch).len(), 0, "the stubs and the arc do not close a region");
    run(&mut s, json!({"op": "trim", "sketch": sk, "entity": circle, "near": [0, -5]}));
    assert!(!s.doc.sketch(sk).unwrap().entities.contains_key(&circle), "with nothing crossing it, the rest goes");
}

fn signed(p: &[DVec2]) -> f64 {
    fr_core::profile::signed_area(p)
}

#[test]
fn faces_extents_taper_and_patterns() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "create_sketch"}));
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "circle", "center": [0, 0], "radius": 20}]}));
    let disc = id(&run(&mut s, json!({"op": "extrude", "distance": 4}))["feature"]);
    let vol = |s: &Session| s.built.bodies.iter().map(|b| b.mesh.volume()).sum::<f64>();
    let base = vol(&s);

    // Project the disc's top face into a sketch above it: one fixed circle.
    let sk = id(&run(&mut s, json!({"op": "create_sketch", "face": {"body": disc, "point": [0, 0, 4]}, "offset": 2}))["sketch"]);
    assert!(close(s.doc.sketch(sk).unwrap().plane.origin.z, 6.0), "an offset plane above the face");
    let p = run(&mut s, json!({"op": "project", "sketch": sk, "body": disc, "point": [0, 0, 4]}));
    assert_eq!(p["new"].as_array().unwrap().len(), 1);
    assert_eq!(p["degrees_of_freedom"], 0, "projected geometry is fixed");
    assert!(close(s.doc.sketch(sk).unwrap().curve(id(&p["new"][0])).unwrap().1, 20.0));

    // One hole cut through everything, then patterned six times around Z.
    run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [{"type": "circle", "center": [12, 0], "radius": 2}]}));
    let hole = run(&mut s, json!({"op": "get_object_info", "id": sk}))["sketch"]["profiles"].as_array().unwrap().iter().position(|p| p["area"].as_f64().unwrap() < 20.0).unwrap();
    let cut = id(&run(&mut s, json!({"op": "extrude", "sketch": sk, "profiles": [hole], "distance": -1, "extent": "all", "operation": "cut"}))["feature"]);
    let one = base - vol(&s);
    assert!(one > 45.0 && one < 51.0, "a 2 mm hole through 4 mm removes about 50, got {one}");
    run(&mut s, json!({"op": "pattern", "feature": cut, "type": "circular", "axis": "z", "count": 6}));
    assert!(close(base - vol(&s), one * 6.0), "{} vs {}", base - vol(&s), one * 6.0);
    assert_eq!(s.built.bodies.len(), 1);
    assert_eq!(s.built.bodies[0].mesh.open_edges(), 0);
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);

    // Pull the top face out by 3, and push it back in by 1.
    run(&mut s, json!({"op": "extrude", "face": {"body": disc, "point": [0, 0, 4]}, "distance": 3}));
    assert!(close(s.built.bodies[0].mesh.bbox().unwrap().1.z, 7.0));
    run(&mut s, json!({"op": "extrude", "face": {"body": disc, "point": [0, 0, 7]}, "distance": -1}));
    assert!(close(s.built.bodies[0].mesh.bbox().unwrap().1.z, 6.0), "a negative distance on a face cuts");

    // A tapered block: a 10 mm square narrowing by 10 degrees over 10 mm is a frustum.
    let mut s = Session::default();
    run(&mut s, json!({"op": "create_sketch"}));
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [10, 10]}]}));
    let block = id(&run(&mut s, json!({"op": "extrude", "distance": 10, "taper": -10}))["feature"]);
    let top = 10.0 - 2.0 * 10.0 * 10f64.to_radians().tan();
    let frustum = 10.0 / 3.0 * (100.0 + top * top + 10.0 * top);
    assert!(close(vol(&s), frustum), "{} vs {frustum}", vol(&s));
    assert_eq!(s.built.bodies[0].mesh.open_edges(), 0);

    // Linear and mirror patterns of a whole body make more bodies.
    run(&mut s, json!({"op": "pattern", "feature": block, "type": "linear", "axis": "x", "count": 3, "spacing": "2 cm"}));
    assert_eq!(s.built.bodies.len(), 3);
    assert!(close(s.built.bodies.iter().filter_map(|b| b.mesh.bbox()).map(|b| b.1.x).fold(0.0, f64::max), 50.0));
    run(&mut s, json!({"op": "pattern", "feature": block, "type": "mirror", "normal": "y"}));
    assert_eq!(s.built.bodies.len(), 4);
    let mirrored = s.built.bodies.last().unwrap();
    assert!(close(mirrored.mesh.bbox().unwrap().0.y, -10.0) && close(mirrored.mesh.volume(), frustum), "a mirrored copy keeps its volume the right way out");
    let ids: std::collections::BTreeSet<u32> = s.built.bodies.iter().map(|b| b.id).collect();
    assert_eq!(ids.len(), 4, "every body has its own id");
}

#[test]
fn pattern_copies_that_miss_the_body_are_skipped() {
    let mut s = Session::default();
    run(&mut s, json!({"op": "create_sketch"}));
    run(&mut s, json!({"op": "add_geometry", "items": [{"type": "rect", "from": [0, 0], "to": [40, 20]}]}));
    run(&mut s, json!({"op": "extrude", "distance": 10}));
    let sk = id(&run(&mut s, json!({"op": "create_sketch", "offset": 10}))["sketch"]);
    run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [{"type": "circle", "center": [6, 10], "radius": 2}]}));
    let cut = id(&run(&mut s, json!({"op": "extrude", "sketch": sk, "distance": -1, "extent": "all", "operation": "cut"}))["feature"]);
    let vol = |s: &Session| s.built.bodies[0].mesh.volume();
    let one = 8000.0 - vol(&s);

    // Four copies 15 apart: the holes at 36 fits, the one at 51 is off the end of a 40 plate.
    run(&mut s, json!({"op": "pattern", "feature": cut, "type": "linear", "axis": "x", "count": 4, "spacing": 15}));
    assert!(close(8000.0 - vol(&s), one * 3.0), "three holes land, the fourth is skipped: {}", 8000.0 - vol(&s));
    assert!(s.built.errors.is_empty());

    // A pattern that lands nowhere says so, with what to try.
    let err = execute(&mut s, &json!({"op": "pattern", "feature": cut, "type": "linear", "axis": "x", "count": 3, "spacing": 100}), None).unwrap_err();
    assert!(err.contains("none of the copies reach a body"), "{err}");
    // Going the other way with a negative spacing works when there is material there.
    run(&mut s, json!({"op": "undo"}));
    run(&mut s, json!({"op": "pattern", "feature": cut, "type": "linear", "axis": "y", "count": 2, "spacing": -5}));
    assert!(close(8000.0 - vol(&s), one * 2.0));
}

#[test]
fn exact_bodies_fillet_chamfer_shell_and_step() {
    let mut s = Session::default();
    let pi = std::f64::consts::PI;
    // A batch that never names an id: each command uses what the one before made.
    let r = run(&mut s, json!({"op": "batch", "commands": [
        {"op": "create_sketch", "plane": "XY"},
        {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "rect", "center": [20, 10], "size": [40, 20]}, {"type": "circle", "center": [10, 10], "radius": 2}]},
        {"op": "extrude", "sketch": "$last_sketch", "distance": 10},
    ]}));
    let body = id(&r[2]["changed_bodies"][0]["id"]);
    assert_eq!(r[2]["changed_bodies"][0]["kind"], "exact");
    let vol = |s: &Session| s.built.bodies[0].solids.iter().map(|x| x.volume()).sum::<f64>();
    assert!((vol(&s) - (8000.0 - pi * 40.0)).abs() < 1e-6, "a true cylinder hole, not a 72-sided one: {}", vol(&s));

    // The body lists its edges and faces, each with a point to name it by.
    let topo = run(&mut s, json!({"op": "get_object_info", "id": body}))["topology"].clone();
    let edges = topo["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 14, "12 box edges and the hole's two rims; the cylinder's seam is not listed");
    assert_eq!(topo["faces"].as_array().unwrap().iter().filter(|f| f["shape"] == "cylinder").count(), 1);

    // Fillet the four vertical corners.
    let q = 1.0 - pi / 4.0;
    let vertical: Vec<J> = edges.iter().filter(|e| e["shape"] == "line" && e["point"][2] == 5.0 && e["length"] == 10.0 && e["point"][0] != 12.0).map(|e| e["point"].clone()).collect();
    assert_eq!(vertical.len(), 4);
    let before = vol(&s);
    let f = run(&mut s, json!({"op": "fillet_edges", "body": body, "edges": vertical, "radius": 3}));
    assert!((before - vol(&s) - 4.0 * 9.0 * q * 10.0).abs() < 1e-6, "{}", before - vol(&s));
    assert_eq!(f["changed_bodies"].as_array().unwrap().len(), 1);
    assert_eq!(s.built.bodies[0].mesh.open_edges(), 0);

    // Chamfer the hole's top rim, then hollow the part with the top open.
    let before = vol(&s);
    run(&mut s, json!({"op": "chamfer_edges", "body": body, "edges": [[12, 10, 10]], "distance": 1}));
    assert!(vol(&s) < before && before - vol(&s) < 10.0);
    let before = vol(&s);
    run(&mut s, json!({"op": "shell", "body": body, "open_faces": [[30, 10, 10]], "thickness": "2 mm"}));
    assert!(vol(&s) < before * 0.6 && vol(&s) > before * 0.3, "{} of {before}", vol(&s));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);

    // Oversized blends and walls are refused, not returned as nonsense.
    for bad in [json!({"op": "fillet_edges", "body": body, "edges": "all", "radius": 30}), json!({"op": "shell", "body": body, "open_faces": [[30, 10, 10]], "thickness": 40}), json!({"op": "fillet_edges", "body": body, "edges": [[500, 0, 0]], "radius": 1})] {
        let doc = s.doc.clone();
        assert!(execute(&mut s, &bad, None).is_err(), "{bad} should fail");
        assert_eq!(s.doc, doc);
    }

    // STEP holds the true surfaces.
    let dir = std::env::temp_dir().join(format!("ferrender-step-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = run(&mut s, json!({"op": "export_step", "path": dir.join("part.step")}));
    let step = std::fs::read_to_string(dir.join("part.step")).unwrap();
    assert_eq!(out["solids"], 1);
    assert!(step.contains("CYLINDRICAL_SURFACE") && step.contains("MANIFOLD_SOLID_BREP"));

    // The dimension still drives everything, fillets included.
    let sk = s.doc.sketches().next().unwrap().0.id;
    let side = s.doc.sketch(sk).unwrap().entities.keys().next().copied().unwrap();
    run(&mut s, json!({"op": "add_constraint", "sketch": sk, "kind": "distance", "refs": [side], "value": 60}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(close(s.built.bodies[0].mesh.bbox().unwrap().1.x - s.built.bodies[0].mesh.bbox().unwrap().0.x, 60.0));

    // Two overlapping exact bodies export as one shell with "union".
    let mut s = Session::default();
    run(&mut s, json!({"op": "batch", "commands": [
        {"op": "create_sketch"},
        {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "rect", "from": [0, 0], "to": [10, 10]}]},
        {"op": "extrude", "sketch": "$last_sketch", "distance": 10, "operation": "new"},
        {"op": "create_sketch", "offset": 5},
        {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "circle", "center": [10, 10], "radius": 4}]},
        {"op": "extrude", "sketch": "$last_sketch", "distance": 10, "operation": "new"},
    ]}));
    assert_eq!(s.built.bodies.len(), 2);
    let u = run(&mut s, json!({"op": "export_stl", "path": dir.join("union.stl"), "union": true}));
    assert_eq!((u["shells"].as_u64(), u["open_edges"].as_u64()), (Some(1), Some(0)));
    let merged = io::read_stl(&dir.join("union.stl"), Unit::Mm).unwrap();
    assert!(merged.volume() < 1000.0 + pi * 160.0 - 1.0, "the overlap is counted once: {}", merged.volume());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn moving_sketch_geometry_rollback_and_axis_by_points() {
    let mut s = Session::default();
    let sk = id(&run(&mut s, json!({"op": "create_sketch"}))["sketch"]);
    let c = run(&mut s, json!({"op": "add_geometry", "sketch": sk, "items": [{"type": "circle", "center": [0, 0], "radius": 5}]}));
    let circle = id(&c["items"][0]["entities"][0]);
    run(&mut s, json!({"op": "extrude", "sketch": sk, "distance": 3}));

    // Moving a sketch entity keeps the extrude built on it.
    let moved = run(&mut s, json!({"op": "move", "sketch": sk, "ids": [circle], "by": [20, 0]}));
    assert!(moved["feature_errors"].as_object().unwrap().is_empty(), "{moved}");
    let (lo, hi) = s.built.bodies[0].mesh.bbox().unwrap();
    assert!(close(lo.x, 15.0) && close(hi.x, 25.0), "{lo} {hi}");

    // Revolve about an axis given by two points; the sketch plane's axes come back from create_sketch.
    let made = run(&mut s, json!({"op": "create_sketch", "plane": {"origin": [0, 0, 0], "normal": [0, -1, 0], "x": [1, 0, 0]}}));
    assert_eq!((made["x"].clone(), made["y"].clone()), (json!([1.0, 0.0, 0.0]), json!([0.0, 0.0, 1.0])));
    let sk2 = id(&made["sketch"]);
    run(&mut s, json!({"op": "add_geometry", "sketch": sk2, "items": [{"type": "rect", "from": [50, 0], "to": [54, 6]}]}));
    run(&mut s, json!({"op": "revolve", "sketch": sk2, "axis": {"from": [40, 0], "to": [40, 10]}, "operation": "new"}));
    let ring = s.built.bodies.last().unwrap();
    let exact = std::f64::consts::PI * (14f64.powi(2) - 10f64.powi(2)) * 6.0;
    assert!((ring.solids[0].volume() - exact).abs() < 1e-6, "{}", ring.solids[0].volume());
    assert!(close(ring.mesh.bbox().unwrap().0.x, 26.0));

    // Roll the timeline back to just after the first extrude: the ring is not built.
    let first = s.doc.features[1].id;
    let rolled = run(&mut s, json!({"op": "rollback", "to": first}));
    assert_eq!((rolled["built_features"].as_u64(), s.built.bodies.len()), (Some(2), 1));
    // A feature added now goes in at the marker, before the ring's sketch.
    let disc = s.built.bodies[0].id;
    run(&mut s, json!({"op": "fillet_edges", "body": disc, "edges": [[25, 0, 3]], "radius": 1}));
    assert_eq!(s.doc.features[2].type_name(), "fillet");
    run(&mut s, json!({"op": "rollback", "to": "end"}));
    assert_eq!(s.built.bodies.len(), 2);
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    run(&mut s, json!({"op": "undo"}));
    assert_eq!(s.built.bodies.len(), 1, "moving the marker is an undo step like any other");
}

fn plate(s: &mut Session, w: f64, t: f64) -> u32 {
    let r = run(s, json!({"op": "batch", "commands": [
        {"op": "create_sketch", "plane": "XY"},
        {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "rect", "center": [0, 0], "size": [w, w]}]},
        {"op": "extrude", "sketch": "$last_sketch", "distance": t},
    ]}));
    id(&r[2]["changed_bodies"][0]["id"])
}

#[test]
fn holes_take_their_sizes_from_the_thread_catalog() {
    use fr_core::threads;
    let pi = std::f64::consts::PI;
    let exact_vol = |s: &Session| s.built.bodies[0].solids.iter().map(|x| x.volume()).sum::<f64>();

    // Names as people write them; a bare size is its coarse pitch.
    assert_eq!(threads::find("m3").unwrap().name, "M3x0.5");
    assert_eq!(threads::find("M3 x 0.35").unwrap().pitch, 0.35);
    assert_eq!(threads::find("1/4-20 UNC").unwrap().major, 6.35);
    assert_eq!(threads::find("#6-32").unwrap().family, threads::Family::Unified);
    assert!(threads::find("M7").unwrap_err().contains("list_threads"));
    for t in threads::CATALOG {
        assert!(t.minor() < t.tap_drill && t.tap_drill < t.major, "{}: the tap drill lies between the thread's minor and major diameters", t.name);
        assert!(t.major < t.clearance[0] && t.clearance[0] <= t.clearance[1] && t.clearance[1] <= t.clearance[2], "{}", t.name);
        assert!(t.counterbore > t.clearance[2] && t.countersink().0 > t.clearance[2], "{}", t.name);
    }

    // An M3 clearance hole straight through a 5 mm plate: 3.4 mm, into the flat face it was put on.
    let mut s = Session::default();
    let body = plate(&mut s, 30.0, 5.0);
    let r = run(&mut s, json!({"op": "hole", "body": body, "at": [5, 5, 5], "thread": "M3", "through": true}));
    assert_eq!(r["diameter"], 3.4);
    assert!((4500.0 - exact_vol(&s) - pi * 1.7 * 1.7 * 5.0).abs() < 1e-6, "{}", 4500.0 - exact_vol(&s));
    assert_eq!(r["changed_bodies"][0]["kind"], "exact", "a plain hole leaves the body exact");

    // Countersunk for a flat head: 90 degrees, 6.3 across, so the cone is (3.15 - 1.7) deep.
    let before = exact_vol(&s);
    run(&mut s, json!({"op": "hole", "body": body, "at": [-5, 5, 5], "thread": "M3", "type": "countersink", "through": true}));
    let (a, b, h) = (3.15f64, 1.7f64, 1.45f64);
    let cone = pi * h / 3.0 * (a * a + a * b + b * b) - pi * b * b * h;
    assert!((before - exact_vol(&s) - pi * b * b * 5.0 - cone).abs() < 1e-6, "{}", before - exact_vol(&s));

    // Counterbored for a cap screw, blind, with a drill point; and several at once from the bottom.
    let before = exact_vol(&s);
    run(&mut s, json!({"op": "hole", "body": body, "at": [5, -5, 5], "thread": "M3", "fit": "close", "type": "counterbore", "depth": 4, "tip_angle": 118}));
    let tip = pi * 1.6 * 1.6 * (1.6 / 59f64.to_radians().tan()) / 3.0;
    let bore = pi * 1.6 * 1.6 * 4.0 + pi * (3.25 * 3.25 - 1.6 * 1.6) * 3.4 + tip;
    assert!((before - exact_vol(&s) - bore).abs() < 1e-6, "{} vs {bore}", before - exact_vol(&s));
    let before = exact_vol(&s);
    run(&mut s, json!({"op": "hole", "body": body, "at": [[-10, -10, 0], [-10, -5, 0], [-5, -10, 0]], "diameter": "2mm", "depth": 2}));
    assert!((before - exact_vol(&s) - 3.0 * pi * 2.0).abs() < 1e-6);
    assert_eq!(s.built.bodies[0].mesh.open_edges(), 0);

    // A tapped hole is drilled at the tap drill size; modeled, the thread itself is cut.
    let before = exact_vol(&s);
    run(&mut s, json!({"op": "hole", "body": body, "at": [10, 10, 5], "thread": "M3", "fit": "tapped", "through": true}));
    assert!((before - exact_vol(&s) - pi * 1.25 * 1.25 * 5.0).abs() < 1e-6);
    let before = exact_vol(&s);
    let r = run(&mut s, json!({"op": "hole", "body": body, "at": [-10, 10, 5], "thread": "M3", "fit": "tapped", "modeled": true, "through": true}));
    // The body is drilled to the thread's full diameter and stays exact; the thread is a shell set into that hole.
    let bed = threads::BED;
    assert!((before - exact_vol(&s) - pi * (1.5 + bed / 2.0).powi(2) * 5.0).abs() < 1e-6, "{}", before - exact_vol(&s));
    assert_eq!(r["changed_bodies"][0]["kind"], "exact");
    let b = &s.built.bodies[0];
    assert_eq!((b.threads.len(), b.mesh.open_edges()), (1, 0));
    let (lo, hi) = b.threads[0].bbox().unwrap();
    assert!(close(lo.z, 0.0) && close(hi.z, 5.0) && close(hi.x - lo.x, 3.0 + 2.0 * bed) && close((lo.x + hi.x) / 2.0, -10.0), "{lo} {hi}");
    // What the thread leaves open is a threaded rod whose roots are the tap drill size.
    let open = pi * (1.5 + bed).powi(2) * 5.0 - b.threads[0].volume();
    assert!(open > pi * 1.25 * 1.25 * 5.0 && open < threads::rod_volume(3.0, 0.5, 5.0) * 1.03, "{open}");
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    // Under a countersink the thread starts where the cone ends, and the body can still be filleted and moved.
    run(&mut s, json!({"op": "hole", "body": body, "at": [10, -10, 5], "thread": "M4", "fit": "tapped", "modeled": true, "type": "countersink", "depth": 4}));
    let (lo, hi) = s.built.bodies[0].threads[1].bbox().unwrap();
    assert!(close(hi.z, 5.0 - (8.4 - 4.05) / 2.0) && close(lo.z, 1.0), "{lo} {hi}");
    run(&mut s, json!({"op": "fillet_edges", "body": body, "edges": [[15, 15, 2.5]], "radius": 2}));
    run(&mut s, json!({"op": "transform", "body": body, "translate": [100, 0, 0], "rotate": [0, 0, 0], "scale": 1}));
    let b = &s.built.bodies[0];
    assert!(close(b.threads[0].bbox().unwrap().0.x, 90.0 - 1.5 - bed) && b.mesh.open_edges() == 0 && b.is_exact());
    run(&mut s, json!({"op": "undo"}));

    // Nonsense is refused and leaves the document alone.
    for bad in [
        json!({"op": "hole", "body": body, "at": [5, 5, 5], "fit": "tapped", "through": true}),
        json!({"op": "hole", "body": body, "at": [5, 5, 5], "thread": "M99", "through": true}),
        json!({"op": "hole", "body": body, "at": [5, 5, 5], "diameter": 3}),
        json!({"op": "hole", "body": body, "at": [500, 5, 5], "diameter": 3, "depth": 2, "direction": [0, 0, -1]}),
        json!({"op": "hole", "body": body, "at": [8, -8, 5], "diameter": 3, "type": "countersink", "head_diameter": 2, "head_angle": 90, "through": true}),
        json!({"op": "hole", "body": body, "at": [8, -8, 5], "diameter": 3, "type": "counterbore", "through": true}),
    ] {
        let doc = s.doc.clone();
        assert!(execute(&mut s, &bad, None).is_err(), "{bad} should fail");
        assert_eq!(s.doc, doc);
    }
    assert_eq!(run(&mut s, json!({"op": "list_threads"}))["threads"].as_array().unwrap().len(), threads::CATALOG.len());
}

#[test]
fn every_catalog_thread_builds_as_a_screw_and_as_a_tapped_hole() {
    use fr_core::threads;
    let pi = std::f64::consts::PI;
    // The thread form itself: closed, and the volume of a 60 degree thread, at every size and many lengths.
    for t in threads::CATALOG {
        for len in [0.3, 1.0, 2.5, 6.0, 9.6, 10.0, 25.0, 60.0] {
            for left in [false, true] {
                let m = threads::rod(t.major, t.pitch, len, left).unwrap();
                assert_eq!(m.open_edges(), 0, "{} x {len}", t.name);
                assert!((m.volume() / threads::rod_volume(t.major, t.pitch, len) - 1.0).abs() < 0.012, "{} x {len}: {}", t.name, m.volume());
                let (lo, hi) = m.bbox().unwrap();
                assert!(lo.z == 0.0 && hi.z == len && (len < 2.0 || (hi.x - t.major / 2.0).abs() < 1e-9), "{} x {len}: it is cut square and reaches the major diameter", t.name);
            }
        }
    }
    assert!(threads::rod(3.0, 4.0, 10.0, false).is_err(), "a pitch deeper than the rod is refused");
    let sleeve = threads::sleeve(3.0, 2.5, 3.1, 0.5, 5.0, false).unwrap();
    assert_eq!(sleeve.open_edges(), 0);
    assert!(sleeve.volume() > 0.0 && sleeve.volume() < pi * (1.55 * 1.55 - 1.25 * 1.25) * 5.0, "a sleeve is the right way out: {}", sleeve.volume());

    for t in threads::CATALOG {
        // A screw: a rod of the major diameter under a head, threaded over the first 8 mm.
        let mut s = Session::default();
        let r = run(&mut s, json!({"op": "batch", "commands": [
            {"op": "create_sketch", "plane": "XY"},
            {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "circle", "center": [0, 0], "radius": t.major / 2.0}]},
            {"op": "extrude", "sketch": "$last_sketch", "distance": 12},
            {"op": "create_sketch", "plane": "XY", "offset": 12},
            {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "circle", "center": [0, 0], "radius": t.major}]},
            {"op": "extrude", "sketch": "$last_sketch", "distance": 3, "operation": "join"},
        ]}));
        let body = id(&r[2]["changed_bodies"][0]["id"]);
        assert_eq!(s.built.bodies.len(), 1, "{}", t.name);
        let exact_vol = |s: &Session| s.built.bodies[0].solids.iter().map(|x| x.volume()).sum::<f64>();
        let before = exact_vol(&s);
        let r = run(&mut s, json!({"op": "thread", "body": body, "face": [t.major / 2.0, 0, 4], "thread": t.name, "length": 8}));
        assert_eq!(r["internal"], false, "{}", t.name);
        // The rod is turned down to just inside the thread's roots, in one piece with its head, and the thread set over it.
        let core = t.minor() / 2.0 - threads::BED;
        assert!((before - exact_vol(&s) - pi * (t.major * t.major / 4.0 - core * core) * 8.0 - pi * core * core * threads::BED).abs() < 1e-6, "{}: took {} wanted {} + {}", t.name, before - exact_vol(&s), pi * (t.major * t.major / 4.0 - core * core) * 8.0, pi * core * core * threads::BED);
        let b = &s.built.bodies[0];
        assert_eq!((s.built.bodies.len(), b.solids.len(), b.threads.len(), b.mesh.open_edges()), (1, 1, 1, 0), "{}", t.name);
        let (lo, hi) = b.threads[0].bbox().unwrap();
        assert!(close(lo.z, 0.0) && close(hi.z, 8.0) && close(hi.x, t.major / 2.0), "{}: {lo} {hi}", t.name);
        assert!((b.threads[0].volume() / threads::rod_with_lead_volume(t.major, t.pitch, 8.0, [true, false]) - 1.0).abs() < 0.012, "{}", t.name);

        // A nut: a hole at the tap drill size, threaded all the way.
        let mut s = Session::default();
        let body = plate(&mut s, t.major * 3.0, 6.0);
        run(&mut s, json!({"op": "hole", "body": body, "at": [0, 0, 6], "thread": t.name, "fit": "tapped", "through": true}));
        let r = run(&mut s, json!({"op": "thread", "body": body, "face": [t.tap_drill / 2.0, 0, 3]}));
        let found = threads::nearest(t.tap_drill, true);
        assert_eq!((r["internal"].clone(), r["thread"].clone()), (json!(true), json!(found.name)), "{}", t.name);
        let b = &s.built.bodies[0];
        assert_eq!((b.threads.len(), b.mesh.open_edges(), b.is_exact()), (1, 0, true), "{}", t.name);
        let (lo, hi) = b.threads[0].bbox().unwrap();
        assert!(close(lo.z, 0.0) && close(hi.z, 6.0), "{}: {lo} {hi}", t.name);
        // The same thread, asked for with the hole, comes out the same.
        if found.name == t.name {
            let mut s2 = Session::default();
            let body = plate(&mut s2, t.major * 3.0, 6.0);
            run(&mut s2, json!({"op": "hole", "body": body, "at": [0, 0, 6], "thread": t.name, "fit": "tapped", "modeled": true, "through": true}));
            assert!((s2.built.bodies[0].threads[0].volume() / b.threads[0].volume() - 1.0).abs() < 1e-6, "{}", t.name);
        }
    }

    // A clearance hole is recognised as its screw's, and threading it remakes it as a tapped hole.
    let mut s = Session::default();
    let body = plate(&mut s, 20.0, 6.0);
    run(&mut s, json!({"op": "hole", "body": body, "at": [0, 0, 6], "thread": "M6", "through": true}));
    for (hole, made_for) in [(6.6, "M6x1"), (3.4, "M3x0.5"), (5.0, "M6x1"), (2.5, "M3x0.5"), (4.5, "M4x0.7")] {
        assert_eq!(threads::nearest(hole, true).name, made_for);
    }
    let r = run(&mut s, json!({"op": "thread", "body": body, "face": [3.3, 0, 3]}));
    assert_eq!(r["thread"], "M6x1");
    let b = &s.built.bodies[0];
    let hole = 2400.0 - b.solids.iter().map(|x| x.volume()).sum::<f64>();
    assert!((hole - pi * (3.0 + threads::BED / 2.0).powi(2) * 6.0).abs() < 1e-6, "the 6.6 mm hole is now the thread's 6 mm: {hole}");
    assert_eq!((b.threads.len(), b.mesh.open_edges()), (1, 0));
    let same = threads::sleeve(6.0, 5.0, 6.0 + 2.0 * threads::BED, 1.0, 6.0, false).unwrap();
    assert!((b.threads[0].volume() / same.volume() - 1.0).abs() < 1e-9, "with crests at the tap drill size");
    // A much smaller thread in the same hole works the same way.
    run(&mut s, json!({"op": "undo"}));
    run(&mut s, json!({"op": "thread", "body": body, "face": [3.3, 0, 3], "thread": "M3"}));
    let hole = 2400.0 - s.built.bodies[0].solids.iter().map(|x| x.volume()).sum::<f64>();
    assert!((hole - pi * (1.5 + threads::BED / 2.0).powi(2) * 6.0).abs() < 1e-6, "{hole}");
    // What cannot be threaded says why.
    let e = execute(&mut s, &json!({"op": "thread", "body": body, "face": [10, 0, 3]}), None).unwrap_err();
    assert!(e.contains("cylindrical"), "{e}");
    let mut s = Session::default();
    let r = run(&mut s, json!({"op": "batch", "commands": [
        {"op": "create_sketch", "plane": "XY"},
        {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "circle", "center": [0, 0], "diameter": 8}]},
        {"op": "extrude", "sketch": "$last_sketch", "distance": 10},
    ]}));
    let rod = id(&r[2]["changed_bodies"][0]["id"]);
    let e = execute(&mut s, &json!({"op": "thread", "body": rod, "face": [4, 0, 5], "thread": "M12"}), None).unwrap_err();
    assert!(e.contains("M12x1.75") && e.contains("8 mm"), "{e}");
    // A thicker rod is turned down to the thread.
    run(&mut s, json!({"op": "thread", "body": rod, "face": [4, 0, 5], "thread": "M6", "length": 6}));
    let (lo, hi) = s.built.bodies[0].threads[0].bbox().unwrap();
    assert!(close(hi.x - lo.x, 6.0) && s.built.bodies[0].mesh.open_edges() == 0);
}

#[test]
fn measuring_between_points_edges_and_faces() {
    use fr_core::measure::{Item, between};
    use glam::DVec3;
    let seg = |a: [f64; 3], b: [f64; 3]| Item::Path(vec![DVec3::from_array(a), DVec3::from_array(b)]);
    let quad = |o: DVec3, u: DVec3, v: DVec3| Item::Surface(vec![[o, o + u, o + u + v], [o, o + u + v, o + v]]);
    let pt = |x: f64, y: f64, z: f64| Item::Point(DVec3::new(x, y, z));

    let m = between(&pt(0.0, 0.0, 0.0), &pt(3.0, 4.0, 12.0));
    assert_eq!((m.distance, m.apart, m.angle), (13.0, None, None));
    // Parallel edges: 5 apart square to each other, though their ends are further apart than that.
    let m = between(&seg([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]), &seg([20.0, 3.0, 4.0], [30.0, 3.0, 4.0]));
    assert!(close(m.apart.unwrap(), 5.0) && close(m.distance, (100.0f64 + 25.0).sqrt()) && m.angle == Some(0.0));
    // Skew edges at right angles pass 7 apart, at the points where they cross in plan.
    let m = between(&seg([-5.0, 0.0, 0.0], [5.0, 0.0, 0.0]), &seg([2.0, -5.0, 7.0], [2.0, 5.0, 7.0]));
    assert!(close(m.distance, 7.0) && m.apart.is_none() && close(m.angle.unwrap(), 90.0) && m.from.distance(DVec3::new(2.0, 0.0, 0.0)) < 1e-9);
    // Facing walls that do not overlap: 6 between their planes, more between the patches themselves.
    let (a, b) = (quad(DVec3::ZERO, DVec3::X * 10.0, DVec3::Y * 10.0), quad(DVec3::new(20.0, 0.0, 6.0), DVec3::X * 10.0, DVec3::Y * 10.0));
    let m = between(&a, &b);
    assert!(close(m.apart.unwrap(), 6.0) && close(m.distance, (100.0f64 + 36.0).sqrt()) && m.angle == Some(0.0));
    // A face and one square to it that stops short of it; a point over a face; an edge through a face.
    let m = between(&a, &quad(DVec3::new(0.0, 0.0, 2.0), DVec3::Y * 10.0, DVec3::Z * 5.0));
    assert!(close(m.distance, 2.0) && m.apart.is_none() && close(m.angle.unwrap(), 90.0));
    let m = between(&pt(5.0, 5.0, 9.0), &a);
    assert!(close(m.distance, 9.0) && close(m.apart.unwrap(), 9.0) && m.to.distance(DVec3::new(5.0, 5.0, 0.0)) < 1e-9);
    let m = between(&a, &seg([5.0, 5.0, -1.0], [5.0, 5.0, 1.0]));
    assert!(m.distance < 1e-12 && close(m.angle.unwrap(), 90.0));
    assert!(close(Item::Surface(vec![]).size().unwrap(), 0.0) && close(a.size().unwrap(), 100.0));

    // Through the API, on a real part: a 40 x 20 x 10 plate with a 6 mm hole.
    let mut s = Session::default();
    let r = run(&mut s, json!({"op": "batch", "commands": [
        {"op": "create_sketch", "plane": "XY"},
        {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "rect", "from": [0, 0], "to": [40, 20]}]},
        {"op": "extrude", "sketch": "$last_sketch", "distance": 10},
        {"op": "hole", "body": "$last_body", "at": [10, 10, 10], "diameter": 6, "through": true},
    ]}));
    let b = id(&r[2]["changed_bodies"][0]["id"]);
    let m = run(&mut s, json!({"op": "measure", "from": {"body": b, "face": [0, 10, 5]}, "to": {"body": b, "face": [40, 10, 5]}}));
    assert_eq!((m["distance"].as_f64(), m["apart"].as_f64(), m["angle"].as_f64()), (Some(40.0), Some(40.0), Some(0.0)));
    let m = run(&mut s, json!({"op": "measure", "from": {"body": b, "edge": [20, 0, 10]}, "to": {"body": b, "edge": [20, 20, 0]}}));
    assert!(close(m["apart"].as_f64().unwrap(), 500f64.sqrt()), "opposite long edges, across the diagonal of the section: {m}");
    // From the hole's wall to the end face: the hole is centred 10 in and is 3 in radius.
    let m = run(&mut s, json!({"op": "measure", "from": {"body": b, "face": [7, 10, 5]}, "to": {"body": b, "face": [0, 10, 5]}}));
    assert!((m["distance"].as_f64().unwrap() - 7.0).abs() < 0.01 && m["apart"].is_null(), "{m}");
    let m = run(&mut s, json!({"op": "measure", "from": {"point": [0, 0, 0]}, "to": {"body": b, "face": [20, 10, 10]}}));
    assert_eq!((m["distance"].as_f64(), m["apart"].as_f64()), (Some(10.0), Some(10.0)), "a bottom corner is 10 below the top face");
}

#[test]
fn an_allowance_gives_a_screw_and_its_hole_room_to_turn() {
    use fr_core::threads;
    let t = threads::find("M6").unwrap();
    // How near the axis at (x, y) and how far from it a thread's surface comes.
    let reach = |m: &fr_core::mesh::Mesh, x: f64, y: f64| m.tris().flatten().map(|v| ((v.x - x).powi(2) + (v.y - y).powi(2)).sqrt()).fold((f64::MAX, 0.0), |(lo, hi): (f64, f64), r| (lo.min(r), hi.max(r)));
    let build = |allowance: Option<f64>| {
        let mut s = Session::default();
        let r = run(&mut s, json!({"op": "batch", "commands": [
            {"op": "create_sketch", "plane": "XY"},
            {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "circle", "center": [0, 0], "radius": 3}]},
            {"op": "extrude", "sketch": "$last_sketch", "distance": 12},
            {"op": "create_sketch", "plane": "XY"},
            {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "rect", "from": [10, -10], "to": [30, 10]}]},
            {"op": "extrude", "sketch": "$last_sketch", "distance": 8},
        ]}));
        let (screw, block) = (id(&r[2]["changed_bodies"][0]["id"]), id(&r[5]["changed_bodies"][0]["id"]));
        run(&mut s, json!({"op": "thread", "body": screw, "face": [3, 0, 6], "thread": "M6", "allowance": allowance}));
        run(&mut s, json!({"op": "hole", "body": block, "at": [20, 0, 8], "thread": "M6", "fit": "tapped", "modeled": true, "through": true, "allowance": allowance}));
        // A plain hole threaded afterwards comes out the same as a tapped one.
        run(&mut s, json!({"op": "hole", "body": block, "at": [14, -6, 8], "fit": "plain", "diameter": 6.6, "through": true}));
        run(&mut s, json!({"op": "thread", "body": block, "face": [17.3, -6, 4], "thread": "M6", "allowance": allowance}));
        assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
        let (rod, holes) = (s.built.body(screw).unwrap().threads.clone(), s.built.body(block).unwrap().threads.clone());
        (reach(&rod[0], 0.0, 0.0), reach(&holes[0], 20.0, 0.0), reach(&holes[1], 14.0, -6.0))
    };
    // Exact: the screw's crests and the hole's roots are both at the 6 mm major diameter, so nothing is between them.
    let (rod, hole, rethreaded) = build(None);
    assert!(close(rod.1, t.major / 2.0), "{rod:?}");
    assert!(close(hole.0, t.tap_drill / 2.0) && close(hole.1, t.major / 2.0 + threads::BED), "{hole:?}");
    assert!(close(rethreaded.0, hole.0) && close(rethreaded.1, hole.1), "{rethreaded:?} {hole:?}");
    // With 0.2 mm on each, the screw is 0.2 thinner and the hole 0.2 wider across.
    let (rod, hole, rethreaded) = build(Some(0.2));
    assert!(close(rod.1, t.major / 2.0 - 0.1), "{rod:?}");
    assert!(close(hole.0, t.tap_drill / 2.0 + 0.1) && close(hole.1, t.major / 2.0 + 0.1 + threads::BED), "{hole:?}");
    assert!(close(rethreaded.0, hole.0) && close(rethreaded.1, hole.1), "{rethreaded:?} {hole:?}");

    // An allowance deeper than the thread is refused.
    let mut s = Session::default();
    let r = run(&mut s, json!({"op": "batch", "commands": [{"op": "create_sketch", "plane": "XY"}, {"op": "add_geometry", "sketch": "$last_sketch", "items": [{"type": "circle", "center": [0, 0], "radius": 3}]}, {"op": "extrude", "sketch": "$last_sketch", "distance": 12}]}));
    let screw = id(&r[2]["changed_bodies"][0]["id"]);
    assert!(fr_core::api::execute(&mut s, &json!({"op": "thread", "body": screw, "face": [3, 0, 6], "thread": "M6", "allowance": 2}), None).is_err());
}
