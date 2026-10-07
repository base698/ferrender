//! Record the source revision in the executable, not from the user's checkout at runtime.
use std::{path::{Path, PathBuf}, process::Command};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    if !output.status.success() { return None; }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

pub fn source_info(root: &Path) -> (String, &'static str) {
    // A source archive inside an unrelated checkout must not inherit its identity.
    let own_repo = git(root, &["rev-parse", "--show-toplevel"])
        .and_then(|p| PathBuf::from(p).canonicalize().ok()) == root.canonicalize().ok();
    if !own_repo { return ("unavailable".into(), "unknown"); }
    let commit = git(root, &["rev-parse", "--verify", "HEAD"])
        .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()));
    let Some(commit) = commit else { return ("unavailable".into(), "unknown") };
    let state = match git(root, &["status", "--porcelain=v1", "--untracked-files=normal", "--ignore-submodules=none"]) {
        Some(status) if status.is_empty() => "clean",
        Some(_) => "modified",
        None => "unknown",
    };
    (commit, state)
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap().parent().unwrap();
    // Watch source files and Git metadata so an incremental build cannot retain
    // the previous commit after commit/checkout/reset, including linked worktrees.
    for name in ["Cargo.toml", "Cargo.lock", "README.md", "crates", "assets", "scripts", ".github"] {
        let path = root.join(name);
        if path.exists() { println!("cargo:rerun-if-changed={}", path.display()); }
    }
    if let Some(files) = git(root, &["ls-files", "-z"]) {
        for file in files.split('\0').filter(|f| !f.is_empty()) {
            println!("cargo:rerun-if-changed={}", root.join(file).display());
        }
    }
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git(root, &["rev-parse", "--git-path", name]) {
            let path = root.join(path);
            if path.exists() { println!("cargo:rerun-if-changed={}", path.display()); }
        }
    }
    if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(root, &["rev-parse", "--git-path", &reference])
    {
        let path = root.join(path);
        if path.exists() { println!("cargo:rerun-if-changed={}", path.display()); }
    }
    let (commit, state) = source_info(root);
    println!("cargo:rustc-env=FERRENDER_COMMIT={commit}");
    println!("cargo:rustc-env=FERRENDER_SOURCE_STATE={state}");
    println!("cargo:rustc-env=FERRENDER_TARGET={}", std::env::var("TARGET").unwrap());
    println!("cargo:rustc-env=FERRENDER_PROFILE={}", std::env::var("PROFILE").unwrap());
}
