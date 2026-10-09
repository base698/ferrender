//! Loft as a feature: the command, its info, edits, operations, patterns, files and scripts.

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

/// A sketch on XY lifted to `z` holding `item`. Returns its id.
fn section(s: &mut Session, z: f64, item: J) -> Id {
    cmd(s, json!({"op": "create_sketch", "plane": "XY", "offset": z}));
    let id = s.doc.sketches().last().unwrap().0.id;
    cmd(s, json!({"op": "add_geometry", "sketch": id, "items": [item]}));
    id
}

fn square(s: &mut Session, z: f64, h: f64) -> Id {
    section(s, z, json!({"type": "rect", "from": [-h, -h], "to": [h, h]}))
}

fn volume(s: &Session) -> f64 {
    s.built.bodies.iter().map(|b| b.solids.iter().map(|x| x.volume()).sum::<f64>()).sum()
}

fn close(a: f64, b: f64) { assert!((a - b).abs() < 1e-6 * b.abs().max(1.0), "{a} is not {b}"); }

/// A frustum between two similar sections of areas `a` and `b`, `h` apart.
fn frustum(a: f64, b: f64, h: f64) -> f64 { h / 3.0 * (a + b + (a * b).sqrt()) }

#[test]
fn the_loft_command_makes_a_body_and_describes_itself() {
    let mut s = Session::default();
    let (base, top) = (square(&mut s, 0.0, 5.0), square(&mut s, 10.0, 2.0));
    let out = cmd(&mut s, json!({"op": "loft", "sections": [base, top], "ruled": true}));
    let id = out["feature"].as_u64().unwrap() as Id;
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies.len(), 1);
    assert!(s.built.bodies[0].is_exact());
    close(volume(&s), frustum(100.0, 16.0, 10.0));
    assert_eq!(out["changed_bodies"][0]["id"], json!(id));
    let f = s.doc.feature(id).unwrap();
    assert_eq!(f.type_name(), "loft");
    let FeatureKind::Loft(l) = &f.kind else { panic!("not a loft") };
    assert_eq!(l.sections.iter().map(|x| x.sketch).collect::<Vec<_>>(), vec![base, top]);
    assert!(l.ruled);
    // The sketches are put away, as an extrude puts its sketch away.
    assert!(!s.doc.sketch(base).unwrap().visible && !s.doc.sketch(top).unwrap().visible);
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": id}));
    assert_eq!(info["type"], "loft");
    assert_eq!(info["sections"], json!([{"sketch": base, "profile": 0, "edges": 4}, {"sketch": top, "profile": 0, "edges": 4}]));
    assert_eq!(info["ruled"], json!(true));
    assert_eq!(info["operation"], "new");
    let scene = cmd(&mut s, json!({"op": "get_scene_info"}));
    assert!(scene["features"].as_array().unwrap().iter().any(|f| f["type"] == "loft"), "{scene}");
    assert!(fr_core::api::REFERENCE.contains("\"op\":\"loft\""));
}

#[test]
fn a_smooth_loft_curves_through_three_sections() {
    let mut s = Session::default();
    let sketches = [square(&mut s, 0.0, 2.0), square(&mut s, 10.0, 6.0), square(&mut s, 20.0, 2.0)];
    let id = cmd(&mut s, json!({"op": "loft", "sections": sketches}))["feature"].as_u64().unwrap() as Id;
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let smooth = volume(&s);
    let straight = 2.0 * frustum(16.0, 144.0, 10.0);
    assert!(smooth > straight, "{smooth} against {straight}");
    let info = cmd(&mut s, json!({"op": "edit_feature", "feature": id, "ruled": true}));
    assert_eq!(info["ruled"], json!(true));
    close(volume(&s), straight);
    cmd(&mut s, json!({"op": "undo"}));
    close(volume(&s), smooth);
}

#[test]
fn bad_requests_are_refused_and_leave_the_design_alone() {
    let mut s = Session::default();
    let (base, top) = (square(&mut s, 0.0, 5.0), square(&mut s, 10.0, 2.0));
    let triangle = section(&mut s, 20.0, json!({"type": "polyline", "points": [[-3, -2], [3, -2], [0, 3]], "closed": true}));
    let round = section(&mut s, 30.0, json!({"type": "circle", "center": [0, 0], "radius": 2}));
    let flat = square(&mut s, 0.0, 1.0);
    let before = s.doc.features.len();
    let e = refused(&mut s, json!({"op": "loft", "sections": [base]}));
    assert!(e.contains("at least two sections"), "{e}");
    let e = refused(&mut s, json!({"op": "loft"}));
    assert!(e.contains("\"sections\" should list"), "{e}");
    let e = refused(&mut s, json!({"op": "loft", "sections": [base, 9999]}));
    assert!(e.contains("section 2: feature 9999 is not a sketch"), "{e}");
    let e = refused(&mut s, json!({"op": "loft", "sections": [base, {"sketch": top, "profile": 3}]}));
    assert!(e.contains("section 2") && e.contains("should be below 1"), "{e}");
    let e = refused(&mut s, json!({"op": "loft", "sections": [base, top], "ruled": "yes"}));
    assert!(e.contains("\"ruled\" should be true or false"), "{e}");
    // Refusals from the kernel name the sections and the counts.
    let e = refused(&mut s, json!({"op": "loft", "sections": [base, triangle]}));
    assert!(e.contains("section 2 has 3 edges where section 1 has 4 edges"), "{e}");
    let e = refused(&mut s, json!({"op": "loft", "sections": [base, round]}));
    assert!(e.contains("section 2 has 1 edge where section 1 has 4 edges"), "{e}");
    let e = refused(&mut s, json!({"op": "loft", "sections": [base, flat]}));
    assert!(e.contains("sections 1 and 2 lie on the same plane"), "{e}");
    assert_eq!(s.doc.features.len(), before, "a refused loft adds nothing");
    assert!(s.built.bodies.is_empty());
    assert!(s.doc.sketch(base).unwrap().visible, "and hides nothing");
    // A sketch with two separate outlines needs to be told which.
    let pair = section(&mut s, 40.0, json!({"type": "rect", "from": [-5, -5], "to": [-1, -1]}));
    cmd(&mut s, json!({"op": "add_geometry", "sketch": pair, "items": [{"type": "rect", "from": [1, 1], "to": [5, 5]}]}));
    let e = refused(&mut s, json!({"op": "loft", "sections": [base, pair]}));
    assert!(e.contains("has 2 separate outlines"), "{e}");
    cmd(&mut s, json!({"op": "loft", "sections": [base, {"sketch": pair, "profile": 1}], "ruled": true}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    close(volume(&s), frustum(100.0, 16.0, 40.0));
}

#[test]
fn a_loft_can_be_edited_and_follows_its_sketches() {
    let mut s = Session::default();
    let (base, top) = (square(&mut s, 0.0, 5.0), square(&mut s, 10.0, 2.0));
    let cap = square(&mut s, 16.0, 1.0);
    let id = cmd(&mut s, json!({"op": "loft", "sections": [base, top], "ruled": true}))["feature"].as_u64().unwrap() as Id;
    close(volume(&s), frustum(100.0, 16.0, 10.0));
    // Add a third section.
    let info = cmd(&mut s, json!({"op": "edit_feature", "feature": id, "sections": [base, top, cap]}));
    assert_eq!(info["sections"].as_array().unwrap().len(), 3);
    close(volume(&s), frustum(100.0, 16.0, 10.0) + frustum(16.0, 4.0, 6.0));
    // A bad edit is refused and changes nothing.
    let e = refused(&mut s, json!({"op": "edit_feature", "feature": id, "sections": [base]}));
    assert!(e.contains("at least two sections"), "{e}");
    close(volume(&s), frustum(100.0, 16.0, 10.0) + frustum(16.0, 4.0, 6.0));
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "sections": [base, top]}));
    // Resize the top section: the loft follows.
    let sk = s.doc.sketch(top).unwrap();
    let moves: Vec<(Id, glam::DVec2)> = sk.points.iter().filter(|(_, p)| p.length() > 0.1).map(|(id, p)| (*id, *p * 2.0)).collect();
    s.edit(|d| { for (id, p) in &moves { d.sketch_mut(top).unwrap().points.insert(*id, *p); } Ok(()) }).unwrap();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    close(volume(&s), frustum(100.0, 64.0, 10.0));
    // Deleting a section's sketch is reported on the loft rather than leaving a stale body.
    cmd(&mut s, json!({"op": "delete_feature", "feature": top}));
    let e = s.built.errors.get(&id).expect("the loft reports its missing section");
    assert!(e.contains("section 2's sketch"), "{e}");
    assert!(s.built.bodies.is_empty());
}

#[test]
fn a_loft_joins_cuts_and_patterns_like_other_features() {
    let mut s = Session::default();
    let slab = square(&mut s, 0.0, 10.0);
    cmd(&mut s, json!({"op": "extrude", "sketch": slab, "distance": 10}));
    let block = volume(&s);
    close(block, 4000.0);
    let (base, top) = (square(&mut s, 0.0, 4.0), square(&mut s, 10.0, 2.0));
    let id = cmd(&mut s, json!({"op": "loft", "sections": [base, top], "ruled": true, "operation": "cut"}))["feature"].as_u64().unwrap() as Id;
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let funnel = frustum(64.0, 16.0, 10.0);
    close(volume(&s), block - funnel);
    assert_eq!(cmd(&mut s, json!({"op": "get_object_info", "id": id}))["operation"], "cut");
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "operation": "new"}));
    assert_eq!(s.built.bodies.len(), 2);
    close(volume(&s), block + funnel);
    cmd(&mut s, json!({"op": "pattern", "feature": id, "type": "linear", "axis": "x", "count": 3, "spacing": 30}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    close(volume(&s), block + 3.0 * funnel);
}

#[test]
fn a_fillet_on_a_loft_finds_its_edge_by_name_and_step_gets_one_solid() {
    let mut s = Session::default();
    let (base, top) = (square(&mut s, 0.0, 5.0), square(&mut s, 10.0, 3.0));
    let id = cmd(&mut s, json!({"op": "loft", "sections": [base, top], "ruled": true}))["feature"].as_u64().unwrap() as Id;
    let plain = volume(&s);
    // The top edge along y = 3.
    let fillet = cmd(&mut s, json!({"op": "fillet_edges", "body": id, "edges": [[0, 3, 10]], "radius": 0.5}))["feature"].as_u64().unwrap() as Id;
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(volume(&s) < plain && volume(&s) > plain - 2.0, "{}", volume(&s));
    assert_eq!(cmd(&mut s, json!({"op": "get_object_info", "id": fillet}))["resolved"], "tag");
    s.rebuild();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let dir = std::env::temp_dir().join(format!("ferrender-loft-step-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = cmd(&mut s, json!({"op": "export_step", "path": dir.join("loft.step").display().to_string()}));
    assert_eq!(out["solids"], json!(1));
}

#[test]
fn a_loft_survives_saving_and_reopening() {
    let dir = std::env::temp_dir().join(format!("ferrender-loft-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    let (base, top) = (square(&mut s, 0.0, 5.0), square(&mut s, 10.0, 2.0));
    cmd(&mut s, json!({"op": "loft", "sections": [base, top], "ruled": true}));
    let file = dir.join("frustum.ferr");
    s.save(&file).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let v: J = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], json!(14), "a design with a loft needs a reader that knows lofts");
    assert!(text.contains("\"loft\""));
    let reopened = Session::open(&file).unwrap();
    assert_eq!(reopened.doc, s.doc);
    assert!(reopened.built.errors.is_empty());
    close(volume(&reopened), frustum(100.0, 16.0, 10.0));
    // `ruled` and `op` are optional on disk: without them the loft is smooth and new.
    let mut plain = v.clone();
    let loft = plain["features"].as_array_mut().unwrap().iter_mut().find(|f| f["kind"].get("loft").is_some()).unwrap();
    let fields = loft["kind"]["loft"].as_object_mut().unwrap();
    fields.remove("ruled");
    fields.remove("op");
    std::fs::write(&file, plain.to_string()).unwrap();
    close(volume(&Session::open(&file).unwrap()), frustum(100.0, 16.0, 10.0));
    // A design without a loft is not stamped with the newer version.
    let mut other = Session::default();
    let sk = square(&mut other, 0.0, 5.0);
    cmd(&mut other, json!({"op": "extrude", "sketch": sk, "distance": 5}));
    let file = dir.join("block.ferr");
    other.save(&file).unwrap();
    let v: J = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(v["version"].as_u64().unwrap() < 14, "{}", v["version"]);
}

#[test]
fn scripts_can_loft() {
    let mut s = Session::default();
    let source = r#"const META = #{ name: "Funnel", description: "A round funnel.", inputs: [ #{ name: "height", kind: "length", initial: "10 mm" } ] };
fn run(inputs) {
    create_sketch(#{ plane: "XY" });
    let wide = scene().features[0].id;
    add_geometry(#{ sketch: wide, items: [ #{ type: "circle", center: [0, 0], radius: 5 } ] });
    create_sketch(#{ plane: "XY", offset: inputs.height });
    let narrow = scene().features[1].id;
    add_geometry(#{ sketch: narrow, items: [ #{ type: "circle", center: [0, 0], radius: 2 } ] });
    let made = loft(#{ sections: [wide, narrow] });
    made.feature
}"#;
    let mut req = fr_core::script::Request::new(source);
    req.inputs = json!({"height": "20 mm"});
    fr_core::script::run(&mut s, &req).unwrap();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let cone = frustum(std::f64::consts::PI * 25.0, std::f64::consts::PI * 4.0, 20.0);
    assert!((volume(&s) - cone).abs() < 1e-3 * cone, "{} against {cone}", volume(&s));
}
