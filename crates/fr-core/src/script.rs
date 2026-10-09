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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rhai::{Dynamic, Engine, EvalAltResult, Map, Position, Scope};
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
    let full = dir.join(name);
    let inside = sandbox.allowed.iter().filter_map(|a| a.canonicalize().ok()).any(|a| full.starts_with(&a));
    if !inside {
        return Err(format!("the script may not {} {}: it is outside the allowed folders ({})", if for_write { "write" } else { "read" }, full.display(), sandbox.allowed.iter().map(|a| a.display().to_string()).collect::<Vec<_>>().join(", ")));
    }
    Ok(full)
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
    if req.source.len() > MAX_SOURCE_BYTES { return Err("the script is larger than 1 MB".into()); }
    let meta = meta(&req.source)?;
    let before: Vec<Id> = session.doc.features.iter().map(|f| f.id).collect();
    let mut engine = engine_with_limits();
    let ast = engine.compile(&req.source).map_err(|e| format!("the script does not parse: {e}"))?;

    let shared = Arc::new(Mutex::new(std::mem::take(session)));
    let outcome = Arc::new(Mutex::new(Outcome::default()));
    let sandbox = Arc::new(req.sandbox.clone());
    let script_dir = req.script_dir.clone();
    let document_dir = shared.lock().unwrap().path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
    let events = req.events.clone();
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
            let (shared, outcome, sandbox) = (shared.clone(), outcome.clone(), sandbox.clone());
            move |args: Map| -> Fallible { command(&shared, &outcome, &sandbox, op, args) }
        };
        let bare = {
            let (shared, outcome, sandbox) = (shared.clone(), outcome.clone(), sandbox.clone());
            move || -> Fallible { command(&shared, &outcome, &sandbox, op, Map::new()) }
        };
        engine.register_fn(name, with_args);
        engine.register_fn(name, bare);
    }
    {
        let (shared, outcome, sandbox) = (shared.clone(), outcome.clone(), sandbox.clone());
        engine.register_fn("run", move |cmd: Map| -> Fallible {
            let op = cmd.get("op").and_then(|o| o.clone().into_string().ok()).ok_or_else(|| runtime("run(map) needs an \"op\""))?;
            if op == "batch" { return Err(runtime("run a script's commands one at a time, not as a batch")); }
            command(&shared, &outcome, &sandbox, Box::leak(op.into_boxed_str()), cmd)
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
            let meta = std::fs::metadata(&full).map_err(|e| runtime(format!("could not read {}: {e}", full.display())))?;
            if meta.len() > MAX_FILE_BYTES as u64 { return Err(runtime("the file is larger than 16 MiB")); }
            std::fs::read_to_string(&full).map(Dynamic::from).map_err(|e| runtime(format!("could not read {}: {e}", full.display())))
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
            let text = std::fs::read_to_string(&full).map_err(|e| runtime(format!("could not read {}: {e}", full.display())))?;
            if text.len() > MAX_FILE_BYTES { return Err(runtime("the file is larger than 16 MiB")); }
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
        let (outcome, events) = (outcome.clone(), events.clone());
        engine.register_fn("log", move |msg: Dynamic| {
            let text = msg.to_string();
            if let Some(e) = &events { let _ = e.send(Event::Log(text.clone())); }
            let mut o = outcome.lock().unwrap();
            if o.log.len() < 10_000 { o.log.push(text); }
        });
    }
    {
        let (events, cancel) = (events.clone(), cancel.clone());
        let report = move |fraction: f64, msg: String| -> Fallible {
            if let Some(e) = &events { let _ = e.send(Event::Progress(fraction.clamp(0.0, 1.0), msg)); }
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
    let inputs = resolve_inputs(&meta, &req.inputs, &shared.lock().unwrap())?;
    let inputs_dynamic = rhai::serde::to_dynamic(&inputs).map_err(|e| e.to_string())?;
    let mut scope = Scope::new();
    engine.run_ast_with_scope(&mut scope, &ast).map_err(|e| format!("the script's top level failed: {e}"))?;
    let result = engine.call_fn::<Dynamic>(&mut scope, &ast, "run", (inputs_dynamic,));
    drop(engine);
    let mut out = Arc::try_unwrap(outcome).map(|m| m.into_inner().unwrap()).unwrap_or_default();
    *session = Arc::try_unwrap(shared).map(|m| m.into_inner().unwrap()).map_err(|_| "the script engine kept a handle on the session")?;
    out.features = session.doc.features.iter().map(|f| f.id).filter(|id| !before.contains(id)).collect();
    match result {
        Ok(v) => { out.result = rhai::serde::from_dynamic(&v).unwrap_or(Value::Null); }
        Err(e) if e.to_string().contains("cancelled") || cancel.load(Ordering::Relaxed) => { out.cancelled = true; }
        Err(e) => return Err(format!("{}: {e}", meta.name)),
    }
    Ok(out)
}

/// The host function name of a command: the command's name unless Rhai reserves it.
pub fn script_name(op: &str) -> &'static str {
    match op {
        "new" => "new_design",
        "thread" => "add_thread",
        other => Box::leak(other.to_owned().into_boxed_str()),
    }
}

/// Runs one command for a script, checking paths against the sandbox and noting exports.
fn command(shared: &Arc<Mutex<Session>>, outcome: &Arc<Mutex<Outcome>>, sandbox: &Sandbox, op: &str, args: Map) -> Fallible {
    let mut cmd = dynamic_to_json(&Dynamic::from(args))?;
    if !cmd.is_object() { cmd = json!({}); }
    cmd["op"] = json!(op);
    let path = cmd.get("path").and_then(Value::as_str).map(str::to_owned);
    let writes = matches!(op, "save" | "export_stl" | "export_step" | "get_viewport_screenshot");
    if let Some(p) = &path && matches!(op, "save" | "export_stl" | "export_step" | "import_stl" | "import_mesh" | "mesh_from_image" | "open") {
        let full = allowed(sandbox, Path::new(p), writes).map_err(runtime)?;
        cmd["path"] = json!(full.display().to_string());
        if writes { outcome.lock().unwrap().exports.push(full); }
    }
    let mut s = shared.lock().unwrap();
    let out = crate::api::execute(&mut s, &cmd, None).map_err(|e| runtime(format!("{op}: {e}")))?;
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

/// A script that rebuilds the document: every parameter becomes an input and
/// every feature a command, so the on-ramp to a generator is a saved design.
pub fn export_timeline(session: &Session) -> R<String> {
    let doc = &session.doc;
    let mut out = String::new();
    out.push_str("// Exported by Ferrender: rebuilds the document it was exported from.\n");
    out.push_str("const META = #{\n    name: \"Exported design\",\n    description: \"Rebuilds the exported timeline; parameters are inputs.\",\n    inputs: [\n");
    for p in &doc.params {
        let kind = if p.expr.contains("deg") { "angle" } else { "length" };
        out.push_str(&format!("        #{{ name: {:?}, kind: {:?}, initial: {:?} }},\n", p.name, kind, p.expr));
    }
    out.push_str("    ],\n};\n\nfn run(inputs) {\n");
    out.push_str(&format!("    new_design(#{{ units: {:?} }});\n", doc.units.name()));
    for p in &doc.params {
        out.push_str(&format!("    set_parameter(#{{ name: {:?}, expr: inputs.expr.{} }});\n", p.name, p.name));
    }
    // Features go in as their JSON, through edit_feature-free replay: each feature is added as it was saved.
    for f in &doc.features {
        let v = serde_json::to_value(f).map_err(|e| e.to_string())?;
        out.push_str(&format!("    add_feature(#{{ feature: {} }});\n", rhai_literal(&v)));
    }
    out.push_str("}\n");
    Ok(out)
}

/// A JSON value as Rhai source.
fn rhai_literal(v: &Value) -> String {
    match v {
        Value::Null => "()".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Array(a) => format!("[{}]", a.iter().map(rhai_literal).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!("#{{{}}}", o.iter().map(|(k, v)| format!("{k:?}: {}", rhai_literal(v))).collect::<Vec<_>>().join(", ")),
    }
}

#[allow(dead_code)]
fn _keep(_: &dyn Fn(&Dynamic) -> Fallible) {}
#[allow(dead_code)]
fn _keep2() { let _ = to_value; }
