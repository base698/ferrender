//! Sweep as a feature: the command, its info, edits, operations, patterns, files and scripts.

use fr_core::api::execute;
use fr_core::doc::{FeatureKind, SweepOrient};
use fr_core::{Id, Session};
use serde_json::{Value as J, json};

fn cmd(s: &mut Session, c: J) -> J {
    execute(s, &c, None).unwrap_or_else(|e| panic!("{c} failed: {e}"))
}

fn newest_sketch(s: &Session) -> Id {
    s.doc.sketches().last().unwrap().0.id
}

/// A path sketch on XY through `points` and a 2 x 2 square profile on YZ at the origin. Returns (profile, path).
fn frame(s: &mut Session, points: J, closed: bool) -> (Id, Id) {
    cmd(s, json!({"op": "create_sketch", "plane": "XY"}));
    let path = newest_sketch(s);
    cmd(s, json!({"op": "add_geometry", "sketch": path, "items": [{"type": "polyline", "points": points, "closed": closed}]}));
    cmd(s, json!({"op": "create_sketch", "plane": "YZ"}));
    let profile = newest_sketch(s);
    cmd(s, json!({"op": "add_geometry", "sketch": profile, "items": [{"type": "rect", "from": [-1, -1], "to": [1, 1]}]}));
    (profile, path)
}

fn volume(s: &Session) -> f64 {
    s.built.bodies.iter().map(|b| b.solids.iter().map(|x| x.volume()).sum::<f64>()).sum()
}

fn close(a: f64, b: f64) { assert!((a - b).abs() < 1e-6 * b.abs().max(1.0), "{a} is not {b}"); }

#[test]
fn the_sweep_command_makes_a_body_and_describes_itself() {
    let mut s = Session::default();
    let (profile, path) = frame(&mut s, json!([[0, 0], [10, 0], [10, 10]]), false);
    let out = cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path}));
    let id = out["feature"].as_u64().unwrap() as Id;
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies.len(), 1);
    assert!(s.built.bodies[0].is_exact());
    close(volume(&s), 80.0);
    assert_eq!(out["changed_bodies"][0]["id"], json!(id));
    let f = s.doc.feature(id).unwrap();
    assert_eq!(f.type_name(), "sweep");
    let FeatureKind::Sweep(w) = &f.kind else { panic!("not a sweep") };
    assert_eq!((w.sketch, w.path_sketch, w.orient), (profile, path, SweepOrient::Follow));
    assert!(w.path.is_empty(), "an unnamed path is the whole sketch");
    // Both sketches are put away, as an extrude puts its sketch away.
    assert!(!s.doc.sketch(profile).unwrap().visible && !s.doc.sketch(path).unwrap().visible);
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": id}));
    assert_eq!(info["type"], "sweep");
    assert_eq!(info["sketch"], json!(profile));
    assert_eq!(info["path_sketch"], json!(path));
    assert_eq!(info["path"].as_array().unwrap().len(), 2, "the path is listed as it is walked: {info}");
    assert_eq!(info["path_closed"], json!(false));
    assert_eq!(info["orientation"], "follow");
    assert_eq!(info["operation"], "new");
    // The profile defaults to the newest sketch that is not the path.
    let mut again = Session::default();
    let (_, path) = frame(&mut again, json!([[0, 0], [10, 0]]), false);
    cmd(&mut again, json!({"op": "sweep", "path_sketch": path}));
    close(volume(&again), 40.0);
}

#[test]
fn bad_requests_are_refused_and_leave_the_design_alone() {
    let mut s = Session::default();
    let (profile, path) = frame(&mut s, json!([[0, 0], [10, 0], [10, 10]]), false);
    let before = s.doc.clone();
    let fails = |s: &mut Session, c: J, needle: &str| {
        let err = execute(s, &c, None).unwrap_err();
        assert!(err.contains(needle), "{c}: {err}");
    };
    fails(&mut s, json!({"op": "sweep", "sketch": profile}), "path_sketch");
    fails(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": profile}), "different sketches");
    fails(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": 999}), "not a sketch");
    fails(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path, "orientation": "sideways"}), "follow or fixed");
    fails(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path, "path": [999]}), "no longer");
    // The square is side-on to the second leg, so a fixed orientation has nothing to sweep there.
    fails(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path, "orientation": "fixed"}), "fixed orientation");
    // The path sketch has a closed profile of its own only when it is closed; sweeping it along the square's outline is refused.
    fails(&mut s, json!({"op": "sweep", "sketch": path, "path_sketch": profile}), "no closed profile");
    assert_eq!(s.doc, before);
    assert!(s.built.bodies.is_empty());
}

#[test]
fn a_sweep_can_be_edited_and_follows_its_sketches() {
    let mut s = Session::default();
    let (profile, path) = frame(&mut s, json!([[0, 0], [10, 0], [10, 10]]), false);
    let id = cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path}))["feature"].as_u64().unwrap() as Id;
    let legs: Vec<Id> = s.doc.sketch(path).unwrap().entities.keys().copied().collect();
    // Follow only the first leg.
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "path": [legs[0]]}));
    close(volume(&s), 40.0);
    // A fixed orientation works along one straight leg.
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "orientation": "fixed"}));
    close(volume(&s), 40.0);
    // Both legs again cannot keep a fixed orientation: the edit is refused and nothing changes.
    let err = execute(&mut s, &json!({"op": "edit_feature", "feature": id, "path": legs}), None).unwrap_err();
    assert!(err.contains("fixed orientation"), "{err}");
    close(volume(&s), 40.0);
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "orientation": "follow", "path": legs}));
    close(volume(&s), 80.0);
    // Growing the profile grows the sweep.
    let corner = *s.doc.sketch(profile).unwrap().points.iter().find(|(_, p)| p.x > 0.5 && p.y > 0.5).unwrap().0;
    s.edit(|d| { d.sketch_mut(profile).unwrap().points.insert(corner, glam::DVec2::new(1.0, 3.0)); Ok(()) }).unwrap();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert!(volume(&s) > 80.0);
    // Undo and redo carry the sweep with them.
    assert!(s.undo());
    close(volume(&s), 80.0);
    // Deleting the path sketch leaves the sweep with an error that names the cause.
    cmd(&mut s, json!({"op": "delete_feature", "feature": path}));
    let err = s.built.errors.get(&id).expect("the sweep reports its missing path");
    assert!(err.contains("path sketch"), "{err}");
}

#[test]
fn a_sweep_joins_cuts_and_patterns_like_other_features() {
    // A channel cut along an L through a slab.
    let mut s = Session::default();
    cmd(&mut s, json!({"op": "primitive", "type": "box", "width": 30, "depth": 30, "height": 4, "position": [-5, -5, -2]}));
    let slab = volume(&s);
    close(slab, 3600.0);
    let (profile, path) = frame(&mut s, json!([[0, 0], [10, 0], [10, 10]]), false);
    cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path, "operation": "cut"}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies.len(), 1);
    close(volume(&s), slab - 80.0);
    // With a body present the default is to join; the bar lies inside the slab, so nothing is added.
    let mut j = Session::default();
    cmd(&mut j, json!({"op": "primitive", "type": "box", "width": 30, "depth": 30, "height": 4, "position": [-5, -5, -2]}));
    let (profile, path) = frame(&mut j, json!([[0, 0], [10, 0], [10, 10]]), false);
    cmd(&mut j, json!({"op": "sweep", "sketch": profile, "path_sketch": path}));
    assert_eq!(j.built.bodies.len(), 1);
    close(volume(&j), slab);
    // A pattern repeats a sweep.
    let mut p = Session::default();
    let (profile, path) = frame(&mut p, json!([[0, 0], [10, 0], [10, 10]]), false);
    let id = cmd(&mut p, json!({"op": "sweep", "sketch": profile, "path_sketch": path}))["feature"].as_u64().unwrap();
    cmd(&mut p, json!({"op": "pattern", "feature": id, "type": "linear", "axis": "z", "count": 3, "spacing": 5}));
    assert!(p.built.errors.is_empty(), "{:?}", p.built.errors);
    close(volume(&p), 240.0);
}

#[test]
fn a_closed_path_and_a_fillet_on_the_result() {
    let mut s = Session::default();
    let (profile, path) = frame(&mut s, json!([[-10, 0], [10, 0], [10, 10], [-10, 10]]), true);
    let id = cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path}))["feature"].as_u64().unwrap() as Id;
    close(volume(&s), 240.0);
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": id}));
    assert_eq!(info["path_closed"], json!(true));
    assert_eq!(info["path"].as_array().unwrap().len(), 4);
    // The frame's faces are named, so a fillet on its edges resolves by tag and survives a rebuild.
    cmd(&mut s, json!({"op": "fillet_edges", "body": id, "edges": "all", "radius": 0.3}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let filleted = volume(&s);
    assert!(filleted < 240.0 && filleted > 230.0, "{filleted}");
    s.rebuild();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    close(volume(&s), filleted);
}

#[test]
fn a_fillet_follows_its_edge_when_the_sweep_changes_and_step_gets_true_surfaces() {
    let mut s = Session::default();
    // A line, then a tangent quarter arc of radius 5 bending to the left.
    cmd(&mut s, json!({"op": "create_sketch", "plane": "XY"}));
    let path = newest_sketch(&s);
    cmd(&mut s, json!({"op": "add_geometry", "sketch": path, "items": [
        {"type": "line", "from": [0, 0], "to": [10, 0]},
        {"type": "arc", "center": [10, 5], "start": [10, 0], "end": [15, 5]},
    ]}));
    cmd(&mut s, json!({"op": "create_sketch", "plane": "YZ"}));
    let profile = newest_sketch(&s);
    cmd(&mut s, json!({"op": "add_geometry", "sketch": profile, "items": [{"type": "rect", "from": [-1, -1], "to": [1, 1]}]}));
    let id = cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path}))["feature"].as_u64().unwrap() as Id;
    let bar = 4.0 * (10.0 + std::f64::consts::FRAC_PI_2 * 5.0);
    close(volume(&s), bar);
    // Round the long edge on the outside of the bend, at the top: it runs along y = -1, z = 1 on the first leg.
    let fillet = cmd(&mut s, json!({"op": "fillet_edges", "body": id, "edges": [[5, -1, 1]], "radius": 0.4}))["feature"].as_u64().unwrap() as Id;
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let rounded = volume(&s);
    assert!(rounded < bar && rounded > bar - 1.0, "{rounded}");
    let how = cmd(&mut s, json!({"op": "get_object_info", "id": fillet}));
    assert_eq!(how["resolved"], "tag", "the fillet finds its edge by the faces' names: {how}");
    // Widen the profile on that side. The edge moves 1 mm away from where it was picked; the fillet goes with it.
    let corner = *s.doc.sketch(profile).unwrap().points.iter().find(|(_, p)| p.x < -0.5 && p.y > 0.5).unwrap().0;
    let lower = *s.doc.sketch(profile).unwrap().points.iter().find(|(_, p)| p.x < -0.5 && p.y < -0.5).unwrap().0;
    s.edit(|d| {
        let sk = d.sketch_mut(profile).unwrap();
        sk.points.insert(corner, glam::DVec2::new(-2.0, 1.0));
        sk.points.insert(lower, glam::DVec2::new(-2.0, -1.0));
        Ok(())
    }).unwrap();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let wider = 6.0 * (10.0 + std::f64::consts::FRAC_PI_2 * 5.5);
    assert!(volume(&s) < wider && volume(&s) > wider - 1.0, "{} against {wider}", volume(&s));
    assert_eq!(cmd(&mut s, json!({"op": "get_object_info", "id": fillet}))["resolved"], "tag");
    let (lo, _) = s.built.bodies[0].mesh.bbox().unwrap();
    assert!((lo.y + 2.0).abs() < 0.02, "the body reaches the new width: {lo:?}");
    // The exact solid goes to STEP, bend and all.
    let dir = std::env::temp_dir().join(format!("ferrender-sweep-step-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = cmd(&mut s, json!({"op": "export_step", "path": dir.join("bend.step").display().to_string()}));
    assert_eq!(out["solids"], json!(1));
    let step = std::fs::read_to_string(dir.join("bend.step")).unwrap();
    assert!(step.contains("TOROIDAL_SURFACE") || step.contains("CYLINDRICAL_SURFACE") || step.contains("SURFACE_OF_REVOLUTION") || step.contains("B_SPLINE_SURFACE"), "the bend is a curved face, not facets");
}

#[test]
fn a_sweep_survives_saving_and_reopening() {
    let dir = std::env::temp_dir().join(format!("ferrender-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    let (profile, path) = frame(&mut s, json!([[0, 0], [10, 0], [10, 10]]), false);
    cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path, "orientation": "follow"}));
    let file = dir.join("bar.ferr");
    s.save(&file).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let v: J = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], json!(13), "a design with a sweep needs a reader that knows sweeps");
    assert!(text.contains("\"sweep\""));
    let reopened = Session::open(&file).unwrap();
    assert_eq!(reopened.doc, s.doc);
    assert!(reopened.built.errors.is_empty());
    close(volume(&reopened), 80.0);
    // A file written before `path` and `orient` had values to store still reads.
    let mut plain = v.clone();
    let sweep = plain["features"].as_array_mut().unwrap().iter_mut().find(|f| f["kind"].get("sweep").is_some()).unwrap();
    let fields = sweep["kind"]["sweep"].as_object_mut().unwrap();
    fields.remove("orient");
    fields.remove("op");
    std::fs::write(&file, plain.to_string()).unwrap();
    close(volume(&Session::open(&file).unwrap()), 80.0);
}

#[test]
fn scripts_can_sweep() {
    let mut s = Session::default();
    let source = r#"const META = #{ name: "Handle", description: "A bar bent through a corner.", inputs: [ #{ name: "reach", kind: "length", initial: "10 mm" } ] };
fn run(inputs) {
    create_sketch(#{ plane: "XY" });
    let path = scene().features[0].id;
    add_geometry(#{ sketch: path, items: [ #{ type: "polyline", points: [[0, 0], [inputs.reach, 0], [inputs.reach, inputs.reach]] } ] });
    create_sketch(#{ plane: "YZ" });
    let profile = scene().features[1].id;
    add_geometry(#{ sketch: profile, items: [ #{ type: "circle", center: [0, 0], radius: 1 } ] });
    let made = sweep(#{ sketch: profile, path_sketch: path });
    made.feature
}"#;
    let mut req = fr_core::script::Request::new(source);
    req.inputs = json!({"reach": "20 mm"});
    fr_core::script::run(&mut s, &req).unwrap();
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    // A round bar of radius 1 along two legs of 20; the profile's area is its 72-sided outline's.
    let area = 36.0 * (std::f64::consts::TAU / 72.0).sin();
    assert!((volume(&s) - std::f64::consts::PI * 40.0).abs() < 0.5, "{}", volume(&s));
    assert!((volume(&s) - area * 40.0).abs() < 0.05 * area * 40.0);
}

#[test]
fn a_sweep_can_cover_only_parts_of_its_path() {
    let mut s = Session::default();
    let (profile, path) = frame(&mut s, json!([[0, 0], [10, 0], [10, 10]]), false);
    // 0.1 to 0.3 and 0.6 to 0.7 of the 20 mm path: 4 mm on the first leg and 2 mm on the second.
    let id = cmd(&mut s, json!({"op": "sweep", "sketch": profile, "path_sketch": path, "spans": [[0.1, 0.3], [0.6, 0.7]]}))["feature"].as_u64().unwrap() as Id;
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(s.built.bodies.len(), 1, "the pieces are one body");
    assert_eq!(s.built.bodies[0].solids.len(), 2, "of two separate lumps");
    close(volume(&s), 4.0 * 6.0);
    let info = cmd(&mut s, json!({"op": "get_object_info", "id": id}));
    assert_eq!(info["spans"], json!([[0.1, 0.3], [0.6, 0.7]]));
    // Editing: the second half, then the whole path again.
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "spans": [[0.5, 1.0]]}));
    close(volume(&s), 4.0 * 10.0);
    let (lo, hi) = s.built.bodies[0].mesh.bbox().unwrap();
    assert!((lo.y - 0.0).abs() < 1e-6 && (hi.y - 10.0).abs() < 1e-6 && (lo.x - 9.0).abs() < 1e-6, "the second leg only: {lo:?} {hi:?}");
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "spans": []}));
    close(volume(&s), 80.0);
    assert_eq!(cmd(&mut s, json!({"op": "get_object_info", "id": id}))["spans"], json!([]));
    // Bad parts are refused and change nothing.
    for bad in [json!([[0.5, 0.2]]), json!([[0.0, 2.0]]), json!([0.1, 0.3]), json!("half"), json!([[0.1]])] {
        let err = execute(&mut s, &json!({"op": "edit_feature", "feature": id, "spans": bad}), None).unwrap_err();
        assert!(err.contains("fraction") || err.contains("after its start"), "{bad}: {err}");
        close(volume(&s), 80.0);
    }
    // Saved only when there are parts, so a whole-path sweep's file is as it was before parts existed.
    let dir = std::env::temp_dir().join(format!("ferrender-sweep-spans-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    s.save(&dir.join("whole.ferr")).unwrap();
    assert!(!std::fs::read_to_string(dir.join("whole.ferr")).unwrap().contains("spans"));
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "spans": [[0.1, 0.3], [0.6, 0.7]]}));
    s.save(&dir.join("parts.ferr")).unwrap();
    assert!(std::fs::read_to_string(dir.join("parts.ferr")).unwrap().contains("spans"));
    let reopened = Session::open(&dir.join("parts.ferr")).unwrap();
    assert_eq!(reopened.doc, s.doc);
    close(volume(&reopened), 24.0);
    // A fillet on one piece keeps its edge when the other piece is resized.
    let fillet = cmd(&mut s, json!({"op": "fillet_edges", "body": id, "edges": [[3, -1, 1]], "radius": 0.3}))["feature"].as_u64().unwrap() as Id;
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    let filleted = volume(&s);
    assert!(filleted < 24.0 && filleted > 23.9, "{filleted}");
    cmd(&mut s, json!({"op": "edit_feature", "feature": id, "spans": [[0.1, 0.3], [0.6, 0.9]]}));
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
    assert_eq!(cmd(&mut s, json!({"op": "get_object_info", "id": fillet}))["resolved"], "tag");
    assert!((volume(&s) - (filleted + 4.0 * 4.0)).abs() < 1e-6, "the fillet is still on the first piece: {}", volume(&s));
}
