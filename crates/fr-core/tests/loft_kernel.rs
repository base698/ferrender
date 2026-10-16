//! Lofts at the kernel level: volumes, section matching, refusals and face tags.

use std::f64::consts::PI;

use fr_core::exact::{self, Lumps, Section};
use fr_core::profile::{self, Profile};
use fr_core::sketch::{Geom, Plane, Sketch};
use fr_core::tag::Origin;
use glam::{DVec2, DVec3};

fn v(x: f64, y: f64) -> DVec2 { DVec2::new(x, y) }

fn at(z: f64) -> Plane { Plane { origin: DVec3::new(0.0, 0.0, z), ..Plane::XY } }

/// A square of half-width `h` centred on the plane's origin.
fn square(plane: Plane, h: f64) -> Sketch {
    let mut sk = Sketch::new(plane);
    let (a, c) = (sk.add_point(v(-h, -h)), sk.add_point(v(h, h)));
    sk.add_rect(a, c, false);
    sk
}

fn circle(plane: Plane, r: f64) -> Sketch {
    let mut sk = Sketch::new(plane);
    let c = sk.add_point(v(0.0, 0.0));
    sk.add(Geom::Circle { c, r }, false);
    sk
}

fn polygon(plane: Plane, points: &[(f64, f64)]) -> Sketch {
    let mut sk = Sketch::new(plane);
    let ids: Vec<_> = points.iter().map(|p| sk.add_point(v(p.0, p.1))).collect();
    for i in 0..ids.len() { sk.add_line(ids[i], ids[(i + 1) % ids.len()]); }
    sk
}

fn run(sketches: &[Sketch], ruled: bool) -> Result<(Lumps, Vec<Vec<Profile>>), String> {
    let all: Vec<Vec<Profile>> = sketches.iter().map(profile::profiles).collect();
    let sections: Vec<Section> = all.iter().zip(sketches).map(|(p, sk)| Section::Outline { profile: p.iter().find(|p| p.depth == 0).expect("a closed outline"), plane: sk.plane }).collect();
    Ok((exact::loft(&sections, ruled)?, all))
}

/// The extent of the triangles; the kernel's own box is padded.
fn extent(l: &Lumps) -> (DVec3, DVec3) { exact::tessellate(l).unwrap().0.bbox().unwrap() }

fn volume(l: &Lumps) -> f64 { l.iter().map(|s| s.volume()).sum() }

fn close(a: f64, b: f64, rel: f64) { assert!((a - b).abs() <= rel * b.abs().max(1.0), "{a} is not {b}"); }

/// A frustum between two similar sections of areas `a` and `b`, `h` apart.
fn frustum(a: f64, b: f64, h: f64) -> f64 { h / 3.0 * (a + b + (a * b).sqrt()) }

#[test]
fn two_squares_make_a_frustum() {
    let (l, _) = run(&[square(at(0.0), 5.0), square(at(10.0), 2.0)], true).unwrap();
    assert_eq!(l.len(), 1);
    close(volume(&l), frustum(100.0, 16.0, 10.0), 1e-9);
    let b = extent(&l);
    assert!((b.0 - DVec3::new(-5.0, -5.0, 0.0)).length() < 1e-6 && (b.1 - DVec3::new(5.0, 5.0, 10.0)).length() < 1e-6, "{b:?}");
}

#[test]
fn two_circles_make_a_cone_frustum() {
    for ruled in [true, false] {
        let (l, _) = run(&[circle(at(0.0), 5.0), circle(at(10.0), 2.0)], ruled).unwrap();
        close(volume(&l), frustum(PI * 25.0, PI * 4.0, 10.0), 1e-6);
    }
}

#[test]
fn a_section_drawn_from_another_corner_or_the_other_way_round_is_not_twisted() {
    let straight = frustum(100.0, 16.0, 10.0);
    // The same top square on a plane turned a quarter turn, so its outline starts at the next corner.
    let turned = Plane { origin: DVec3::new(0.0, 0.0, 10.0), x: DVec3::Y, y: -DVec3::X };
    let (l, _) = run(&[square(at(0.0), 5.0), square(turned, 2.0)], true).unwrap();
    close(volume(&l), straight, 1e-9);
    // On a plane facing down, so its outline runs the other way round.
    let down = Plane { origin: DVec3::new(0.0, 0.0, 10.0), x: DVec3::X, y: -DVec3::Y };
    assert!(down.normal().z < 0.0);
    let (l, _) = run(&[square(at(0.0), 5.0), square(down, 2.0)], true).unwrap();
    close(volume(&l), straight, 1e-9);
    // Circles on opposed planes.
    let (l, _) = run(&[circle(at(0.0), 5.0), circle(down, 2.0)], true).unwrap();
    close(volume(&l), frustum(PI * 25.0, PI * 4.0, 10.0), 1e-6);
}

#[test]
fn a_section_off_to_one_side_keeps_its_corners_matched() {
    // An oblique frustum has the volume of the upright one.
    let aside = Plane { origin: DVec3::new(30.0, 4.0, 10.0), x: DVec3::Y, y: -DVec3::X };
    let (l, _) = run(&[square(at(0.0), 5.0), square(aside, 2.0)], true).unwrap();
    close(volume(&l), frustum(100.0, 16.0, 10.0), 1e-9);
}

#[test]
fn three_sections_pass_through_the_middle_one() {
    let sketches = [square(at(0.0), 2.0), square(at(10.0), 6.0), square(at(20.0), 2.0)];
    let (ruled, _) = run(&sketches, true).unwrap();
    close(volume(&ruled), 2.0 * frustum(16.0, 144.0, 10.0), 1e-9);
    let (smooth, _) = run(&sketches, false).unwrap();
    let b = extent(&smooth);
    assert!(b.0.z.abs() < 1e-6 && (b.1.z - 20.0).abs() < 1e-6, "{b:?}");
    assert!(b.1.x >= 6.0 - 1e-6 && b.1.x < 7.5, "the smooth loft reaches the widest section: {b:?}");
    // A curve through the three sections bulges past the straight walls near the ends.
    assert!(volume(&smooth) > volume(&ruled), "{} vs {}", volume(&smooth), volume(&ruled));
    // A smooth loft has one wall per edge, a ruled one a wall per edge between each pair of sections.
    assert_eq!(smooth[0].iter_face().count(), 4 + 2);
    assert_eq!(ruled[0].iter_face().count(), 8 + 2);
}

#[test]
fn sections_on_tilted_planes_loft() {
    let tilted = Plane::from_normal(DVec3::new(0.0, 0.0, 12.0), DVec3::new(0.3, 0.0, 1.0).normalize());
    let (l, _) = run(&[square(at(0.0), 4.0), square(tilted, 4.0)], true).unwrap();
    assert!(volume(&l) > 0.0);
    let (l, _) = run(&[circle(at(0.0), 4.0), circle(tilted, 3.0)], false).unwrap();
    assert!(volume(&l) > 0.0);
}

#[test]
fn unequal_sections_are_refused_with_their_counts() {
    let triangle = polygon(at(10.0), &[(-3.0, -2.0), (3.0, -2.0), (0.0, 3.0)]);
    let e = run(&[square(at(0.0), 5.0), triangle], true).unwrap_err();
    assert!(e.contains("section 2 has 3 edges where section 1 has 4 edges"), "{e}");
    let e = run(&[square(at(0.0), 5.0), circle(at(10.0), 2.0)], false).unwrap_err();
    assert!(e.contains("section 2 has 1 edge where section 1 has 4 edges"), "{e}");
}

#[test]
fn too_few_coplanar_and_holed_sections_are_refused() {
    let e = run(&[square(at(0.0), 5.0)], true).unwrap_err();
    assert!(e.contains("at least two sections"), "{e}");
    let e = run(&[square(at(0.0), 5.0), square(at(0.0), 2.0)], true).unwrap_err();
    assert!(e.contains("sections 1 and 2 lie on the same plane"), "{e}");
    // The first and last may share a plane only through one between them; neighbours may not.
    let e = run(&[square(at(0.0), 5.0), square(at(5.0), 3.0), square(at(5.0), 2.0)], true).unwrap_err();
    assert!(e.contains("sections 2 and 3 lie on the same plane"), "{e}");
    let mut holed = square(at(10.0), 4.0);
    let c = holed.add_point(v(0.0, 0.0));
    holed.add(Geom::Circle { c, r: 1.0 }, false);
    let e = run(&[square(at(0.0), 5.0), holed], true).unwrap_err();
    assert!(e.contains("section 2 has a hole"), "{e}");
}

#[test]
fn faces_are_named_by_the_first_section() {
    let sketches = [square(at(0.0), 5.0), square(at(10.0), 2.0), square(at(20.0), 4.0)];
    for ruled in [true, false] {
        let (l, all) = run(&sketches, ruled).unwrap();
        let sections: Vec<Section> = all.iter().zip(&sketches).map(|(p, sk)| Section::Outline { profile: &p[0], plane: sk.plane }).collect();
        let tags = exact::tag_loft(&l, &sections, 7);
        let tags: Vec<_> = tags[0].iter().map(|t| t.clone().expect("every face is named")).collect();
        let caps: Vec<bool> = tags.iter().filter_map(|t| match &t.origin { Origin::ProfileCap { feature: 7, end, .. } => Some(*end), _ => None }).collect();
        assert_eq!(caps.len(), 2, "{tags:?}");
        assert!(caps.contains(&false) && caps.contains(&true));
        let mut walls: Vec<_> = tags.iter().filter_map(|t| match &t.origin { Origin::Swept { feature: 7, entity } => Some(*entity), _ => None }).collect();
        assert_eq!(walls.len(), tags.len() - 2, "every wall is named by an edge of the first section: {tags:?}");
        walls.sort();
        walls.dedup();
        let mut edges = all[0][0].path_ids.clone();
        edges.sort();
        assert_eq!(walls, edges);
    }
}
