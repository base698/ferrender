use super::*;

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-8 * expected.abs().max(1.0),
        "{actual} != {expected}"
    );
}

fn beam() -> Frame {
    Frame {
        nodes: vec![
            Node {
                position: [0.0, 0.0],
                restrained: [true, true, false],
                load: [0.0; 3],
            },
            Node {
                position: [3000.0, 0.0],
                restrained: [false, true, false],
                load: [0.0; 3],
            },
        ],
        members: vec![Member {
            nodes: [0, 1],
            e: 10000.0,
            area: 38.0 * 184.0,
            inertia: 38.0 * 184.0_f64.powi(3) / 12.0,
            uniform_load: [0.0, -1.0],
        }],
    }
}

#[test]
fn simply_supported_uniform_load_matches_closed_form() {
    let f = beam();
    let out = f.solve().unwrap();
    let ei = f.members[0].e * f.members[0].inertia;
    close(out.reactions[0][1], 1500.0);
    close(out.reactions[1][1], 1500.0);
    close(out.members[0].max_abs_moment, 3000.0_f64.powi(2) / 8.0);
    close(out.members[0].max_abs_shear, 1500.0);
    close(
        out.members[0].samples[SAMPLE_INTERVALS / 2].displacement[1],
        -5.0 * 3000.0_f64.powi(4) / (384.0 * ei),
    );
}

#[test]
fn cantilever_tip_force_and_rotation_match_closed_form() {
    let mut f = beam();
    f.nodes[0].restrained = [true; 3];
    f.nodes[1].restrained = [false; 3];
    f.nodes[1].load[1] = -1000.0;
    f.members[0].uniform_load = [0.0; 2];
    let out = f.solve().unwrap();
    let ei = f.members[0].e * f.members[0].inertia;
    close(
        out.displacements[1][1],
        -1000.0 * 3000.0_f64.powi(3) / (3.0 * ei),
    );
    close(
        out.displacements[1][2],
        -1000.0 * 3000.0_f64.powi(2) / (2.0 * ei),
    );
    close(out.reactions[0][2], 3_000_000.0);
    close(out.members[0].max_abs_moment, 3_000_000.0);
}

#[test]
fn fixed_fixed_uniform_load_has_interior_sag_despite_zero_nodal_motion() {
    let mut f = beam();
    for n in &mut f.nodes {
        n.restrained = [true; 3];
    }
    let out = f.solve().unwrap();
    let ei = f.members[0].e * f.members[0].inertia;
    close(
        out.members[0].samples[SAMPLE_INTERVALS / 2].displacement[1],
        -3000.0_f64.powi(4) / (384.0 * ei),
    );
    close(out.reactions[0][2], 3000.0_f64.powi(2) / 12.0);
    close(out.reactions[1][2], -3000.0_f64.powi(2) / 12.0);
}

#[test]
fn center_point_load_and_multiple_elements() {
    let mut f = beam();
    f.nodes.push(Node {
        position: [1500.0, 0.0],
        restrained: [false; 3],
        load: [0.0, -1000.0, 0.0],
    });
    f.members[0].nodes = [0, 2];
    f.members[0].uniform_load = [0.0; 2];
    f.members.push(Member {
        nodes: [2, 1],
        ..f.members[0].clone()
    });
    let out = f.solve().unwrap();
    let ei = f.members[0].e * f.members[0].inertia;
    close(
        out.displacements[2][1],
        -1000.0 * 3000.0_f64.powi(3) / (48.0 * ei),
    );
    close(out.reactions[0][1], 500.0);
    close(out.members[0].max_abs_moment, 750_000.0);
}

#[test]
fn two_span_continuous_beam_transfers_moment_across_joint() {
    let mut f = beam();
    f.nodes.push(Node {
        position: [6000.0, 0.0],
        ..f.nodes[1].clone()
    });
    f.members.push(Member {
        nodes: [1, 2],
        ..f.members[0].clone()
    });
    let out = f.solve().unwrap();
    close(out.reactions[0][1], 1125.0);
    close(out.reactions[1][1], 3750.0);
    close(out.reactions[2][1], 1125.0);
    close(out.members[0].end_forces[5].abs(), 1_125_000.0);
}

#[test]
fn rotation_preserves_response_including_axial_load() {
    let mut f = beam();
    f.nodes[0].restrained = [true; 3];
    f.nodes[1].restrained = [false; 3];
    f.nodes[1].load = [500.0, -1000.0, 0.0];
    f.members[0].uniform_load = [0.0; 2];
    let baseline = f.solve().unwrap();
    let angle = 0.7_f64;
    let rotate = |[x, y]: [f64; 2]| {
        [
            angle.cos() * x - angle.sin() * y,
            angle.sin() * x + angle.cos() * y,
        ]
    };
    for n in &mut f.nodes {
        n.position = rotate(n.position);
        let force = rotate([n.load[0], n.load[1]]);
        n.load[0] = force[0];
        n.load[1] = force[1];
    }
    let out = f.solve().unwrap();
    let expected = rotate([baseline.displacements[1][0], baseline.displacements[1][1]]);
    close(out.displacements[1][0], expected[0]);
    close(out.displacements[1][1], expected[1]);
    close(out.members[0].max_abs_axial, 500.0);
    close(
        baseline.displacements[1][0],
        500.0 * 3000.0 / (f.members[0].e * f.members[0].area),
    );
}

#[test]
fn invalid_and_unstable_models_are_errors() {
    let mut f = beam();
    f.nodes[0].restrained = [false; 3];
    f.nodes[1].restrained = [false; 3];
    assert!(f.solve().unwrap_err().contains("unstable"));
    f = beam();
    f.members[0].e = f64::NAN;
    assert!(f.solve().is_err());
    f = beam();
    f.nodes[1].position = f.nodes[0].position;
    assert!(f.solve().is_err());
    f = beam();
    f.members[0].nodes[1] = 100;
    assert!(f.solve().is_err());
    f = beam();
    f.nodes.push(f.nodes[1].clone());
    assert!(f.solve().unwrap_err().contains("isolated"));
}
