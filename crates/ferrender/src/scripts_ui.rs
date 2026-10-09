//! The Scripts menu: user scripts, the open document's scripts, the bundled
//! samples, an inputs dialog made from each script's META, runs on a worker
//! thread with progress and Cancel, the `ScriptRun` chip's actions, Export
//! Timeline as Script and the script log.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

use egui::{Context, RichText, Ui};
use fr_core::script::{Event, Input, Meta, Outcome, Request, SAMPLES};
use fr_core::{FeatureKind, Id, Kind, Session};
use serde_json::{Value, json};

use crate::app::{Action, App, Dialog};
use crate::panels::{confirm, dialog_window, value_row};
use crate::theme::Palette;

/// A script the menu offers.
#[derive(Clone, Debug)]
pub struct Entry {
    /// A file, or `None` for a bundled sample.
    pub path: Option<PathBuf>,
    pub file_name: String,
    pub source: String,
    pub meta: Result<Meta, String>,
    pub sample: bool,
}

impl Entry {
    pub fn title(&self) -> String {
        match &self.meta {
            Ok(m) => m.name.clone(),
            Err(_) => self.file_name.clone(),
        }
    }
}

/// One input as the dialog edits it.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub input: Input,
    /// Text for lengths, angles, numbers, text, folders, files; "true"/"false" for bools; the choice text.
    pub text: String,
    /// An id for body, face (as "body,x,y,z") and sketch picks.
    pub pick: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptDlg {
    pub name: String,
    pub source: String,
    pub script_dir: Option<PathBuf>,
    pub fields: Vec<Field>,
    /// Editing the inputs of an existing run: that chip is replaced.
    pub rerun: Option<Id>,
    pub description: String,
}

/// A `confirm` or `ask` from the running script, waiting for the person at the screen.
pub struct Question {
    pub text: String,
    /// `None` for confirm (yes or no); the offered answer for ask.
    pub default: Option<String>,
    pub reply: Sender<Option<String>>,
    /// What is typed so far.
    pub answer: String,
}

/// How long Cancel waits for a modeling call before offering to stop waiting.
pub const PATIENCE: std::time::Duration = std::time::Duration::from_secs(2);

/// A run in progress on its worker thread.
pub struct Running {
    pub name: String,
    pub events: Receiver<Event>,
    pub done: Receiver<Result<(Session, Outcome), String>>,
    pub questions: Receiver<Question>,
    pub pending: Option<Question>,
    pub cancel: Arc<AtomicBool>,
    /// When Cancel was pressed; the script stops at its next operation, which a
    /// modeling call under way delays.
    pub cancel_at: Option<std::time::Instant>,
    pub progress: f32,
    pub message: String,
    pub log: Vec<String>,
    pub started: std::time::Instant,
    /// The chip this run replaces, if it is a re-run, and where the chip goes.
    pub rerun: Option<Id>,
    pub insert_at: usize,
    pub owner: Id,
    pub base_doc: fr_core::Document,
    pub base_path: Option<PathBuf>,
    pub base_rev: u64,
    /// Features after the re-run's position, put back after it.
    pub tail: Vec<fr_core::Feature>,
    pub script_name: String,
    pub source: String,
    pub inputs: Value,
}

#[derive(Default)]
pub struct Scripts {
    pub entries: Vec<Entry>,
    pub loaded: bool,
    pub running: Option<Running>,
    pub log: Vec<String>,
    pub show_log: bool,
    pub last_error: Option<String>,
}

/// The user's scripts folder beside config.toml.
pub fn user_dir() -> PathBuf {
    crate::config::Config::path().parent().map(|p| p.join("scripts")).unwrap_or_else(|| PathBuf::from("scripts"))
}

fn read_dir(dir: &PathBuf) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "rhai")).collect();
    paths.sort();
    paths.into_iter().filter_map(|p| {
        let source = fr_core::script::read_source(&p).ok()?;
        let meta = fr_core::script::meta(&source);
        Some(Entry { file_name: p.file_name()?.to_string_lossy().into_owned(), path: Some(p), source, meta, sample: false })
    }).collect()
}

impl Scripts {
    /// Rereads the user folder, the document's folder and the samples.
    pub fn reload(&mut self, document: Option<&std::path::Path>) {
        self.entries = read_dir(&user_dir());
        if let Some(dir) = document.and_then(|p| p.parent()).map(|d| d.join("scripts")) {
            for e in read_dir(&dir) {
                if !self.entries.iter().any(|x| x.path == e.path) { self.entries.push(e); }
            }
        }
        for (name, source) in SAMPLES {
            self.entries.push(Entry { path: None, file_name: (*name).to_owned(), source: (*source).to_owned(), meta: fr_core::script::meta(source), sample: true });
        }
        self.loaded = true;
    }

    pub fn busy(&self) -> bool {
        self.running.is_some()
    }
}

fn default_text(input: &Input, unit: fr_core::Unit) -> String {
    match &input.initial {
        Value::Null => match input.kind.as_str() { "bool" => "false".into(), "number" | "integer" => "0".into(), "length" => format!("0 {}", unit.name()), "angle" => "0 deg".into(), _ => input.choices.first().cloned().unwrap_or_default() },
        Value::String(s) => s.clone(),
        Value::Number(n) => match input.kind.as_str() { "length" => format!("{n} {}", unit.name()), "angle" => format!("{n} deg"), _ => n.to_string() },
        other => other.to_string(),
    }
}

impl App {
    fn scripts_loaded(&mut self) {
        if !self.scripts.loaded {
            let path = self.session.path.clone();
            self.scripts.reload(path.as_deref());
        }
    }

    /// Opens the inputs dialog for an entry, with the last values typed for that script.
    pub fn open_script(&mut self, index: usize) {
        self.scripts_loaded();
        let Some(entry) = self.scripts.entries.get(index).cloned() else { return };
        let meta = match &entry.meta {
            Ok(m) => m.clone(),
            Err(e) => { self.toast(format!("{}: {e}", entry.file_name)); return; }
        };
        let remembered: Value = self.config.scripts.last_inputs.get(&meta.name).and_then(|t| serde_json::from_str(t).ok()).unwrap_or(json!({}));
        self.open_script_dialog(&meta, &entry.source, entry.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())), &remembered, None);
    }

    fn open_script_dialog(&mut self, meta: &Meta, source: &str, script_dir: Option<PathBuf>, values: &Value, rerun: Option<Id>) {
        self.finish_sketch();
        let unit = self.doc().units;
        let selected_body = self.sel_body.or_else(|| self.sel_face.as_ref().map(|f| f.body));
        let selected_sketch = self.doc().sketches().find(|(f, _)| Some(f.id) == self.sel_sketch()).map(|(f, _)| f.id);
        let fields = meta.inputs.iter().map(|input| {
            let given = values.get(&input.name).cloned().filter(|v| !v.is_null());
            let (text, pick) = match input.kind.as_str() {
                "body" => (String::new(), given.or(selected_body.map(|b| json!(b)))),
                "sketch" => (String::new(), given.or(selected_sketch.map(|s| json!(s)))),
                "face" => (String::new(), given.or(self.sel_face.as_ref().map(|f| json!({"body": f.body, "point": (f.at / unit.mm()).to_array()})))),
                _ => (given.map(|v| match v { Value::String(s) => s, other => other.to_string() }).unwrap_or_else(|| default_text(input, unit)), None),
            };
            Field { input: input.clone(), text, pick }
        }).collect();
        self.dialog = Dialog::Script(ScriptDlg { name: meta.name.clone(), source: source.to_owned(), script_dir, fields, rerun, description: meta.description.clone() });
    }

    /// The inputs a dialog holds, as the script receives them.
    fn script_inputs(fields: &[Field]) -> Value {
        let mut m = serde_json::Map::new();
        for f in fields {
            let v = match f.input.kind.as_str() {
                "body" | "sketch" | "face" => f.pick.clone().unwrap_or(Value::Null),
                "bool" => json!(f.text == "true"),
                "number" => f.text.trim().parse::<f64>().map(Value::from).unwrap_or(json!(f.text)),
                "integer" => f.text.trim().parse::<i64>().map(Value::from).unwrap_or(json!(f.text)),
                _ => json!(f.text),
            };
            m.insert(f.input.name.clone(), v);
        }
        Value::Object(m)
    }

    /// Starts the run on a worker thread; the result is committed when it arrives.
    pub fn start_script(&mut self, dlg: ScriptDlg) {
        if self.session.read_only { self.toast("Scripts cannot run on this read-only design; update Ferrender first."); return; }
        if self.scripts.busy() { self.toast("A script is already running."); return; }
        let inputs = Self::script_inputs(&dlg.fields);
        self.config.scripts.last_inputs.insert(dlg.name.clone(), inputs.to_string());
        let _ = self.config.save();
        // A re-run replaces the chip and everything it made, and starts where the chip was: the
        // features after it are set aside and put back once the run has appended its own.
        let base_doc = self.doc().clone();
        let base_path = self.session.path.clone();
        let base_rev = self.session.rev;
        let (insert_at, owner, tail, mut session) = match dlg.rerun {
            Some(chip) => {
                let doc = self.doc();
                let at = doc.features.iter().position(|f| f.id == chip).unwrap_or(doc.features.len());
                let previous: Vec<_> = doc.features.iter().filter(|f| f.made_by == Some(chip)).cloned().collect();
                if previous.iter().any(|f| f.script_key.is_none())
                    && doc.features.iter().skip(at + 1).any(|f| f.made_by != Some(chip)) {
                    self.toast("This earlier script run has no stable output identities. Re-run it before adding dependent features, or detach it and edit its existing features; Ferrender will not guess which new output your later features meant.");
                    return;
                }
                let owner = doc.feature(chip).map_or(doc.active_component, |f| f.owner);
                let mut s = self.session.fork();
                s.doc.features.retain(|f| f.id != chip && f.made_by != Some(chip));
                let tail = s.doc.features.split_off(at.min(s.doc.features.len()));
                s.doc.reuse_feature_ids(&previous);
                s.doc.active_component = owner;
                s.doc.rollback = None;
                s.rebuild();
                (at, owner, tail, s)
            }
            None => {
                let s = self.session.fork();
                let at = s.doc.active();
                (at, s.doc.active_component, Vec::new(), s)
            }
        };
        let mut req = Request::new(dlg.source.clone());
        req.script_dir = dlg.script_dir.clone();
        req.inputs = inputs.clone();
        req.selection = json!({"bodies": self.sel_body.into_iter().collect::<Vec<_>>(), "sketches": self.sel_sketch().into_iter().collect::<Vec<_>>()});
        req.sandbox.allowed = self.config.scripts.allowed_dirs.clone();
        if let Some(d) = &dlg.script_dir { req.sandbox.allowed.push(d.clone()); }
        if let Some(d) = self.session.path.as_ref().and_then(|p| p.parent()) { req.sandbox.allowed.push(d.to_path_buf()); }
        req.sandbox.allowed.push(user_dir());
        let (etx, erx) = channel();
        req.events = Some(etx);
        let (dtx, drx): (Sender<Result<(Session, Outcome), String>>, _) = channel();
        let cancel = req.cancel.clone();
        let ctx = self.ctx.clone();
        // confirm and ask block the worker until the progress window has an answer,
        // or until Cancel, which declines.
        let (qtx, qrx) = channel::<Question>();
        {
            let (ctx, cancel) = (ctx.clone(), cancel.clone());
            req.ask = Some(Arc::new(move |text: &str, default: Option<&str>| {
                let (rtx, rrx) = channel();
                if qtx.send(Question { text: text.to_owned(), default: default.map(str::to_owned), reply: rtx, answer: default.unwrap_or_default().to_owned() }).is_err() { return None; }
                ctx.request_repaint();
                loop {
                    match rrx.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(answer) => return answer,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => if cancel.load(Ordering::Relaxed) { return None; },
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
                    }
                }
            }));
        }
        std::thread::Builder::new().name("ferrender-script".into()).spawn(move || {
            let result = fr_core::script::run(&mut session, &req).map(|o| (session, o));
            let _ = dtx.send(result);
            ctx.request_repaint();
        }).ok();
        self.scripts.running = Some(Running { name: dlg.name.clone(), events: erx, done: drx, questions: qrx, pending: None, cancel, cancel_at: None, progress: 0.0, message: "starting".into(), log: Vec::new(), started: std::time::Instant::now(), rerun: dlg.rerun, insert_at, owner, base_doc, base_path, base_rev, tail, script_name: dlg.name, source: dlg.source, inputs });
        self.dialog = Dialog::None;
    }

    /// Polls the worker: progress, log lines, and the finished run.
    pub fn poll_script(&mut self) {
        let Some(run) = &mut self.scripts.running else { return };
        for e in run.events.try_iter() {
            match e {
                Event::Log(l) => run.log.push(l),
                Event::Progress(f, m) => { run.progress = f as f32; if !m.is_empty() { run.message = m; } }
            }
        }
        // The worker blocks on one question at a time.
        for q in run.questions.try_iter() { run.pending = Some(q); }
        let result = match run.done.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => { self.ctx.request_repaint_after(std::time::Duration::from_millis(100)); return; }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the script worker stopped unexpectedly; the document was left unchanged".into()),
        };
        let run = self.scripts.running.take().unwrap();
        self.scripts.log.extend(run.log.iter().cloned());
        match result {
            Ok((worked, outcome)) => {
                self.scripts.log.extend(outcome.log.iter().cloned());
                if outcome.cancelled { self.toast(format!("{} cancelled.", run.name)); return; }
                if self.session.rev != run.base_rev || self.session.path != run.base_path || self.doc() != &run.base_doc {
                    self.toast(format!("{} finished, but the document changed while it ran. Its modeling changes were not applied; run it again to use the current document.", run.name));
                    return;
                }
                let made: Vec<Id> = outcome.features.clone();
                let script_name = run.script_name.clone();
                let (source, inputs) = (run.source.clone(), run.inputs.clone());
                let new_doc = worked.doc.clone();
                let insert_at = run.insert_at;
                let tail = run.tail.clone();
                let r = self.session.edit(|d| {
                    *d = new_doc;
                    d.finish_feature_id_reuse();
                    d.rollback = None;
                    for f in &tail { d.next_id = d.next_id.max(f.id + 1); }
                    d.features.extend(tail.iter().cloned());
                    if made.is_empty() { return Ok(()); }
                    // The chip goes where the run started; what it made points back at it.
                    let chip = run.rerun.unwrap_or(d.next_id);
                    d.next_id = d.next_id.max(chip + 1);
                    let hash = crc32fast::hash(source.as_bytes());
                    let at = d.features.iter().position(|f| made.contains(&f.id)).unwrap_or(insert_at.min(d.features.len()));
                    d.features.insert(at, fr_core::Feature { id: chip, name: script_name.clone(), suppressed: false, owner: run.owner, made_by: None, script_key: None, kind: FeatureKind::ScriptRun(fr_core::doc::ScriptRun { script_name: script_name.clone(), source: source.clone(), source_hash: hash, inputs: inputs.clone() }) });
                    for f in &mut d.features { if made.contains(&f.id) { f.made_by = Some(chip); } }
                    fr_core::validation::document(d)?;
                    Ok(())
                });
                match r {
                    Ok(()) => {
                        let errors = self.session.built.errors.len();
                        let what = if outcome.exports.is_empty() { String::new() } else { format!(", wrote {} file{}", outcome.exports.len(), if outcome.exports.len() == 1 { "" } else { "s" }) };
                        self.toast(format!("{} finished in {:.1} s: {} feature{}{}{}", run.name, run.started.elapsed().as_secs_f64(), made.len(), if made.len() == 1 { "" } else { "s" }, what, if errors > 0 { format!("; {errors} timeline error{}", if errors == 1 { "" } else { "s" }) } else { String::new() }));
                        self.fit_pending = made.len() > 0 && self.session.built.bodies.len() <= 1;
                    }
                    Err(e) => self.toast(e),
                }
                self.refresh();
            }
            Err(e) => {
                self.scripts.last_error = Some(e.clone());
                self.scripts.log.push(format!("error: {e}"));
                self.scripts.show_log = true;
                self.toast(format!("{} failed: {e}", run.name));
            }
        }
    }

    /// Cancel: the script stops at its next operation; a pending question is declined.
    /// A modeling call already under way runs to its end first.
    pub fn cancel_script(&mut self) {
        if let Some(run) = &mut self.scripts.running {
            run.cancel.store(true, Ordering::Relaxed);
            run.cancel_at.get_or_insert_with(std::time::Instant::now);
            if let Some(q) = run.pending.take() { let _ = q.reply.send(None); }
        }
    }

    /// Edit inputs and run again, or run again as it was.
    pub fn rerun_script(&mut self, chip: Id, edit_inputs: bool) {
        let Some(FeatureKind::ScriptRun(r)) = self.doc().feature(chip).map(|f| f.kind.clone()) else { return };
        let meta = match fr_core::script::meta(&r.source) { Ok(m) => m, Err(e) => { self.toast(e); return; } };
        // Prefer the current file of the same name, so edits to the script are picked up.
        self.scripts_loaded();
        let current = self.scripts.entries.iter().find(|e| !e.sample && e.meta.as_ref().is_ok_and(|m| m.name == meta.name)).cloned();
        let (source, dir) = match current { Some(e) => (e.source.clone(), e.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()))), None => (r.source.clone(), None) };
        let meta = match fr_core::script::meta(&source) { Ok(m) => m, Err(e) => { self.toast(e); return; } };
        self.open_script_dialog(&meta, &source, dir, &r.inputs, Some(chip));
        if !edit_inputs && let Dialog::Script(d) = self.dialog.clone() {
            self.start_script(d);
        }
    }

    /// Turns the chip's features into ordinary ones and removes the chip.
    pub fn detach_script(&mut self, chip: Id) {
        let _ = self.session.edit(|d| {
            if let Some(at) = d.rollback {
                d.rollback = Some(at - d.features.iter().take(at).filter(|f| f.id == chip).count());
            }
            d.features.retain(|f| f.id != chip);
            for f in &mut d.features { if f.made_by == Some(chip) { f.made_by = None; } }
            Ok(())
        });
        self.refresh();
    }

    /// Deletes the chip and everything it made.
    pub fn delete_script_run(&mut self, chip: Id) {
        let _ = self.session.edit(|d| {
            d.delete_feature(chip)?;
            Ok(())
        });
        self.refresh();
    }

    pub fn export_timeline_script(&mut self) {
        if self.session.read_only { self.toast("A read-only cached design has no editable timeline to export."); return; }
        let Some(path) = rfd::FileDialog::new().add_filter("Rhai script", &["rhai"]).set_file_name(format!("{}.rhai", self.doc_name())).save_file() else { return };
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "design".into());
        match self.write_exported_script(&path, &stem) {
            Ok(export) => {
                for n in &export.notes { self.scripts.log.push(format!("export {}: {n}", path.display())); }
                let files = if export.files.is_empty() { String::new() } else { format!(" and {} file{} beside it", export.files.len(), if export.files.len() == 1 { "" } else { "s" }) };
                let notes = if export.notes.is_empty() { String::new() } else { format!("; {} note{} in the script log", export.notes.len(), if export.notes.len() == 1 { "" } else { "s" }) };
                self.toast(format!("Wrote {}{files}{notes}.", path.display()));
                self.scripts.loaded = false;
            }
            Err(e) => self.toast(e),
        }
    }

    /// Exports the timeline as a script at `path`, with its sidecar files beside it.
    pub fn write_exported_script(&self, path: &std::path::Path, stem: &str) -> Result<fr_core::script::Export, String> {
        let export = fr_core::script::export_timeline_named(&self.session, stem)?;
        fr_core::script::write_export(&export, path)?;
        Ok(export)
    }

    pub fn new_script(&mut self) {
        let dir = user_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) { self.toast(format!("Could not create {}: {e}", dir.display())); return; }
        let mut n = 1;
        let path = loop { let p = dir.join(format!("script-{n}.rhai")); if !p.exists() { break p; } n += 1; };
        let template = "// A Ferrender script. Inputs are declared in META and arrive in run(inputs).\nconst META = #{\n    name: \"My script\",\n    description: \"What it does.\",\n    inputs: [\n        #{ name: \"size\", kind: \"length\", initial: \"10 mm\" },\n    ],\n};\n\nfn run(inputs) {\n    primitive(#{ type: \"box\", width: inputs.expr.size, depth: inputs.expr.size, height: inputs.expr.size });\n    log(\"made a box of \" + inputs.size + \" mm\");\n}\n";
        match std::fs::write(&path, template) {
            Ok(()) => { self.scripts.loaded = false; let _ = open::that(&path); self.toast(format!("Created {}.", path.display())); }
            Err(e) => self.toast(format!("Could not write {}: {e}", path.display())),
        }
    }

    pub fn copy_sample_to_scripts(&mut self, index: usize) {
        let Some(entry) = self.scripts.entries.get(index).cloned() else { return };
        let dir = user_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) { self.toast(format!("Could not create {}: {e}", dir.display())); return; }
        let path = dir.join(&entry.file_name);
        if path.exists() { self.toast(format!("{} already exists in your scripts.", entry.file_name)); return; }
        match std::fs::write(&path, &entry.source) {
            Ok(()) => { self.scripts.loaded = false; self.toast(format!("Copied to {}.", path.display())); }
            Err(e) => self.toast(format!("Could not write {}: {e}", path.display())),
        }
    }

    fn sel_sketch(&self) -> Option<Id> {
        self.sel_feature.filter(|id| self.doc().sketch(*id).is_some())
    }
}

/// The Scripts menu.
pub fn menu(app: &mut App, ui: &mut Ui) {
    app.scripts_loaded();
    let busy = app.scripts.busy();
    let entries: Vec<(usize, String, bool, bool)> = app.scripts.entries.iter().enumerate().map(|(i, e)| (i, e.title(), e.sample, e.meta.is_err())).collect();
    let users: Vec<_> = entries.iter().filter(|e| !e.2).collect();
    if users.is_empty() {
        ui.label(RichText::new("No scripts in your scripts folder yet.").weak());
    }
    for (i, title, _, broken) in users {
        let label = if *broken { format!("{title} (does not parse)") } else { title.clone() };
        if ui.add_enabled(!busy, egui::Button::new(label)).clicked() { app.open_script(*i); ui.close(); }
    }
    ui.separator();
    ui.menu_button("Samples", |ui| {
        for (i, title, sample, _) in entries.iter().filter(|e| e.2) {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && *sample, egui::Button::new(title)).clicked() { app.open_script(*i); ui.close(); }
                if ui.small_button("Copy to My Scripts").on_hover_text("Puts an editable copy in your scripts folder").clicked() { app.copy_sample_to_scripts(*i); ui.close(); }
            });
        }
    });
    ui.separator();
    if ui.button("New Script").clicked() { app.new_script(); ui.close(); }
    if ui.button("Open Scripts Folder").clicked() { let d = user_dir(); let _ = std::fs::create_dir_all(&d); let _ = open::that(&d); ui.close(); }
    if ui.button("Reload").clicked() { app.scripts.loaded = false; ui.close(); }
    ui.separator();
    if ui.add_enabled(!app.doc().features.is_empty(), egui::Button::new("Export Timeline as Script\u{2026}")).clicked() {
        let ctx = ui.ctx().clone();
        app.run(&ctx, Action::ExportTimelineScript);
        ui.close();
    }
    if ui.button("Show Script Log").clicked() {
        let ctx = ui.ctx().clone();
        app.run(&ctx, Action::ScriptLog);
        ui.close();
    }
}

/// The inputs dialog.
pub fn dialog(app: &mut App, ctx: &Context, mut d: ScriptDlg) {
    let colors = Palette::from_ctx(ctx);
    dialog_window(app, &d.name.clone()).show(ctx, |ui| {
        if !d.description.is_empty() { ui.label(RichText::new(&d.description).color(colors.muted)); }
        egui::Grid::new("script-inputs").num_columns(3).show(ui, |ui| {
            for (k, f) in d.fields.iter_mut().enumerate() {
                let label = f.input.label.clone().unwrap_or_else(|| f.input.name.clone());
                match f.input.kind.as_str() {
                    "length" => value_row(app, ui, &label, &mut f.text, Kind::Length),
                    "angle" => value_row(app, ui, &label, &mut f.text, Kind::Angle),
                    "bool" => { ui.label(&label); let mut b = f.text == "true"; ui.checkbox(&mut b, ""); f.text = b.to_string(); ui.end_row(); }
                    "choice" => {
                        ui.label(&label);
                        egui::ComboBox::from_id_salt(("choice", k)).selected_text(f.text.clone()).show_ui(ui, |ui| { for c in &f.input.choices { ui.selectable_value(&mut f.text, c.clone(), c); } });
                        ui.end_row();
                    }
                    "folder" | "file" => {
                        ui.label(&label);
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(220.0));
                            if ui.button("\u{2026}").clicked() {
                                let picked = if f.input.kind == "folder" { rfd::FileDialog::new().pick_folder() } else { rfd::FileDialog::new().pick_file() };
                                if let Some(p) = picked { f.text = p.display().to_string(); }
                            }
                        });
                        ui.end_row();
                    }
                    "body" | "sketch" | "face" => {
                        ui.label(&label);
                        let shown = match &f.pick { Some(Value::Number(n)) => app.doc().feature(n.as_u64().unwrap_or(0) as Id).map(|x| x.name.clone()).or_else(|| app.session.built.body(n.as_u64().unwrap_or(0) as Id).map(|b| b.name.clone())).unwrap_or_else(|| n.to_string()), Some(v) => v.to_string(), None => "none selected".into() };
                        ui.label(RichText::new(shown).color(if f.pick.is_some() { colors.accent } else { colors.muted }));
                        ui.end_row();
                    }
                    _ => { ui.label(&label); ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(220.0)); ui.end_row(); }
                }
                if let Some(h) = &f.input.help { ui.label(""); ui.label(RichText::new(h).small().color(colors.muted)); ui.end_row(); }
            }
        });
        if d.rerun.is_some() { ui.label(RichText::new("Re-running replaces what the earlier run made.").color(colors.muted)); }
        app.dialog = Dialog::Script(d.clone());
        confirm(app, ui, if d.rerun.is_some() { "Run again" } else { "Run" });
    });
}

/// The progress window while a script runs, and the log window.
pub fn windows(app: &mut App, ctx: &Context) {
    app.poll_script();
    let colors = Palette::from_ctx(ctx);
    let (mut cancel, mut abandon) = (false, false);
    if let Some(run) = &mut app.scripts.running {
        let (name, progress, message, elapsed) = (run.name.clone(), run.progress, run.message.clone(), run.started.elapsed().as_secs_f64());
        let last = run.log.last().cloned();
        egui::Window::new(format!("Running {name}")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.add(egui::ProgressBar::new(progress).text(format!("{message} ({elapsed:.0} s)")).animate(progress <= 0.0));
            if let Some(l) = last { ui.label(RichText::new(l).weak()); }
            // A question from the script: the worker waits for the answer.
            let mut answered: Option<Option<String>> = None;
            if let Some(q) = &mut run.pending {
                ui.separator();
                ui.label(&q.text);
                match &q.default {
                    None => ui.horizontal(|ui| {
                        if ui.button("Yes").clicked() { answered = Some(Some(String::new())); }
                        if ui.button("No").clicked() { answered = Some(None); }
                    }),
                    Some(_) => ui.horizontal(|ui| {
                        let edit = ui.add(egui::TextEdit::singleline(&mut q.answer).desired_width(220.0));
                        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { answered = Some(Some(q.answer.clone())); }
                        if ui.button("OK").clicked() { answered = Some(Some(q.answer.clone())); }
                        if ui.button("Decline").on_hover_text("The script's ask() fails; what it does then is up to it").clicked() { answered = Some(None); }
                    }),
                };
                ui.separator();
            }
            if let Some(answer) = answered && let Some(q) = run.pending.take() {
                let _ = q.reply.send(answer);
            }
            match run.cancel_at {
                None => {
                    if ui.button("Cancel").clicked() { cancel = true; }
                }
                Some(at) if at.elapsed() < PATIENCE => { ui.label(RichText::new("Cancelling\u{2026}").color(colors.muted)); }
                Some(_) => {
                    ui.label(RichText::new("The script stops at its next step, but the modeling call it is in cannot be interrupted.").color(colors.muted));
                    if ui.button("Stop waiting").on_hover_text("Go back to the design now; the run's result is discarded when that call ends").clicked() { abandon = true; }
                }
            }
        });
    }
    if cancel { app.cancel_script(); }
    if abandon && let Some(run) = app.scripts.running.take() {
        app.scripts.log.extend(run.log.iter().cloned());
        app.scripts.log.push(format!("{} was cancelled while in a modeling call; its result is discarded when the call ends.", run.name));
        app.toast(format!("{} cancelled. It finishes its current modeling call in the background and its result is discarded.", run.name));
    }
    if app.scripts.show_log {
        let mut open = true;
        egui::Window::new("Script log").open(&mut open).default_width(520.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(360.0).stick_to_bottom(true).show(ui, |ui| {
                if app.scripts.log.is_empty() { ui.label(RichText::new("Nothing logged yet.").weak()); }
                for line in &app.scripts.log { ui.label(line); }
            });
            ui.horizontal(|ui| {
                if ui.button("Clear").clicked() { app.scripts.log.clear(); app.scripts.last_error = None; }
                if ui.button("Copy").clicked() { ui.ctx().copy_text(app.scripts.log.join("\n")); }
            });
        });
        if !open { app.scripts.show_log = false; }
    }
}

/// The chip's own menu items, shown before the usual ones.
pub fn chip_menu(app: &mut App, ui: &mut Ui, id: Id) -> bool {
    let Some(FeatureKind::ScriptRun(_)) = app.doc().feature(id).map(|f| &f.kind) else { return false };
    let busy = app.scripts.busy();
    if ui.add_enabled(!busy, egui::Button::new("Edit Inputs and Re-run")).clicked() { app.rerun_script(id, true); ui.close(); }
    if ui.add_enabled(!busy, egui::Button::new("Re-run")).clicked() { app.rerun_script(id, false); ui.close(); }
    if ui.button("Detach").on_hover_text("Keep what the run made as ordinary features and drop the chip").clicked() { app.detach_script(id); ui.close(); }
    true
}

pub fn action(app: &mut App, a: &Action) -> bool {
    match a {
        Action::ScriptLog => { app.scripts.show_log = true; true }
        Action::ExportTimelineScript => { app.export_timeline_script(); true }
        _ => false,
    }
}
