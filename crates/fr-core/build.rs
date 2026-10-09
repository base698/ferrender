//! Record whether cadrum links the OpenCascade archive pinned in
//! scripts/occt-pins.json, so the executable itself states how it was built.
use std::{env, fs, path::{Path, PathBuf}};

use serde_json::Value;

const ALLOW: &str = "FERRENDER_ALLOW_UNVERIFIED_OCCT";

/// The pinned release and digest, when `root` is the directory that
/// scripts/prepare-occt.py extracted from the pinned archive for `target`.
pub(crate) fn verified(pins: &Value, target: &str, root: Option<&Path>) -> Result<String, String> {
    let root = root.ok_or("OCCT_ROOT is not set, so cadrum chooses or downloads its own archive")?;
    // Cargo runs each build script in its own package directory. A relative
    // value would name different directories here and in cadrum's build script.
    if !root.is_absolute() {
        return Err("OCCT_ROOT must be absolute so Ferrender and cadrum verify the same directory".into());
    }
    let asset = &pins["assets"][target];
    let (Some(digest), Some(size)) = (asset["sha256"].as_str(), asset["size"].as_u64()) else {
        return Err(format!("there is no pinned OpenCascade archive for {target}"));
    };
    let marker = fs::read_to_string(root.join(".ferrender-verified.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .ok_or_else(|| format!("{} was not prepared by scripts/prepare-occt.py", root.display()))?;
    if marker["sha256"].as_str() != Some(digest) || marker["size"].as_u64() != Some(size) {
        return Err(format!("{} does not match the archive pinned for {target}", root.display()));
    }
    Ok(format!("{} sha256:{digest}", pins["tag"].as_str().unwrap_or("unknown")))
}

fn main() {
    let pins = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../scripts/occt-pins.json");
    println!("cargo:rerun-if-changed={}", pins.display());
    // The same variable cadrum's build script reads, in the same Cargo invocation.
    println!("cargo:rerun-if-env-changed=OCCT_ROOT");
    println!("cargo:rerun-if-env-changed={ALLOW}");
    let root = env::var_os("OCCT_ROOT").map(PathBuf::from);
    if let Some(root) = &root {
        println!("cargo:rerun-if-changed={}", root.join(".ferrender-verified.json").display());
    }
    let pins: Value = serde_json::from_str(&fs::read_to_string(&pins).expect("scripts/occt-pins.json is missing"))
        .expect("scripts/occt-pins.json is not valid JSON");
    let target = env::var("TARGET").unwrap();
    match verified(&pins, &target, root.as_deref()) {
        Ok(pinned) => println!("cargo:rustc-env=FR_CORE_OCCT=verified {pinned}"),
        Err(reason) => {
            // Development builds may use cadrum's own download. An optimized
            // build is what gets distributed, so it has to opt out explicitly.
            if env::var("PROFILE").as_deref() == Ok("release") && env::var(ALLOW).as_deref() != Ok("1") {
                panic!(
                    "\nRelease builds link the verified OpenCascade archive, but {reason}.\n\
                     Run:  export OCCT_ROOT=\"$(python3 scripts/prepare-occt.py)\"\n\
                     or set {ALLOW}=1 to build an executable marked as unverified.\n"
                );
            }
            println!("cargo:rustc-env=FR_CORE_OCCT=unverified");
        }
    }
}
