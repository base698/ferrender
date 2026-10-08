use super::*;

#[test]
fn recent_files_track_successful_ui_and_automation_opens_and_saves_only() {
    let mut h = state_harness();
    let a = out_dir().join("recent-open A 雪.ferr");
    let b = out_dir().join("recent-open B.ferr");
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().save_path(&a);
    assert_eq!(h.state().recent.files(), &[a.canonicalize().unwrap()]);
    h.state_mut().save_path(&b); // Save As
    h.state_mut().open_path(&a);
    assert_eq!(h.state().recent.files(), &[a.canonicalize().unwrap(), b.canonicalize().unwrap()]);
    let before = h.state().doc().clone();
    h.state_mut().open_path(&out_dir().join("absent-recent-design.ferr"));
    assert!(h.state().file_error.is_some());
    assert_eq!(h.state().doc(), &before);
    assert_eq!(h.state().recent.files().len(), 2);
    h.state_mut().save_path(&out_dir()); // A directory is not a save destination.
    assert!(h.state().file_error.is_some());
    assert_eq!(h.state().recent.files().len(), 2);
    h.state_mut().execute(&json!({"op":"open", "path":b})).unwrap();
    assert_eq!(h.state().recent.files()[0], b.canonicalize().unwrap());
    h.state_mut().execute(&json!({"op":"save", "path":a})).unwrap();
    assert_eq!(h.state().recent.files()[0], a.canonicalize().unwrap());
    h.state_mut().open_path(&out_dir().join("mesh-only.STL"));
    assert!(matches!(h.state().dialog, Dialog::Import(_, _)));
    assert_eq!(h.state().recent.files().len(), 2, "imports do not belong in design history");
}

#[test]
fn recent_files_menu_reopens_persisted_entries_and_clear_keeps_documents() {
    let dir = out_dir().join("recent-menu");
    std::fs::create_dir_all(&dir).unwrap();
    let history = dir.join("recent.json");
    let a = dir.join("Menu alpha 雪.ferr"); let b = dir.join("Menu beta.ferr");
    let mut h = state_harness();
    h.state_mut().recent = crate::recent::RecentFiles::load(history.clone()).0;
    h.state_mut().recent.clear().unwrap();
    h.state_mut().save_path(&a);
    h.state_mut().create_sketch(Plane::XY);
    h.state_mut().save_path(&b);
    drop(h);
    let mut h = state_harness();
    h.state_mut().recent = crate::recent::RecentFiles::load(history.clone()).0;
    h.get_by_label("File").click(); h.run_steps(2);
    h.get_by_label_contains("Open Recent").click(); h.run_steps(2);
    h.get_by_label_contains("Menu alpha 雪.ferr").click(); h.run_steps(3);
    assert_eq!(h.state().session.path.as_ref().unwrap().canonicalize().unwrap(), a.canonicalize().unwrap());
    assert!(h.state().doc().features.is_empty(), "the selected file actually opened");
    assert_eq!(h.state().recent.files()[0], a.canonicalize().unwrap());
    h.get_by_label("File").click(); h.run_steps(2);
    h.get_by_label_contains("Open Recent").click(); h.run_steps(2);
    h.get_by_label("Clear Recent").click(); h.run_steps(2);
    assert!(h.state().recent.files().is_empty());
    assert!(crate::recent::RecentFiles::load(history).0.files().is_empty());
    assert!(a.exists() && b.exists());
    assert_eq!(h.state().session.path.as_ref().unwrap().canonicalize().unwrap(), a.canonicalize().unwrap());
    assert!(!h.state().session.dirty);
}

#[test]
fn recent_files_requests_preserve_unsaved_work_on_cancel_and_missing_file() {
    let mut h = state_harness();
    let a = out_dir().join("recent-deleted.ferr");
    h.state_mut().save_path(&a);
    h.state_mut().create_sketch(Plane::XY);
    let before = h.state().doc().clone();
    h.state().open_requests.push(Ok(a.clone()));
    let mut asked = false;
    h.state_mut().process_open_request(|app| { asked = true; assert!(app.session.dirty); false });
    assert!(asked);
    assert_eq!(h.state().doc(), &before);
    assert!(h.state().session.dirty);
    std::fs::remove_file(&a).unwrap();
    h.state().open_requests.push(Ok(a));
    h.state_mut().process_open_request(|_| true);
    assert_eq!(h.state().doc(), &before);
    assert!(h.state().session.dirty);
    assert!(h.state().file_error.is_some());
    assert_eq!(h.state().recent.files().len(), 1, "missing files remain available if a removable drive returns");
}
