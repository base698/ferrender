use fr_core::{Session, api::execute, script::{self, Request}};
use serde_json::json;
use std::{path::PathBuf, sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};

fn folder() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!("ferrender-script-review-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&path).unwrap();
    path
}

#[test]
fn cancellation_during_the_last_host_call_discards_staged_files() {
    let dir = folder(); let file = dir.join("keep.txt");
    std::fs::write(&file, "original").unwrap();
    let mut req = Request::new(format!("const META = #{{name: \"last call\"}}; fn run(i) {{ write_text({:?}, \"replacement\"); confirm(\"finish?\") }}", file.to_str().unwrap()));
    req.sandbox.allowed.push(dir.clone());
    let cancel = req.cancel.clone();
    req.ask = Some(Arc::new(move |_, _| { cancel.store(true, Ordering::Relaxed); Some(String::new()) }));
    let out = script::run(&mut Session::default(), &req).unwrap();
    assert!(out.cancelled, "a final host call must not bypass cancellation");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn timeout_during_the_last_host_call_discards_staged_files() {
    let dir = folder(); let file = dir.join("keep.txt");
    std::fs::write(&file, "original").unwrap();
    let mut req = Request::new(format!("const META = #{{name: \"last call\"}}; fn run(i) {{ write_text({:?}, \"replacement\"); confirm(\"finish?\") }}", file.to_str().unwrap()));
    req.sandbox.allowed.push(dir.clone());
    req.time_limit = Some(Duration::from_millis(20));
    req.ask = Some(Arc::new(|_, _| { std::thread::sleep(Duration::from_millis(50)); Some(String::new()) }));
    assert!(script::run(&mut Session::default(), &req).unwrap_err().contains("time limit"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_late_destination_failure_does_not_publish_earlier_files() {
    let dir = folder(); let first = dir.join("first.txt"); let second = dir.join("second.txt");
    std::fs::write(&first, "original").unwrap();
    let mut req = Request::new(format!("const META = #{{name: \"commit failure\"}}; fn run(i) {{ write_text({:?}, \"replacement\"); write_text({:?}, \"second\"); confirm(\"finish?\") }}", first.to_str().unwrap(), second.to_str().unwrap()));
    req.sandbox.allowed.push(dir.clone());
    let blocked = second.clone();
    req.ask = Some(Arc::new(move |_, _| { std::fs::create_dir(&blocked).unwrap(); Some(String::new()) }));
    assert!(script::run(&mut Session::default(), &req).is_err());
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "original");
    assert!(second.is_dir());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_mesh_export_can_be_imported_again_before_commit() {
    let dir = folder(); let file = dir.join("part.stl");
    let mut req = Request::new(format!("const META = #{{name: \"mesh readback\"}}; fn run(i) {{ primitive(#{{type: \"box\", width: 2, depth: 3, height: 4}}); export_stl(#{{path: {:?}}}); import_mesh(#{{path: {:?}}}); }}", file.to_str().unwrap(), file.to_str().unwrap()));
    req.sandbox.allowed.push(dir.clone());
    let mut session = Session::default();
    script::run(&mut session, &req).unwrap();
    assert_eq!(session.built.bodies.len(), 2);
    assert!(file.is_file());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn oversized_timeout_is_an_error_without_panicking_or_losing_the_session() {
    let mut session = Session::default();
    execute(&mut session, &json!({"op":"primitive","type":"box","width":2,"depth":3,"height":4}), None).unwrap();
    let original = session.doc.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(&mut session, &json!({"op":"run_script","source":"const META = #{name: \"empty\"}; fn run(i) {}", "timeout": 1e300}), None)));
    assert!(result.is_ok(), "an out-of-range timeout must not panic");
    assert!(result.unwrap().is_err());
    assert_eq!(session.doc, original);
    let mut req = Request::new("const META = #{name: \"empty\"}; fn run(i) {}");
    req.time_limit = Some(Duration::MAX);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| script::run(&mut session, &req)));
    assert!(result.is_ok(), "an unrepresentable deadline must not panic");
    assert!(result.unwrap().is_err());
    assert_eq!(session.doc, original);
}

#[test]
fn exporting_a_script_preserves_colliding_assets_and_publishes_as_a_set() {
    let dir = folder(); let path = dir.join("design.rhai");
    std::fs::write(&path, "original script").unwrap();
    let mut export = script::Export { source: "replacement script".into(), files: vec![("asset.png".into(), b"pixels".to_vec())], notes: vec![] };
    std::fs::write(dir.join("asset.png"), "unrelated file").unwrap();
    assert!(script::write_export(&export, &path).unwrap_err().contains("different data"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original script");
    assert_eq!(std::fs::read_to_string(dir.join("asset.png")).unwrap(), "unrelated file");
    std::fs::remove_file(dir.join("asset.png")).unwrap();
    script::write_export(&export, &path).unwrap();
    export.source = "updated script, same asset".into();
    script::write_export(&export, &path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), export.source);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    let invalid = dir.join("directory.rhai"); std::fs::create_dir(&invalid).unwrap();
    export.files[0].0 = "new.png".into();
    assert!(script::write_export(&export, &invalid).is_err());
    assert!(!dir.join("new.png").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn exported_assets_do_not_write_through_links_outside_the_folder() {
    let dir = folder(); let outside = folder(); let target = outside.join("keep.png");
    std::fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("asset.png")).unwrap();
    let export = script::Export { source: "script".into(), files: vec![("asset.png".into(), b"replace".to_vec())], notes: vec![] };
    assert!(script::write_export(&export, &dir.join("design.rhai")).is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    assert!(!dir.join("design.rhai").exists());
    std::fs::remove_dir_all(dir).unwrap(); std::fs::remove_dir_all(outside).unwrap();
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_before_open_can_block() {
    use std::os::unix::ffi::OsStrExt;
    let dir = folder(); let fifo = dir.join("pipe");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let opened = fr_core::sandboxfs::open_beneath(&dir, std::path::Path::new("")).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || { tx.send(opened.open_read(std::ffi::OsStr::new("pipe")).map(|_| ())).unwrap(); });
    let response = rx.recv_timeout(Duration::from_secs(1));
    let rescue = if response.is_err() {
        // Unblock a broken implementation, so this regression never hangs CI.
        // Holding both ends also works if a heavily loaded runner has not
        // scheduled the reader yet.
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK) };
        assert!(fd >= 0);
        Some(fd)
    } else { None };
    thread.join().unwrap();
    if let Some(fd) = rescue { unsafe { libc::close(fd); } }
    assert!(response.unwrap().unwrap_err().contains("not a regular file"));
    std::fs::remove_dir_all(dir).unwrap();
}
