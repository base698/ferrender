//! Recovery copies: every unsaved change is written to a file beside the settings, so a
//! crash or kill normally loses at most the last second of work. Storage failures are
//! reported to the app and retried; writes cannot guarantee survival of a power cut.
//!
//! The copy belongs to one running app. It is removed when the design is saved and when the
//! app closes normally, so a copy with no live owner is work that was lost. Each app holds a
//! lock file for as long as it runs; the system drops the lock however the app ends.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fr_core::doc::{Document, Session};
use fr_core::io;

const FORMAT: &str = "ferrender-recovery";
const EXT: &str = "ferr-recovery";
// A maximum-size native document plus bounded recovery metadata.
const MAX_RECOVERY_BYTES: u64 = 64 * 1024 * 1024 + 64 * 1024;
/// Seconds between copies while changes keep coming.
const EVERY: f64 = 1.0;

/// Jobs handed to the writer that it has not finished.
type Pending = Arc<(Mutex<usize>, Condvar)>;
static PENDING: OnceLock<Pending> = OnceLock::new();

enum Job {
    Write(Box<Document>, Option<PathBuf>, u64, u64),
    Remove,
}

/// Acknowledged changes to the file, with a document generation to reject stale revisions.
enum Written {
    Copy { generation: u64, edits: u64 },
    Removed,
}

/// A copy left behind by an app that did not close normally.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub file: PathBuf,
    /// The file the design was last saved to, if it ever was.
    pub path: Option<PathBuf>,
    /// When the copy was written, in seconds since 1970.
    pub saved: u64,
}

impl Found {
    pub fn name(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_stem()).map_or("Untitled".to_owned(), |n| n.to_string_lossy().into_owned())
    }

    /// How long ago the copy was written, in words.
    pub fn age(&self) -> String {
        if self.saved == 0 {
            return "date unavailable".to_owned();
        }
        let s = now().saturating_sub(self.saved);
        let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
        match s {
            0..60 => "just now".to_owned(),
            60..3600 => plural(s / 60, "minute"),
            3600..86400 => plural(s / 3600, "hour"),
            _ => plural(s / 86400, "day"),
        }
    }

    /// The design in the copy.
    pub fn load(&self) -> Result<Document, String> {
        let text = read_copy(&self.file).map_err(|e| format!("could not read the recovery copy: {e}"))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("the recovery copy is damaged: {e}"))?;
        if v["format"] != FORMAT {
            return Err("the recovery copy has an unrecognized or damaged format".into());
        }
        io::from_json(&v["doc"].to_string())
    }

    pub fn delete(&self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_file(self.file.with_extension("lock"));
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn read_copy(file: &Path) -> std::io::Result<String> {
    let mut text = String::new();
    File::open(file)?.take(MAX_RECOVERY_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_RECOVERY_BYTES {
        return Err(std::io::Error::other("the recovery copy exceeds the supported size limit"));
    }
    Ok(text)
}

fn write(file: &Path, doc: &Document, path: Option<&Path>) -> std::io::Result<()> {
    // Never acknowledge a backup that our native loader would reject.
    let doc = io::validated_json(doc).map_err(std::io::Error::other)?;
    let head = serde_json::json!({ "format": FORMAT, "path": path, "saved": now() }).to_string();
    // The design goes in as it would be saved, after the header's fields.
    let text = format!("{},\"doc\":{doc}}}", &head[..head.len() - 1]);
    if text.len() as u64 > MAX_RECOVERY_BYTES {
        return Err(std::io::Error::other("the recovery copy exceeds the supported size limit"));
    }
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
}

/// Copies in `dir` whose app is no longer running.
pub fn found(dir: &Path) -> Vec<Found> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for file in entries.flatten().map(|e| e.path()) {
        let ext = file.extension().and_then(|e| e.to_str());
        if ext != Some(EXT) && ext != Some("lock") {
            continue;
        }
        // A lock that cannot be taken is held by an app that is still running.
        let lock = file.with_extension("lock");
        if File::open(&lock).is_ok_and(|l| l.try_lock().is_err()) {
            continue;
        }
        if ext == Some("lock") {
            // Left by an app that had nothing unsaved.
            if !file.with_extension(EXT).exists() {
                let _ = std::fs::remove_file(&file);
            }
            continue;
        }
        let head = read_copy(&file).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v["format"] == FORMAT);
        match head {
            Some(v) => out.push(Found { path: v["path"].as_str().map(PathBuf::from), saved: v["saved"].as_u64().unwrap_or(0), file }),
            // A read failure or damaged header is not permission to delete someone's work.
            // Keep it visible so the user can inspect the error and choose whether to delete it.
            None => out.push(Found { file, path: None, saved: 0 }),
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.saved));
    out
}

/// Gives the writer a moment to finish what it has; called when the app is going down.
pub fn flush() {
    if let Some(p) = PENDING.get()
        && let Ok(n) = p.0.lock()
    {
        let _ = p.1.wait_timeout_while(n, Duration::from_secs(3), |n| *n > 0);
    }
}

/// This app's recovery copy.
pub struct Recovery {
    pub dir: PathBuf,
    file: PathBuf,
    lock: Option<File>,
    jobs: Sender<Job>,
    results: Receiver<Result<Written, String>>,
    pending: Pending,
    generation: u64,
    /// Only an acknowledged successful write advances this revision.
    seen: Option<u64>,
    sent_at: f64,
    on_disk: bool,
    in_flight: bool,
    error: Option<String>,
}

impl Recovery {
    pub fn start(dir: PathBuf) -> Recovery {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = dir.join(format!("{}-{}-{n}.{EXT}", std::process::id(), now()));
        let pending = Pending::default();
        let _ = PENDING.set(pending.clone());
        let (jobs, rx) = channel();
        let (done, results) = channel();
        let (target, counter) = (file.clone(), pending.clone());
        std::thread::spawn(move || writer(rx, &target, &counter, done));
        let mut recovery = Recovery { dir, file, lock: None, jobs, results, pending, generation: 0, seen: None, sent_at: f64::MIN, on_disk: false, in_flight: false, error: None };
        if let Err(e) = recovery.ensure_lock() {
            recovery.error = Some(e);
        }
        recovery
    }

    fn ensure_lock(&mut self) -> Result<(), String> {
        if self.lock.is_some() {
            return Ok(());
        }
        let acquire = || -> std::io::Result<File> {
            std::fs::create_dir_all(&self.dir)?;
            let lock = File::create(self.file.with_extension("lock"))?;
            lock.try_lock().map_err(std::io::Error::other)?;
            Ok(lock)
        };
        self.lock = Some(acquire().map_err(|e| format!("Could not prepare recovery storage: {e}"))?);
        Ok(())
    }

    fn send(&mut self, job: Job) {
        *self.pending.0.lock().unwrap() += 1;
        if self.jobs.send(job).is_err() {
            *self.pending.0.lock().unwrap() -= 1;
            self.pending.1.notify_all();
            self.error = Some("The recovery writer stopped. Save your design and restart Ferrender.".into());
        } else {
            self.in_flight = true;
        }
    }

    fn receive(&mut self) {
        while let Ok(result) = self.results.try_recv() {
            self.in_flight = false;
            match result {
                Ok(Written::Copy { generation, edits }) => {
                    self.on_disk = true;
                    if generation == self.generation {
                        self.seen = Some(edits);
                    }
                    self.error = None;
                }
                Ok(Written::Removed) => {
                    self.on_disk = false;
                    self.seen = None;
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// Revisions restart when a new document replaces the old one.
    pub fn new_document(&mut self) {
        self.generation += 1;
        self.seen = None;
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Keeps the copy up with the session, including polling the writer and retrying failures.
    pub fn tick(&mut self, s: &Session, time: f64) -> Option<f64> {
        self.receive();
        if self.in_flight {
            return Some(0.1);
        }
        if !s.dirty && !self.on_disk {
            return None;
        }
        if s.dirty && self.seen == Some(s.edits) && self.on_disk {
            return None;
        }
        let wait = self.sent_at + EVERY - time;
        if wait > 0.0 {
            return Some(wait);
        }
        self.sent_at = time;
        if let Err(e) = self.ensure_lock() {
            self.error = Some(e);
            return Some(EVERY);
        }
        if s.dirty {
            self.send(Job::Write(Box::new(s.doc.clone()), s.path.clone(), self.generation, s.edits));
        } else {
            self.send(Job::Remove);
        }
        Some(if self.in_flight { 0.1 } else { EVERY })
    }

    /// Writes the copy before returning, for a design that exists nowhere else.
    pub fn write_now(&mut self, s: &Session) -> Result<(), String> {
        self.wait();
        self.receive();
        if self.in_flight {
            return Err("The previous recovery write has not finished. The original copy was kept.".into());
        }
        let result = self.ensure_lock().and_then(|_| write(&self.file, &s.doc, s.path.as_deref()).map_err(|e| format!("Could not write the recovery copy: {e}")));
        if let Err(e) = result {
            self.error = Some(e.clone());
            return Err(e);
        }
        (self.seen, self.on_disk) = (Some(s.edits), true);
        self.error = None;
        Ok(())
    }

    /// Waits for the writer to finish what it has.
    pub fn wait(&self) {
        let n = self.pending.0.lock().unwrap();
        let _ = self.pending.1.wait_timeout_while(n, Duration::from_secs(10), |n| *n > 0);
    }

    /// Removes the copy and the lock: the app is closing with nothing to recover.
    pub fn close(&mut self) {
        // FIFO cleanup cannot be overtaken by a slow write that finishes after the wait.
        self.send(Job::Remove);
        self.wait();
        self.receive();
        if *self.pending.0.lock().unwrap() == 0 {
            Found { file: self.file.clone(), path: None, saved: 0 }.delete();
            self.on_disk = false;
        }
    }

    /// Copies left by apps that are no longer running.
    pub fn found(&self) -> Vec<Found> {
        found(&self.dir).into_iter().filter(|f| f.file != self.file).collect()
    }
}

fn writer(rx: Receiver<Job>, file: &Path, pending: &Pending, done: Sender<Result<Written, String>>) {
    while let Ok(job) = rx.recv() {
        let result = match job {
            Job::Write(doc, path, generation, edits) => write(file, &doc, path.as_deref()).map(|_| Written::Copy { generation, edits }),
            Job::Remove => match std::fs::remove_file(file) {
                Ok(()) => Ok(Written::Removed),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Written::Removed),
                Err(e) => Err(e),
            },
        };
        let _ = done.send(result.map_err(|e| format!("Could not update the recovery copy: {e}")));
        *pending.0.lock().unwrap() -= 1;
        pending.1.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fr_core::units::Unit;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ferrender-recovery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn changed(s: &mut Session, name: &str) {
        s.edit(|d| d.set_param(name, "5 mm")).unwrap();
    }

    #[test]
    fn a_copy_follows_the_changes_and_goes_when_saved() {
        let d = dir("follows");
        let mut r = Recovery::start(d.clone());
        let mut s = Session::new(Document::new(Unit::Mm));
        assert_eq!(r.tick(&s, 0.0), None);
        r.wait();
        assert!(!r.file.exists(), "nothing to recover from an untouched design");

        changed(&mut s, "a");
        assert_eq!(r.tick(&s, 10.0), Some(0.1));
        r.wait();
        assert!(r.file.exists());
        assert!(found(&d).is_empty(), "a running app's copy is not offered for recovery");

        // A change straight after waits its turn, then is written.
        changed(&mut s, "b");
        assert_eq!(r.tick(&s, 10.5), Some(0.5));
        assert_eq!(r.tick(&s, 11.0), Some(0.1));
        r.wait();
        let copy = Found { file: r.file.clone(), path: None, saved: 0 }.load().unwrap();
        assert_eq!(copy.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);

        let saved = d.join("part.ferr");
        s.save(&saved).unwrap();
        assert_eq!(r.tick(&s, 12.0), Some(0.1));
        r.wait();
        assert!(!r.file.exists(), "a saved design needs no copy");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_copy_outlives_its_app_only_when_the_app_did_not_close() {
        let d = dir("outlives");
        let mut s = Session::new(Document::new(Unit::Mm));
        s.path = Some(PathBuf::from("/designs/bracket.ferr"));
        changed(&mut s, "w");

        // Closed normally: nothing is left.
        let mut r = Recovery::start(d.clone());
        r.tick(&s, 0.0);
        r.close();
        drop(r);
        assert!(found(&d).is_empty());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);

        // Gone without closing: the copy is found, with the design and where it lived.
        let mut r = Recovery::start(d.clone());
        r.tick(&s, 0.0);
        r.wait();
        drop(r);
        let left = found(&d);
        assert_eq!(left.len(), 1);
        assert_eq!((left[0].name().as_str(), left[0].path.as_deref()), ("bracket", Some(Path::new("/designs/bracket.ferr"))));
        assert_eq!(left[0].age(), "just now");
        assert_eq!(left[0].load().unwrap().params[0].name, "w");

        // Another app starting up sees it too, and deleting it leaves nothing.
        let other = Recovery::start(d.clone());
        assert_eq!(other.found(), left);
        left[0].delete();
        assert!(other.found().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_copy_is_preserved_for_user_review() {
        let d = dir("damaged");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("1-1-0.{EXT}")), "{\"format\":\"ferrender-recov").unwrap();
        let copies = found(&d);
        assert_eq!(copies.len(), 1);
        assert!(copies[0].load().is_err());
        assert_eq!(std::fs::read_to_string(&copies[0].file).unwrap(), "{\"format\":\"ferrender-recov");
        copies[0].delete();
        assert!(found(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn failed_writes_are_reported_and_retried_without_another_edit() {
        let d = dir("retry");
        let mut r = Recovery::start(d.clone());
        let mut s = Session::default();
        changed(&mut s, "w");
        // The lock is valid, but the temporary output cannot be written.
        let blocked = r.file.with_extension("tmp");
        std::fs::create_dir(&blocked).unwrap();
        assert_eq!(r.tick(&s, 0.0), Some(0.1));
        r.wait();
        assert!(r.tick(&s, 0.2).is_some());
        assert!(r.error().is_some());
        assert_eq!(r.seen, None, "a queued or failed write is not an acknowledged copy");
        assert!(!r.file.exists());

        std::fs::remove_dir(blocked).unwrap();
        assert_eq!(r.tick(&s, 1.0), Some(0.1));
        r.wait();
        assert_eq!(r.tick(&s, 1.1), None);
        assert!(r.error().is_none());
        assert_eq!(r.seen, Some(s.edits));
        assert_eq!(Found { file: r.file.clone(), path: None, saved: 0 }.load().unwrap().params[0].name, "w");
        r.close();
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_replacement_document_cannot_reuse_the_previous_acknowledgment() {
        let d = dir("replacement");
        let mut r = Recovery::start(d.clone());
        let mut first = Session::default();
        changed(&mut first, "first");
        r.tick(&first, 0.0);
        let mut second = Session::default();
        changed(&mut second, "second");
        assert_eq!(first.edits, second.edits);
        r.new_document();
        r.wait();
        assert_eq!(r.tick(&second, 1.0), Some(0.1));
        r.wait();
        assert_eq!(r.tick(&second, 1.1), None);
        assert_eq!(Found { file: r.file.clone(), path: None, saved: 0 }.load().unwrap().params[0].name, "second");
        r.close();
        let _ = std::fs::remove_dir_all(d);
    }


    #[test]
    fn invalid_recovery_payload_keeps_the_last_readable_copy() {
        let d = dir("invalid-payload");
        let mut r = Recovery::start(d.clone());
        let mut s = Session::default();
        changed(&mut s, "w");
        r.tick(&s, 0.0);
        r.wait();
        assert_eq!(r.tick(&s, 0.1), None);
        let previous = std::fs::read(&r.file).unwrap();
        let previous_edits = r.seen;
        s.doc.next_id = 0;
        s.edits += 1;
        r.tick(&s, 1.0);
        r.wait();
        assert!(r.tick(&s, 1.1).is_some());
        assert!(r.error().is_some());
        assert_eq!(r.seen, previous_edits);
        assert_eq!(std::fs::read(&r.file).unwrap(), previous);
        r.close();
        let _ = std::fs::remove_dir_all(d);
    }

}
