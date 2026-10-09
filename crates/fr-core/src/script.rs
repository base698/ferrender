//! Scripts: Rhai programs that drive the command API, with declared inputs,
//! a file sandbox, limits and progress. A script is a text file with a
//! `META` map at the top level and a `run(inputs)` function:
//!
//! ```rhai
//! const META = #{
//!     name: "Spacer",
//!     description: "A round spacer of a given height and bore.",
//!     inputs: [
//!         #{ name: "height", kind: "length", initial: "10 mm" },
//!         #{ name: "bore", kind: "length", initial: "5 mm" },
//!     ],
//! };
//!
//! fn run(inputs) {
//!     primitive(#{ type: "cylinder", diameter: 20, height: inputs.height });
//!     hole(#{ body: scene().bodies[0].id, at: [0, 0, inputs.height], diameter: inputs.bore, through: true });
//! }
//! ```
//!
//! Every command in [`crate::api::OPS`] is a host function taking a map of
//! arguments (the same JSON as the API) and returning the command's result.
//! Files are reachable only inside the script's folder, the document's folder
//! and any folders the caller allows. Files a script writes are staged beside
//! their target and moved into place only when the run succeeds, so a
//! cancelled or failed run leaves nothing behind.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhai::{Dynamic, Engine, EvalAltResult, Map, NativeCallContext, Position, Scope};
use serde_json::{Value, json};

use crate::doc::Session;
use crate::sandboxfs::{Dir, Entry};
use crate::sketch::Id;

type R<T> = Result<T, String>;
type Fallible = Result<Dynamic, Box<EvalAltResult>>;

pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_OPERATIONS: u64 = 50_000_000;
pub const MAX_CALL_LEVELS: usize = 64;
pub const MAX_STRING: usize = 1024 * 1024;
pub const MAX_ARRAY: usize = 1_000_000;
pub const MAX_MAP: usize = 100_000;
/// Embedded data above this size leaves an exported script as a file beside it.
pub const SIDECAR_BYTES: usize = 64 * 1024;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_EVENTS: usize = 10_000;

/// The bundled sample scripts: name and source.
pub const SAMPLES: &[(&str, &str)] = &[
    ("variants.rhai", include_str!("../assets/scripts/variants.rhai")),
    ("export-folder.rhai", include_str!("../assets/scripts/export-folder.rhai")),
    ("spur-gear.rhai", include_str!("../assets/scripts/spur-gear.rhai")),
    ("boss-at-points.rhai", include_str!("../assets/scripts/boss-at-points.rhai")),
    ("ci-check.rhai", include_str!("../assets/scripts/ci-check.rhai")),
    ("face-relief.rhai", include_str!("../assets/scripts/face-relief.rhai")),
];

/// What a script declares about an input.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Input {
    pub name: String,
    /// `length`, `angle`, `number`, `integer`, `bool`, `choice`, `text`, `folder`, `file`, `body`, `face`, `sketch`.
    #[serde(default = "text_kind")]
    pub kind: String,
    /// The value offered before the user changes it (`initial` in META, since `default` is a Rhai keyword).
    #[serde(default, alias = "default")]
    pub initial: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

fn text_kind() -> String { "text".into() }

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Meta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
}

/// What a run may touch and how questions are answered.
#[derive(Clone, Default)]
pub struct Sandbox {
    /// Folders whose files the script may read and write (and anything below them).
    pub allowed: Vec<PathBuf>,
    /// Answer every `confirm` with yes and every `ask` with its default.
    pub yes: bool,
}

/// Something the script reports while it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Log(String),
    Progress(f64, String),
}

/// A run's request.
pub struct Request {
    pub source: String,
    pub script_dir: Option<PathBuf>,
    /// The inputs as the caller collected them: an object of name to value.
    pub inputs: Value,
    pub sandbox: Sandbox,
    /// What the app had selected: `{"bodies": [...], "faces": [...], "sketches": [...]}`.
    pub selection: Value,
    /// Set by the caller to stop the script at its next operation.
    pub cancel: Arc<AtomicBool>,
    /// Receives logs and progress as they happen.
    pub events: Option<std::sync::mpsc::Sender<Event>>,
    /// Answers `confirm` (default `None`) and `ask` (default `Some(text)`); `None` declines.
    pub ask: Option<Arc<dyn Fn(&str, Option<&str>) -> Option<String> + Send + Sync>>,
    /// The run fails at its next script operation or command once this much time has passed.
    /// A modeling call already under way runs to its end first.
    pub time_limit: Option<Duration>,
}

impl Request {
    pub fn new(source: impl Into<String>) -> Request {
        Request { source: source.into(), script_dir: None, inputs: json!({}), sandbox: Sandbox::default(), selection: json!({}), cancel: Arc::new(AtomicBool::new(false)), events: None, ask: None, time_limit: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Outcome {
    pub log: Vec<String>,
    /// Files the script wrote through save, export, screenshot or the write helpers,
    /// moved into place when the run finished.
    pub exports: Vec<PathBuf>,
    /// What `run` returned, if anything JSON-like.
    pub result: Value,
    /// Features the script added, in order.
    pub features: Vec<Id>,
    pub cancelled: bool,
}

fn runtime(msg: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(Dynamic::from(msg.into()), Position::NONE))
}

fn engine_with_limits() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_string_size(MAX_STRING);
    engine.set_max_array_size(MAX_ARRAY);
    engine.set_max_map_size(MAX_MAP);
    engine.set_max_expr_depths(64, 64);
    engine.disable_symbol("eval");
    // Engine::new installs a filesystem module resolver. Imports must not bypass
    // the host file sandbox, including while the Scripts menu reads META.
    engine.disable_symbol("import");
    engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
    // Metadata inspection must not write into the MCP JSON-RPC stdout stream.
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});
    engine
}

/// Reads a script's `META` without running it. The top level of a script
/// must only declare things; `run` is never called here.
pub fn meta(source: &str) -> R<Meta> {
    if source.len() > MAX_SOURCE_BYTES { return Err("the script is larger than 1 MB".into()); }
    let mut engine = engine_with_limits();
    engine.set_max_operations(100_000);
    let ast = engine.compile(source).map_err(|e| format!("the script does not parse: {e}"))?;
    let mut scope = Scope::new();
    engine.run_ast_with_scope(&mut scope, &ast).map_err(|e| format!("the script's top level failed: {e}"))?;
    let meta: Dynamic = scope.get_value("META").ok_or("the script has no META map at the top level")?;
    let value: Value = rhai::serde::from_dynamic(&meta).map_err(|e| format!("META is not a plain map: {e}"))?;
    let meta: Meta = serde_json::from_value(value).map_err(|e| format!("META is malformed: {e}"))?;
    if meta.name.trim().is_empty() { return Err("META needs a name".into()); }
    if !ast.iter_functions().any(|f| f.name == "run") { return Err("the script has no `fn run(inputs)`".into()); }
    Ok(meta)
}

/// A path the sandbox has admitted: the file itself and the allowed root it is under.
struct Resolved {
    full: PathBuf,
    root: PathBuf,
}

impl Resolved {
    fn name(&self) -> &OsStr {
        self.full.file_name().unwrap_or_default()
    }

    /// The file's folder, opened from the allowed root without following links,
    /// so a link or a swapped folder planted after the check is refused.
    fn dir(&self) -> R<Dir> {
        let parent = self.full.parent().ok_or_else(|| format!("{} has no folder", self.full.display()))?;
        let rel = parent.strip_prefix(&self.root).map_err(|_| format!("{} is outside {}", self.full.display(), self.root.display()))?;
        crate::sandboxfs::open_beneath(&self.root, rel)
    }
}

/// A path the script may touch: inside one of the allowed folders, after resolving `..` and links.
fn allowed(sandbox: &Sandbox, path: &Path, for_write: bool) -> R<Resolved> {
    let absolute = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().map_err(|e| e.to_string())?.join(path) };
    // The file itself may not exist yet; its folder must.
    let (dir, name) = match (absolute.parent(), absolute.file_name()) {
        (Some(d), Some(n)) => (d, n),
        _ => return Err(format!("{} is not a file path", path.display())),
    };
    let dir = dir.canonicalize().map_err(|_| format!("the folder of {} does not exist", path.display()))?;
    let unresolved = dir.join(name);
    let full = match std::fs::symlink_metadata(&unresolved) {
        Ok(_) => unresolved.canonicalize().map_err(|_| format!("could not resolve {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => unresolved,
        Err(e) => return Err(format!("could not inspect {}: {e}", path.display())),
    };
    let root = sandbox.allowed.iter().filter_map(|a| a.canonicalize().ok()).find(|a| full.starts_with(a));
    match root {
        Some(root) => Ok(Resolved { full, root }),
        None => Err(format!("the script may not {} {}: it is outside the allowed folders ({})", if for_write { "write" } else { "read" }, full.display(), sandbox.allowed.iter().map(|a| a.display().to_string()).collect::<Vec<_>>().join(", "))),
    }
}

/// Bounded source loading shared by the app, CLI and API. In particular, reject
/// devices and pipes before opening them so inspecting script metadata cannot hang.
pub fn read_source(path: &Path) -> R<String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("could not inspect {}: {e}", path.display()))?;
    if !metadata.is_file() { return Err(format!("{} is not a regular file", path.display())); }
    let file = File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    read_text_bounded(file, path, MAX_SOURCE_BYTES)
}

fn read_text_bounded(file: File, path: &Path, max: usize) -> R<String> {
    let metadata = file.metadata().map_err(|e| format!("could not inspect {}: {e}", path.display()))?;
    if !metadata.is_file() { return Err(format!("{} is not a regular file", path.display())); }
    if metadata.len() > max as u64 { return Err(format!("{} exceeds the {} byte limit", path.display(), max)); }
    let mut text = String::new();
    file.take(max as u64 + 1).read_to_string(&mut text).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if text.len() > max { return Err(format!("{} exceeds the {} byte limit", path.display(), max)); }
    Ok(text)
}

/// A file the script is writing: a temporary beside its target, moved into
/// place when the run succeeds and removed when it does not.
struct Staged {
    dir: Dir,
    name: OsString,
    tmp: OsString,
    full: PathBuf,
    /// Whether the write that was meant for it actually happened.
    written: bool,
}

#[derive(Default)]
struct Stage {
    entries: Vec<Staged>,
}

impl Stage {
    fn find(&self, full: &Path) -> Option<usize> {
        self.entries.iter().position(|e| e.full == full)
    }

    /// Creates a fresh temporary for `r`'s target and hands it over.
    fn prepare(&mut self, r: &Resolved) -> R<(usize, File)> {
        if let Some(i) = self.find(&r.full) {
            // Written twice in one run: the earlier temporary goes.
            let old = self.entries.remove(i);
            let _ = old.dir.unlink(&old.tmp);
        }
        let dir = r.dir()?;
        let name = r.name().to_owned();
        for _ in 0..16 {
            let tmp = crate::sandboxfs::temp_name(&name);
            match dir.create_new(&tmp) {
                Ok(file) => {
                    self.entries.push(Staged { dir, name, tmp, full: r.full.clone(), written: false });
                    return Ok((self.entries.len() - 1, file));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("could not create a file beside {}: {e}", r.full.display())),
            }
        }
        Err(format!("could not find a free temporary name beside {}", r.full.display()))
    }

    fn tmp_path(&self, i: usize) -> PathBuf {
        self.entries[i].dir.path().join(&self.entries[i].tmp)
    }

    fn mark_written(&mut self, i: usize) {
        self.entries[i].written = true;
    }

    /// Forgets a staged file whose write failed.
    fn drop_entry(&mut self, i: usize) {
        let e = self.entries.remove(i);
        let _ = e.dir.unlink(&e.tmp);
    }

    /// Where a file the run already wrote lives right now.
    fn written(&self, full: &Path) -> Option<usize> {
        self.find(full).filter(|i| self.entries[*i].written)
    }

    fn open_written(&self, i: usize) -> R<File> {
        self.entries[i].dir.open_read(&self.entries[i].tmp)
    }

    /// Moves every written file onto its target; returns the targets.
    fn commit(&mut self) -> R<Vec<PathBuf>> {
        let mut done = Vec::new();
        let mut failed = Vec::new();
        for e in self.entries.drain(..) {
            if !e.written { let _ = e.dir.unlink(&e.tmp); continue; }
            let result = (|| -> R<()> {
                // A design saved over a plain 0.3 file keeps the same backup a save from the app would.
                if e.full.extension().is_some_and(|x| x == "ferr") {
                    let tmp = e.dir.path().join(&e.tmp);
                    crate::io::keep_backup(&e.full, crate::io::is_container(&tmp))?;
                }
                e.dir.rename(&e.tmp, &e.name)
            })();
            match result {
                Ok(()) => done.push(e.full),
                Err(err) => { let _ = e.dir.unlink(&e.tmp); failed.push(err); }
            }
        }
        if failed.is_empty() { Ok(done) } else { Err(format!("{} of its files could not be put in place: {}", failed.len(), failed.join("; "))) }
    }

    fn discard(&mut self) {
        for e in self.entries.drain(..) { let _ = e.dir.unlink(&e.tmp); }
    }
}

impl Drop for Stage {
    fn drop(&mut self) { self.discard(); }
}

/// Everything the host functions share.
struct Host {
    shared: Mutex<Session>,
    outcome: Mutex<Outcome>,
    sandbox: Sandbox,
    stage: Mutex<Stage>,
    cancel: Arc<AtomicBool>,
    deadline: Option<Instant>,
    timed_out: AtomicBool,
}

impl Host {
    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Fails once the time limit has passed; checked before every command and helper.
    fn check_time(&self) -> Result<(), Box<EvalAltResult>> {
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            self.timed_out.store(true, Ordering::Relaxed);
            return Err(runtime("time limit"));
        }
        Ok(())
    }

    /// Opens a file for reading: the run's own staged copy if it wrote it, else the file itself.
    fn read(&self, path: &str) -> R<File> {
        let r = allowed(&self.sandbox, Path::new(path), false)?;
        let stage = self.stage.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = stage.written(&r.full) { return stage.open_written(i); }
        r.dir()?.open_read(r.name())
    }

    /// Writes a file through the stage; it appears at `path` when the run succeeds.
    fn write(&self, path: &str, bytes: &[u8]) -> R<PathBuf> {
        let r = allowed(&self.sandbox, Path::new(path), true)?;
        let mut stage = self.stage.lock().unwrap_or_else(|e| e.into_inner());
        let (i, mut file) = stage.prepare(&r)?;
        match file.write_all(bytes).and_then(|_| file.sync_all()) {
            Ok(()) => { stage.mark_written(i); Ok(r.full) }
            Err(e) => { stage.drop_entry(i); Err(format!("could not write {}: {e}", r.full.display())) }
        }
    }

    fn exists(&self, path: &str) -> bool {
        let Ok(r) = allowed(&self.sandbox, Path::new(path), false) else { return false };
        if self.stage.lock().unwrap_or_else(|e| e.into_inner()).written(&r.full).is_some() { return true; }
        r.dir().is_ok_and(|d| d.entry(r.name()) != Entry::Missing)
    }
}

fn to_value(d: &Dynamic) -> Fallible {
    Ok(d.clone())
}

fn json_to_dynamic(v: &Value) -> Fallible {
    rhai::serde::to_dynamic(v).map_err(|e| runtime(format!("could not convert a result: {e}")))
}

fn dynamic_to_json(d: &Dynamic) -> Result<Value, Box<EvalAltResult>> {
    rhai::serde::from_dynamic::<Value>(d).map_err(|e| runtime(format!("could not convert an argument: {e}")))
}

/// Runs a script against the session. The session is edited in place: the
/// caller decides what the run counts as (one undo step in the app).
pub fn run(session: &mut Session, req: &Request) -> R<Outcome> {
    if session.read_only { return Err("this newer design is read-only; scripts cannot run against it".into()); }
    if req.source.len() > MAX_SOURCE_BYTES { return Err("the script is larger than 1 MB".into()); }
    let meta = meta(&req.source)?;
    // Validate before moving the session into the host callbacks: an invalid
    // input must never replace the caller's document with Session::default().
    let inputs = resolve_inputs(&meta, &req.inputs, session)?;
    let inputs_dynamic = rhai::serde::to_dynamic(&inputs).map_err(|e| e.to_string())?;
    let before: Vec<Id> = session.doc.features.iter().map(|f| f.id).collect();
    session.doc.begin_script_run();
    let source_id = Arc::new(ring::digest::digest(&ring::digest::SHA256, req.source.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect::<String>());
    let mut engine = engine_with_limits();
    let ast = engine.compile(&req.source).map_err(|e| format!("the script does not parse: {e}"))?;

    let started = Instant::now();
    let host = Arc::new(Host {
        shared: Mutex::new(std::mem::take(session)),
        outcome: Mutex::new(Outcome::default()),
        sandbox: req.sandbox.clone(),
        stage: Mutex::new(Stage::default()),
        cancel: req.cancel.clone(),
        deadline: req.time_limit.map(|t| started + t),
        timed_out: AtomicBool::new(false),
    });
    let script_dir = req.script_dir.clone();
    let document_dir = host.session().path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
    let events = req.events.clone();
    let event_budget = Arc::new(Mutex::new((0usize, 0usize)));
    let cancel = req.cancel.clone();
    let asker = req.ask.clone();
    let selection = req.selection.clone();

    // Progress: check the cancel flag and the clock every so often.
    {
        let host = host.clone();
        engine.on_progress(move |ops| {
            if ops % 10_000 != 0 { return None; }
            if host.cancel.load(Ordering::Relaxed) { return Some(Dynamic::from("cancelled")); }
            if host.check_time().is_err() { return Some(Dynamic::from("time limit")); }
            None
        });
    }

    // One host function per command, taking the command's arguments as a map, plus a no-argument form.
    // Two commands are Rhai keywords and get other names: `new` is `new_design`, `thread` is `add_thread`.
    for op in crate::api::OPS.iter().copied().filter(|o| !matches!(*o, "run_script" | "script_meta" | "batch")) {
        let name = script_name(op);
        let with_args = { let (host, source_id) = (host.clone(), source_id.clone()); move |ctx: NativeCallContext, args: Map| -> Fallible { command(&host, op, args, &source_id, ctx.call_position()) } };
        let bare = { let (host, source_id) = (host.clone(), source_id.clone()); move |ctx: NativeCallContext| -> Fallible { command(&host, op, Map::new(), &source_id, ctx.call_position()) } };
        engine.register_fn(name, with_args);
        engine.register_fn(name, bare);
    }
    {
        let (host, source_id) = (host.clone(), source_id.clone());
        engine.register_fn("command", move |ctx: NativeCallContext, cmd: Map| -> Fallible {
            let op = cmd.get("op").and_then(|o| o.clone().into_string().ok()).ok_or_else(|| runtime("command(map) needs an \"op\""))?;
            if op == "batch" { return Err(runtime("run a script's commands one at a time, not as a batch")); }
            command(&host, &op, cmd, &source_id, ctx.call_position())
        });
    }
    // Reading helpers.
    {
        let host = host.clone();
        engine.register_fn("scene", move || -> Fallible { let mut s = host.session(); json_to_dynamic(&crate::api::execute(&mut s, &json!({"op": "get_scene_info"}), None).map_err(runtime)?) });
    }
    {
        let host = host.clone();
        engine.register_fn("info", move |id: i64| -> Fallible { let mut s = host.session(); json_to_dynamic(&crate::api::execute(&mut s, &json!({"op": "get_object_info", "id": id}), None).map_err(runtime)?) });
    }
    {
        let host = host.clone();
        engine.register_fn("params", move || -> Fallible {
            let s = host.session();
            let m: serde_json::Map<String, Value> = s.doc.params.iter().map(|p| (p.name.clone(), json!({"expr": p.expr, "value": s.doc.show_param(&p.name)}))).collect();
            json_to_dynamic(&Value::Object(m))
        });
    }
    {
        let host = host.clone();
        engine.register_fn("errors", move || -> Fallible {
            let s = host.session();
            let list: Vec<Value> = s.built.errors.iter().map(|(id, e)| json!({"feature": id, "name": s.doc.feature(*id).map(|f| f.name.clone()), "error": e})).collect();
            json_to_dynamic(&Value::Array(list))
        });
    }
    {
        let selection = selection.clone();
        engine.register_fn("selection", move || -> Fallible { json_to_dynamic(&selection) });
    }
    {
        let host = host.clone();
        let shot = move |path: String, opts: Map| -> Fallible {
            host.check_time()?;
            let mut cmd = Value::Object(dynamic_to_json(&Dynamic::from(opts))?.as_object().cloned().unwrap_or_default());
            cmd["op"] = json!("get_viewport_screenshot");
            let png = { let mut s = host.session(); crate::api::execute(&mut s, &cmd, None).map_err(runtime)? };
            let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, png["png_base64"].as_str().unwrap_or("")).map_err(|e| runtime(e.to_string()))?;
            let full = host.write(&path, &bytes).map_err(runtime)?;
            Ok(Dynamic::from(full.display().to_string()))
        };
        let shot2 = shot.clone();
        engine.register_fn("screenshot", shot);
        engine.register_fn("screenshot", move |path: String| shot2(path, Map::new()));
    }
    {
        let host = host.clone();
        engine.register_fn("measure", move |from: Map, to: Map| -> Fallible {
            let mut s = host.session();
            let cmd = json!({"op": "measure", "from": dynamic_to_json(&Dynamic::from(from))?, "to": dynamic_to_json(&Dynamic::from(to))?});
            json_to_dynamic(&crate::api::execute(&mut s, &cmd, None).map_err(runtime)?)
        });
    }
    // Files.
    {
        let host = host.clone();
        engine.register_fn("read_text", move |path: String| -> Fallible {
            host.check_time()?;
            let file = host.read(&path).map_err(runtime)?;
            read_text_bounded(file, Path::new(&path), MAX_FILE_BYTES).map(Dynamic::from).map_err(runtime)
        });
    }
    {
        let host = host.clone();
        engine.register_fn("write_text", move |path: String, text: String| -> Fallible {
            host.check_time()?;
            host.write(&path, text.as_bytes()).map_err(runtime)?;
            Ok(Dynamic::UNIT)
        });
    }
    {
        let host = host.clone();
        engine.register_fn("read_csv", move |path: String| -> Fallible {
            host.check_time()?;
            let file = host.read(&path).map_err(runtime)?;
            let text = read_text_bounded(file, Path::new(&path), MAX_FILE_BYTES).map_err(runtime)?;
            let rows: Vec<Value> = text.lines().filter(|l| !l.trim().is_empty()).map(|l| Value::Array(csv_fields(l).into_iter().map(Value::String).collect())).collect();
            json_to_dynamic(&Value::Array(rows))
        });
    }
    {
        let host = host.clone();
        engine.register_fn("write_csv", move |path: String, rows: rhai::Array| -> Fallible {
            host.check_time()?;
            let mut text = String::new();
            for row in rows {
                let cells: Vec<String> = row.into_array().map_err(|_| runtime("write_csv takes an array of rows, each an array of cells"))?.into_iter().map(|c| csv_quote(&c.to_string())).collect();
                text.push_str(&cells.join(","));
                text.push('\n');
                if text.len() > MAX_FILE_BYTES { return Err(runtime("the CSV exceeds 16 MiB")); }
            }
            host.write(&path, text.as_bytes()).map_err(runtime)?;
            Ok(Dynamic::UNIT)
        });
    }
    {
        let host = host.clone();
        let list = move |dir: String, suffix: String| -> Fallible {
            let probe = Path::new(&dir).join("x");
            let r = allowed(&host.sandbox, &probe, false).map_err(runtime)?;
            let dir = r.dir().map_err(runtime)?;
            let mut names: Vec<String> = std::fs::read_dir(dir.path()).map_err(|e| runtime(format!("could not list {}: {e}", dir.path().display())))?
                .filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file() && p.to_string_lossy().to_ascii_lowercase().ends_with(&suffix.to_ascii_lowercase())).map(|p| p.display().to_string()).collect();
            names.sort();
            Ok(Dynamic::from(names.into_iter().map(Dynamic::from).collect::<rhai::Array>()))
        };
        let list2 = list.clone();
        engine.register_fn("list_files", list);
        engine.register_fn("list_files", move |dir: String| list2(dir, String::new()));
    }
    {
        let host = host.clone();
        engine.register_fn("exists", move |path: String| -> bool { host.exists(&path) });
    }
    {
        let host = host.clone();
        engine.register_fn("mkdir", move |path: String| -> Fallible {
            let r = allowed(&host.sandbox, Path::new(&path), true).map_err(runtime)?;
            r.dir().map_err(runtime)?.mkdir(r.name()).map_err(runtime)?;
            Ok(Dynamic::UNIT)
        });
    }
    engine.register_fn("join", |a: String, b: String| Path::new(&a).join(b).display().to_string());
    engine.register_fn("basename", |a: String| Path::new(&a).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    {
        let d = document_dir.clone();
        engine.register_fn("document_dir", move || -> Fallible { d.as_ref().map(|p| Dynamic::from(p.display().to_string())).ok_or_else(|| runtime("the document has not been saved, so it has no folder; save it first")) });
    }
    {
        let d = script_dir.clone();
        engine.register_fn("script_dir", move || -> Fallible { d.as_ref().map(|p| Dynamic::from(p.display().to_string())).ok_or_else(|| runtime("the script was given as text, so it has no folder")) });
    }
    // Flow.
    {
        let (host, events, budget) = (host.clone(), events.clone(), event_budget.clone());
        let log = move |text: String| report_event(&host.outcome, &events, &budget, Event::Log(text));
        let print = log.clone();
        engine.on_print(move |text| print(text.to_owned()));
        let debug = log.clone();
        engine.on_debug(move |text, _, _| debug(text.to_owned()));
        engine.register_fn("log", move |msg: Dynamic| log(msg.to_string()));
    }
    {
        let (host, events, cancel, budget) = (host.clone(), events.clone(), cancel.clone(), event_budget.clone());
        let report = move |fraction: f64, msg: String| -> Fallible {
            report_event(&host.outcome, &events, &budget, Event::Progress(fraction.clamp(0.0, 1.0), msg));
            if cancel.load(Ordering::Relaxed) { return Err(runtime("cancelled")); }
            host.check_time()?;
            Ok(Dynamic::UNIT)
        };
        let (r1, r2) = (report.clone(), report.clone());
        engine.register_fn("progress", report);
        engine.register_fn("progress", move |fraction: i64, msg: String| r1(fraction as f64, msg));
        engine.register_fn("progress", move |fraction: f64| r2(fraction, String::new()));
    }
    {
        let (asker, yes) = (asker.clone(), req.sandbox.yes);
        engine.register_fn("confirm", move |msg: String| -> bool { if yes { true } else { asker.as_ref().is_some_and(|a| a(&msg, None).is_some()) } });
    }
    {
        let (asker, yes) = (asker.clone(), req.sandbox.yes);
        engine.register_fn("ask", move |msg: String, default: String| -> Fallible {
            if yes { return Ok(Dynamic::from(default)); }
            match asker.as_ref().and_then(|a| a(&msg, Some(&default))) { Some(answer) => Ok(Dynamic::from(answer)), None => Err(runtime("the question was declined")) }
        });
    }
    engine.register_fn("fail", |msg: String| -> Fallible { Err(runtime(msg)) });
    engine.register_fn("name_template", |template: String, values: Map| -> String {
        let mut out = template;
        for (k, v) in values { out = out.replace(&format!("{{{k}}}"), &v.to_string()); }
        out
    });

    // The inputs: declared defaults filled in, lengths and angles as numbers in millimetres and degrees,
    // with the typed expressions beside them in `inputs.expr`.
    let mut scope = Scope::new();
    // Restore ownership on every evaluation result, including a failed top level.
    let result = engine.run_ast_with_scope(&mut scope, &ast)
        .and_then(|_| engine.call_fn::<Dynamic>(&mut scope, &ast, "run", (inputs_dynamic,)));
    drop(engine);
    let host = Arc::try_unwrap(host).map_err(|_| "the script engine kept a handle on the session".to_string())?;
    let Host { shared, outcome, mut stage, timed_out, .. } = host;
    let mut stage = std::mem::take(stage.get_mut().unwrap_or_else(|e| e.into_inner()));
    let mut out = outcome.into_inner().unwrap_or_else(|e| e.into_inner());
    *session = shared.into_inner().unwrap_or_else(|e| e.into_inner());
    session.doc.finish_feature_id_reuse();
    out.features = session.doc.features.iter().map(|f| f.id).filter(|id| !before.contains(id)).collect();
    match result {
        Ok(v) => {
            out.result = rhai::serde::from_dynamic(&v).unwrap_or(Value::Null);
            out.exports = stage.commit().map_err(|e| format!("{} finished, but {e}", meta.name))?;
        }
        Err(_) if timed_out.load(Ordering::Relaxed) => {
            stage.discard();
            return Err(format!("{}: the run exceeded its time limit of {:.0} s", meta.name, req.time_limit.unwrap_or_default().as_secs_f64()));
        }
        Err(e) if e.to_string().contains("cancelled") || cancel.load(Ordering::Relaxed) => { stage.discard(); out.cancelled = true; }
        Err(e) => { stage.discard(); return Err(format!("{}: {e}", meta.name)); }
    }
    Ok(out)
}

/// Both the retained log and an unconsumed UI event queue have a shared budget.
/// Exceeding it drops further reporting, without preventing cancellation or work.
fn report_event(outcome: &Mutex<Outcome>, events: &Option<std::sync::mpsc::Sender<Event>>, budget: &Arc<Mutex<(usize, usize)>>, event: Event) {
    let bytes = match &event { Event::Log(s) | Event::Progress(_, s) => s.len() };
    let mut budget = budget.lock().unwrap();
    if budget.0 >= MAX_EVENTS || bytes > MAX_EVENT_BYTES.saturating_sub(budget.1) { return; }
    budget.0 += 1;
    budget.1 += bytes;
    if let Event::Log(text) = &event { outcome.lock().unwrap().log.push(text.clone()); }
    if let Some(events) = events { let _ = events.send(event); }
}

/// The host function name of a command: the command's name unless Rhai reserves it.
pub fn script_name(op: &str) -> &str {
    match op {
        "new" => "new_design",
        "thread" => "add_thread",
        other => other,
    }
}

/// Commands whose `path` argument names a file to read.
const READS_PATH: &[&str] = &["import_stl", "import_mesh", "mesh_from_image", "open"];
/// Commands whose `path` argument names a file to write; those writes are staged.
const WRITES_PATH: &[&str] = &["save", "export_stl", "export_step", "get_viewport_screenshot"];

/// Runs one command for a script, checking paths against the sandbox, staging
/// files it writes and noting where a `save` really went.
fn command(host: &Host, op: &str, args: Map, source_id: &str, at: Position) -> Fallible {
    // The generic command(map) entry point must have exactly the same privileges as
    // named host functions; nested scripts could otherwise supply a wider allow list.
    if matches!(op, "run_script" | "script_meta" | "batch") {
        return Err(runtime(format!("{op} cannot be called from a script")));
    }
    host.check_time()?;
    let mut cmd = dynamic_to_json(&Dynamic::from(args))?;
    if !cmd.is_object() { cmd = json!({}); }
    cmd["op"] = json!(op);
    // A stable identity for what this command makes, so a re-run can give the same
    // outputs the same ids: an explicit key, the feature's name, or the call site.
    let explicit = cmd.as_object_mut().and_then(|o| o.remove("output_key"));
    let key = if let Some(value) = explicit {
        let key = value.as_str().filter(|key| !key.trim().is_empty() && key.len() <= 256)
            .ok_or_else(|| runtime("output_key must be a nonempty string of at most 256 bytes"))?;
        format!("explicit:{key}")
    } else if let Some(name) = cmd.get("name").and_then(Value::as_str).filter(|name| !name.trim().is_empty() && name.len() <= 256) {
        format!("named:{op}:{name}")
    } else {
        format!("source:{source_id}:{}:{}", at.line().unwrap_or(0), at.position().unwrap_or(0))
    };
    if op == "save" && cmd.get("path").and_then(Value::as_str).is_none() {
        if let Some(path) = host.session().path.as_ref() {
            cmd["path"] = json!(path.display().to_string());
        }
    }
    let mut stage = host.stage.lock().unwrap_or_else(|e| e.into_inner());
    let mut staged = None;
    let mut saved_to = None;
    // `path` on the file commands; `mesh_path` and `image_path` on add_feature.
    let keys: &[&str] = if op == "add_feature" { &["mesh_path", "image_path"] } else if READS_PATH.contains(&op) || WRITES_PATH.contains(&op) { &["path"] } else { &[] };
    for key in keys {
        let Some(p) = cmd.get(*key).and_then(Value::as_str).map(str::to_owned) else { continue };
        let writes = WRITES_PATH.contains(&op);
        let r = allowed(&host.sandbox, Path::new(&p), writes).map_err(runtime)?;
        if writes {
            let (i, _file) = stage.prepare(&r).map_err(runtime)?;
            cmd[*key] = json!(stage.tmp_path(i).display().to_string());
            staged = Some(i);
            if op == "save" { saved_to = Some(r.full.clone()); }
        } else if let Some(i) = stage.written(&r.full) {
            // A file this run wrote: read the staged copy.
            cmd[*key] = json!(stage.tmp_path(i).display().to_string());
        } else {
            // Look at the entry from the opened folder right before the command reads it by path.
            match r.dir().map_err(runtime)?.entry(r.name()) {
                Entry::File => {}
                Entry::Missing => return Err(runtime(format!("{op}: there is no file {}", r.full.display()))),
                _ => return Err(runtime(format!("{op}: {} is not a regular file", r.full.display()))),
            }
            cmd[*key] = json!(r.full.display().to_string());
        }
    }
    let mut s = host.session();
    s.doc.begin_script_command(key);
    let out = crate::api::execute(&mut s, &cmd, None);
    let identity = s.doc.end_script_command();
    match (&out, staged) {
        (Ok(_), Some(i)) => {
            stage.mark_written(i);
            if let Some(full) = saved_to { s.path = Some(full); }
        }
        (Err(_), Some(i)) => stage.drop_entry(i),
        _ => {}
    }
    let out = out.map_err(|e| runtime(format!("{op}: {e}")))?;
    identity.map_err(runtime)?;
    json_to_dynamic(&out)
}

/// The inputs a script receives: every declared input present, numbers in document-independent
/// millimetres and degrees, plus `expr` with the typed text of each length or angle.
pub fn resolve_inputs(meta: &Meta, given: &Value, session: &Session) -> R<Value> {
    let mut out = serde_json::Map::new();
    let mut expr = serde_json::Map::new();
    for input in &meta.inputs {
        let raw = given.get(&input.name).filter(|v| !v.is_null()).cloned().unwrap_or_else(|| input.initial.clone());
        let value = match input.kind.as_str() {
            "length" | "angle" => {
                let kind = if input.kind == "length" { crate::expr::Kind::Length } else { crate::expr::Kind::Angle };
                let text = match &raw { Value::String(s) => s.clone(), Value::Number(n) => format!("{n} {}", if kind == crate::expr::Kind::Length { session.doc.units.name() } else { "deg" }), Value::Null => return Err(format!("input {} has no value", input.name)), other => other.to_string() };
                let v = session.doc.value(&text, kind).map_err(|e| format!("input {}: {e}", input.name))?;
                expr.insert(input.name.clone(), json!(text));
                json!(v.v)
            }
            "number" => json!(raw.as_f64().or_else(|| raw.as_str().and_then(|s| s.trim().parse::<f64>().ok())).ok_or(format!("input {} must be a number", input.name))?),
            "integer" => json!(raw.as_i64().or_else(|| raw.as_str().and_then(|s| s.trim().parse::<i64>().ok())).or_else(|| raw.as_f64().map(|f| f as i64)).ok_or(format!("input {} must be a whole number", input.name))?),
            "bool" => json!(raw.as_bool().or_else(|| raw.as_str().map(|s| matches!(s.trim(), "true" | "yes" | "1" | "on"))).unwrap_or(false)),
            "choice" => {
                let s = raw.as_str().unwrap_or("").to_owned();
                if !input.choices.is_empty() && !input.choices.contains(&s) { return Err(format!("input {} must be one of {}", input.name, input.choices.join(", "))); }
                json!(s)
            }
            "body" | "face" | "sketch" => raw,
            _ => match raw { Value::String(s) => json!(s), Value::Null => json!(""), other => json!(other.to_string()) },
        };
        out.insert(input.name.clone(), value);
    }
    // Inputs the script did not declare still pass through, as text.
    if let Some(o) = given.as_object() {
        for (k, v) in o {
            if !out.contains_key(k) && k != "expr" { out.insert(k.clone(), v.clone()); }
        }
    }
    out.insert("expr".into(), Value::Object(expr));
    Ok(Value::Object(out))
}

fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut quoted) = (String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => { cur.push('"'); chars.next(); }
            '"' => quoted = !quoted,
            ',' if !quoted => { out.push(std::mem::take(&mut cur)); }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_owned()).collect()
}

fn csv_quote(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() }
}

/// An exported timeline: the script, the files it reads from its own folder,
/// and what the export had to adjust to make a runnable script.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Export {
    pub source: String,
    /// Sidecar files by name, to be written beside the script: large imported
    /// meshes as STL and large embedded images as PNG.
    pub files: Vec<(String, Vec<u8>)>,
    pub notes: Vec<String>,
}

/// A script that rebuilds the document: every parameter becomes an expression
/// input and every feature a command. Sidecar files are named after `design`.
pub fn export_timeline(session: &Session) -> R<Export> {
    export_timeline_named(session, "design")
}

/// Like [`export_timeline`], naming sidecar files `<stem>-<feature id>.<ext>`.
///
/// Failed features are exported suppressed with a note, a timeline marker that
/// is not at the end is restored at the end of the script, hidden bodies that no
/// longer exist are dropped, and embedded data above [`SIDECAR_BYTES`] goes into
/// a file beside the script, which the script reads through `script_dir()`.
pub fn export_timeline_named(session: &Session, stem: &str) -> R<Export> {
    let doc = &session.doc;
    if session.read_only { return Err("a read-only cached design has no editable timeline to export".into()); }
    crate::validation::document(doc)?;
    let stem: String = stem.chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.')).collect::<String>().trim().to_owned();
    let stem = if stem.is_empty() { "design".to_owned() } else { stem };
    let mut export = Export::default();
    let out = &mut export.source;
    source_push(out, "// Exported by Ferrender: rebuilds the complete document.\n// Parameter inputs are expressions, evaluated in this design's units.\n")?;
    source_push(out, "const META = #{\n    name: \"Exported design\",\n    description: \"Rebuilds the exported timeline; parameters accept expressions.\",\n    inputs: [\n")?;
    // Text inputs retain dependencies, scalar values and radians. Typed script
    // inputs would be evaluated before new_design, against the caller's document.
    // `expr` is reserved by resolve_inputs for its per-input expression map.
    let names: Vec<_> = doc.params.iter().map(|p| {
        let mut name = p.name.clone();
        if name == "expr" {
            name = "parameter_expr".into();
            while doc.params.iter().any(|p| p.name == name) { name.push('_'); }
        }
        name
    }).collect();
    for (p, name) in doc.params.iter().zip(&names) {
        source_push(out, &format!("        #{{ name: {name:?}, label: {:?}, kind: \"text\", initial: {:?}, help: \"A parameter expression in the exported design's units.\" }},\n", p.name, p.expr))?;
    }
    source_push(out, "    ],\n};\n\nfn run(inputs) {\n")?;
    source_push(out, &format!("    new_design(#{{ units: {:?} }});\n", doc.units.name()))?;
    // Seed every name before installing its expression: valid saved parameter
    // tables may contain forward references. Seeds preserve each original unit.
    for p in &doc.params {
        let q = doc.quantity(&format!("${}", p.name))?;
        let suffix = match q.dim { crate::expr::Dim::None => "", crate::expr::Dim::Length => " mm", crate::expr::Dim::Angle => " deg" };
        source_push(out, &format!("    set_parameter(#{{ name: {:?}, expr: {:?} }});\n", p.name, format!("{}{suffix}", q.v)))?;
    }
    for (p, name) in doc.params.iter().zip(&names) {
        source_push(out, &format!("    set_parameter(#{{ name: {:?}, expr: inputs[{name:?}] }});\n", p.name))?;
    }
    for f in &doc.features {
        // Large embedded data goes beside the script: inline mesh serialization
        // expands to 48 bytes per triangle, and an image is its PNG in base64.
        let mut sidecar: Option<(&str, String)> = None;
        let mut value = match &f.kind {
            crate::FeatureKind::Import(mesh) if mesh.len() * 48 > SIDECAR_BYTES => {
                let file = format!("{stem}-{}.stl", f.id);
                export.files.push((file.clone(), crate::io::mesh_stl_bytes(mesh, crate::units::Unit::Mm)));
                export.notes.push(format!("the mesh of \"{}\" ({} triangles) is written as {file} beside the script", f.name, mesh.len()));
                sidecar = Some(("mesh_path", file));
                let head = crate::Feature { id: f.id, name: f.name.clone(), suppressed: f.suppressed, owner: f.owner, made_by: f.made_by, script_key: f.script_key.clone(), kind: crate::FeatureKind::Import(crate::mesh::Mesh::default()) };
                serde_json::to_value(&head).map_err(|e| format!("cannot export feature {}: {e}", f.name))?
            }
            crate::FeatureKind::Relief(r) if r.image.embedded_len() > SIDECAR_BYTES => {
                let file = format!("{stem}-{}.png", f.id);
                export.files.push((file.clone(), r.image.png_bytes()?));
                export.notes.push(format!("the image of \"{}\" is written as {file} beside the script", f.name));
                sidecar = Some(("image_path", file));
                let mut v = serde_json::to_value(f).map_err(|e| format!("cannot export feature {}: {e}", f.name))?;
                v["kind"]["relief"]["image"]["png"] = json!("");
                v
            }
            crate::FeatureKind::Sketch(sk) if sk.reference.as_ref().is_some_and(|i| i.embedded_len() > SIDECAR_BYTES) => {
                let file = format!("{stem}-{}.png", f.id);
                export.files.push((file.clone(), sk.reference.as_ref().unwrap().png_bytes()?));
                export.notes.push(format!("the reference image of \"{}\" is written as {file} beside the script", f.name));
                sidecar = Some(("image_path", file));
                let mut v = serde_json::to_value(f).map_err(|e| format!("cannot export feature {}: {e}", f.name))?;
                v["kind"]["sketch"]["reference"]["png"] = json!("");
                v
            }
            _ => {
                let mut bytes = LimitedJson { bytes: Vec::new(), limit: MAX_SOURCE_BYTES.saturating_sub(out.len()) };
                serde_json::to_writer(&mut bytes, f).map_err(|e| format!("cannot export feature {}: {e}", f.name))?;
                serde_json::from_slice(&bytes.bytes).map_err(|e| e.to_string())?
            }
        };
        if let Some(error) = session.built.errors.get(&f.id).filter(|_| !f.suppressed) {
            // A failing feature would stop the replay; it goes in suppressed, so the
            // rest builds and the user can fix it and unsuppress it afterwards.
            value["suppressed"] = json!(true);
            source_push(out, &format!("    // {:?} failed in the exported design ({}) and is suppressed here.\n", f.name, error.replace('\n', " ")))?;
            export.notes.push(format!("\"{}\" failed in the design ({error}); it is exported suppressed", f.name));
        }
        source_push(out, "    add_feature(#{ feature: ")?;
        rhai_literal(&value, out)?;
        if let Some((key, file)) = sidecar {
            source_push(out, &format!(", {key}: join(script_dir(), {file:?})"))?;
            if key == "mesh_path" { source_push(out, ", units: \"mm\"")?; }
        }
        source_push(out, " });\n")?;
    }
    let mut dropped = 0;
    for id in &doc.hidden_bodies {
        if doc.feature(*id).is_none() && session.built.body(*id).is_none() { dropped += 1; continue; }
        // Hidden only if the replay has that body: a suppressed or consumed one is not an error.
        source_push(out, &format!("    for b in scene().bodies {{ if b.id == {id} {{ set_visible(#{{ id: {id}, visible: false }}); }} }}\n"))?;
    }
    if dropped > 0 {
        export.notes.push(format!("{dropped} hidden body id{} no longer exist{} in the design and {} dropped", if dropped == 1 { "" } else { "s" }, if dropped == 1 { "s" } else { "" }, if dropped == 1 { "was" } else { "were" }));
    }
    if doc.active_component != 0 {
        source_push(out, &format!("    activate_component(#{{ id: {} }});\n", doc.active_component))?;
    }
    if let Some(at) = doc.rollback.filter(|at| *at < doc.features.len()) {
        // The whole timeline is exported; the marker goes back where it was at the end.
        match at.checked_sub(1).map(|i| doc.features[i].id) {
            Some(id) => source_push(out, &format!("    rollback(#{{ to: {id} }});\n"))?,
            None => source_push(out, "    rollback(#{ to: \"start\" });\n")?,
        }
        export.notes.push(format!("the timeline marker was after feature {at} of {}; the script restores it there", doc.features.len()));
    }
    source_push(out, "}\n")?;
    // Rhai has narrower literal/depth limits than JSON (for example u64 inputs).
    // Refuse those cases here, rather than saving a script the user cannot run.
    meta(&export.source).map_err(|e| format!("this timeline cannot be represented as a runnable script: {e}"))?;
    Ok(export)
}

fn source_push(out: &mut String, text: &str) -> R<()> {
    if text.len() > MAX_SOURCE_BYTES.saturating_sub(out.len()) {
        return Err("the exported script exceeds the 1 MB limit; save a .ferr design or reduce embedded mesh/image data".into());
    }
    out.push_str(text);
    Ok(())
}

struct LimitedJson { bytes: Vec<u8>, limit: usize }
impl std::io::Write for LimitedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("the exported script exceeds the 1 MB limit; save a .ferr design or reduce embedded mesh/image data"));
        }
        self.bytes.extend_from_slice(bytes); Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// A bounded JSON value as Rhai source. Strings remain data, including script
/// sources held by existing ScriptRun features; they are never evaluated here.
fn rhai_literal(v: &Value, out: &mut String) -> R<()> {
    match v {
        Value::Null => source_push(out, "()"),
        Value::Bool(b) => source_push(out, &b.to_string()),
        Value::Number(n) => {
            if n.as_u64().is_some_and(|v| v > i64::MAX as u64) {
                return Err("this timeline cannot be represented as a runnable script: an integer exceeds Rhai's signed 64-bit range".into());
            }
            source_push(out, &n.to_string())
        },
        Value::String(s) => source_push(out, &format!("{s:?}")),
        Value::Array(a) => {
            source_push(out, "[")?;
            for (i, v) in a.iter().enumerate() { if i > 0 { source_push(out, ", ")?; } rhai_literal(v, out)?; }
            source_push(out, "]")
        }
        Value::Object(o) => {
            source_push(out, "#{")?;
            for (i, (k, v)) in o.iter().enumerate() {
                if i > 0 { source_push(out, ", ")?; }
                source_push(out, &format!("{k:?}: "))?; rhai_literal(v, out)?;
            }
            source_push(out, "}")
        }
    }
}

#[allow(dead_code)]
fn _keep(_: &dyn Fn(&Dynamic) -> Fallible) {}
#[allow(dead_code)]
fn _keep2() { let _ = to_value; }

#[cfg(test)]
mod sandbox_tests {
    use super::*;

    #[test]
    fn dynamic_commands_cannot_expand_script_authority() {
        let host = Host {
            shared: Mutex::new(Session::default()), outcome: Mutex::new(Outcome::default()), sandbox: Sandbox::default(),
            stage: Mutex::new(Stage::default()), cancel: Arc::new(AtomicBool::new(false)), deadline: None, timed_out: AtomicBool::new(false),
        };
        for op in ["run_script", "script_meta", "batch"] {
            let err = command(&host, op, Map::new(), "test", Position::NONE).unwrap_err();
            assert!(err.to_string().contains("cannot be called from a script"));
        }
    }
}
