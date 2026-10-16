use fr_core::tag::{EdgeTag, Kind, Origin, Tag};
#[test]
fn tag_json_round_trips() {
    let a = Tag::new(Origin::Swept { feature: 2, entity: 6 }, Kind::Plane);
    let b = Tag::new(Origin::Cap { feature: 2, end: true }, Kind::Plane);
    let e = EdgeTag::new(a.clone(), b.split(1));
    let text = serde_json::to_string(&e).unwrap();
    println!("{text}");
    let back: EdgeTag = serde_json::from_str(&text).unwrap();
    assert_eq!(back, e);
    let blend = fr_core::doc::Blend { body: 1, edges: vec![glam::DVec3::ZERO], size: fr_core::Value { expr: "1".into(), v: 1.0 }, chamfer: false, frame: None, tags: vec![Some(e)] };
    let text = serde_json::to_string(&blend).unwrap();
    println!("{text}");
    let back: fr_core::doc::Blend = serde_json::from_str(&text).unwrap();
    assert_eq!(back, blend);
}
