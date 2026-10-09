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
//! and any folders the caller allows.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rhai::{Dynamic, Engine, EvalAltResult, Map, NativeCallContext, Position, Scope};
use serde_json::{Value, json};

use crate::doc::Session;
use crate::sketch::Id;

type R<T> = Result<T, String>;
type Fallible = Result<Dynamic, Box<EvalAltResult>>;

pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_OPERATIONS: u64 = 50_000_000;
pub const MAX_CALL_LEVELS: usize = 64;
pub const MAX_STRING: usize = 1024 * 1024;
pub const MAX_ARRAY: usize = 1_000_000;
pub const MAX_MAP: usize = 100_000;
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
}

impl Request {
    pub fn new(source: impl Into<String>) -> Request {
        Request { source: source.into(), script_dir: None, inputs: json!({}), sandbox: Sandbox::default(), selection: json!({}), cancel: Arc::new(AtomicBool::new(false)), events: None, ask: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Outcome {
    pub log: Vec<String>,
    /// Files the script wrote through save, export or screenshot.
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

/// A path the script may touch: inside one of the allowed folders, after resolving `..` and links.
fn allowed(sandbox: &Sandbox, path: &Path, for_write: bool) -> R<PathBuf> {
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
    let inside = sandbox.allowed.iter().filter_map(|a| a.canonicalize().ok()).any(|a| full.starts_with(&a));
    if !inside {
        return Err(format!("the script may not {} {}: it is outside the allowed folders ({})", if for_write { "write" } else { "read" }, full.display(), sandbox.allowed.iter().map(|a| a.display().to_string()).collect::<Vec<_>>().join(", ")));
    }
    Ok(full)
}

/// Bounded source loading shared by the app, CLI and API. In particular, reject
/// devices and pipes before opening them so inspecting script metadata cannot hang.
pub fn read_source(path: &Path) -> R<String> {
    read_text_bounded(path, MAX_SOURCE_BYTES)
}

fn read_text_bounded(path: &Path, max: usize) -> R<String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("could not inspect {}: {e}", path.display()))?;
    if !metadata.is_file() { return Err(format!("{} is not a regular file", path.display())); }
    if metadata.len() > max as u64 { return Err(format!("{} exceeds the {} byte limit", path.display(), max)); }
    let mut text = String::new();
    std::fs::File::open(path).and_then(|f| f.take(max as u64 + 1).read_to_string(&mut text))
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if text.len() > max { return Err(format!("{} exceeds the {} byte limit", path.display(), max)); }
    Ok(text)
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

    let shared = Arc::new(Mutex::new(std::mem::take(session)));
    let outcome = Arc::new(Mutex::new(Outcome::default()));
    let sandbox = Arc::new(req.sandbox.clone());
    let script_dir = req.script_dir.clone();
    let document_dir = shared.lock().unwrap().path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
    let events = req.events.clone();
    let event_budget = Arc::new(Mutex::new((0usize, 0usize)));
    let cancel = req.cancel.clone();
    let asker = req.ask.clone();
    let selection = req.selection.clone();

    // Progress: check the cancel flag every so often.
    {
        let cancel = cancel.clone();
        engine.on_progress(move |ops| if ops % 10_000 == 0 && cancel.load(Ordering::Relaxed) { Some(Dynamic::from("cancelled")) } else { None });
    }

    // One host function per command, taking the command's arguments as a map, plus a no-argument form.
    // Two commands are Rhai keywords and get other names: `new` is `new_design`, `thread` is `add_thread`.
    for op in crate::api::OPS.iter().copied().filter(|o| !matches!(*o, "run_script" | "script_meta" | "batch")) {
        let name = script_name(op);
        let with_args = {
            let (shared, outcome, sandbox, source_id) = (shared.clone(), outcome.clone(), sandbox.clone(), source_id.clone());
            move |ctx: NativeCallContext, args: Map| -> Fallible { command(&shared, &outcome, &sandbox, op, args, &source_id, ctx.call_position()) }
        };
        let bare = {
            let (shared, outcome, sandbox, source_id) = (shared.clone(), outcome.clone(), sandbox.clone(), source_id.clone());
            move |ctx: NativeCallContext| -> Fallible { command(&shared, &outcome, &sandbox, op, Map::new(), &source_id, ctx.call_position()) }
        };
        engine.register_fn(name, with_args);
        engine.register_fn(name, bare);
    }
    {
        let (shared, outcome, sandbox, source_id) = (shared.clone(), outcome.clone(), sandbox.clone(), source_id.clone());
        engine.register_fn("command", move |ctx: NativeCallContext, cmd: Map| -> Fallible {
            let op = cmd.get("op").and_then(|o| o.clone().into_string().ok()).ok_or_else(|| runtime("command(map) needs an \"op\""))?;
            if op == "batch" { return Err(runtime("run a script's commands one at a time, not as a batch")); }
            command(&shared, &outcome, &sandbox, &op, cmd, &source_id, ctx.call_position())
        });
    }
    // Reading helpers.
    {
        let shared = shared.clone();
        engine.register_fn("scene", move || -> Fallible { let mut s = shared.lock().unwrap(); json_to_dynamic(&crate::api::execute(&mut s, &json!({"op": "get_scene_info"}), None).map_err(runtime)?) });
    }
    {
        let shared = shared.clone();
        engine.register_fn("info", move |id: i64| -> Fallible { let mut s = shared.lock().unwrap(); json_to_dynamic(&crate::api::execute(&mut s, &json!({"op": "get_object_info", "id": id}), None).map_err(runtime)?) });
    }
    {
        let shared = shared.clone();
        engine.register_fn("params", move || -> Fallible {
            let s = shared.lock().unwrap();
            let m: serde_json::Map<String, Value> = s.doc.params.iter().map(|p| (p.name.clone(), json!({"expr": p.expr, "value": s.doc.show_param(&p.name)}))).collect();
            json_to_dynamic(&Value::Object(m))
        });
    }
    {
        let shared = shared.clone();
        engine.register_fn("errors", move || -> Fallible {
            let s = shared.lock().unwrap();
            let list: Vec<Value> = s.built.errors.iter().map(|(id, e)| json!({"feature": id, "name": s.doc.feature(*id).map(|f| f.name.clone()), "error": e})).collect();
            json_to_dynamic(&Value::Array(list))
        });
    }
    {
        let selection = selection.clone();
        engine.register_fn("selection", move || -> Fallible { json_to_dynamic(&selection) });
    }
    {
        let (shared, outcome, sandbox) = (shared.clone(), outcome.clone(), sandbox.clone());
        let shot = move |path: String, opts: Map| -> Fallible {
            let full = allowed(&sandbox, Path::new(&path), true).map_err(runtime)?;
            let mut cmd = Value::Object(dynamic_to_json(&Dynamic::from(opts))?.as_object().cloned().unwrap_or_default());
            cmd["op"] = json!("get_viewport_screenshot");
            let png = { let mut s = shared.lock().unwrap(); crate::api::execute(&mut s, &cmd, None).map_err(runtime)? };
            let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, png["png_base64"].as_str().unwrap_or("")).map_err(|e| runtime(e.to_string()))?;
            std::fs::write(&full, bytes).map_err(|e| runtime(format!("could not write {}: {e}", full.display())))?;
            outcome.lock().unwrap().exports.push(full.clone());
            Ok(Dynamic::from(full.display().to_string()))
        };
        let shot2 = shot.clone();
        engine.register_fn("screenshot", shot);
        engine.register_fn("screenshot", move |path: String| shot2(path, Map::new()));
    }
    {
        let shared = shared.clone();
        engine.register_fn("measure", move |from: Map, to: Map| -> Fallible {
            let mut s = shared.lock().unwrap();
            let cmd = json!({"op": "measure", "from": dynamic_to_json(&Dynamic::from(from))?, "to": dynamic_to_json(&Dynamic::from(to))?});
            json_to_dynamic(&crate::api::execute(&mut s, &cmd, None).map_err(runtime)?)
        });
    }
    // Files.
    {
        let sandbox = sandbox.clone();
        engine.register_fn("read_text", move |path: String| -> Fallible {
            let full = allowed(&sandbox, Path::new(&path), false).map_err(runtime)?;
            read_text_bounded(&full, MAX_FILE_BYTES).map(Dynamic::from).map_err(runtime)
        });
    }
    {
        let (sandbox, outcome) = (sandbox.clone(), outcome.clone());
        engine.register_fn("write_text", move |path: String, text: String| -> Fallible {
            let full = allowed(&sandbox, Path::new(&path), true).map_err(runtime)?;
            std::fs::write(&full, text).map_err(|e| runtime(format!("could not write {}: {e}", full.display())))?;
            outcome.lock().unwrap().exports.push(full);
            Ok(Dynamic::UNIT)
        });
    }
    {
        let sandbox = sandbox.clone();
        engine.register_fn("read_csv", move |path: String| -> Fallible {
            let full = allowed(&sandbox, Path::new(&path), false).map_err(runtime)?;
            let text = read_text_bounded(&full, MAX_FILE_BYTES).map_err(runtime)?;
            let rows: Vec<Value> = text.lines().filter(|l| !l.trim().is_empty()).map(|l| Value::Array(csv_fields(l).into_iter().map(Value::String).collect())).collect();
            json_to_dynamic(&Value::Array(rows))
        });
    }
    {
        let (sandbox, outcome) = (sandbox.clone(), outcome.clone());
        engine.register_fn("write_csv", move |path: String, rows: rhai::Array| -> Fallible {
            let full = allowed(&sandbox, Path::new(&path), true).map_err(runtime)?;
            let mut text = String::new();
            for row in rows {
                let cells: Vec<String> = row.into_array().map_err(|_| runtime("write_csv takes an array of rows, each an array of cells"))?.into_iter().map(|c| csv_quote(&c.to_string())).collect();
                text.push_str(&cells.join(","));
                text.push('\n');
                if text.len() > MAX_FILE_BYTES { return Err(runtime("the CSV exceeds 16 MiB")); }
            }
            std::fs::write(&full, text).map_err(|e| runtime(format!("could not write {}: {e}", full.display())))?;
            outcome.lock().unwrap().exports.push(full);
            Ok(Dynamic::UNIT)
        });
    }
    {
        let sandbox = sandbox.clone();
        let list = move |dir: String, suffix: String| -> Fallible {
            let probe = Path::new(&dir).join("x");
            let full = allowed(&sandbox, &probe, false).map_err(runtime)?;
            let dir = full.parent().unwrap().to_path_buf();
            let mut names: Vec<String> = std::fs::read_dir(&dir).map_err(|e| runtime(format!("could not list {}: {e}", dir.display())))?
                .filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file() && p.to_string_lossy().to_ascii_lowercase().ends_with(&suffix.to_ascii_lowercase())).map(|p| p.display().to_string()).collect();
            names.sort();
            Ok(Dynamic::from(names.into_iter().map(Dynamic::from).collect::<rhai::Array>()))
        };
        let list2 = list.clone();
        engine.register_fn("list_files", list);
        engine.register_fn("list_files", move |dir: String| list2(dir, String::new()));
    }
    {
        let sandbox = sandbox.clone();
        engine.register_fn("exists", move |path: String| -> bool { allowed(&sandbox, Path::new(&path), false).map(|p| p.exists()).unwrap_or(false) });
    }
    {
        let sandbox = sandbox.clone();
        engine.register_fn("mkdir", move |path: String| -> Fallible {
            let full = allowed(&sandbox, Path::new(&path), true).map_err(runtime)?;
            std::fs::create_dir_all(&full).map_err(|e| runtime(format!("could not create {}: {e}", full.display())))?;
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
        let (outcome, events, budget) = (outcome.clone(), events.clone(), event_budget.clone());
        let log = move |text: String| report_event(&outcome, &events, &budget, Event::Log(text));
        let print = log.clone();
        engine.on_print(move |text| print(text.to_owned()));
        let debug = log.clone();
        engine.on_debug(move |text, _, _| debug(text.to_owned()));
        engine.register_fn("log", move |msg: Dynamic| log(msg.to_string()));
    }
    {
        let (outcome, events, cancel, budget) = (outcome.clone(), events.clone(), cancel.clone(), event_budget.clone());
        let report = move |fraction: f64, msg: String| -> Fallible {
            report_event(&outcome, &events, &budget, Event::Progress(fraction.clamp(0.0, 1.0), msg));
            if cancel.load(Ordering::Relaxed) { return Err(runtime("cancelled")); }
            Ok(Dynamic::UNIT)
        };
        let (r1, r2) = (report.clone(), report.clone());
        engine.register_fn("progress", report);
        engine.register_fn("progress", move |fraction: i64, msg: String| r1(fraction as f64, msg));
        engine.register_fn("progress", move |fraction: f64| r2(fraction, String::new()));
    }
    {
        let (asker, yes) = (asker.clone(), sandbox.yes);
        engine.register_fn("confirm", move |msg: String| -> bool { if yes { true } else { asker.as_ref().is_some_and(|a| a(&msg, None).is_some()) } });
    }
    {
        let (asker, yes) = (asker.clone(), sandbox.yes);
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
    let mut out = Arc::try_unwrap(outcome).map(|m| m.into_inner().unwrap()).unwrap_or_default();
    *session = Arc::try_unwrap(shared).map(|m| m.into_inner().unwrap()).map_err(|_| "the script engine kept a handle on the session")?;
    session.doc.finish_feature_id_reuse();
    out.features = session.doc.features.iter().map(|f| f.id).filter(|id| !before.contains(id)).collect();
    match result {
        Ok(v) => { out.result = rhai::serde::from_dynamic(&v).unwrap_or(Value::Null); }
        Err(e) if e.to_string().contains("cancelled") || cancel.load(Ordering::Relaxed) => { out.cancelled = true; }
        Err(e) => return Err(format!("{}: {e}", meta.name)),
    }
    Ok(out)
}

/// Both the retained log and an unconsumed UI event queue have a shared budget.
/// Exceeding it drops further reporting, without preventing cancellation or work.
fn report_event(outcome: &Arc<Mutex<Outcome>>, events: &Option<std::sync::mpsc::Sender<Event>>, budget: &Arc<Mutex<(usize, usize)>>, event: Event) {
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

/// Runs one command for a script, checking paths against the sandbox and noting exports.
fn command(shared: &Arc<Mutex<Session>>, outcome: &Arc<Mutex<Outcome>>, sandbox: &Sandbox, op: &str, args: Map, source_id: &str, at: Position) -> Fallible {
    // The generic command(map) entry point must have exactly the same privileges as
    // named host functions; nested scripts could otherwise supply a wider allow list.
    if matches!(op, "run_script" | "script_meta" | "batch") {
        return Err(runtime(format!("{op} cannot be called from a script")));
    }
    let mut cmd = dynamic_to_json(&Dynamic::from(args))?;
    if !cmd.is_object() { cmd = json!({}); }
    cmd["op"] = json!(op);
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
        if let Some(path) = shared.lock().unwrap().path.as_ref() {
            cmd["path"] = json!(path.display().to_string());
        }
    }
    let path = cmd.get("path").and_then(Value::as_str).map(str::to_owned);
    let writes = matches!(op, "save" | "export_stl" | "export_step" | "get_viewport_screenshot");
    if let Some(p) = &path && matches!(op, "save" | "export_stl" | "export_step" | "import_stl" | "import_mesh" | "mesh_from_image" | "open") {
        let full = allowed(sandbox, Path::new(p), writes).map_err(runtime)?;
        cmd["path"] = json!(full.display().to_string());
        if writes { outcome.lock().unwrap().exports.push(full); }
    }
    let mut s = shared.lock().unwrap();
    s.doc.begin_script_command(key);
    let result = crate::api::execute(&mut s, &cmd, None);
    let identity = s.doc.end_script_command();
    let out = result.map_err(|e| runtime(format!("{op}: {e}")))?;
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

/// A script that rebuilds the document: every parameter becomes an expression
/// input and every feature a command. Unsupported replay states are refused
/// before the caller writes a script that cannot reproduce the saved design.
pub fn export_timeline(session: &Session) -> R<String> {
    let doc = &session.doc;
    if session.read_only { return Err("a read-only cached design has no editable timeline to export".into()); }
    if doc.rollback.is_some() { return Err("move the timeline marker to the end before exporting it as a script".into()); }
    if !session.built.errors.is_empty() { return Err("fix or suppress failed timeline features before exporting a script".into()); }
    if doc.hidden_bodies.iter().any(|id| session.built.body(*id).is_none()) {
        return Err("the timeline has hidden bodies that are currently suppressed or removed; show those bodies before exporting a script".into());
    }
    crate::validation::document(doc)?;
    let mut out = String::new();
    source_push(&mut out, "// Exported by Ferrender: rebuilds the complete document.\n// Parameter inputs are expressions, evaluated in this design's units.\n")?;
    source_push(&mut out, "const META = #{\n    name: \"Exported design\",\n    description: \"Rebuilds the exported timeline; parameters accept expressions.\",\n    inputs: [\n")?;
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
        source_push(&mut out, &format!("        #{{ name: {name:?}, label: {:?}, kind: \"text\", initial: {:?}, help: \"A parameter expression in the exported design's units.\" }},\n", p.name, p.expr))?;
    }
    source_push(&mut out, "    ],\n};\n\nfn run(inputs) {\n")?;
    source_push(&mut out, &format!("    new_design(#{{ units: {:?} }});\n", doc.units.name()))?;
    // Seed every name before installing its expression: valid saved parameter
    // tables may contain forward references. Seeds preserve each original unit.
    for p in &doc.params {
        let q = doc.quantity(&format!("${}", p.name))?;
        let suffix = match q.dim { crate::expr::Dim::None => "", crate::expr::Dim::Length => " mm", crate::expr::Dim::Angle => " deg" };
        source_push(&mut out, &format!("    set_parameter(#{{ name: {:?}, expr: {:?} }});\n", p.name, format!("{}{suffix}", q.v)))?;
    }
    for (p, name) in doc.params.iter().zip(&names) {
        source_push(&mut out, &format!("    set_parameter(#{{ name: {:?}, expr: inputs[{name:?}] }});\n", p.name))?;
    }
    for f in &doc.features {
        // Inline mesh serialization expands to 48 bytes per triangle and itself
        // allocates a temporary buffer. Reject large meshes before invoking it.
        if let crate::FeatureKind::Import(mesh) = &f.kind
            && mesh.len() > MAX_SOURCE_BYTES.saturating_sub(out.len()) / 48 {
            return Err("the embedded mesh exceeds the script's 1 MB limit; save a .ferr design or write a script that imports the mesh file instead".into());
        }
        let mut bytes = LimitedJson { bytes: Vec::new(), limit: MAX_SOURCE_BYTES.saturating_sub(out.len()) };
        serde_json::to_writer(&mut bytes, f).map_err(|e| format!("cannot export feature {}: {e}", f.name))?;
        let value: Value = serde_json::from_slice(&bytes.bytes).map_err(|e| e.to_string())?;
        source_push(&mut out, "    add_feature(#{ feature: ")?;
        rhai_literal(&value, &mut out)?;
        source_push(&mut out, " });\n")?;
    }
    for id in &doc.hidden_bodies {
        source_push(&mut out, &format!("    set_visible(#{{ id: {id}, visible: false }});\n"))?;
    }
    if doc.active_component != 0 {
        source_push(&mut out, &format!("    activate_component(#{{ id: {} }});\n", doc.active_component))?;
    }
    source_push(&mut out, "}\n")?;
    // Rhai has narrower literal/depth limits than JSON (for example u64 inputs).
    // Refuse those cases here, rather than saving a script the user cannot run.
    meta(&out).map_err(|e| format!("this timeline cannot be represented as a runnable script: {e}"))?;
    Ok(out)
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
        let shared = Arc::new(Mutex::new(Session::default()));
        let outcome = Arc::new(Mutex::new(Outcome::default()));
        for op in ["run_script", "script_meta", "batch"] {
            let err = command(&shared, &outcome, &Sandbox::default(), op, Map::new(), "test", Position::NONE).unwrap_err();
            assert!(err.to_string().contains("cannot be called from a script"));
        }
    }
}
