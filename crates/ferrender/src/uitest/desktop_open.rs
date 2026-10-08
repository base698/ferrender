use super::*;

fn saved_reference() -> PathBuf {
    let mut document = fr_core::Document::new(fr_core::Unit::Mm);
    let mut sketch = fr_core::Sketch::new(Plane::XY);
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 2, image::Rgba([50, 160, 210, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    sketch.reference = Some(fr_core::reference::ReferenceImage::from_bytes("tracing grid.png", png.get_ref(), 40.0).unwrap());
    document.add_feature(fr_core::FeatureKind::Sketch(sketch));
    let file = out_dir().join("Desktop open 100% 雪.ferr");
    fr_core::io::save(&document, &file).unwrap();
    file
}

#[test]
fn desktop_open_loads_embedded_image_and_preserves_current_design_on_error() {
    let file = saved_reference();
    let requests = crate::open_requests::OpenRequests::default();
    requests.push(Ok(file.clone())); // delivered before app/context attachment
    let mut h = state_harness();
    requests.attach(&h.ctx);
    h.state_mut().open_requests = requests;
    h.run_steps(2);
    assert_eq!(h.state().session.path, Some(file.clone()));
    assert!(!h.state().session.dirty);
    let reference = h.state().doc().sketches().next().unwrap().1.reference.as_ref().unwrap();
    assert_eq!(reference.width, 40.0);
    assert_eq!((reference.pixel_width, reference.pixel_height), (4,2));
    assert!(h.state().file_error.is_none());
    let before = h.state().doc().clone();
    h.state().open_requests.push(Ok(out_dir().join("missing-desktop-file.ferr")));
    h.run_steps(2);
    assert_eq!(h.state().doc(), &before);
    assert!(h.state().file_error.is_some());
    h.state().open_requests.push(Ok(file.clone()));
    h.step();
    assert!(h.state().file_error.is_some(), "new desktop events must not erase the error");
    h.state_mut().file_error = None;
    h.step();
    assert_eq!(h.state().session.path, Some(file));
}

#[test]
fn desktop_open_cancel_keeps_unsaved_work_and_stl_uses_import_flow() {
    let mut h = state_harness();
    h.state_mut().create_sketch(Plane::XY);
    let before = h.state().doc().clone();
    assert!(h.state().session.dirty);
    h.state().open_requests.push(Ok(PathBuf::from("/tmp/another-design.ferr")));
    let mut asked = false;
    h.state_mut().process_open_request(|app| { asked = true; assert!(app.session.dirty); false });
    assert!(asked);
    assert_eq!(h.state().doc(), &before);
    assert!(h.state().session.dirty);
    assert!(h.state().open_requests.pop().is_none());
    h.state().open_requests.push(Ok(PathBuf::from("/tmp/part.STL")));
    h.state_mut().process_open_request(|_| panic!("STL imports must not discard the current document"));
    assert!(matches!(h.state().dialog, Dialog::Import(_, _)));
    assert_eq!(h.state().doc(), &before);
}
