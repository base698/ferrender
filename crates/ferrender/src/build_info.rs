//! The identity of this executable, captured when Cargo compiled it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("FERRENDER_COMMIT");
pub const TARGET: &str = env!("FERRENDER_TARGET");
pub const PROFILE: &str = env!("FERRENDER_PROFILE");
pub const OCCT: &str = fr_core::OCCT_PROVENANCE;

pub fn source_status() -> &'static str {
    match env!("FERRENDER_SOURCE_STATE") {
        "clean" => "Clean checkout",
        "modified" => "Local changes included",
        _ => "Source status unavailable",
    }
}

pub fn summary() -> String {
    format!("Ferrender {VERSION}\nCommit: {COMMIT}\nSource: {}\nBuild: {PROFILE}\nPlatform: {TARGET}\nOpenCascade: {OCCT}", source_status())
}
