use std::collections::BTreeMap;

use fr_core::{Plane, mesh::{self, Mesh}, profile::Profile};
use glam::{DVec2, DVec3};

fn rectangle(width: f64, depth: f64, center: DVec2) -> Vec<DVec2> {
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| center + DVec2::new(x * width / 2.0, y * depth / 2.0)).to_vec()
}

fn profile(width: f64, depth: f64) -> Profile {
    Profile { outer: rectangle(width, depth, DVec2::ZERO), holes: Vec::new(), edges: Vec::new(), depth: 0, path: Vec::new(), hole_paths: Vec::new() }
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() <= 1e-8 * expected.abs().max(1.0), "{actual} != {expected}");
}

// Integral of (width + 2*t*z)*(depth + 2*t*z) from the sketch to h.
// This independently describes the volume of one rectangular frustum.
fn frustum(width: f64, depth: f64, t: f64, h: f64) -> f64 {
    width * depth * h + (width + depth) * t * h * h + 4.0 / 3.0 * t * t * h * h * h
}

fn ring_points(mesh: &Mesh, plane: &Plane, z: f64) -> Vec<DVec2> {
    mesh.tris().flatten().filter(|p| (((*p - plane.origin).dot(plane.normal())) - z).abs() < 1e-8).map(|p| plane.to_local(p)).collect()
}

fn assert_ring(mesh: &Mesh, plane: &Plane, z: f64, width: f64, depth: f64) {
    let points = ring_points(mesh, plane, z);
    assert!(!points.is_empty(), "missing ring at sketch-normal offset {z}");
    let (lo, hi) = points.iter().fold((DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    close(lo.x, -width / 2.0);
    close(hi.x, width / 2.0);
    close(lo.y, -depth / 2.0);
    close(hi.y, depth / 2.0);
}

fn assert_closed(mesh: &Mesh) {
    assert_eq!(mesh.open_edges(), 0);
    assert!(mesh.volume() > 0.0, "surface winding must point outward");
    // Require exactly two opposite uses of every edge, not just balanced
    // counts: duplicate internal caps must not pass as a closed surface.
    let key = |p: DVec3| ((p.x * 1e7).round() as i64, (p.y * 1e7).round() as i64, (p.z * 1e7).round() as i64);
    let mut edges = BTreeMap::<_, (usize, i32)>::new();
    for tri in mesh.tris() {
        assert!((tri[1] - tri[0]).cross(tri[2] - tri[0]).length_squared() > 1e-16);
        for i in 0..3 {
            let (a, b) = (key(tri[i]), key(tri[(i + 1) % 3]));
            let entry = edges.entry((a.min(b), a.max(b))).or_default();
            entry.0 += 1;
            entry.1 += if a < b { 1 } else { -1 };
        }
    }
    assert!(edges.values().all(|value| *value == (2, 0)), "mesh contains open, duplicated, or inconsistently oriented edges");
}

#[test]
fn symmetric_taper_keeps_the_sketch_section_and_tapers_both_halves() {
    let p = profile(20.0, 12.0);
    for angle in [-18.0_f64, 18.0] {
        let t = angle.to_radians().tan();
        for (z0, z1) in [(-6.0, 6.0), (6.0, -6.0)] {
            let mesh = mesh::extrude_tapered(&[&p], &Plane::XY, z0, z1, angle).unwrap();
            assert_ring(&mesh, &Plane::XY, 0.0, 20.0, 12.0);
            for z in [-6.0_f64, 6.0] { assert_ring(&mesh, &Plane::XY, z, 20.0 + 2.0 * z.abs() * t, 12.0 + 2.0 * z.abs() * t); }
            close(mesh.volume(), 2.0 * frustum(20.0, 12.0, t, 6.0));
            assert!(!mesh.tris().any(|tri| tri.iter().all(|p| p.z.abs() < 1e-8)), "the sketch plane must be a wall seam, not an internal cap");
            assert_closed(&mesh);
        }
    }
}

#[test]
fn unequal_spans_crossing_the_sketch_have_two_independent_slopes() {
    let p = profile(16.0, 10.0);
    for angle in [-12.0_f64, 12.0] {
        let t = angle.to_radians().tan();
        let mesh = mesh::extrude_tapered(&[&p], &Plane::XY, -3.0, 8.0, angle).unwrap();
        for z in [-3.0_f64, 0.0, 8.0] { assert_ring(&mesh, &Plane::XY, z, 16.0 + 2.0 * z.abs() * t, 10.0 + 2.0 * z.abs() * t); }
        close(mesh.volume(), frustum(16.0, 10.0, t, 3.0) + frustum(16.0, 10.0, t, 8.0));
        assert_closed(&mesh);
    }
}

#[test]
fn symmetric_taper_keeps_multiple_holes_open_and_oriented() {
    let mut p = profile(24.0, 20.0);
    for x in [-6.0, 6.0] {
        let mut hole = rectangle(4.0, 4.0, DVec2::new(x, 0.0));
        hole.reverse();
        p.holes.push(hole);
    }
    for angle in [-20.0_f64, 20.0] {
        let t = angle.to_radians().tan();
        let mesh = mesh::extrude_tapered(&[&p], &Plane::XY, -4.0, 4.0, angle).unwrap();
        close(mesh.volume(), 2.0 * (frustum(24.0, 20.0, t, 4.0) - 2.0 * frustum(4.0, 4.0, -t, 4.0)));
        for z in [-4.0_f64, 0.0, 4.0] {
            let points = ring_points(&mesh, &Plane::XY, z);
            let hole_half_width = 2.0 - z.abs() * t;
            for x in [-6.0, 6.0] {
                for dx in [-hole_half_width, hole_half_width] {
                    for y in [-hole_half_width, hole_half_width] {
                        assert!(points.iter().any(|p| p.distance(DVec2::new(x + dx, y)) < 1e-8), "missing hole corner at z={z}");
                    }
                }
            }
        }
        assert_closed(&mesh);
    }
}

#[test]
fn one_sided_taper_keeps_its_existing_frustum_geometry() {
    let p = profile(20.0, 12.0);
    for angle in [-15.0_f64, 15.0] {
        let t = angle.to_radians().tan();
        for distance in [-7.0_f64, 7.0] {
            let mesh = mesh::extrude_tapered(&[&p], &Plane::XY, 0.0, distance, angle).unwrap();
            assert_ring(&mesh, &Plane::XY, 0.0, 20.0, 12.0);
            assert_ring(&mesh, &Plane::XY, distance, 20.0 + 2.0 * distance.abs() * t, 12.0 + 2.0 * distance.abs() * t);
            close(mesh.volume(), frustum(20.0, 12.0, t, distance.abs()));
            assert_eq!(mesh.len(), 12, "one-sided rectangular extrusion should need no extra wall seam");
            assert_closed(&mesh);
        }
    }
}

#[test]
fn taper_uses_the_local_sketch_plane_on_rotated_offset_sketches() {
    let p = profile(20.0, 12.0);
    let plane = Plane::from_normal(DVec3::new(30.0, -40.0, 50.0), DVec3::new(1.0, 2.0, 3.0));
    let angle = -10.0_f64;
    let t = angle.to_radians().tan();
    let mesh = mesh::extrude_tapered(&[&p], &plane, -5.0, 5.0, angle).unwrap();
    for z in [-5.0_f64, 0.0, 5.0] { assert_ring(&mesh, &plane, z, 20.0 + 2.0 * z.abs() * t, 12.0 + 2.0 * z.abs() * t); }
    close(mesh.volume(), 2.0 * frustum(20.0, 12.0, t, 5.0));
    assert_closed(&mesh);
}

#[test]
fn zero_taper_remains_a_closed_prism() {
    let p = profile(20.0, 12.0);
    let mesh = mesh::extrude_tapered(&[&p], &Plane::XY, -5.0, 5.0, 0.0).unwrap();
    close(mesh.volume(), 20.0 * 12.0 * 10.0);
    assert_closed(&mesh);
}
