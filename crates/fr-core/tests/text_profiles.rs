use fr_core::{
    Plane, exact,
    profile::{Profile, signed_area},
    text::{self, Align},
};
use glam::DVec2;

fn bounds(profiles: &[Profile]) -> (DVec2, DVec2) {
    profiles.iter().flat_map(|p| &p.outer).copied().fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    )
}

#[test]
fn counters_and_disconnected_glyph_parts_are_kept() {
    for (letter, holes) in [("A", 1), ("B", 2), ("O", 1)] {
        let profiles = text::profiles(letter, 8., 0., Align::Left).unwrap();
        assert_eq!(profiles.len(), 1, "{letter}");
        let p = &profiles[0];
        assert_eq!(p.holes.len(), holes, "{letter}");
        assert_eq!(p.hole_paths.len(), holes);
        assert!(signed_area(&p.outer) > 0.);
        assert!(p.holes.iter().all(|hole| signed_area(hole) < 0.));
        assert!(p.area() > 0. && p.area() < signed_area(&p.outer));
        assert_eq!(p.path.len(), p.outer.len());
    }
    assert_eq!(text::profiles("i", 8., 0., Align::Left).unwrap().len(), 2);
    assert_eq!(text::profiles("j", 8., 0., Align::Left).unwrap().len(), 2);
    assert!(
        bounds(&text::profiles("j", 8., 0., Align::Left).unwrap())
            .0
            .y
            < 0.,
        "descenders belong below the baseline"
    );
    assert!(
        text::profiles("Café Ω Ж", 8., 0., Align::Left).is_ok(),
        "precomposed Latin, Greek and Cyrillic are available in Noto Sans"
    );
}

#[test]
fn dimensions_alignment_and_tracking_have_stable_meaning() {
    let h = text::profiles("H", 10., 0., Align::Left).unwrap();
    let (lo, hi) = bounds(&h);
    assert!(lo.x.abs() < 1e-12 && lo.y.abs() < 1e-12);
    assert!(
        (hi.y - 10.).abs() < 1e-10,
        "height means cap height, not em height"
    );
    let small = text::profiles("BO", 5., 0., Align::Left).unwrap();
    let large = text::profiles("BO", 10., 0., Align::Left).unwrap();
    assert!(
        (large.iter().map(Profile::area).sum::<f64>()
            / small.iter().map(Profile::area).sum::<f64>()
            - 4.)
            .abs()
            < 1e-10
    );
    let natural = bounds(&text::profiles("HI", 10., 0., Align::Left).unwrap());
    let spaced = bounds(&text::profiles("HI", 10., 2., Align::Left).unwrap());
    assert!((spaced.1.x - natural.1.x - 2.).abs() < 1e-10);
    let (lo, hi) = bounds(&text::profiles("CAD", 10., 0.5, Align::Center).unwrap());
    assert!((lo.x + hi.x).abs() < 1e-10);
    assert!(
        bounds(&text::profiles("CAD", 10., 0.5, Align::Right).unwrap())
            .1
            .x
            .abs()
            < 1e-10
    );
    assert_eq!(serde_json::to_value(Align::Center).unwrap(), "center");
    assert_eq!(
        serde_json::from_str::<Align>("\"right\"").unwrap(),
        Align::Right
    );
}

#[test]
fn font_profiles_extrude_with_their_counters_open() {
    let profiles = text::profiles("BO", 5., 0.5, Align::Left).unwrap();
    let refs: Vec<_> = profiles.iter().collect();
    let solids = exact::extrude(&refs, &Plane::XY, 0., 1.5).unwrap();
    let expected = profiles.iter().map(Profile::area).sum::<f64>() * 1.5;
    let actual = solids.iter().map(|solid| solid.volume()).sum::<f64>();
    assert!(
        (actual - expected).abs() < 1e-6,
        "solid volume must exclude the counters: {actual} versus {expected}"
    );
    let mesh = exact::tessellate(&solids).unwrap().0;
    assert_eq!(mesh.open_edges(), 0);
}

#[test]
fn input_and_geometry_work_are_bounded() {
    for content in [
        "",
        "  ",
        "line\nbreak",
        "tab\tstop",
        "a\u{0000}",
        "a\u{2028}b",
        "😀",
        "\u{10FFFF}",
        "e\u{0301}",
    ] {
        assert!(
            text::profiles(content, 5., 0., Align::Left).is_err(),
            "must explicitly reject {content:?}"
        );
    }
    assert!(text::profiles(&"H".repeat(text::MAX_CHARACTERS), 5., 0., Align::Left).is_ok());
    assert!(text::profiles(&"H".repeat(text::MAX_CHARACTERS + 1), 5., 0., Align::Left).is_err());
    for height in [0., -1., f64::NAN, f64::INFINITY, 10_001.] {
        assert!(text::profiles("A", height, 0., Align::Left).is_err());
    }
    for spacing in [-1., f64::NAN, f64::INFINITY, 10_001.] {
        assert!(text::profiles("A", 5., spacing, Align::Left).is_err());
    }
    assert!(
        text::profiles(&"O".repeat(text::MAX_CHARACTERS), 10_000., 0., Align::Left).is_err(),
        "very detailed geometry must stop at its vertex budget"
    );
    let ascii: String = ('!'..='~').collect();
    assert!(text::profiles(&ascii, 5., 0., Align::Left).is_ok());
}
