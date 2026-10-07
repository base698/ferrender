use fr_core::{Session, api, io, mesh::Mesh};
use serde_json::json;
use base64::Engine;

fn design() -> Session {
    let mut s=Session::default();
    for c in [json!({"op":"create_sketch"}),json!({"op":"add_geometry","items":[{"type":"rect","from":[0,0],"to":[10,10]}]})] { api::execute(&mut s,&c,None).unwrap(); }
    s
}
#[test]
fn malformed_constraints_and_entity_references_are_rejected_before_rebuild() {
    let s=design();
    let original=io::to_json(&s.doc);
    let mut v:serde_json::Value=serde_json::from_str(&original).unwrap();
    let constraints=v["features"][0]["kind"]["sketch"]["constraints"].as_object_mut().unwrap();
    constraints.values_mut().next().unwrap()["refs"]=json!([]);
    assert!(io::from_json(&v.to_string()).unwrap_err().contains("constraint"));
    let mut v:serde_json::Value=serde_json::from_str(&original).unwrap();
    v["features"][0]["kind"]["sketch"]["entities"].as_object_mut().unwrap().values_mut().next().unwrap()["a"]=json!(999);
    assert!(io::from_json(&v.to_string()).unwrap_err().contains("missing point"));
    assert_eq!(io::from_json(&original).unwrap(),s.doc);
}
#[test]
fn duplicate_ids_bad_planes_and_invalid_meshes_are_rejected() {
    let s=design();
    let mut v:serde_json::Value=serde_json::from_str(&io::to_json(&s.doc)).unwrap();
    v["features"][0]["kind"]["sketch"]["plane"]["x"]=json!([0,0,0]);
    assert!(io::from_json(&v.to_string()).unwrap_err().contains("plane"));
    let mut d=s.doc.clone();d.features.push(d.features[0].clone());
    assert!(io::from_json(&io::to_json(&d)).unwrap_err().contains("unique"));
    for bytes in [vec![0u8;35], [f32::NAN.to_le_bytes().as_slice(),&[0u8;32]].concat()] {
        let b64=base64::engine::general_purpose::STANDARD.encode(bytes);
        assert!(serde_json::from_value::<Mesh>(json!(b64)).is_err());
    }
}
#[test]
fn ascii_stl_does_not_accept_overflow_or_incomplete_triangles() {
    for text in ["vertex 1e100 0 0\nvertex 0 1 0\nvertex 0 0 1", "vertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nvertex 2 2 2", "vertex 0 nope 0 0\nvertex 1 0 0\nvertex 0 1 0"] {
        assert!(io::parse_stl(text.as_bytes(),fr_core::Unit::Mm).is_err());
    }
}
#[test]
fn clipboard_constraints_are_validated_before_paste() {
    let s=design();let sk=s.doc.sketch(1).unwrap();
    let mut clip=sk.copy(&sk.entities.keys().copied().collect::<Vec<_>>());
    clip.validate().unwrap();
    clip.constraints[0].refs.clear();
    assert!(clip.validate().is_err());
}

#[test]
fn invalid_save_keeps_the_last_readable_document() {
    let s = design();
    let dir = std::env::temp_dir().join(format!("ferrender-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("part.ferr");
    io::save(&s.doc, &path).unwrap();
    let mut invalid = s.doc.clone();
    invalid.features.push(invalid.features[0].clone());
    assert!(io::save(&invalid, &path).is_err());
    assert_eq!(io::load(&path).unwrap(), s.doc);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[cfg(unix)]
fn save_does_not_follow_the_legacy_temporary_file_symlink() {
    use std::os::unix::fs::symlink;
    let s = design();
    let dir = std::env::temp_dir().join(format!("ferrender-save-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("part.ferr");
    let victim = dir.join("other-file");
    std::fs::write(&victim, "keep this").unwrap();
    symlink(&victim, path.with_extension("ferr.tmp")).unwrap();
    io::save(&s.doc, &path).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep this");
    assert_eq!(io::load(&path).unwrap(), s.doc);
    std::fs::remove_dir_all(dir).unwrap();
}
