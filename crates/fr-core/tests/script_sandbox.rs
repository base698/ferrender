//! Script runs and their files: staged writes, read-back, saves, the time
//! limit, questions, and links inside the allowed folders.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fr_core::api::execute;
use fr_core::script::{self, Request};
use fr_core::Session;
use serde_json::{Value as J, json};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferrender-sandbox-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_cmd(s: &mut Session, cmd: J) -> J {
    execute(s, &cmd, None).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
}

fn leftovers(dir: &PathBuf) -> Vec<String> {
    std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.contains("ferrtmp")).collect()
}

fn s(p: &PathBuf) -> String { p.display().to_string() }

/// A script that writes a file, then fails or is cancelled, leaves nothing behind.
#[test]
fn files_a_script_writes_appear_only_when_the_run_succeeds() {
    let inside = dir("staged");
    let mut session = Session::default();
    run_cmd(&mut session, json!({"op": "primitive", "type": "box", "width": 5, "depth": 5, "height": 5}));
    let note = inside.join("note.txt");
    let stl = inside.join("box.stl");
    // Success: the files are there, with their final names, and listed as exports.
    let mut req = Request::new(format!("const META = #{{name: \"ok\"}}; fn run(i) {{ write_text({:?}, \"hi\"); export_stl(#{{ path: {:?} }}); }}", s(&note), s(&stl)));
    req.sandbox.allowed = vec![inside.clone()];
    let out = script::run(&mut session, &req).unwrap();
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "hi");
    assert!(stl.is_file() && std::fs::metadata(&stl).unwrap().len() > 84);
    assert_eq!(out.exports, vec![note.canonicalize().unwrap(), stl.canonicalize().unwrap()]);
    // Failure after writing: the old files are untouched and no temporaries remain.
    let mut req = Request::new(format!("const META = #{{name: \"bad\"}}; fn run(i) {{ write_text({:?}, \"changed\"); export_stl(#{{ path: {:?} }}); fail(\"no\"); }}", s(&note), s(&stl)));
    req.sandbox.allowed = vec![inside.clone()];
    assert!(script::run(&mut session, &req).unwrap_err().contains("no"));
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "hi");
    assert!(leftovers(&inside).is_empty(), "{:?}", leftovers(&inside));
    // Cancelled after writing: the same.
    let mut req = Request::new(format!("const META = #{{name: \"slow\"}}; fn run(i) {{ write_text({:?}, \"changed\"); let n = 0; while true {{ n += 1; progress(0.0, \"\"); }} }}", s(&note)));
    req.sandbox.allowed = vec![inside.clone()];
    let cancel = req.cancel.clone();
    std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(200)); cancel.store(true, Ordering::Relaxed); });
    let out = script::run(&mut session, &req).unwrap();
    assert!(out.cancelled && out.exports.is_empty());
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "hi");
    assert!(leftovers(&inside).is_empty(), "{:?}", leftovers(&inside));
    // A command that fails leaves no file either.
    let none = inside.join("none.step");
    let mut req = Request::new(format!("const META = #{{name: \"partial\"}}; fn run(i) {{ export_step(#{{ path: {:?}, bodies: [12345] }}); }}", s(&none)));
    req.sandbox.allowed = vec![inside.clone()];
    assert!(script::run(&mut session, &req).is_err());
    assert!(!none.exists());
    assert!(leftovers(&inside).is_empty(), "{:?}", leftovers(&inside));
}

/// Within one run a script sees what it wrote, even though the file is not in place yet.
#[test]
fn a_script_reads_back_what_it_wrote_in_the_same_run() {
    let inside = dir("readback");
    let mut session = Session::default();
    let csv = s(&inside.join("rows.csv"));
    let source = format!(r#"const META = #{{name: "rb"}};
fn run(i) {{
    if exists({csv:?}) {{ fail("should not exist yet"); }}
    write_csv({csv:?}, [["a", "b"], [1, 2]]);
    if !exists({csv:?}) {{ fail("should exist now"); }}
    let rows = read_csv({csv:?});
    write_text({csv:?}, "second");
    #{{ rows: rows.len(), text: read_text({csv:?}) }}
}}"#);
    let mut req = Request::new(source);
    req.sandbox.allowed = vec![inside.clone()];
    let out = script::run(&mut session, &req).unwrap();
    assert_eq!(out.result, json!({"rows": 2, "text": "second"}));
    assert_eq!(std::fs::read_to_string(inside.join("rows.csv")).unwrap(), "second");
    assert_eq!(out.exports.len(), 1, "written twice, in place once");
    assert!(leftovers(&inside).is_empty());
}

/// `save` from a script lands at the real path, names the design, and keeps the 0.3 backup rule.
#[test]
fn a_script_save_goes_to_its_target_with_the_backup_rule() {
    let inside = dir("script-save");
    let path = inside.join("part.ferr");
    let mut session = Session::default();
    run_cmd(&mut session, json!({"op": "primitive", "type": "box", "width": 5, "depth": 5, "height": 5}));
    session.save(&path).unwrap();
    assert!(!fr_core::io::is_container(&path), "a plain design is plain JSON");
    // The script adds an import (a mesh), so the next save is a container.
    let stl = inside.join("tool.stl");
    fr_core::io::write_stl(session.built.bodies.iter(), fr_core::Unit::Mm, &stl).unwrap();
    let mut req = Request::new(format!("const META = #{{name: \"save\"}}; fn run(i) {{ import_mesh(#{{ path: {:?} }}); save(); }}", s(&stl)));
    req.sandbox.allowed = vec![inside.clone()];
    let out = script::run(&mut session, &req).unwrap();
    assert_eq!(out.exports, vec![path.canonicalize().unwrap()]);
    assert_eq!(session.path.as_deref().map(|p| p.canonicalize().unwrap()), Some(path.canonicalize().unwrap()), "the session points at the real file, not the staged one");
    assert!(fr_core::io::is_container(&path));
    assert!(fr_core::io::backup_path(&path).is_file(), "the plain original is kept as the 0.3 backup");
    let reopened = Session::open(&path).unwrap();
    assert_eq!(reopened.doc.features.len(), 2);
    assert!(leftovers(&inside).is_empty());
}

#[test]
fn a_time_limit_stops_a_run_between_operations() {
    let mut session = Session::default();
    let mut req = Request::new("const META = #{name: \"forever\"}; fn run(i) { let n = 0; while true { n += 1; } }");
    req.time_limit = Some(Duration::from_millis(300));
    let t = Instant::now();
    let err = script::run(&mut session, &req).unwrap_err();
    assert!(err.contains("time limit"), "{err}");
    assert!(t.elapsed().as_secs() < 10);
    // Between commands too; the caller decides what to do with the partial document.
    let mut req = Request::new("const META = #{name: \"boxes\"}; fn run(i) { for k in 0..1000 { primitive(#{ type: \"box\", width: 1, depth: 1, height: 1, output_key: \"box\" + k }); } }");
    req.time_limit = Some(Duration::from_millis(200));
    let err = script::run(&mut session, &req).unwrap_err();
    assert!(err.contains("time limit"), "{err}");
    assert!(!session.doc.features.is_empty());
    // Through the API, in seconds; a failed run leaves the document alone.
    let mut api = Session::default();
    let err = execute(&mut api, &json!({"op": "run_script", "source": "const META = #{name: \"f\"}; fn run(i) { for k in 0..100000 { primitive(#{ type: \"box\", width: 1, depth: 1, height: 1, output_key: \"box\" + k }); } }", "timeout": 0.2}), None).unwrap_err();
    assert!(err.contains("time limit"), "{err}");
    assert!(api.doc.features.is_empty());
    assert!(execute(&mut api, &json!({"op": "run_script", "source": "const META = #{name: \"f\"}; fn run(i) {}", "timeout": -1}), None).is_err());
}

/// Questions reach the caller's callback; a declined confirm is false and a declined ask fails.
#[test]
fn confirm_and_ask_go_through_the_callback() {
    let mut session = Session::default();
    let source = "const META = #{name: \"q\"}; fn run(i) { let yes = confirm(\"Go on?\"); let name = ask(\"Name?\", \"part\"); #{ yes: yes, name: name } }";
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut req = Request::new(source);
    let log = asked.clone();
    req.ask = Some(Arc::new(move |q: &str, default: Option<&str>| { log.lock().unwrap().push(q.to_owned()); default.map(|_| "bracket".to_owned()).or(Some(String::new())) }));
    let out = script::run(&mut session, &req).unwrap();
    assert_eq!(out.result, json!({"yes": true, "name": "bracket"}));
    assert_eq!(*asked.lock().unwrap(), ["Go on?", "Name?"]);
    // Declining: confirm is false, ask fails the run.
    let mut req = Request::new(source);
    req.ask = Some(Arc::new(|_: &str, _: Option<&str>| None));
    let err = script::run(&mut session, &req).unwrap_err();
    assert!(err.contains("declined"), "{err}");
    // Headless yes answers both.
    let mut req = Request::new(source);
    req.sandbox.yes = true;
    assert_eq!(script::run(&mut session, &req).unwrap().result, json!({"yes": true, "name": "part"}));
    // No callback at all declines.
    let out = script::run(&mut session, &Request::new("const META = #{name: \"q\"}; fn run(i) { confirm(\"?\") }")).unwrap();
    assert_eq!(out.result, json!(false));
}

/// A folder inside the allowed tree that links outside is refused when used, not only when
/// checked; a link that stays inside works; a hard link at the target is replaced, never written through.
#[cfg(unix)]
#[test]
fn links_inside_the_allowed_folders_are_not_followed_when_used() {
    use std::os::unix::fs::symlink;
    let inside = dir("link-inside");
    let outside = dir("link-outside");
    symlink(&outside, inside.join("out")).unwrap();
    let mut session = Session::default();
    for expr in [format!("write_text({:?}, \"x\")", s(&inside.join("out").join("a.txt"))), format!("mkdir({:?})", s(&inside.join("out").join("sub"))), format!("list_files({:?})", s(&inside.join("out")))] {
        let mut req = Request::new(format!("const META = #{{name: \"l\"}}; fn run(i) {{ {expr} }}"));
        req.sandbox.allowed = vec![inside.clone()];
        let err = script::run(&mut session, &req).unwrap_err();
        assert!(err.contains("outside the allowed folders"), "{expr}: {err}");
    }
    assert!(!outside.join("a.txt").exists() && !outside.join("sub").exists());
    // A link pointing inside the allowed tree: the check resolves it and the walk sees the real folder.
    std::fs::create_dir_all(inside.join("real")).unwrap();
    symlink(inside.join("real"), inside.join("alias")).unwrap();
    let mut req = Request::new(format!("const META = #{{name: \"l\"}}; fn run(i) {{ write_text({:?}, \"ok\") }}", s(&inside.join("alias").join("b.txt"))));
    req.sandbox.allowed = vec![inside.clone()];
    script::run(&mut session, &req).unwrap();
    assert_eq!(std::fs::read_to_string(inside.join("real").join("b.txt")).unwrap(), "ok");
    // A hard link at the target: the write replaces the directory entry; the other name keeps the old bytes.
    let target = outside.join("keep.txt");
    std::fs::write(&target, "original").unwrap();
    std::fs::hard_link(&target, inside.join("linked.txt")).unwrap();
    let mut req = Request::new(format!("const META = #{{name: \"l\"}}; fn run(i) {{ write_text({:?}, \"replaced\") }}", s(&inside.join("linked.txt"))));
    req.sandbox.allowed = vec![inside.clone()];
    script::run(&mut session, &req).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    assert_eq!(std::fs::read_to_string(inside.join("linked.txt")).unwrap(), "replaced");
    // The same for a design the script saves over a hard link.
    let design = outside.join("shared.ferr");
    Session::default().save(&design).unwrap();
    let bytes = std::fs::read(&design).unwrap();
    std::fs::hard_link(&design, inside.join("mine.ferr")).unwrap();
    run_cmd(&mut session, json!({"op": "primitive", "type": "box", "width": 5, "depth": 5, "height": 5}));
    let mut req = Request::new(format!("const META = #{{name: \"l\"}}; fn run(i) {{ save(#{{ path: {:?} }}) }}", s(&inside.join("mine.ferr"))));
    req.sandbox.allowed = vec![inside.clone()];
    script::run(&mut session, &req).unwrap();
    assert_eq!(std::fs::read(&design).unwrap(), bytes, "the other name of the hard link is untouched");
    assert_eq!(Session::open(&inside.join("mine.ferr")).unwrap().doc.features.len(), 1);
}
