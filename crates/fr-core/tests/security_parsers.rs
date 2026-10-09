//! Adversarial parser inputs must be rejected before expansion/allocation.
use std::io::{Cursor, Write};
use fr_core::{io, meshfile};

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        archive.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

fn file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ferrender-security-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Overwrite declared expanded sizes, without allocating the claimed data.
fn expanded_sizes(bytes: &mut [u8], size: u32) {
    let mut i = 0;
    while i + 46 <= bytes.len() {
        if bytes[i..i + 4] == *b"PK\x01\x02" {
            bytes[i + 24..i + 28].copy_from_slice(&size.to_le_bytes());
            let name = u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]) as usize;
            let extra = u16::from_le_bytes([bytes[i + 30], bytes[i + 31]]) as usize;
            let comment = u16::from_le_bytes([bytes[i + 32], bytes[i + 33]]) as usize;
            i += 46 + name + extra + comment;
        } else { i += 1; }
    }
}

#[test]
fn zip_aggregate_expansion_is_bounded_before_decompression() {
    let mut bytes = zip(&[("meshes/1.mesh", b"x"), ("meshes/2.mesh", b"x"), ("meshes/3.mesh", b"x")]);
    expanded_sizes(&mut bytes, 750 * 1024 * 1024);
    let err = io::decode(&bytes).unwrap_err();
    assert!(err.contains("expanded container"), "{err}");
}

#[test]
fn expanded_3mf_xml_size_is_checked_before_decompression() {
    let mut bytes = zip(&[("3D/3dmodel.model", b"<model/>")]);
    expanded_sizes(&mut bytes, 900_000_000);
    let err = meshfile::read_3mf(&file("oversized.3mf", &bytes)).unwrap_err();
    assert!(err.contains("expanded 3MF"), "{err}");
}

#[test]
fn empty_3mf_component_graph_cannot_expand_without_bound() {
    let mut xml = String::from("<model><resources><object id=\"0\"><components></components></object>");
    for id in 1..9 {
        xml.push_str(&format!("<object id=\"{id}\"><components>"));
        for _ in 0..10 { xml.push_str(&format!("<component objectid=\"{}\"/>", id - 1)); }
        xml.push_str("</components></object>");
    }
    xml.push_str("</resources><build><item objectid=\"8\"/></build></model>");
    let err = meshfile::read_3mf(&file("component-expansion.3mf", &zip(&[("3D/3dmodel.model", xml.as_bytes())]))).unwrap_err();
    assert!(err.contains("too many component instances"), "{err}");
}

#[test]
fn three_mf_scales_component_and_build_translations_with_units() {
    let xml = r#"<model unit="inch"><resources>
      <object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>
      <object id="2"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 2 0 0"/></components></object>
      </resources><build><item objectid="2" transform="1 0 0 0 1 0 0 0 1 0 3 0"/></build></model>"#;
    let mesh = meshfile::read_3mf(&file("inch-translations.3mf", &zip(&[("3D/3dmodel.model", xml.as_bytes())]))).unwrap();
    let (lo, hi) = mesh.bbox().unwrap();
    assert!(lo.distance(glam::DVec3::new(50.8, 76.2, 0.0)) < 1e-4, "{lo}");
    assert!(hi.distance(glam::DVec3::new(76.2, 101.6, 0.0)) < 1e-4, "{hi}");
}
