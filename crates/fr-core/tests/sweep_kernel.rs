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
    run_spans(profile, path, follow, &[])
}

/// Sweeps only the given parts of the path.
fn run_spans(profile: &(Sketch, Plane), path: &Sketch, follow: bool, spans: &[[f64; 2]]) -> Result<(Lumps, Vec<Profile>, Chain), String> {
    let all = profile::profiles(&profile.0);
    let chain = profile::chain(path, &[])?;
    let picked: Vec<&Profile> = all.iter().filter(|p| p.depth == 0).collect();
    let lumps = exact::sweep(&picked, &profile.1, &chain, &path.plane, follow, spans)?;
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
    let tags = exact::tag_swept(&l, &picked, &profile.1, &chain, &path.plane, true, &[], 7);
    let origins: Vec<&Origin> = tags[0].iter().map(|t| &t.as_ref().unwrap().origin).collect();
    assert_eq!(origins.iter().filter(|o| matches!(o, Origin::Swept { feature: 7, .. })).count(), 4);
    assert_eq!(origins.iter().filter(|o| matches!(o, Origin::ProfileCap { feature: 7, end: false, .. })).count(), 1);
    assert_eq!(origins.iter().filter(|o| matches!(o, Origin::ProfileCap { feature: 7, end: true, .. })).count(), 1);
    // Two legs. The four upright side faces each have their own name, from one profile entity and one
    // path entity. The top and the bottom are each one flat face across the corner, named by both legs.
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let (l, all, chain) = run(&profile, &path, true).unwrap();
    let picked: Vec<&Profile> = all.iter().collect();
    let tags = exact::tag_swept(&l, &picked, &profile.1, &chain, &path.plane, true, &[], 7);
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

/// Each lump's bounds, sorted along X then Y.
fn boxes(l: &Lumps) -> Vec<(DVec3, DVec3)> {
    let mut b: Vec<(DVec3, DVec3)> = l.iter().map(|s| exact::bounds(std::slice::from_ref(s)).unwrap()).collect();
    b.sort_by(|a, b| (a.0.x, a.0.y).partial_cmp(&(b.0.x, b.0.y)).unwrap());
    b
}

#[test]
fn parts_of_a_straight_path() {
    let profile = square(0.0, 0.0, 0.0, 1.0);
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0)]);
    // 0.1 to 0.3 and 0.6 to 0.7 of a 10 mm path: 2 mm and 1 mm of bar.
    let (l, ..) = run_spans(&profile, &path, true, &[[0.1, 0.3], [0.6, 0.7]]).unwrap();
    assert_eq!(l.len(), 2, "two separate pieces");
    close(volume(&l), 4.0 * 3.0);
    let b = boxes(&l);
    near(b[0].0, (1.0, -1.0, -1.0));
    near(b[0].1, (3.0, 1.0, 1.0));
    near(b[1].0, (6.0, -1.0, -1.0));
    near(b[1].1, (7.0, 1.0, 1.0));
    // Only the start, only the end.
    let (l, ..) = run_spans(&profile, &path, true, &[[0.0, 0.25]]).unwrap();
    near(bounds(&l).1, (2.5, 1.0, 1.0));
    let (l, ..) = run_spans(&profile, &path, true, &[[0.75, 1.0]]).unwrap();
    near(bounds(&l).0, (7.5, -1.0, -1.0));
    close(volume(&l), 10.0);
    // The order they are given in does not matter, and a fixed orientation cuts the same pieces here.
    let (l, ..) = run_spans(&profile, &path, false, &[[0.6, 0.7], [0.1, 0.3]]).unwrap();
    assert_eq!(l.len(), 2);
    close(volume(&l), 12.0);
}

#[test]
fn parts_are_joined_checked_and_the_whole_path_is_unchanged() {
    let profile = square(0.0, 0.0, 0.0, 1.0);
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let whole = run(&profile, &path, true).unwrap().0;
    // Nothing, and all of it, are the whole path.
    for spans in [&[][..], &[[0.0, 1.0]][..], &[[0.0, 0.4], [0.4, 1.0]][..], &[[0.0, 0.7], [0.3, 1.0]][..]] {
        let (l, ..) = run_spans(&profile, &path, true, spans).unwrap();
        assert_eq!(l.len(), 1);
        close(volume(&l), volume(&whole));
    }
    // Touching and overlapping parts are one part.
    assert_eq!(exact::sweep_spans(&[[0.1, 0.3], [0.3, 0.5]]).unwrap(), vec![Some((0.1, 0.5))]);
    assert_eq!(exact::sweep_spans(&[[0.4, 0.9], [0.1, 0.5]]).unwrap(), vec![Some((0.1, 0.9))]);
    assert_eq!(exact::sweep_spans(&[]).unwrap(), vec![None]);
    assert_eq!(exact::sweep_spans(&[[0.0, 1.0]]).unwrap(), vec![None]);
    // Refusals.
    for bad in [[0.5, 0.2], [0.3, 0.3], [-0.5, 0.5], [0.5, 1.5], [f64::NAN, 0.5]] {
        assert!(exact::sweep_spans(&[bad]).is_err(), "{bad:?}");
        assert!(run_spans(&profile, &path, true, &[bad]).is_err(), "{bad:?}");
    }
}

#[test]
fn a_part_beyond_a_corner_is_where_the_whole_sweep_would_be() {
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    // 0.6 to 0.9 of 20 mm is y = 2 to 8 on the second leg. The profile was drawn at the start of the first.
    let (l, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &path, true, &[[0.6, 0.9]]).unwrap();
    assert_eq!(l.len(), 1);
    close(volume(&l), 4.0 * 6.0);
    near(bounds(&l).0, (9.0, 2.0, -1.0));
    near(bounds(&l).1, (11.0, 8.0, 1.0));
    // A profile 3 to the left of the path is 3 to the left of the second leg too: around x = 7.
    let (l, ..) = run_spans(&square(0.0, 3.0, 0.0, 1.0), &path, true, &[[0.6, 0.9]]).unwrap();
    close(volume(&l), 24.0);
    near(bounds(&l).0, (6.0, 2.0, -1.0));
    near(bounds(&l).1, (8.0, 8.0, 1.0));
    // A part that spans the corner is mitred there: the same as the whole sweep between the two cuts.
    let (l, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &path, true, &[[0.25, 0.75]]).unwrap();
    assert_eq!(l.len(), 1);
    close(volume(&l), 4.0 * 10.0);
    near(bounds(&l).0, (5.0, -1.0, -1.0));
    near(bounds(&l).1, (11.0, 5.0, 1.0));
    assert!(l[0].contains(cadrum::DVec3::new(10.9, -0.9, 0.0)), "the outside of the corner is filled");
    // One part on each leg, neither touching the corner.
    let (l, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &path, true, &[[0.1, 0.3], [0.6, 0.7]]).unwrap();
    assert_eq!(l.len(), 2);
    close(volume(&l), 4.0 * 6.0);
    let b = boxes(&l);
    near(b[0].0, (2.0, -1.0, -1.0));
    near(b[0].1, (6.0, 1.0, 1.0));
    near(b[1].0, (9.0, 2.0, -1.0));
    near(b[1].1, (11.0, 4.0, 1.0));
    // A fixed profile is side-on to the second leg, so a part there is refused, but a part on the first leg is fine.
    assert!(run_spans(&square(0.0, 0.0, 0.0, 1.0), &path, false, &[[0.6, 0.9]]).unwrap_err().contains("fixed orientation"));
    let (l, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &path, false, &[[0.1, 0.4]]).unwrap();
    close(volume(&l), 4.0 * 6.0);
    // A hairpin elsewhere on the path does not stop a part that avoids it.
    let hairpin = polyline(&[(0.0, 0.0), (10.0, 0.0), (0.0, 1.0)]);
    assert!(run(&square(0.0, 0.0, 0.0, 1.0), &hairpin, true).is_err());
    let (l, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &hairpin, true, &[[0.05, 0.3]]).unwrap();
    assert_eq!(l.len(), 1);
}

#[test]
fn parts_of_arcs_circles_and_splines() {
    use std::f64::consts::FRAC_PI_2;
    // A 10 mm line then a quarter arc of radius 5 turning left: 17.85 mm in all.
    let mut path = polyline(&[(0.0, 0.0), (10.0, 0.0)]);
    let (c, s, e) = (path.add_point(v(10.0, 5.0)), path.point_at(v(10.0, 0.0), 1e-6), path.add_point(v(15.0, 5.0)));
    path.add(Geom::Arc { c, s, e }, false);
    let total = 10.0 + FRAC_PI_2 * 5.0;
    // From the middle of the line to the middle of the arc.
    let (l, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &path, true, &[[5.0 / total, (10.0 + FRAC_PI_2 * 2.5) / total]]).unwrap();
    assert_eq!(l.len(), 1);
    close(volume(&l), 4.0 * (5.0 + FRAC_PI_2 * 2.5));
    // It ends half way round the bend, at 45 degrees: the end of the path there is (10 + 5 sin 45, 5 - 5 cos 45).
    let h = std::f64::consts::FRAC_1_SQRT_2;
    assert!(l[0].contains(cadrum::DVec3::new(10.0 + 5.0 * h - 0.1 * h, 5.0 - 5.0 * h - 0.1 * h, 0.0)), "just inside the end");
    assert!(!l[0].contains(cadrum::DVec3::new(10.0 + 5.0 * h + 0.1 * h, 5.0 - 5.0 * h + 0.1 * h, 0.0)), "just past the end");
    assert!(!l[0].contains(cadrum::DVec3::new(4.9, 0.0, 0.0)), "before the start");
    // Only on the arc, with the profile off to the left: the inside of the bend is shorter.
    let (l, ..) = run_spans(&square(0.0, 3.0, 0.0, 1.0), &path, true, &[[10.0 / total, 1.0]]).unwrap();
    close(volume(&l), 4.0 * FRAC_PI_2 * 2.0);
    // A quarter of a ring. On a circle the fractions start at the sketch's +X side and run anticlockwise.
    let mut ring = Sketch::new(Plane::XY);
    let c = ring.add_point(v(0.0, 10.0));
    ring.add(Geom::Circle { c, r: 10.0 }, false);
    let (l, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &ring, true, &[[0.0, 0.25]]).unwrap();
    assert_eq!(l.len(), 1);
    close(volume(&l), 4.0 * TAU * 10.0 / 4.0);
    assert!(l[0].contains(cadrum::DVec3::new(10.0 * h, 10.0 + 10.0 * h, 0.0)), "the middle of the first quarter, up and to the right of the centre");
    assert!(!l[0].contains(cadrum::DVec3::new(-10.0 * h, 10.0 + 10.0 * h, 0.0)));
    assert!(!l[0].contains(cadrum::DVec3::new(0.0, 0.0, 0.0)), "the profile's own place is not in this part");
    // Two arcs of the ring.
    let (l, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &ring, true, &[[0.1, 0.3], [0.6, 0.7]]).unwrap();
    assert_eq!(l.len(), 2);
    close(volume(&l), 4.0 * TAU * 10.0 * 0.3);
    // A spline: a part of it is a part of the whole sweep, to the accuracy of the refitted curve.
    let mut curve = Sketch::new(Plane::XY);
    let pts = [v(0.0, 0.0), v(8.0, 4.0), v(16.0, -4.0), v(24.0, 0.0)].map(|p| curve.add_point(p));
    curve.add_spline(pts, false).unwrap();
    let whole = volume(&run(&square(0.0, 0.0, 0.0, 1.0), &curve, true).unwrap().0);
    let (first, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &curve, true, &[[0.0, 0.4]]).unwrap();
    let (rest, ..) = run_spans(&square(0.0, 0.0, 0.0, 1.0), &curve, true, &[[0.4, 1.0]]).unwrap();
    assert!((volume(&first) + volume(&rest) - whole).abs() < 1e-3 * whole, "{} + {} against {whole}", volume(&first), volume(&rest));
    assert!((volume(&first) - 0.4 * whole).abs() < 0.02 * whole, "{} is about 0.4 of {whole}", volume(&first));
}

#[test]
fn a_hole_runs_through_each_part_and_every_face_has_its_own_name() {
    let (mut sk, plane) = square(0.0, 0.0, 0.0, 2.0);
    let c = sk.add_point(v(0.0, 0.0));
    sk.add(Geom::Circle { c, r: 1.0 }, false);
    let path = polyline(&[(0.0, 0.0), (10.0, 0.0)]);
    let profile = (sk, plane);
    let (l, all, chain) = run_spans(&profile, &path, true, &[[0.1, 0.3], [0.6, 0.7]]).unwrap();
    assert_eq!(l.len(), 2);
    assert!(l.iter().all(|s| !s.contains(cadrum::DVec3::new(2.0, 0.0, 0.0)) && !s.contains(cadrum::DVec3::new(6.5, 0.0, 0.0))), "the bore is open in both");
    assert!(l.iter().any(|s| s.contains(cadrum::DVec3::new(2.0, 1.5, 0.0))));
    let picked: Vec<&Profile> = all.iter().filter(|p| p.depth == 0).collect();
    let tags = exact::tag_swept(&l, &picked, &profile.1, &chain, &path.plane, true, &[[0.1, 0.3], [0.6, 0.7]], 7);
    let mut names: Vec<_> = tags.iter().flatten().flatten().cloned().collect();
    let faces = names.len();
    assert_eq!(faces, 14, "four sides, a bore and two caps on each piece");
    names.sort();
    names.dedup();
    assert_eq!(names.len(), faces, "no two faces share a name, across the two parts");
    let roles: Vec<String> = tags.iter().flatten().flatten().filter_map(|t| match &t.origin { Origin::Semantic { role, .. } => Some(role.clone()), _ => None }).collect();
    assert_eq!(roles.len(), faces, "{tags:?}");
    for part in 0..2 {
        assert_eq!(roles.iter().filter(|r| r.starts_with("sweep:cap:") && r.contains(&format!(":{part}:start:"))).count(), 1);
        assert_eq!(roles.iter().filter(|r| r.starts_with("sweep:cap:") && r.contains(&format!(":{part}:end:"))).count(), 1);
        assert_eq!(roles.iter().filter(|r| !r.starts_with("sweep:cap:") && r.ends_with(&format!(":{part}"))).count(), 5);
    }
    // The whole path keeps the names it had before parts existed.
    let (l, ..) = run(&profile, &path, true).unwrap();
    let tags = exact::tag_swept(&l, &picked, &profile.1, &chain, &path.plane, true, &[], 7);
    assert_eq!(tags[0].iter().flatten().filter(|t| matches!(t.origin, Origin::Swept { .. })).count(), 5);
    assert_eq!(tags[0].iter().flatten().filter(|t| matches!(t.origin, Origin::ProfileCap { .. })).count(), 2);
}
