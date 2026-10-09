//! Sweeps at the kernel level: volumes, bounds, corners, refusals and face tags.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use fr_core::exact::{self, Lumps};
use fr_core::profile::{self, Chain, Profile};
use fr_core::sketch::{Geom, Plane, Sketch};
use fr_core::tag::Origin;
use glam::{DVec2, DVec3};

fn v(x: f64, y: f64) -> DVec2 { DVec2::new(x, y) }

/// A sketch on the YZ plane moved to `x` (its normal is +X) holding a square of half-width `h` centred at `(y, z)`.
fn square(x: f64, y: f64, z: f64, h: f64) -> (Sketch, Plane) {
    let plane = Plane { origin: DVec3::new(x, 0.0, 0.0), ..Plane::YZ };
    let mut sk = Sketch::new(plane);
    let (a, c) = (sk.add_point(v(y - h, z - h)), sk.add_point(v(y + h, z + h)));
    sk.add_rect(a, c, false);
    (sk, plane)
}

/// A path on XY through `points`, as lines.
fn polyline(points: &[(f64, f64)]) -> Sketch {
    let mut sk = Sketch::new(Plane::XY);
    let ids: Vec<_> = points.iter().map(|p| sk.add_point(v(p.0, p.1))).collect();
    for w in ids.windows(2) { sk.add_line(w[0], w[1]); }
    sk
}

fn run(profile: &(Sketch, Plane), path: &Sketch, follow: bool) -> Result<(Lumps, Vec<Profile>, Chain), String> {
    let all = profile::profiles(&profile.0);
    let chain = profile::chain(path, &[])?;
    let picked: Vec<&Profile> = all.iter().filter(|p| p.depth == 0).collect();
    let lumps = exact::sweep(&picked, &profile.1, &chain, &path.plane, follow)?;
    Ok((lumps, all, chain))
}

fn volume(l: &Lumps) -> f64 { l.iter().map(|s| s.volume()).sum() }

fn close(a: f64, b: f64) { assert!((a - b).abs() < 1e-6 * b.abs().max(1.0), "{a} is not {b}"); }

fn bounds(l: &Lumps) -> (DVec3, DVec3) { exact::bounds(l).unwrap() }

fn near(a: DVec3, b: (f64, f64, f64)) { assert!(a.distance(DVec3::new(b.0, b.1, b.2)) < 1e-6, "{a:?} is not {b:?}"); }

#[test]
fn a_straight_path_is_an_extrusion() {
    let (l, ..) = run(&square(0.0, 0.0, 0.0, 1.0), &polyline(&[(0.0, 0.0), (10.0, 0.0)]), true).unwrap();
    assert_eq!(l.len(), 1);
    close(volume(&l), 40.0);
    let (lo, hi) = bounds(&l);
    near(lo, (0.0, -1.0, -1.0));
    near(hi, (10.0, 1.0, 1.0));
}

#[test]
fn a_tangent_arc_is_followed_exactly() {
    let mut path = polyline(&[(0.0, 0.0), (10.0, 0.0)]);
    let (c, s, e) = (path.add_point(v(10.0, 5.0)), path.point_at(v(10.0, 0.0), 1e-6), path.add_point(v(15.0, 5.0)));
    path.add(Geom::Arc { c, s, e }, false);
    let (l, ..) = run(&square(0.0, 0.0, 0.0, 1.0), &path, true).unwrap();
    close(volume(&l), 4.0 * (10.0 + FRAC_PI_2 * 5.0));
    // The kernel's box around a curved face is a little loose.
    let (lo, hi) = bounds(&l);
    assert!(lo.distance(DVec3::new(0.0, -1.0, -1.0)) < 0.02 && hi.distance(DVec3::new(16.0, 5.0, 1.0)) < 0.02, "{lo:?} {hi:?}");
    // A profile to the left of the path travels a shorter way round the bend.
    let (l, ..) = run(&square(0.0, 3.0, 0.0, 1.0), &path, true).unwrap();
    close(volume(&l), 4.0 * (10.0 + FRAC_PI_2 * 2.0));
}

#[test]
fn a_sharp_corner_is_mitred() {
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let (l, ..) = run(&square(0.0, 0.0, 0.0, 1.0), &path, true).unwrap();
    assert_eq!(l.len(), 1, "the two legs join into one body");
    close(volume(&l), 80.0);
    let (lo, hi) = bounds(&l);
    near(lo, (0.0, -1.0, -1.0));
    near(hi, (11.0, 10.0, 1.0));
    // The outside of the corner is filled and the inside is not doubled.
    assert!(l[0].contains(cadrum::DVec3::new(10.9, -0.9, 0.0)));
    assert!(!l[0].contains(cadrum::DVec3::new(8.5, 1.5, 0.0)));
    // Off the path: the inside leg is shorter by the reach times tan(45 degrees), at each side of the corner.
    let (l, ..) = run(&square(0.0, 3.0, 0.0, 1.0), &path, true).unwrap();
    close(volume(&l), 4.0 * (20.0 - 2.0 * 3.0));
    // A gentler and a sharper corner.
    for degrees in [30.0f64, 120.0] {
        let (s, c) = degrees.to_radians().sin_cos();
        let (l, ..) = run(&square(0.0, 0.0, 0.0, 1.0), &polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0 + 10.0 * c, 10.0 * s)]), true).unwrap();
        close(volume(&l), 80.0);
    }
}

#[test]
fn the_profile_may_sit_anywhere_along_the_path() {
    // In the middle of the first leg.
    let (l, ..) = run(&square(0.0, 0.0, 0.0, 1.0), &polyline(&[(-5.0, 0.0), (10.0, 0.0), (10.0, 10.0)]), true).unwrap();
    close(volume(&l), 100.0);
    near(bounds(&l).0, (-5.0, -1.0, -1.0));
    // At the far end of a path drawn towards it.
    let (l, ..) = run(&square(0.0, 0.0, 0.0, 1.0), &polyline(&[(10.0, 10.0), (10.0, 0.0), (0.0, 0.0)]), true).unwrap();
    close(volume(&l), 80.0);
    near(bounds(&l).1, (11.0, 10.0, 1.0));
    // Off to one side of a path drawn towards it: the same solid as drawing the path away from it.
    let (l, ..) = run(&square(0.0, 3.0, 0.0, 1.0), &polyline(&[(10.0, 10.0), (10.0, 0.0), (0.0, 0.0)]), true).unwrap();
    close(volume(&l), 4.0 * 14.0);
}

#[test]
fn a_closed_path_makes_a_frame_or_a_ring() {
    // A 20 x 10 rectangle with the profile in the middle of its bottom side.
    let path = polyline(&[(-10.0, 0.0), (10.0, 0.0), (10.0, 10.0), (-10.0, 10.0), (-10.0, 0.0)]);
    let (l, _, chain) = run(&square(0.0, 0.0, 0.0, 1.0), &path, true).unwrap();
    assert!(chain.closed);
    assert_eq!(l.len(), 1);
    close(volume(&l), 4.0 * 60.0);
    let (lo, hi) = bounds(&l);
    near(lo, (-11.0, -1.0, -1.0));
    near(hi, (11.0, 11.0, 1.0));
    assert!(!l[0].contains(cadrum::DVec3::new(0.0, 5.0, 0.0)), "the middle of the frame is open");
    // A circle of radius 10 through the origin.
    let mut ring = Sketch::new(Plane::XY);
    let c = ring.add_point(v(0.0, 10.0));
    ring.add(Geom::Circle { c, r: 10.0 }, false);
    let (l, _, chain) = run(&square(0.0, 0.0, 0.0, 1.0), &ring, true).unwrap();
    assert!(chain.closed);
    close(volume(&l), 4.0 * TAU * 10.0);
}

#[test]
fn a_hole_in_the_profile_runs_the_whole_way() {
    let (mut sk, plane) = square(0.0, 0.0, 0.0, 2.0);
    let c = sk.add_point(v(0.0, 0.0));
    sk.add(Geom::Circle { c, r: 1.0 }, false);
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let (l, ..) = run(&(sk, plane), &path, true).unwrap();
    assert_eq!(l.len(), 1);
    // The profile's area is taken from its 72-sided outline of the circle.
    let hole = 36.0 * (TAU / 72.0).sin();
    assert!((volume(&l) - (16.0 - PI) * 20.0).abs() < 0.01 * 320.0, "{}", volume(&l));
    assert!((volume(&l) - (16.0 - hole) * 20.0).abs() < 0.5);
    assert!(!l[0].contains(cadrum::DVec3::new(5.0, 0.0, 0.0)), "the bore is open along the first leg");
    assert!(!l[0].contains(cadrum::DVec3::new(10.0, 5.0, 0.0)), "and along the second");
    assert!(l[0].contains(cadrum::DVec3::new(5.0, 1.5, 0.0)));
}

#[test]
fn a_fixed_profile_keeps_its_orientation() {
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0), (15.0, 5.0)]);
    let (l, ..) = run(&square(0.0, 0.0, 0.0, 1.0), &path, false).unwrap();
    // The section stays square to X, so the volume is its area times the travel along X.
    close(volume(&l), 60.0);
    let (lo, hi) = bounds(&l);
    near(lo, (0.0, -1.0, -1.0));
    near(hi, (15.0, 6.0, 1.0));
    // A path that turns side-on to the profile has no thickness there.
    let err = run(&square(0.0, 0.0, 0.0, 1.0), &polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]), false).unwrap_err();
    assert!(err.contains("fixed orientation"), "{err}");
}

#[test]
fn a_spline_path_is_followed() {
    let mut path = Sketch::new(Plane::XY);
    let pts = [v(0.0, 0.0), v(8.0, 4.0), v(16.0, -4.0), v(24.0, 0.0)].map(|p| path.add_point(p));
    path.add_spline(pts, false).unwrap();
    // The spline leaves the origin at an angle, so the profile plane is tilted to it; the check allows for that.
    let (l, ..) = run(&square(0.0, 0.0, 0.0, 1.0), &path, true).unwrap();
    assert_eq!(l.len(), 1);
    assert!(volume(&l) > 60.0 && volume(&l) < 4.0 * 30.0, "{}", volume(&l));
    let (lo, hi) = bounds(&l);
    assert!(lo.x < 0.5 && hi.x > 23.5 && hi.y > 2.0 && lo.y < -2.0, "{lo:?} {hi:?}");
}

#[test]
fn impossible_sweeps_are_refused_with_a_reason() {
    let unit = square(0.0, 0.0, 0.0, 1.0);
    // A bend of radius 0.5 with a profile reaching 1 to its inside.
    let mut tight = polyline(&[(0.0, 0.0), (10.0, 0.0)]);
    let (c, s, e) = (tight.add_point(v(10.0, 0.5)), tight.point_at(v(10.0, 0.0), 1e-6), tight.add_point(v(10.5, 0.5)));
    tight.add(Geom::Arc { c, s, e }, false);
    let err = run(&unit, &tight, true).unwrap_err();
    assert!(err.contains("bends more tightly"), "{err}");
    // The profile's plane contains the path.
    let mut flat = Sketch::new(Plane::XY);
    let (a, c) = (flat.add_point(v(-1.0, -1.0)), flat.add_point(v(1.0, 1.0)));
    flat.add_rect(a, c, false);
    let err = run(&(flat, Plane::XY), &polyline(&[(0.0, 0.0), (10.0, 0.0)]), true).unwrap_err();
    assert!(err.contains("runs along the path"), "{err}");
    // A hairpin.
    let err = run(&unit, &polyline(&[(0.0, 0.0), (10.0, 0.0), (0.0, 1.0)]), true).unwrap_err();
    assert!(err.contains("doubles back"), "{err}");
    // A middle stretch shorter than the mitres at its two ends need.
    let err = run(&square(0.0, 0.0, 0.0, 2.0), &polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 1.0), (0.0, 1.0)]), true).unwrap_err();
    assert!(err.contains("too short"), "{err}");
    // Paths that are not one run.
    let mut fork = polyline(&[(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)]);
    let (a, b) = (fork.point_at(v(10.0, 0.0), 1e-6), fork.add_point(v(10.0, 10.0)));
    fork.add_line(a, b);
    assert!(profile::chain(&fork, &[]).unwrap_err().contains("branches"));
    let mut apart = polyline(&[(0.0, 0.0), (10.0, 0.0)]);
    let (a, b) = (apart.add_point(v(0.0, 5.0)), apart.add_point(v(10.0, 5.0)));
    apart.add_line(a, b);
    assert!(profile::chain(&apart, &[]).unwrap_err().contains("separate pieces"));
    assert!(profile::chain(&Sketch::new(Plane::XY), &[]).unwrap_err().contains("no lines"));
}

#[test]
fn the_path_can_be_part_of_a_sketch() {
    // Naming the entities picks one run out of a branching sketch, in path order whatever order they are named in.
    let mut sk = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let (a, b) = (sk.point_at(v(10.0, 0.0), 1e-6), sk.add_point(v(20.0, 0.0)));
    let spur = sk.add_line(a, b);
    let ids: Vec<_> = sk.entities.keys().copied().filter(|id| *id != spur).collect();
    let chain = profile::chain(&sk, &[ids[1], ids[0]]).unwrap();
    assert_eq!(chain.ids, vec![ids[1], ids[0]], "an open path starts at the free end of the first entity named");
    assert_eq!(chain.segs[0].ends().0, v(10.0, 10.0));
    assert!(!chain.closed);
    assert!(profile::chain(&sk, &[ids[0], ids[0]]).unwrap_err().contains("twice"));
    assert!(profile::chain(&sk, &[9999]).unwrap_err().contains("no longer"));
}

#[test]
fn faces_are_named_by_the_profile_and_path_entities_that_made_them() {
    let profile = square(0.0, 0.0, 0.0, 1.0);
    // One piece: the same names an extrude gives.
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0)]);
    let (l, all, chain) = run(&profile, &path, true).unwrap();
    let picked: Vec<&Profile> = all.iter().collect();
    let tags = exact::tag_swept(&l, &picked, &profile.1, &chain, &path.plane, true, 7);
    let origins: Vec<&Origin> = tags[0].iter().map(|t| &t.as_ref().unwrap().origin).collect();
    assert_eq!(origins.iter().filter(|o| matches!(o, Origin::Swept { feature: 7, .. })).count(), 4);
    assert_eq!(origins.iter().filter(|o| matches!(o, Origin::ProfileCap { feature: 7, end: false, .. })).count(), 1);
    assert_eq!(origins.iter().filter(|o| matches!(o, Origin::ProfileCap { feature: 7, end: true, .. })).count(), 1);
    // Two legs. The four upright side faces each have their own name, from one profile entity and one
    // path entity. The top and the bottom are each one flat face across the corner, named by both legs.
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let (l, all, chain) = run(&profile, &path, true).unwrap();
    let picked: Vec<&Profile> = all.iter().collect();
    let tags = exact::tag_swept(&l, &picked, &profile.1, &chain, &path.plane, true, 7);
    assert_eq!(tags[0].len(), 8);
    let mut roles: Vec<String> = tags[0].iter().filter_map(|t| match &t.as_ref().unwrap().origin { Origin::Semantic { role, .. } => Some(role.clone()), _ => None }).collect();
    assert_eq!(roles.len(), 4, "{:?}", tags[0]);
    roles.sort();
    roles.dedup();
    assert_eq!(roles.len(), 4, "no two faces share a name");
    assert!(roles.iter().all(|r| r.starts_with("sweep:")));
    let merged: Vec<usize> = tags[0].iter().filter_map(|t| match &t.as_ref().unwrap().origin { Origin::Merged { sources } => Some(sources.len()), _ => None }).collect();
    assert_eq!(merged, vec![2, 2]);
    assert_eq!(tags[0].iter().filter(|t| matches!(t.as_ref().unwrap().origin, Origin::ProfileCap { .. })).count(), 2);
    let mut all_tags: Vec<_> = tags[0].iter().flatten().cloned().collect();
    all_tags.sort();
    all_tags.dedup();
    assert_eq!(all_tags.len(), 8, "every face of the sweep has a distinct tag");
}
