use fr_core::{Session, api::execute, threads};
use glam::DVec3;
use serde_json::{Value, json};

fn run(s: &mut Session, command: Value) -> Value {
    execute(s, &command, None).unwrap_or_else(|error| panic!("{command}: {error}"))
}

#[test]
fn lead_meshes_are_closed_for_both_hands_and_short_spans() {
    for name in ["M1.6", "M3", "M6", "M12", "1/4-20"] {
        let spec = threads::find(name).unwrap();
        let major = spec.major - 0.2;
        let root = (spec.minor() - 0.2) / 2.;
        for length in [
            0.2 * spec.pitch,
            0.9 * spec.pitch,
            2. * spec.pitch,
            7.3 * spec.pitch,
        ] {
            for left in [false, true] {
                let square = threads::rod(major, spec.pitch, length, left).unwrap();
                for ends in [[false, false], [true, false], [false, true], [true, true]] {
                    let mesh =
                        threads::rod_with_lead(major, spec.pitch, length, left, ends).unwrap();
                    assert_eq!(
                        mesh.open_edges(),
                        0,
                        "{name}, length={length}, left={left}, ends={ends:?}"
                    );
                    assert!(mesh.volume() > 0. && mesh.volume() <= square.volume() + 1e-8);
                    assert!(
                        mesh.tris()
                            .all(|t| (t[1] - t[0]).cross(t[2] - t[0]).length() > 0.),
                        "zero-area triangle: {name}, length={length}, left={left}, ends={ends:?}"
                    );
                    for vertex in mesh.tris().flatten() {
                        let radius = vertex.truncate().length();
                        assert!(radius <= major / 2. + 1e-9);
                        if (ends[0] && vertex.z == 0.) || (ends[1] && vertex.z == length) {
                            assert!(
                                radius <= root + 1e-9,
                                "a free rim must fit through the mating crests"
                            );
                        }
                    }
                    if ends == [false, false] {
                        assert_eq!(mesh, square);
                    }
                }
            }
        }
    }
}

#[test]
fn partial_threads_preserve_the_unthreaded_shaft_and_head() {
    for chamfer in [false, true] {
        let mut s = Session::default();
        run(&mut s, json!({"op":"create_sketch"}));
        run(
            &mut s,
            json!({"op":"add_geometry","items":[{"type":"circle","center":[0,0],"diameter":6}]}),
        );
        let body = run(&mut s, json!({"op":"extrude","distance":12}))["feature"]
            .as_u64()
            .unwrap();
        run(&mut s, json!({"op":"create_sketch","offset":12}));
        run(
            &mut s,
            json!({"op":"add_geometry","items":[{"type":"circle","center":[0,0],"diameter":12}]}),
        );
        run(
            &mut s,
            json!({"op":"extrude","distance":3,"operation":"join"}),
        );
        if chamfer {
            run(
                &mut s,
                json!({"op":"chamfer_edges","body":body,"edges":[[3,0,0]],"distance":0.4}),
            );
        }
        let before = s.built.bodies[0]
            .solids
            .iter()
            .map(|solid| solid.volume())
            .sum::<f64>();
        run(
            &mut s,
            json!({"op":"thread","body":body,"face":[3,0,6],"thread":"M6","offset":2,"length":5,"allowance":0.2}),
        );
        let b = &s.built.bodies[0];
        let root = (threads::find("M6").unwrap().minor() - 0.2) / 2. - threads::BED;
        let removed = std::f64::consts::PI * (9. - root * root) * 5.;
        let after = b.solids.iter().map(|solid| solid.volume()).sum::<f64>();
        assert!(
            (before - after - removed).abs() < 1e-6,
            "only the requested five millimeters should change"
        );
        let (lo, hi) = b.mesh.bbox().unwrap();
        assert!((lo - DVec3::new(-6., -6., 0.)).length() < 1e-7);
        assert!((hi - DVec3::new(6., 6., 15.)).length() < 1e-7);
        assert_eq!(b.mesh.open_edges(), 0);
    }
}

#[test]
fn a_widening_conical_head_is_not_treated_as_a_tip_chamfer() {
    let mut s = Session::default();
    run(&mut s, json!({"op":"create_sketch","plane":"XZ"}));
    run(
        &mut s,
        json!({"op":"add_geometry","items":[{"type":"polyline","closed":true,"points":[[0,0],[3,0],[3,10],[6,13],[6,15],[0,15]]}]}),
    );
    let body = run(&mut s, json!({"op":"revolve","axis":"y"}))["feature"]
        .as_u64()
        .unwrap();
    assert!(
        fr_core::exact::end_chamfer(&s.built.bodies[0].solids, DVec3::Z * 10., DVec3::Z, 3.)
            .is_none()
    );
    let before = s.built.bodies[0]
        .solids
        .iter()
        .map(|solid| solid.volume())
        .sum::<f64>();
    run(
        &mut s,
        json!({"op":"thread","body":body,"face":[3,0,5],"thread":"M6","allowance":0.2}),
    );
    let b = &s.built.bodies[0];
    let core = (threads::find("M6").unwrap().minor() - 0.2) / 2. - threads::BED;
    let removed = std::f64::consts::PI * ((9. - core * core) * 10. + core * core * threads::BED);
    let after = b.solids.iter().map(|solid| solid.volume()).sum::<f64>();
    assert!((before - after - removed).abs() < 1e-6);
    assert_eq!(b.mesh.open_edges(), 0);
}

fn radius_at(mesh: &fr_core::mesh::Mesh, z: f64, angle: f64) -> f64 {
    let origin = DVec3::Z * z;
    let direction = DVec3::new(angle.cos(), angle.sin(), 0.);
    mesh.tris()
        .filter_map(|t| {
            let (a, b) = (t[1] - t[0], t[2] - t[0]);
            let cross = direction.cross(b);
            let det = a.dot(cross);
            if det.abs() < 1e-14 {
                return None;
            }
            let delta = origin - t[0];
            let u = delta.dot(cross) / det;
            let q = delta.cross(a);
            let v = direction.dot(q) / det;
            let distance = b.dot(q) / det;
            (u >= -1e-10 && v >= -1e-10 && u + v <= 1. + 1e-10 && distance >= 0.)
                .then_some(distance)
        })
        .fold(f64::INFINITY, f64::min)
}

#[test]
fn mating_meshes_have_clearance_on_flanks_and_beyond_the_lead() {
    for name in ["M3", "M6", "M12", "1/4-20"] {
        let spec = threads::find(name).unwrap();
        let length = 6. * spec.pitch;
        for left in [false, true] {
            let screw =
                threads::rod_with_lead(spec.major - 0.2, spec.pitch, length, left, [true, true])
                    .unwrap();
            let nut = threads::sleeve(
                spec.major + 0.2,
                spec.tap_drill + 0.2,
                spec.major + 0.2 + 2. * threads::BED,
                spec.pitch,
                length,
                left,
            )
            .unwrap();
            let mut minimum_gap = f64::INFINITY;
            // Sample several phases on both flank slopes, crests and roots, as
            // well as sections after the one-pitch lead has reached full height.
            for fraction in [
                0.125, 0.375, 0.625, 0.875, 1.125, 1.375, 2.625, 3.875, 5.625,
            ] {
                for step in 0..32 {
                    let angle = (step as f64 + 0.37) * std::f64::consts::TAU / 32.;
                    let (outside, inside) = (
                        radius_at(&screw, spec.pitch * fraction, angle),
                        radius_at(&nut, spec.pitch * fraction, angle),
                    );
                    assert!(outside.is_finite() && inside.is_finite());
                    minimum_gap = minimum_gap.min(inside - outside);
                }
            }
            assert!(
                minimum_gap > 0.02,
                "{name}, left={left}: sampled radial mesh clearance {minimum_gap}"
            );
        }
    }
}
