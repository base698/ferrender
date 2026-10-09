//! Recovery copies: every unsaved change is written to a file beside the settings, so a
//! crash or kill normally loses at most the last second of work. Storage failures are
//! reported to the app and retried; writes cannot guarantee survival of a power cut.
//!
//! The copy belongs to one running app. It is removed when the design is saved and when the
//! app closes normally, so a copy with no live owner is work that was lost. Each app holds a
//! lock file for as long as it runs; the system drops the lock however the app ends.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fr_core::doc::{Document, Session};
use fr_core::io;

const FORMAT: &str = "ferrender-recovery";
const EXT: &str = "ferr-recovery";
// Recovery v2 has a bounded JSON header followed by the native JSON/ZIP bytes.
// Keep the payload allowance aligned with the native container reader. Legacy
// recovery remains bounded by its original plain-JSON allowance.
const MAGIC: &[u8; 8] = b"FERRREC2";
const MAX_HEADER_BYTES: u64 = 64 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_RECOVERY_BYTES: u64 = MAX_PAYLOAD_BYTES + MAX_HEADER_BYTES + 12;
const MAX_LEGACY_BYTES: u64 = 64 * 1024 * 1024 + MAX_HEADER_BYTES;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Header {
    format: String,
    version: u32,
    path: Option<PathBuf>,
    saved: u64,
    payload_bytes: u64,
    payload_crc32: u32,
}
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
        load_copy(&self.file).map_err(|e| format!("could not read the recovery copy: {e}"))
    }

    pub fn delete(&self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_file(self.file.with_extension("lock"));
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Open only regular files, checking the size before reading any payload.
fn open_copy(path: &Path) -> std::io::Result<(File, u64)> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(std::io::Error::other("the recovery copy is not a regular file"));
    }
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_RECOVERY_BYTES {
        return Err(std::io::Error::other("the recovery copy exceeds the supported size limit or is not a regular file"));
    }
    Ok((file, metadata.len()))
}

/// The magic has already been read. This reads metadata only, never geometry.
fn read_header(reader: &mut impl Read, file_bytes: u64) -> std::io::Result<Header> {
    let mut size = [0; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as u64;
    if size == 0 || size > MAX_HEADER_BYTES {
        return Err(std::io::Error::other("the recovery metadata exceeds the supported size limit"));
    }
    let mut bytes = vec![0; size as usize];
    reader.read_exact(&mut bytes)?;
    let head: Header = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    if head.format != FORMAT || head.version != 2 {
        return Err(std::io::Error::other("the recovery copy has an unrecognized format or version"));
    }
    if head.payload_bytes == 0 || head.payload_bytes > MAX_PAYLOAD_BYTES {
        return Err(std::io::Error::other("the recovery payload exceeds the supported size limit"));
    }
    if file_bytes != MAGIC.len() as u64 + 4 + size + head.payload_bytes {
        return Err(std::io::Error::other("the recovery copy is truncated or its size is damaged"));
    }
    Ok(head)
}

fn load_copy(path: &Path) -> Result<Document, String> {
    let (mut file, size) = open_copy(path).map_err(|e| e.to_string())?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic).map_err(|e| e.to_string())?;
    if &magic == MAGIC {
        let head = read_header(&mut file, size).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(head.payload_bytes + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() as u64 != head.payload_bytes || crc32fast::hash(&bytes) != head.payload_crc32 {
            return Err("the recovery payload is truncated or damaged (checksum mismatch)".into());
        }
        return io::decode(&bytes);
    }
    if size > MAX_LEGACY_BYTES {
        return Err("the legacy recovery copy exceeds the supported size limit".into());
    }
    let mut bytes = magic.to_vec();
    file.take(MAX_LEGACY_BYTES + 1 - bytes.len() as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_LEGACY_BYTES { return Err("the legacy recovery copy exceeds the supported size limit".into()); }
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("the recovery copy is damaged: {e}"))?;
    if v["format"] != FORMAT { return Err("the recovery copy has an unrecognized or damaged format".into()); }
    io::from_json(&v["doc"].to_string())
}

fn metadata(path: &Path) -> std::io::Result<(Option<PathBuf>, u64)> {
    let (mut file, size) = open_copy(path)?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    if &magic == MAGIC {
        let head = read_header(&mut file, size)?;
        return Ok((head.path, head.saved));
    }
    // The legacy writer placed format/path/saved before ,"doc":. Parse only
    // that prefix. A quoted path cannot contain this literal delimiter because
    // its quotes are escaped. Noncanonical/damaged legacy metadata is kept in
    // the list with an unknown name; loading still validates the entire copy.
    let mut prefix = magic.to_vec();
    file.take(MAX_HEADER_BYTES.saturating_sub(prefix.len() as u64)).read_to_end(&mut prefix)?;
    let marker = b",\"doc\":";
    if let Some(at) = prefix.windows(marker.len()).position(|w| w == marker) {
        prefix.truncate(at);
        prefix.push(b'}');
    }
    let v: serde_json::Value = serde_json::from_slice(&prefix).map_err(std::io::Error::other)?;
    if v["format"] != FORMAT { return Err(std::io::Error::other("unrecognized recovery metadata")); }
    Ok((v["path"].as_str().map(PathBuf::from), v["saved"].as_u64().unwrap_or(0)))
}

fn write(file: &Path, doc: &Document, path: Option<&Path>) -> std::io::Result<()> {
    // The native codec validates the design and detaches image/mesh payloads.
    // Never expand a large mesh into legacy base64 JSON for autosave.
    let (payload, _) = io::encode(doc, &io::Extras::default()).map_err(std::io::Error::other)?;
    if payload.is_empty() || payload.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err(std::io::Error::other("the recovery payload exceeds the supported size limit"));
    }
    let head = Header { format: FORMAT.into(), version: 2, path: path.map(Path::to_path_buf), saved: now(), payload_bytes: payload.len() as u64, payload_crc32: crc32fast::hash(&payload) };
    let head = serde_json::to_vec(&head).map_err(std::io::Error::other)?;
    if head.len() as u64 > MAX_HEADER_BYTES {
        return Err(std::io::Error::other("the recovery metadata exceeds the supported size limit"));
    }
    let tmp = file.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Exclusive creation never follows a leftover link or truncates another
    // file. Only our newly created temporary file is cleaned up on failure.
    let mut output = options.open(&tmp)?;
    let result = (|| {
        output.write_all(MAGIC)?;
        output.write_all(&(head.len() as u32).to_le_bytes())?;
        output.write_all(&head)?;
        output.write_all(&payload)?;
        output.sync_all()?;
        drop(output);
        std::fs::rename(&tmp, file)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
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
        match metadata(&file) {
            Ok((path, saved)) => out.push(Found { path, saved, file }),
            // A read failure or damaged header is not permission to delete someone's work.
            // Keep it visible so the user can inspect the error and choose whether to delete it.
            Err(_) => out.push(Found { file, path: None, saved: 0 }),
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
            // A successful explicit save makes an earlier failed recovery
            // attempt irrelevant; there is no unsaved work left at risk.
            self.error = None;
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

    #[test]
    fn legacy_json_recovery_still_discovers_and_loads() {
        let d = dir("legacy-json");
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(format!("legacy.{EXT}"));
        let mut doc = Document::new(Unit::Mm);
        doc.set_param("width", "12.5 mm").unwrap();
        // The exact old writer layout, including an awkward quoted filename.
        let path = PathBuf::from("/designs/a,\"doc\":name.ferr");
        let head = serde_json::json!({"format": FORMAT, "path": path, "saved": 42}).to_string();
        let old = format!("{},\"doc\":{}}}", &head[..head.len()-1], io::validated_json(&doc).unwrap());
        std::fs::write(&file, &old).unwrap();
        let copies = found(&d);
        assert_eq!(copies.len(), 1);
        assert_eq!((copies[0].path.as_ref(), copies[0].saved), (Some(&path), 42));
        assert_eq!(copies[0].load().unwrap(), doc);
        assert_eq!(std::fs::read_to_string(file).unwrap(), old, "reading never rewrites a legacy copy");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn large_mesh_and_embedded_image_recovery_uses_native_container() {
        use fr_core::{doc::FeatureKind, mesh::Mesh, reference::ReferenceImage, sketch::{Plane, Sketch}};
        use glam::DVec3;
        let d = dir("large-native");
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(format!("large.{EXT}"));
        let triangles = 1_500_000;
        assert!(triangles * 48 > 64 * 1024 * 1024, "the old base64 recovery exceeds its JSON allowance");
        // Indexed storage is the important boundary here; repeated geometry
        // keeps this I/O regression inexpensive without invoking mesh repair.
        let mesh = Mesh::from_indexed(vec![DVec3::ZERO, DVec3::X, DVec3::Y], vec![[0, 1, 2]; triangles], true).unwrap();
        let mut doc = Document::new(Unit::Mm);
        doc.add_feature(FeatureKind::Import(mesh));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 3).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut sketch = Sketch::new(Plane::XY);
        sketch.reference = Some(ReferenceImage::from_bytes("trace.png", png.get_ref(), 20.0).unwrap());
        doc.add_feature(FeatureKind::Sketch(sketch));
        write(&file, &doc, Some(Path::new("/designs/scan.ferr"))).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        assert!(bytes.starts_with(MAGIC));
        let head_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        assert!(bytes[12 + head_len..].starts_with(b"PK"));
        let copies = found(&d);
        assert_eq!(copies[0].name(), "scan");
        let recovered = copies[0].load().unwrap();
        assert_eq!(recovered, doc);
        assert_eq!(recovered.sketches().next().unwrap().1.reference.as_ref().unwrap().pixel_height, 3);
        // Developer-only real scan exercise; CI always covers the synthetic
        // large document above and never depends on a personal local fixture.
        if let Some(path) = std::env::var_os("FERRENDER_RECOVERY_TEST_FILE") {
            let doc = io::open(Path::new(&path)).unwrap().doc.unwrap();
            let started = std::time::Instant::now();
            write(&file, &doc, Some(Path::new(&path))).unwrap();
            assert_eq!(load_copy(&file).unwrap(), doc);
            eprintln!("real native recovery round trip: {:.3}s, {} bytes", started.elapsed().as_secs_f64(), std::fs::metadata(&file).unwrap().len());
        }
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn discovery_reads_bounded_metadata_without_loading_geometry() {
        let d = dir("metadata-only");
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(format!("large.{EXT}"));
        let head = Header { format: FORMAT.into(), version: 2, path: Some("/designs/large.ferr".into()), saved: 99, payload_bytes: 1024 * 1024 * 1024, payload_crc32: 0 };
        let json = serde_json::to_vec(&head).unwrap();
        let total = 12 + json.len() as u64 + head.payload_bytes;
        let mut only_header = (json.len() as u32).to_le_bytes().to_vec();
        only_header.extend(&json);
        // Reader contains no payload at all: metadata parsing must not ask for it.
        assert_eq!(read_header(&mut &only_header[..], total).unwrap().saved, 99);
        let mut out = File::create(&file).unwrap();
        out.write_all(MAGIC).unwrap();
        out.write_all(&only_header).unwrap();
        out.set_len(total).unwrap(); // sparse dummy payload, never decoded by discovery
        assert_eq!(metadata(&file).unwrap(), (Some("/designs/large.ferr".into()), 99));
        assert_eq!(found(&d)[0].name(), "large");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn corrupt_or_oversized_recovery_is_refused_but_kept_visible() {
        let d = dir("corrupt-v2");
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(format!("copy.{EXT}"));
        write(&file, &Document::new(Unit::Mm), None).unwrap();
        let good = std::fs::read(&file).unwrap();
        let mut corrupt = good.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        for bytes in [corrupt, good[..good.len()-1].to_vec(), [MAGIC.as_slice(), &((MAX_HEADER_BYTES + 1) as u32).to_le_bytes()].concat()] {
            std::fs::write(&file, &bytes).unwrap();
            assert!(load_copy(&file).is_err());
            assert_eq!(found(&d).len(), 1);
            assert_eq!(std::fs::read(&file).unwrap(), bytes);
        }
        let head = Header { format: FORMAT.into(), version: 2, path: None, saved: 0, payload_bytes: MAX_PAYLOAD_BYTES + 1, payload_crc32: 0 };
        let json = serde_json::to_vec(&head).unwrap();
        let bytes = [(json.len() as u32).to_le_bytes().as_slice(), &json].concat();
        assert!(read_header(&mut &bytes[..], 12 + json.len() as u64 + head.payload_bytes).unwrap_err().to_string().contains("size limit"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn failed_replacement_keeps_the_last_readable_native_copy() {
        let d = dir("failed-replace");
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(format!("copy.{EXT}"));
        let mut doc = Document::new(Unit::Mm);
        doc.set_param("before", "1 mm").unwrap();
        write(&file, &doc, None).unwrap();
        let previous = std::fs::read(&file).unwrap();
        let blocked = file.with_extension("tmp");
        std::fs::create_dir(&blocked).unwrap();
        let mut changed = doc.clone();
        changed.set_param("after", "2 mm").unwrap();
        assert!(write(&file, &changed, None).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), previous);
        assert_eq!(load_copy(&file).unwrap(), doc);
        std::fs::remove_dir(&blocked).unwrap();
        write(&file, &changed, None).unwrap();
        assert_eq!(load_copy(&file).unwrap(), changed);
        assert!(!blocked.exists());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn saving_clears_a_prior_failed_recovery_warning() {
        let d = dir("saved-after-error");
        let mut r = Recovery::start(d.clone());
        let mut s = Session::default();
        changed(&mut s, "width");
        std::fs::create_dir(r.file.with_extension("tmp")).unwrap();
        r.tick(&s, 0.0);
        r.wait();
        r.tick(&s, 0.1);
        assert!(r.error().is_some() && !r.on_disk);
        s.save(&d.join("saved.ferr")).unwrap();
        assert_eq!(r.tick(&s, 0.2), None);
        assert!(r.error().is_none(), "saved work no longer needs the failed recovery copy");
        r.close();
        std::fs::remove_dir_all(d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn temporary_recovery_links_cannot_overwrite_another_file() {
        use std::os::unix::fs::symlink;
        let d = dir("temporary-link");
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(format!("copy.{EXT}"));
        let sentinel = d.join("unrelated.txt");
        std::fs::write(&sentinel, "keep this").unwrap();
        let doc = Document::new(Unit::Mm);
        write(&file, &doc, None).unwrap();
        let previous = std::fs::read(&file).unwrap();
        symlink(&sentinel, file.with_extension("tmp")).unwrap();
        assert!(write(&file, &doc, None).is_err());
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep this");
        assert_eq!(std::fs::read(&file).unwrap(), previous);
        assert_eq!(load_copy(&file).unwrap(), doc);
        std::fs::remove_dir_all(d).unwrap();
    }

}
