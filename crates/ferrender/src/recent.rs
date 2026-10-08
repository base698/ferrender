//! Persistent document history, separate from settings that may contain credentials.
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

const MAX_FILES: usize = 12;

#[derive(Default, Serialize, Deserialize)]
struct History {
    files: Vec<PathBuf>,
}

/// No storage path in UI tests, so tests cannot change the user's history.
#[derive(Default)]
pub struct RecentFiles {
    path: Option<PathBuf>,
    files: Vec<PathBuf>,
}

impl RecentFiles {
    pub fn load(path: PathBuf) -> (Self, Option<String>) {
        let mut recent = Self { path: Some(path.clone()), files: Vec::new() };
        let result = (|| -> Result<(), String> {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.to_string()),
            };
            let history: History = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            for file in history.files {
                // Absolute paths remain meaningful after a restart from another folder.
                if file.is_absolute() && !recent.files.contains(&file) {
                    recent.files.push(file);
                    if recent.files.len() == MAX_FILES { break; }
                }
            }
            Ok(())
        })();
        (recent, result.err().map(|e| format!("Couldn't read recent files from {}: {e}", path.display())))
    }

    pub fn files(&self) -> &[PathBuf] { &self.files }

    /// Call only after a successful design open/save. Failed opens and exports
    /// must not displace useful entries. Canonical paths also deduplicate aliases.
    pub fn remember(&mut self, path: &Path) -> Result<(), String> {
        let path = std::fs::canonicalize(path).or_else(|_| std::path::absolute(path)).map_err(|e| e.to_string())?;
        self.files.retain(|p| p != &path);
        self.files.insert(0, path);
        self.files.truncate(MAX_FILES);
        self.persist()
    }

    pub fn clear(&mut self) -> Result<(), String> {
        self.files.clear();
        self.persist()
    }

    fn persist(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(&History { files: self.files.clone() }).map_err(|e| e.to_string())?;
        crate::config::atomic_private_write(path, &bytes).map_err(|e| format!("Couldn't save recent files to {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferrender-recent-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recent_history_survives_restart_deduplicates_limits_and_clears_without_deleting_designs() {
        let dir = dir("roundtrip");
        let path = dir.join("recent.json");
        let (mut recent, error) = RecentFiles::load(path.clone());
        assert!(error.is_none());
        assert!(recent.files().is_empty());
        for i in 0..15 {
            let file = dir.join(format!("design {i} 雪.ferr"));
            std::fs::write(&file, "untouched").unwrap();
            recent.remember(&file).unwrap();
        }
        let first = dir.join("design 4 雪.ferr");
        recent.remember(&first).unwrap();
        recent.remember(&dir.join(".").join("design 4 雪.ferr")).unwrap();
        let (mut reopened, error) = RecentFiles::load(path.clone());
        assert!(error.is_none());
        assert_eq!(reopened.files(), recent.files());
        assert_eq!(reopened.files().len(), 12);
        assert_eq!(reopened.files()[0], first.canonicalize().unwrap());
        assert!(reopened.files()[1].ends_with("design 14 雪.ferr"));
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        reopened.clear().unwrap();
        assert!(RecentFiles::load(path).0.files().is_empty());
        assert_eq!(std::fs::read_to_string(first).unwrap(), "untouched");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recent_bad_history_reports_error_and_a_later_success_can_repair_it() {
        let dir = dir("bad"); let path = dir.join("recent.json");
        std::fs::write(&path, "invalid json").unwrap();
        let (mut recent, error) = RecentFiles::load(path.clone());
        assert!(error.unwrap().contains("Couldn't read recent files"));
        assert!(recent.files().is_empty());
        recent.remember(&dir.join("design.ferr")).unwrap();
        assert!(RecentFiles::load(path).1.is_none());
        // A failed history write is reported without losing the in-memory list.
        let (mut blocked, _) = RecentFiles::load(dir.clone());
        assert!(blocked.remember(&dir.join("design.ferr")).is_err());
        assert_eq!(blocked.files().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
