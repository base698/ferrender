//! Settings in `~/.config/ferrender/config.toml` (hand-editable, mode 0600).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ai {
    /// Anthropic API key; `ANTHROPIC_API_KEY` in the environment wins.
    pub api_key: String,
    /// Empty means the app default.
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BridgeCfg {
    /// Allow same-user MCP clients to control this window.
    pub enabled: bool,
    /// Local socket channel (the old TCP port number, retained for compatibility).
    pub port: u16,
}

impl Default for BridgeCfg {
    fn default() -> Self {
        BridgeCfg { enabled: true, port: DEFAULT_PORT }
    }
}

pub const DEFAULT_PORT: u16 = 47821;
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub appearance: Appearance,
    pub ai: Ai,
    pub bridge: BridgeCfg,
}

impl Config {
    pub fn path() -> PathBuf {
        let set = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default());
        set("FERRENDER_CONFIG_DIR").or_else(|| set("XDG_CONFIG_HOME").map(|d| d.join("ferrender"))).unwrap_or_else(|| home.join(".config/ferrender")).join("config.toml")
    }

    /// The saved settings, or defaults with the reason they could not be read.
    pub fn load() -> (Config, Option<String>) {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => match toml::from_str(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(e.to_string())),
            },
            Err(_) => (Config::default(), None),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        atomic_private_write(&path, text.as_bytes()).map_err(|e| format!("Couldn't save settings to {}: {e}", path.display()))
    }

    pub fn api_key(&self) -> Option<String> {
        std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty()).or_else(|| Some(self.ai.api_key.trim().to_owned()).filter(|k| !k.is_empty()))
    }

    pub fn model(&self) -> &str {
        if self.ai.model.trim().is_empty() { DEFAULT_MODEL } else { self.ai.model.trim() }
    }
}

/// Create a new, private temporary file before writing any secret. `create_new`
/// refuses existing files and symlinks; rename only replaces the final entry.
fn atomic_private_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(|| std::io::Error::other("settings path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    for _ in 0..128 {
        let tmp = parent.join(format!(".ferrender-settings-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&tmp) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let result = file.write_all(bytes).and_then(|_| file.sync_all()).and_then(|_| std::fs::rename(&tmp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        return result;
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "could not create a private settings temporary file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_defaults_to_system_and_round_trips_without_losing_other_settings() {
        let old: Config = toml::from_str("[ai]\nmodel = 'custom-model'\n[bridge]\nport = 42\nenabled = false").unwrap();
        assert_eq!(old.appearance, Appearance::System);
        for appearance in [Appearance::System, Appearance::Light, Appearance::Dark] {
            let config = Config { appearance, ..old.clone() };
            let decoded: Config = toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
            assert_eq!(decoded.appearance, appearance);
            assert_eq!(decoded.ai.model, "custom-model");
            assert_eq!(decoded.bridge.port, 42);
            assert!(!decoded.bridge.enabled);
        }
    }

    #[test]
    fn old_config_keeps_bridge_enabled_and_new_setting_can_disable_it() {
        let old: Config = toml::from_str("[bridge]\nport = 42").unwrap();
        assert!(old.bridge.enabled);
        assert_eq!(old.bridge.port, 42);
        let off: Config = toml::from_str("[bridge]\nenabled = false").unwrap();
        assert!(!off.bridge.enabled);
    }

    #[test]
    #[cfg(unix)]
    fn private_atomic_settings_do_not_follow_preexisting_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = std::env::temp_dir().join(format!("ferrender-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("unrelated");
        std::fs::write(&victim, "unchanged").unwrap();
        let path = dir.join("config.toml");
        symlink(&victim, &path).unwrap();
        symlink(&victim, path.with_extension("toml.tmp")).unwrap();
        atomic_private_write(&path, b"secret").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "unchanged");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret");
        assert!(!std::fs::symlink_metadata(&path).unwrap().is_symlink());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
