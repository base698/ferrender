//! User-level component workflows. Geometry assertions use physical dimensions,
//! world-space picks and exported files so frame/ownership mistakes are observable.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use fr_core::{api, io, Body, FeatureKind, Session, Unit};
use glam::{DAffine3, DVec3};
use serde_json::{json, Value};

fn run(s: &mut Session, command: Value) -> Value {
    api::execute(s, &command, None).unwrap_or_else(|error| panic!("{command}: {error}"))
}
fn id(reply: &Value, key: &str) -> u32 { reply[key].as_u64().unwrap_or_else(|| panic!("missing {key}: {reply}")) as u32 }
fn component(s: &mut Session, name: &str) -> u32 { id(&run(s, json!({"op":"create_component", "name":name})), "component") }
fn activate(s: &mut Session, component: u32) { run(s, json!({"op":"activate_component", "id":component})); }
fn rect(s: &mut Session, x: f64, y: f64, width: f64, height: f64, z: f64) -> u32 {
    let sketch = id(&run(s, json!({"op":"create_sketch", "plane":"XY", "offset":z})), "sketch");
    run(s, json!({"op":"add_geometry", "sketch":sketch, "items":[{"type":"rect", "from":[x,y], "to":[x+width,y+height]}]}));
    sketch
}
fn block(s: &mut Session, x: f64, y: f64, z: f64, size: [f64;3], operation: &str) -> u32 {
    let sketch = rect(s, x, y, size[0], size[1], z);
    id(&run(s, json!({"op":"extrude", "sketch":sketch, "distance":size[2], "operation":operation})), "feature")
}
fn volume(body: &Body) -> f64 {
    if body.is_exact() { body.solids.iter().map(|solid| solid.volume()).sum() } else { body.mesh.volume().abs() }
}
fn near(a: f64, b: f64) { assert!((a-b).abs() <= 1e-5 * b.abs().max(1.0), "{a} != {b}"); }
fn point_near(a: DVec3, b: DVec3) { assert!(a.distance(b) < 1e-5, "{a:?} != {b:?}"); }
fn json_point(value: &Value) -> DVec3 {
    DVec3::new(value[0].as_f64().unwrap(), value[1].as_f64().unwrap(), value[2].as_f64().unwrap())
}
fn bounds(body: &Body, lo: DVec3, hi: DVec3) {
    let (actual_lo, actual_hi) = body.mesh.bbox().unwrap();
    point_near(actual_lo, lo); point_near(actual_hi, hi);
}
fn tree_node(value: &Value, wanted: u32) -> Option<&Value> {
    match value {
        Value::Array(values) => values.iter().find_map(|value| tree_node(value, wanted)),
        Value::Object(object) => {
            if object.get("id").and_then(Value::as_u64) == Some(wanted as u64) { Some(value) }
            else { object.get("children").and_then(|children| tree_node(children, wanted)) }
        },
        _ => None,
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("ferrender-components-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf { self.0.join(name) }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

#[test]
fn overlapping_join_cut_and_intersect_stay_inside_the_active_component() {
    let mut s = Session::default();
    let a = component(&mut s, "Left part");
    let body_a = block(&mut s, 0., 0., 0., [10.,10.,10.], "join");
    activate(&mut s, 0);
    let b = component(&mut s, "Right part");
    let body_b = block(&mut s, 0., 0., 0., [10.,10.,10.], "join");
    assert_eq!(s.built.bodies.len(), 2, "overlapping parts must not auto-join across components");
    assert_eq!(s.built.body(body_a).unwrap().component, a);
    assert_eq!(s.built.body(body_b).unwrap().component, b);
    assert_eq!(s.built.body(body_a).unwrap().name, "Body1");
    assert_eq!(s.built.body(body_b).unwrap().name, "Body1");
    block(&mut s, 0., 0., 0., [5.,10.,10.], "cut");
    near(volume(s.built.body(body_a).unwrap()), 1000.);
    near(volume(s.built.body(body_b).unwrap()), 500.);
    block(&mut s, 5., 0., 0., [5.,5.,10.], "intersect");
    near(volume(s.built.body(body_a).unwrap()), 1000.);
    near(volume(s.built.body(body_b).unwrap()), 250.);
    assert!(s.built.errors.is_empty(), "{:?}", s.built.errors);
}

#[test]
fn through_all_uses_only_bodies_in_its_own_component() {
    let mut s = Session::default();
    let distant = component(&mut s, "Distant");
    let outside = block(&mut s, 0., 0., 1000., [10.,10.,10.], "new");
    activate(&mut s, 0);
    let local = component(&mut s, "Local");
    block(&mut s, 0., 0., 0., [10.,10.,10.], "new");
    let sketch = rect(&mut s, 3., 3., 2., 2., 5.);
    let feature = id(&run(&mut s, json!({"op":"extrude", "sketch":sketch, "extent":"all", "operation":"new"})), "feature");
    let body = s.built.body(feature).unwrap();
    assert_eq!(body.component, local);
    let (lo, hi) = body.mesh.bbox().unwrap();
    assert!(lo.z > -100. && hi.z < 100., "through-all reached an unrelated component: {lo:?} .. {hi:?}");
    assert_eq!(s.built.body(outside).unwrap().component, distant);
    near(volume(s.built.body(outside).unwrap()), 1000.);
}

#[test]
fn suppression_rollback_and_insertion_use_effective_component_lifetime() {
    let mut s = Session::default();
    let stable = block(&mut s, -30., 0., 0., [4.,4.,4.], "new");
    let parent = component(&mut s, "Assembly");
    let parent_body = block(&mut s, 0., 0., 0., [10.,10.,10.], "new");
    let child = component(&mut s, "Child");
    let child_body = block(&mut s, 0., 0., 0., [2.,2.,2.], "join");
    assert_eq!(s.built.bodies.len(), 3);
    run(&mut s, json!({"op":"edit_feature", "feature":parent, "suppressed":true}));
    assert_eq!(s.built.bodies.len(), 1);
    assert!(s.built.body(stable).is_some());
    assert!(s.built.body(parent_body).is_none() && s.built.body(child_body).is_none());
    assert!(s.built.errors.is_empty(), "effective suppression must not leave orphan feature errors");
    run(&mut s, json!({"op":"edit_feature", "feature":parent, "suppressed":false}));
    assert_eq!(s.built.bodies.len(), 3);
    activate(&mut s, child);
    run(&mut s, json!({"op":"rollback", "to":stable}));
    assert_eq!(s.doc.active_component, 0, "rolling before active component must return activation to root");
    assert_eq!(s.built.bodies.len(), 1);
    let inserted = block(&mut s, -20., 0., 0., [2.,2.,2.], "new");
    assert_eq!(s.doc.feature(inserted).unwrap().owner, 0);
    run(&mut s, json!({"op":"rollback", "to":"end"}));
    assert_eq!(s.built.bodies.len(), 4);
    assert!(s.built.body(child_body).is_some());
}

#[test]
fn patterns_stay_with_their_source_and_scene_reports_the_component_tree() {
    let mut s = Session::default();
    let parent = component(&mut s, "Assembly");
    let child = component(&mut s, "Fasteners");
    let source = block(&mut s, 0., 0., 0., [2.,2.,2.], "new");
    activate(&mut s, 0);
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":3, "spacing":5})), "feature");
    assert_eq!(s.doc.feature(pattern).unwrap().owner, child);
    assert_eq!(s.built.bodies.len(), 3);
    assert!(s.built.bodies.iter().all(|body| body.component == child));
    let mut starts: Vec<_> = s.built.bodies.iter().map(|body| body.mesh.bbox().unwrap().0.x).collect();
    starts.sort_by(f64::total_cmp);
    assert_eq!(starts, vec![0.,5.,10.]);
    let scene = run(&mut s, json!({"op":"get_scene_info"}));
    assert_eq!(scene["active_component"], 0);
    let parent_node = tree_node(&scene["components"], parent).expect("parent component missing from scene");
    let child_node = tree_node(&parent_node["children"], child).expect("nested component missing from parent");
    assert_eq!(child_node["name"], "Fasteners");
    assert!(scene["bodies"].as_array().unwrap().iter().all(|body| body["component"] == child));
    let feature = scene["features"].as_array().unwrap().iter().find(|f| f["id"] == pattern).unwrap();
    assert_eq!(feature["component"], child);
    let object = run(&mut s, json!({"op":"get_object_info", "id":child}));
    assert_eq!(object["name"], "Fasteners");
}

#[test]
fn cross_component_combine_uses_world_overlap_and_keeps_the_target_frame() {
    for operation in ["join", "cut", "intersect"] {
        for keep in [false, true] {
            let mut s = Session::default();
            let a = component(&mut s, "Target");
            let target = block(&mut s, 0., 0., 0., [10.,10.,10.], "new");
            run(&mut s, json!({"op":"move_component", "id":a, "translate":[100,0,0]}));
            activate(&mut s, 0);
            let b = component(&mut s, "Tool");
            let tool = block(&mut s, 0., 0., 0., [10.,10.,10.], "new");
            run(&mut s, json!({"op":"move_component", "id":b, "translate":[115,0,0], "rotate":[0,0,90]}));
            // Target spans x=100..110; the rotated tool spans x=105..115.
            run(&mut s, json!({"op":"combine", "target":target, "tools":[tool], "operation":operation, "keep_tools":keep}));
            let result = s.built.body(target).unwrap();
            assert_eq!(result.component, a);
            near(volume(result), if operation == "join" { 1500. } else { 500. });
            let (x0, x1) = match operation { "join" => (100.,115.), "cut" => (100.,105.), _ => (105.,110.) };
            bounds(result, DVec3::new(x0,0.,0.), DVec3::new(x1,10.,10.));
            point_near(result.to_local(DVec3::new(100.,0.,0.)), DVec3::ZERO);
            assert_eq!(s.built.body(tool).is_some(), keep);
            if keep { assert_eq!(s.built.body(tool).unwrap().component, b); }
            assert!(s.built.errors.is_empty());
        }
    }
}

#[test]
fn nested_placement_moves_exact_mesh_and_modeled_threads_without_changing_local_history() {
    let scratch = Scratch::new();
    let mut s = Session::default();
    let parent = component(&mut s, "Parent");
    let child = component(&mut s, "Child");
    let exact_id = block(&mut s, 10., 0., 0., [4.,6.,8.], "new");
    let mesh_path = scratch.file("mesh.stl");
    std::fs::write(&mesh_path, io::stl_bytes([s.built.body(exact_id).unwrap()], Unit::Mm)).unwrap();
    let mesh_id = id(&run(&mut s, json!({"op":"import_stl", "path":mesh_path, "units":"mm"})), "feature");
    let sketch = id(&run(&mut s, json!({"op":"create_sketch"})), "sketch");
    run(&mut s, json!({"op":"add_geometry", "sketch":sketch, "items":[{"type":"circle", "center":[0,0], "radius":3}]}));
    let rod = id(&run(&mut s, json!({"op":"extrude", "sketch":sketch, "distance":10, "operation":"new"})), "feature");
    run(&mut s, json!({"op":"thread", "body":rod, "face":[3,0,5], "thread":"M6", "allowance":0.2}));
    let originals: Vec<_> = [exact_id,mesh_id,rod].map(|id| s.built.body(id).unwrap().clone()).into();
    let local_sketch = s.doc.sketch(sketch).unwrap().clone();
    run(&mut s, json!({"op":"move_component", "id":child, "translate":[10,2,3], "rotate":[0,90,0]}));
    run(&mut s, json!({"op":"move_component", "id":parent, "translate":[100,200,300], "rotate":[0,0,90]}));
    let parent_frame = DAffine3::from_rotation_translation(glam::DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2), DVec3::new(100.,200.,300.));
    let child_frame = DAffine3::from_rotation_translation(glam::DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2), DVec3::new(10.,2.,3.));
    let world = parent_frame * child_frame;
    for original in originals {
        let placed = s.built.body(original.id).unwrap();
        assert_eq!(placed.component, child);
        assert_eq!(placed.is_exact(), original.is_exact());
        near(volume(placed), volume(&original));
        assert_eq!(placed.threads.len(), original.threads.len());
        let mut expected_lo = DVec3::splat(f64::INFINITY);
        let mut expected_hi = DVec3::splat(f64::NEG_INFINITY);
        for point in original.mesh.tris.iter().flatten() {
            let point = world.transform_point3(*point);
            expected_lo = expected_lo.min(point); expected_hi = expected_hi.max(point);
        }
        bounds(placed, expected_lo, expected_hi);
        point_near(placed.to_local(world.transform_point3(DVec3::new(1.,2.,3.))), DVec3::new(1.,2.,3.));
        for (thread, original_thread) in placed.threads.iter().zip(&original.threads) {
            assert_eq!(thread.open_edges(), 0);
            let mut lo = DVec3::splat(f64::INFINITY); let mut hi = DVec3::splat(f64::NEG_INFINITY);
            for point in original_thread.tris.iter().flatten() { let point = world.transform_point3(*point); lo=lo.min(point); hi=hi.max(point); }
            let (actual_lo, actual_hi) = thread.bbox().unwrap();
            point_near(actual_lo, lo); point_near(actual_hi, hi);
        }
    }
    assert_eq!(s.doc.sketch(sketch).unwrap(), &local_sketch, "placement must not rewrite local sketch history");
    let reopened = Session::new(io::from_json(&io::validated_json(&s.doc).unwrap()).unwrap());
    bounds(reopened.built.body(exact_id).unwrap(), s.built.body(exact_id).unwrap().mesh.bbox().unwrap().0, s.built.body(exact_id).unwrap().mesh.bbox().unwrap().1);
    assert_eq!(reopened.doc.active_component, child);
}

#[test]
fn world_face_picks_create_local_sketches_and_holes_on_rotated_components() {
    let mut s = Session::default();
    let a = component(&mut s, "Placed part");
    let body = block(&mut s, 0., 0., 0., [10.,10.,10.], "new");
    run(&mut s, json!({"op":"move_component", "id":a, "translate":[100,200,300], "rotate":[90,0,0]}));
    let top = [105.,190.,305.]; // local (5,5,10), rotated about +X and then translated.
    let created = run(&mut s, json!({"op":"create_sketch", "face":{"body":body, "point":top}}));
    let sk = id(&created, "sketch");
    assert_eq!(s.doc.feature(sk).unwrap().owner, a);
    let plane = s.doc.sketch(sk).unwrap().plane;
    near((DVec3::new(5.,5.,10.) - plane.origin).dot(plane.normal()), 0.);
    let world_origin = json_point(&created["origin"]);
    point_near(json_point(&created["normal"]), -DVec3::Y);
    near((DVec3::from_array(top) - world_origin).dot(-DVec3::Y), 0.);
    let query = run(&mut s, json!({"op":"get_object_info", "id":sk}));
    point_near(json_point(&query["sketch"]["plane"]["origin"]), world_origin);
    point_near(json_point(&query["sketch"]["plane"]["normal"]), -DVec3::Y);
    point_near(json_point(&query["sketch"]["local_plane"]["origin"]), plane.origin);
    let point = plane.to_local(DVec3::new(5.,5.,10.));
    run(&mut s, json!({"op":"add_geometry", "sketch":sk, "items":[{"type":"circle", "center":[point.x,point.y], "radius":2}]}));
    let post = id(&run(&mut s, json!({"op":"extrude", "sketch":sk, "distance":3, "operation":"new"})), "feature");
    bounds(s.built.body(post).unwrap(), DVec3::new(103.,187.,303.), DVec3::new(107.,190.,307.));
    activate(&mut s, 0);
    run(&mut s, json!({"op":"hole", "body":body, "at":top, "diameter":2, "fit":"plain", "through":true}));
    near(volume(s.built.body(body).unwrap()), 1000. - 10.*std::f64::consts::PI);
    assert_eq!(s.built.body(body).unwrap().component, a);
    assert!(s.built.errors.is_empty());
    run(&mut s, json!({"op":"move_component", "id":a, "translate":[120,200,300]}));
    let moved = run(&mut s, json!({"op":"get_object_info", "id":sk}));
    point_near(json_point(&moved["sketch"]["plane"]["origin"]), world_origin + DVec3::new(20.,0.,0.));
    point_near(json_point(&moved["sketch"]["local_plane"]["origin"]), plane.origin);
    run(&mut s, json!({"op":"set_units", "units":"in"}));
    let inches = run(&mut s, json!({"op":"get_object_info", "id":sk}));
    point_near(json_point(&inches["sketch"]["plane"]["origin"]), (world_origin + DVec3::new(20.,0.,0.)) / 25.4);
    point_near(json_point(&inches["sketch"]["local_plane"]["origin"]), plane.origin / 25.4);
    point_near(json_point(&inches["sketch"]["plane"]["normal"]), -DVec3::Y);
    run(&mut s, json!({"op":"edit_feature", "feature":a, "suppressed":true}));
    let unavailable = run(&mut s, json!({"op":"get_object_info", "id":sk}));
    assert!(unavailable["sketch"]["plane"].is_null(), "a suppressed component must not publish a stale world sketch plane");
    point_near(json_point(&unavailable["sketch"]["local_plane"]["origin"]), plane.origin / 25.4);
}

#[test]
fn thread_and_edge_picks_after_rotation_are_stored_in_the_body_frame() {
    let mut s = Session::default();
    let a = component(&mut s, "Turned screw");
    run(&mut s, json!({"op":"create_sketch"}));
    run(&mut s, json!({"op":"add_geometry", "items":[{"type":"circle", "center":[0,0], "radius":3}]}));
    let body = id(&run(&mut s, json!({"op":"extrude", "distance":10, "operation":"new"})), "feature");
    run(&mut s, json!({"op":"move_component", "id":a, "translate":[20,30,40], "rotate":[0,90,0]}));
    activate(&mut s, 0);
    run(&mut s, json!({"op":"chamfer_edges", "body":body, "edges":[[30,30,37]], "distance":0.5}));
    run(&mut s, json!({"op":"thread", "body":body, "face":[25,30,37], "thread":"M6", "allowance":0.2}));
    let placed = s.built.body(body).unwrap();
    assert_eq!(placed.component, a);
    assert_eq!(placed.threads.len(), 1);
    let (lo,hi) = placed.mesh.bbox().unwrap();
    near(lo.x,20.); near(hi.x,30.);
    assert!(lo.y >= 26.9 && hi.y <= 33.1 && lo.z >= 36.9 && hi.z <= 43.1);
    assert_eq!(placed.threads[0].open_edges(), 0);
}

#[test]
fn construction_planes_can_reference_another_placed_component() {
    let mut s = Session::default();
    let source_component = component(&mut s, "Source");
    let source = block(&mut s, 0., 0., 0., [10.,10.,10.], "new");
    run(&mut s, json!({"op":"move_component", "id":source_component, "translate":[100,0,0]}));
    activate(&mut s, 0);
    let owner = component(&mut s, "Dependent");
    run(&mut s, json!({"op":"move_component", "id":owner, "translate":[90,0,0], "rotate":[0,0,90]}));
    let plane = id(&run(&mut s, json!({"op":"create_plane", "kind":"offset", "base":{"face":{"body":source, "point":[105,5,10]}}, "distance":2})), "feature");
    let resolved = s.built.planes[&plane].plane;
    near((DVec3::new(105.,5.,12.)-resolved.origin).dot(resolved.normal()), 0.);
    point_near(resolved.normal(), DVec3::Z);
    let sketch = id(&run(&mut s, json!({"op":"create_sketch", "plane":{"id":plane}})), "sketch");
    let local_plane = s.doc.sketch(sketch).unwrap().plane;
    // World (105,5,12) is local (5,-15,12) in the dependent component.
    let center = local_plane.to_local(DVec3::new(5.,-15.,12.));
    run(&mut s, json!({"op":"add_geometry", "sketch":sketch, "items":[{"type":"circle", "center":[center.x,center.y], "radius":1}]}));
    let post = id(&run(&mut s, json!({"op":"extrude", "sketch":sketch, "distance":2, "operation":"new"})), "feature");
    bounds(s.built.body(post).unwrap(), DVec3::new(104.,4.,12.), DVec3::new(106.,6.,14.));
    assert_eq!(s.built.body(post).unwrap().component, owner);
    run(&mut s, json!({"op":"move_component", "id":source_component, "translate":[120,0,0]}));
    bounds(s.built.body(post).unwrap(), DVec3::new(124.,4.,12.), DVec3::new(126.,6.,14.));
}

#[test]
fn hidden_subtrees_still_build_but_visible_exports_filter_subtrees_and_world_placement() {
    let scratch = Scratch::new();
    let mut s = Session::default();
    let root_body = block(&mut s, -100., 0., 0., [2.,2.,2.], "new");
    let parent = component(&mut s, "Assembly");
    let parent_body = block(&mut s, 0., 0., 0., [2.,2.,2.], "new");
    let child = component(&mut s, "Child");
    let child_body = block(&mut s, 5., 0., 0., [2.,2.,2.], "new");
    run(&mut s, json!({"op":"move_component", "id":parent, "translate":[100,0,0]}));
    let path = scratch.file("assembly.stl");
    run(&mut s, json!({"op":"export_stl", "path":path, "component":parent}));
    let exported = io::parse_stl(&std::fs::read(&path).unwrap(), Unit::Mm).unwrap();
    let (lo,hi) = exported.bbox().unwrap();
    point_near(lo, DVec3::new(100.,0.,0.)); point_near(hi, DVec3::new(107.,2.,2.));
    near(exported.volume().abs(), 16.);
    let step = run(&mut s, json!({"op":"export_step", "path":scratch.file("child.step"), "component":child}));
    assert_eq!(step["solids"], 1);
    run(&mut s, json!({"op":"set_visible", "id":parent, "visible":false}));
    assert_eq!(s.built.bodies.len(), 3, "hiding is not suppression");
    assert_eq!(s.visible_bodies().map(|body| body.id).collect::<Vec<_>>(), vec![root_body]);
    assert!(s.built.body(parent_body).is_some() && s.built.body(child_body).is_some());
    run(&mut s, json!({"op":"export_stl", "path":scratch.file("visible.stl")}));
    let visible = io::parse_stl(&std::fs::read(scratch.file("visible.stl")).unwrap(), Unit::Mm).unwrap();
    point_near(visible.bbox().unwrap().0, DVec3::new(-100.,0.,0.));
    run(&mut s, json!({"op":"set_visible", "id":parent, "visible":true}));
    run(&mut s, json!({"op":"set_visible", "id":child, "visible":false}));
    assert_eq!(s.visible_bodies().count(), 2);
    let step = run(&mut s, json!({"op":"export_step", "path":scratch.file("parent-visible.step"), "component":parent}));
    assert_eq!(step["solids"], 1);
    assert!(api::execute(&mut s, &json!({"op":"export_stl", "path":scratch.file("invalid.stl"), "component":999999}), None).is_err(), "invalid component must not silently export all parts");
}

#[test]
fn component_creation_activation_and_subtree_deletion_are_undoable_and_atomic() {
    let mut s = Session::default();
    let root_body = block(&mut s, -10., 0., 0., [2.,2.,2.], "new");
    let parent = component(&mut s, "Parent");
    let before = s.doc.clone();
    let child = component(&mut s, "Child");
    assert_eq!(s.doc.active_component, child);
    assert!(s.undo()); assert_eq!(s.doc, before);
    assert!(s.redo()); assert_eq!(s.doc.active_component, child);
    let child_body = block(&mut s, 0., 0., 0., [3.,3.,3.], "new");
    let descendant_ids: Vec<_> = s.doc.features.iter().filter(|f| f.id == parent || f.owner == parent || f.owner == child).map(|f|f.id).collect();
    let before_delete = s.doc.clone();
    let reply = run(&mut s, json!({"op":"delete_feature", "feature":parent}));
    assert_eq!(s.doc.active_component, 0);
    assert!(descendant_ids.iter().all(|id| s.doc.feature(*id).is_none()));
    assert!(s.built.body(root_body).is_some() && s.built.body(child_body).is_none());
    assert!(reply.get("removed_features").is_some(), "subtree deletion must report what it removed");
    assert!(s.undo()); assert_eq!(s.doc, before_delete);
    assert!(s.built.body(child_body).is_some());
    let before_bad = s.doc.clone();
    for command in [json!({"op":"activate_component", "id":root_body}), json!({"op":"activate_component", "id":999999}), json!({"op":"move_component", "id":0, "translate":[1,2,3]}), json!({"op":"delete_feature", "feature":0})] {
        assert!(api::execute(&mut s, &command, None).is_err(), "accepted {command}");
        assert_eq!(s.doc, before_bad, "failed component operation changed document");
    }
}

#[test]
fn native_components_roundtrip_and_reject_invalid_owners_active_ids_and_depth() {
    let mut s = Session::default();
    let root_body = block(&mut s, 0., 0., 0., [2.,2.,2.], "new");
    assert!(serde_json::from_str::<Value>(&io::to_json(&s.doc)).unwrap()["version"].as_u64().unwrap() <= 4);
    let parent = component(&mut s, "Parent");
    let child = component(&mut s, "Child");
    let child_body = block(&mut s, 0., 0., 0., [2.,2.,2.], "new");
    run(&mut s, json!({"op":"move_component", "id":parent, "translate":["2 in",0,0], "rotate":[0,0,45]}));
    let data = io::validated_json(&s.doc).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&data).unwrap()["version"], 5);
    assert_eq!(io::from_json(&data).unwrap(), s.doc);
    for (feature, owner) in [(root_body,child), (child_body,root_body), (parent,child), (child,parent+99999)] {
        let mut invalid = s.doc.clone(); invalid.feature_mut(feature).unwrap().owner = owner;
        assert!(io::from_json(&io::to_json(&invalid)).is_err(), "accepted owner {owner} on {feature}");
    }
    let mut invalid = s.doc.clone(); invalid.active_component = root_body;
    assert!(io::from_json(&io::to_json(&invalid)).is_err());
    let mut invalid = s.doc.clone();
    let FeatureKind::Component(c) = &mut invalid.feature_mut(parent).unwrap().kind else { panic!() };
    c.placement.translate[0].v = f64::INFINITY;
    assert!(io::validated_json(&invalid).is_err(), "non-finite placement must not reach native files");
    // Check both boundaries: 32 levels are legal, 33 cannot be persisted or created.
    let mut deep = Session::default();
    for level in 1..=32 { component(&mut deep, &format!("Level{level}")); }
    io::validated_json(&deep.doc).unwrap();
    let before = deep.doc.clone();
    assert!(api::execute(&mut deep, &json!({"op":"create_component", "name":"Too deep"}), None).is_err());
    assert_eq!(deep.doc, before);
}

#[test]
fn explicit_parent_activation_and_parameter_placements_survive_undo_and_units() {
    let mut s = Session::default();
    let parent = component(&mut s, "Parent");
    let child = component(&mut s, "Child");
    let sibling = id(&run(&mut s, json!({"op":"create_component", "name":"Sibling", "parent":0, "activate":false})), "component");
    assert_eq!(s.doc.feature(sibling).unwrap().owner, 0);
    assert_eq!(s.doc.active_component, child);
    let body = block(&mut s, 0., 0., 0., [2.,3.,4.], "new");
    run(&mut s, json!({"op":"set_parameter", "name":"shift", "expr":"10 mm"}));
    run(&mut s, json!({"op":"move_component", "id":parent, "translate":["$shift",0,0]}));
    bounds(s.built.body(body).unwrap(), DVec3::new(10.,0.,0.), DVec3::new(12.,3.,4.));
    run(&mut s, json!({"op":"set_parameter", "name":"shift", "expr":"30 mm"}));
    bounds(s.built.body(body).unwrap(), DVec3::new(30.,0.,0.), DVec3::new(32.,3.,4.));
    assert!(s.undo());
    bounds(s.built.body(body).unwrap(), DVec3::new(10.,0.,0.), DVec3::new(12.,3.,4.));
    assert!(s.redo());
    run(&mut s, json!({"op":"set_units", "units":"in"}));
    bounds(s.built.body(body).unwrap(), DVec3::new(30.,0.,0.), DVec3::new(32.,3.,4.));
    run(&mut s, json!({"op":"activate_component"}));
    assert_eq!(s.doc.active_component, 0);
    let before = s.doc.clone();
    for command in [json!({"op":"move_component", "id":parent, "translate":[1,2]}), json!({"op":"move_component", "id":parent, "rotate":["1 / 0",0,0]}), json!({"op":"create_component", "parent":body})] {
        assert!(api::execute(&mut s, &command, None).is_err());
        assert_eq!(s.doc, before, "rejected component edit must be atomic");
    }
}

#[test]
fn suppressed_or_rolled_back_components_do_not_publish_child_evaluation_errors() {
    let mut s = Session::default();
    let root_body = block(&mut s, -20., 0., 0., [2.,2.,2.], "new");
    let parent = component(&mut s, "Temporarily disabled");
    let body = block(&mut s, 0., 0., 0., [3.,3.,3.], "new");
    let FeatureKind::Extrude(extrude) = &s.doc.feature(body).unwrap().kind else { panic!() };
    let sketch = extrude.sketch;
    run(&mut s, json!({"op":"set_parameter", "name":"width", "expr":"10 mm"}));
    run(&mut s, json!({"op":"point_coordinates", "sketch":sketch, "x":"$width", "y":2}));
    // A damaged-but-loadable expression is a timeline error, not structural
    // corruption; disabling its component should let unaffected parts be built.
    s.doc.params.clear();
    s = Session::new(io::from_json(&io::validated_json(&s.doc).unwrap()).unwrap());
    assert!(s.built.errors.contains_key(&sketch));
    run(&mut s, json!({"op":"edit_feature", "feature":parent, "suppressed":true}));
    assert!(s.built.errors.is_empty(), "suppressed component leaked child errors: {:?}", s.built.errors);
    assert!(s.built.body(root_body).is_some());
    assert!(s.built.body(body).is_none());
    assert!(s.undo());
    run(&mut s, json!({"op":"rollback", "to":root_body}));
    assert!(s.built.errors.is_empty(), "rolled-back component leaked child errors: {:?}", s.built.errors);
}

#[test]
fn new_features_never_take_current_or_future_pattern_copy_ids() {
    let mut s = Session::default();
    let owner = component(&mut s, "Pattern owner");
    let source = block(&mut s, 0., 0., 0., [2.,2.,2.], "new");
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":2, "spacing":5})), "feature");
    let copy = pattern * 1000 + 1;
    assert_eq!(s.built.body(copy).unwrap().component, owner);
    activate(&mut s, 0);
    // Reach the allocation boundary without generating thousands of unrelated
    // features; the observable guarantee is that the existing copy stays a body.
    s.doc.next_id = copy;
    io::validated_json(&s.doc).unwrap();
    let later = id(&run(&mut s, json!({"op":"create_component", "name":"Later", "activate":false})), "component");
    assert!(later >= pattern * 1000 + 1000);
    assert!(s.doc.feature(copy).is_none());
    assert_eq!(s.built.body(copy).unwrap().component, owner);
    let info = run(&mut s, json!({"op":"get_object_info", "id":copy}));
    assert_eq!(info["component"], owner);
    for collision in [copy, pattern * 1000 + 900] {
        let mut bad = s.doc.clone();
        bad.feature_mut(later).unwrap().id = collision;
        assert!(io::from_json(&io::to_json(&bad)).is_err(), "native file accepted a feature in a pattern's reserved copy range");
    }
}

#[test]
fn features_in_other_components_cannot_use_sketches_in_a_suppressed_ancestor() {
    let mut s = Session::default();
    let parent = component(&mut s, "Source assembly");
    component(&mut s, "Source child");
    let source_sketch = rect(&mut s, 0., 0., 3., 3., 0.);
    let point = id(&run(&mut s, json!({"op":"point_coordinates", "sketch":source_sketch, "x":5, "y":5})), "point");
    activate(&mut s, 0);
    let dependent = id(&run(&mut s, json!({"op":"extrude", "sketch":source_sketch, "distance":2, "operation":"new"})), "feature");
    let plane = id(&run(&mut s, json!({"op":"create_plane", "kind":"three_point", "points":[[0,0,0],[0,0,10],{"sketch":source_sketch,"point":point}]})), "feature");
    assert!(s.built.body(dependent).is_some() && s.built.planes.contains_key(&plane));
    run(&mut s, json!({"op":"edit_feature", "feature":parent, "suppressed":true}));
    assert!(s.built.body(dependent).is_none(), "dependent extrude used a stale sketch from a suppressed component");
    assert!(!s.built.planes.contains_key(&plane), "dependent plane used a stale sketch point from a suppressed component");
    assert!(s.built.errors.contains_key(&dependent));
    assert!(s.built.errors.contains_key(&plane));
    run(&mut s, json!({"op":"edit_feature", "feature":parent, "suppressed":false}));
    assert!(s.built.errors.is_empty());
    near(volume(s.built.body(dependent).unwrap()), 18.);
    assert!(s.built.planes.contains_key(&plane));
}

#[test]
fn deleting_a_component_preserves_unrelated_high_id_body_visibility() {
    let mut s = Session::default();
    let assembly = component(&mut s, "Remove this assembly");
    let source = block(&mut s, 0., 0., 0., [2.,2.,2.], "new");
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":2, "spacing":5})), "feature");
    let copy = pattern * 1000 + 1;
    run(&mut s, json!({"op":"set_visible", "id":copy, "visible":false}));
    activate(&mut s, 0);
    // This ordinary body's ID resembles a copy of component 1. It is not a
    // pattern body, and deleting that component must not change its visibility.
    s.doc.next_id = assembly * 1000;
    let unrelated = block(&mut s, 20., 0., 0., [2.,2.,2.], "new");
    assert_eq!(unrelated, assembly * 1000 + 1);
    run(&mut s, json!({"op":"set_visible", "id":unrelated, "visible":false}));
    let before = s.doc.clone();
    run(&mut s, json!({"op":"delete_feature", "feature":assembly}));
    assert!(s.built.body(unrelated).is_some());
    assert!(s.doc.hidden_bodies.contains(&unrelated), "deleting an unrelated component unhid an ordinary body");
    assert!(!s.doc.hidden_bodies.contains(&copy), "deleted pattern copy left a stale visibility entry");
    assert_eq!(s.visible_bodies().count(), 0);
    assert!(s.undo());
    assert_eq!(s.doc, before);
}

#[test]
fn a_pattern_never_rebuilds_a_failed_suppressed_deleted_or_later_source() {
    let mut s = Session::default();
    component(&mut s, "Pattern part");
    run(&mut s, json!({"op":"set_parameter", "name":"part_height", "expr":"2 mm"}));
    let sketch = rect(&mut s, 0., 0., 2., 2., 0.);
    let source = id(&run(&mut s, json!({"op":"extrude", "sketch":sketch, "distance":"$part_height", "operation":"new"})), "feature");
    let pattern = id(&run(&mut s, json!({"op":"pattern", "feature":source, "type":"linear", "axis":"x", "count":3, "spacing":5})), "feature");
    assert_eq!(s.built.bodies.len(), 3);
    let valid = s.doc.clone();
    for problem in ["failed", "suppressed", "deleted", "later"] {
        let mut document = valid.clone();
        match problem {
            // A saved file can retain the last successful numerical height even
            // though its expression no longer evaluates. Copies must not use it.
            "failed" => document.params.clear(),
            "suppressed" => document.feature_mut(source).unwrap().suppressed = true,
            "deleted" => { document.delete_feature(source).unwrap(); },
            "later" => {
                let source_index = document.features.iter().position(|f| f.id == source).unwrap();
                let pattern_index = document.features.iter().position(|f| f.id == pattern).unwrap();
                document.features.swap(source_index, pattern_index);
            },
            _ => unreachable!(),
        }
        let loaded = io::from_json(&io::to_json(&document));
        if problem == "later" && loaded.is_err() { continue; }
        let reopened = Session::new(loaded.unwrap());
        assert!(reopened.built.errors.contains_key(&pattern), "{problem} pattern source did not mark the dependent pattern as failed");
        for copy in [pattern * 1000 + 1, pattern * 1000 + 2] {
            assert!(reopened.built.body(copy).is_none(), "pattern produced a body from its {problem} source");
        }
    }
    let repaired = Session::new(valid);
    assert!(repaired.built.errors.is_empty());
    assert_eq!(repaired.built.bodies.len(), 3);
}
