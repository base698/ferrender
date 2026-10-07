#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use std::{fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn metadata_distinguishes_clean_modified_detached_and_archived_sources() {
    let root = std::env::temp_dir().join(format!("ferrender-build-info-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&root).unwrap();
    assert_eq!(build_script::source_info(&root), ("unavailable".into(), "unknown"));
    git(&root, &["init", "-q"]);
    fs::write(root.join("source.rs"), "first revision").unwrap();
    git(&root, &["add", "source.rs"]);
    git(&root, &["-c", "user.name=Build Test", "-c", "user.email=build-test@example.invalid", "-c", "commit.gpgsign=false", "commit", "-qm", "initial"]);
    let first = git(&root, &["rev-parse", "HEAD"]);
    assert_eq!(build_script::source_info(&root), (first.clone(), "clean"));
    fs::write(root.join("source.rs"), "local changes").unwrap();
    assert_eq!(build_script::source_info(&root), (first.clone(), "modified"));
    git(&root, &["add", "source.rs"]);
    assert_eq!(build_script::source_info(&root), (first.clone(), "modified"));
    git(&root, &["-c", "user.name=Build Test", "-c", "user.email=build-test@example.invalid", "-c", "commit.gpgsign=false", "commit", "-qm", "second"]);
    let second = git(&root, &["rev-parse", "HEAD"]);
    assert_ne!(first, second);
    assert_eq!(build_script::source_info(&root), (second, "clean"));
    git(&root, &["checkout", "--detach", &first]);
    assert_eq!(build_script::source_info(&root), (first.clone(), "clean"));
    fs::write(root.join("new-source.rs"), "untracked source").unwrap();
    assert_eq!(build_script::source_info(&root), (first, "modified"));
    // Nested source archives must not claim their unrelated parent repository's commit.
    fs::create_dir(root.join("archive")).unwrap();
    assert_eq!(build_script::source_info(&root.join("archive")), ("unavailable".into(), "unknown"));
    fs::remove_dir_all(root).unwrap();
}
