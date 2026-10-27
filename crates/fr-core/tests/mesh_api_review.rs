use fr_core::{Session, Unit, api, io};
use serde_json::{Value, json};

fn run(s: &mut Session, command: Value) -> Value { api::execute(s, &command, None).unwrap() }

fn sphere(unit: Unit) -> Session {
    let mut s = Session::new(fr_core::Document::new(unit));
    run(&mut s, json!({"op":"primitive","type":"sphere","diameter":"20 mm"}));
    run(&mut s, json!({"op":"mesh_subdivide","body":1,"levels":1}));
    s
}

#[test]
fn sculpt_smooth_and_flatten_strength_are_dimensionless_in_every_unit() {
    for brush in ["smooth", "flatten"] {
        let mut expected = None;
        for unit in [Unit::Mm, Unit::Cm, Unit::In] {
            let mut s = sphere(unit);
            run(&mut s, json!({"op":"mesh_sculpt","body":1,"brush":brush,"at":[0,0,10.0/unit.mm()],"radius":"8 mm","strength":0.25}));
            assert!(s.built.errors.is_empty());
            let vertices = s.built.bodies[0].mesh.positions().to_vec();
            if let Some(before) = &expected { assert_eq!(&vertices, before); } else { expected = Some(vertices); }
            let reopened = Session::new(io::from_json(&io::to_json(&s.doc)).unwrap());
            assert_eq!(reopened.built.bodies[0].mesh.positions(), s.built.bodies[0].mesh.positions());
            let before = s.doc.clone();
            let error = api::execute(&mut s, &json!({"op":"mesh_sculpt","body":1,"brush":brush,"at":[0,0,10.0/unit.mm()],"radius":"8 mm","strength":"0.5 mm"}), None);
            assert!(error.is_err());
            assert_eq!(s.doc, before);
        }
    }
}

#[test]
fn oversized_mesh_integer_arguments_never_wrap_or_edit_the_document() {
    let mut s = sphere(Unit::Mm);
    for command in [
        json!({"op":"mesh_subdivide","body":1,"levels":4294967296_u64}),
        json!({"op":"mesh_smooth","body":1,"iterations":4294967296_u64}),
        json!({"op":"mesh_decimate","body":1,"target":4294967300_u64}),
        json!({"op":"mesh_repair","body":1,"fill_holes":4294967296_u64}),
    ] {
        let before = s.doc.clone();
        let error = api::execute(&mut s, &command, None).unwrap_err();
        assert!(error.contains("whole-number range"), "{error}");
        assert_eq!(s.doc, before);
    }
}

#[test]
fn relief_integer_arguments_reject_overflow_and_fractional_values() {
    let dir = std::env::temp_dir().join(format!("ferrender-relief-integers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("height.png");
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, 2, 2);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&[0, 64, 128, 255]).unwrap();
    std::fs::write(&path, data).unwrap();
    let mut s = Session::default();
    for key in ["resolution", "blur"] {
        for value in [json!(4294967300_u64), json!(-1), json!(2.5), json!("many")] {
            let mut command = json!({"op":"mesh_from_image", "path":path});
            command[key] = value;
            let before = s.doc.clone();
            let error = api::execute(&mut s, &command, None).unwrap_err();
            assert!(error.contains("whole"), "{error}");
            assert_eq!(s.doc, before);
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
