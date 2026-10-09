//! Verify the boundary between the archive verifier and the compile-time claim.
#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("ferrender-provenance-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn marker(&self, value: Value) {
        fs::write(self.0.join(".ferrender-verified.json"), value.to_string()).unwrap();
    }
}
impl Drop for Root {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn pins() -> Value {
    json!({"tag": "fixture", "assets": {"target-a": {"sha256": "aaa", "size": 100}, "target-b": {"sha256": "bbb", "size": 200}}})
}

#[test]
fn only_a_marker_for_the_selected_target_is_accepted() {
    let root = Root::new();
    root.marker(json!({"sha256": "aaa", "size": 100}));
    assert_eq!(build_script::verified(&pins(), "target-a", Some(&root.0)).unwrap(), "fixture sha256:aaa");
    assert!(build_script::verified(&pins(), "target-b", Some(&root.0)).is_err());
    assert!(build_script::verified(&pins(), "unsupported", Some(&root.0)).is_err());
    root.marker(json!({"sha256": "aaa", "size": 101}));
    assert!(build_script::verified(&pins(), "target-a", Some(&root.0)).is_err());
}

#[test]
fn missing_malformed_and_relative_roots_do_not_claim_verification() {
    let root = Root::new();
    assert!(build_script::verified(&pins(), "target-a", None).is_err());
    assert!(build_script::verified(&pins(), "target-a", Some(&root.0)).is_err());
    fs::write(root.0.join(".ferrender-verified.json"), "not JSON").unwrap();
    assert!(build_script::verified(&pins(), "target-a", Some(&root.0)).is_err());
    let relative = PathBuf::from("../../target/verified-occt");
    assert!(build_script::verified(&pins(), "target-a", Some(&relative)).unwrap_err().contains("must be absolute"));
}
