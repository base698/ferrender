mod ai;
mod app;
mod command_search;
mod bridge;
mod build_info;
mod config;
mod construction;
mod primitives;
mod body_ops_ui;
mod model_drag;
mod sketch_capture;
mod gizmo;
mod construction_view;
mod components_ui;
mod gpu;
mod mcp;
mod native_theme;
mod open_requests;
#[cfg(target_os = "macos")]
mod macos_open;
mod panels;
mod recovery;
mod scripts_ui;
mod recent;
mod reference;
mod reference_drag;
mod theme;
mod timeline;
mod view;

#[cfg(test)]
mod uitest;
#[cfg(test)]
mod demo;

use std::path::PathBuf;

const HELP: &str = "usage: ferrender [FILE]
       ferrender mcp [--headless] [--port N]
       ferrender run SCRIPT.rhai [DESIGN.ferr] [--input NAME=VALUE]... [--allow DIR]... [--yes] [--timeout SECONDS] [--save [OUT.ferr]] [--strict]
       ferrender check DESIGN.ferr... [--rebuild] [--save]
       ferrender --version

Opens a design (.ferr) or imports a mesh (.stl, .obj, .3mf).

`ferrender run` runs a script headless against a design (or a new one), printing
its log; exit code 0 when it finishes, 1 when it fails, 2 for a usage error and
3 (with --strict) when the design has timeline errors afterwards. Scripts may
read and write files in their own folder, the design's folder and --allow folders;
files they write appear only when the run succeeds. --timeout fails the run at its
next operation after that many seconds. --yes answers confirm/ask without a prompt.
`ferrender check` opens designs and lists their timeline errors (exit 1 if any);
--rebuild compares the saved geometry cache with a fresh rebuild; --save writes
each design back (which also records face tags for files from before 0.4).

`ferrender mcp` runs a Model Context Protocol server on stdin/stdout for AI
clients. It drives the running Ferrender window if there is one, and
otherwise (or with --headless) works on a document of its own. The selected
backend stays fixed until the MCP client restarts. On macOS/Linux, --port
selects a private local socket channel; no TCP port is opened.";

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", build_info::summary());
        return Ok(());
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "run" || a == "check") {
        std::process::exit(cli(&args));
    }
    if args.first().is_some_and(|a| a == "mcp") {
        let port = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).unwrap_or_else(|| config::Config::load().0.bridge.port);
        mcp::serve(port, args.iter().any(|a| a == "--headless"));
        return Ok(());
    }
    // Older macOS versions pass a process serial number when launched from Finder.
    let file = args.iter().find(|a| !a.starts_with("-psn_") && !a.starts_with('-')).map(PathBuf::from);
    // A crash should not take the last change with it: let the recovery copy finish.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report(info);
        recovery::flush();
        eprintln!("Ferrender crashed. Unsaved work is offered for recovery the next time it starts.");
    }));
    let requests = open_requests::OpenRequests::default();
    #[cfg(target_os = "macos")]
    let _document_events = macos_open::install(requests.clone());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Ferrender").with_inner_size([1440.0, 900.0]).with_min_inner_size([900.0, 560.0]).with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Ferrender", options, Box::new(move |cc| {
        let mut app = app::App::new(cc, file);
        requests.attach(&cc.egui_ctx);
        app.open_requests = requests;
        Ok(Box::new(app))
    }))
}

/// `ferrender run` and `ferrender check`; returns the exit code.
fn cli(args: &[String]) -> i32 {
    let flag = |name: &str| args.iter().any(|a| a == name);
    let values = |name: &str| -> Vec<String> { args.windows(2).filter(|w| w[0] == name).map(|w| w[1].clone()).collect() };
    let positional: Vec<&String> = {
        let mut out = Vec::new();
        let mut skip = false;
        for (i, a) in args.iter().enumerate().skip(1) {
            if skip { skip = false; continue; }
            if a.starts_with("--") {
                if matches!(a.as_str(), "--input" | "--allow" | "--timeout") { skip = true; }
                if a == "--save" && args.get(i + 1).is_some_and(|n| n.ends_with(".ferr")) { skip = true; }
                continue;
            }
            out.push(a);
        }
        out
    };
    if args[0] == "check" {
        if positional.is_empty() { eprintln!("usage: ferrender check DESIGN.ferr... [--rebuild] [--save]"); return 2; }
        let mut failed = 0;
        for path in &positional {
            let path = PathBuf::from(path);
            let t = std::time::Instant::now();
            let mut s = match fr_core::Session::open(&path) { Ok(s) => s, Err(e) => { eprintln!("{}: {e}", path.display()); failed += 1; continue; } };
            let how = if s.read_only { "UNVERIFIED read-only cached preview" } else if s.from_cache { "from locally authenticated cache" } else { "rebuilt" };
            println!("{}: {} features, {} bodies, {how} in {} ms", path.display(), s.doc.features.len(), s.built.bodies.len(), t.elapsed().as_millis());
            for (id, e) in &s.built.errors {
                let name = s.doc.feature(*id).map(|f| f.name.clone()).unwrap_or_default();
                println!("  error in {id} {name}: {e}");
            }
            if !s.built.errors.is_empty() { failed += 1; }
            if s.read_only {
                eprintln!("  unverified preview: this Ferrender cannot validate the newer timeline");
                failed += 1;
            }
            if flag("--rebuild") && s.read_only {
                eprintln!("  cannot verify a rebuild of this newer read-only design; update Ferrender first");
                failed += 1;
            } else if flag("--rebuild") && s.from_cache {
                let cached: Vec<(u32, f64)> = s.built.bodies.iter().map(|b| (b.id, if b.is_exact() { b.solids.iter().map(|l| l.volume()).sum() } else { b.mesh.volume() })).collect();
                s.rebuild();
                let fresh: Vec<(u32, f64)> = s.built.bodies.iter().map(|b| (b.id, if b.is_exact() { b.solids.iter().map(|l| l.volume()).sum() } else { b.mesh.volume() })).collect();
                let same = cached.len() == fresh.len() && cached.iter().zip(&fresh).all(|(a, b)| a.0 == b.0 && (a.1 - b.1).abs() <= 1e-6 * a.1.abs().max(1.0));
                println!("  cache {} the rebuild", if same { "matches" } else { "DIFFERS FROM" });
                if !same { failed += 1; }
            }
            if flag("--save") && !s.read_only {
                match s.save(&path) { Ok(saved) => println!("  saved{}", if saved.cached_bodies > 0 { format!(" with {} cached bodies", saved.cached_bodies) } else { String::new() }), Err(e) => { eprintln!("  could not save: {e}"); failed += 1; } }
            }
        }
        return if failed > 0 { 1 } else { 0 };
    }
    // run
    let Some(script) = positional.first().map(PathBuf::from) else { eprintln!("usage: ferrender run SCRIPT.rhai [DESIGN.ferr] [--input NAME=VALUE]... [--allow DIR]... [--yes] [--timeout SECONDS] [--save [OUT.ferr]] [--strict]"); return 2; };
    let source = match fr_core::script::read_source(&script) { Ok(s) => s, Err(e) => { eprintln!("could not read {}: {e}", script.display()); return 2; } };
    let design = positional.get(1).map(PathBuf::from);
    let mut session = match &design {
        Some(d) => match fr_core::Session::open(d) { Ok(s) => s, Err(e) => { eprintln!("could not open {}: {e}", d.display()); return 2; } },
        None => fr_core::Session::default(),
    };
    let mut req = fr_core::script::Request::new(source);
    req.script_dir = script.parent().map(|p| if p.as_os_str().is_empty() { PathBuf::from(".") } else { p.to_path_buf() });
    let mut inputs = serde_json::Map::new();
    for kv in values("--input") {
        let Some((k, v)) = kv.split_once('=') else { eprintln!("--input takes NAME=VALUE, not {kv}"); return 2; };
        let value = if let Ok(b) = v.parse::<bool>() { serde_json::Value::Bool(b) } else if let Ok(n) = v.parse::<i64>() { serde_json::Value::from(n) } else if let Ok(f) = v.parse::<f64>() { serde_json::Value::from(f) } else { serde_json::Value::String(v.to_owned()) };
        inputs.insert(k.to_owned(), value);
    }
    req.inputs = serde_json::Value::Object(inputs);
    req.sandbox.yes = flag("--yes");
    if flag("--timeout") && values("--timeout").is_empty() {
        eprintln!("--timeout needs a positive number of seconds");
        return 2;
    }
    if let Some(t) = values("--timeout").last() {
        match t.parse::<f64>().ok().and_then(|secs| fr_core::script::timeout_duration(secs).ok()) {
            Some(duration) => req.time_limit = Some(duration),
            _ => { eprintln!("--timeout takes a positive number of seconds, not {t}"); return 2; }
        }
    }
    req.sandbox.allowed = values("--allow").into_iter().map(PathBuf::from).collect();
    if let Some(d) = &req.script_dir { req.sandbox.allowed.push(d.clone()); }
    if let Some(d) = design.as_ref().and_then(|d| d.parent()) { req.sandbox.allowed.push(if d.as_os_str().is_empty() { PathBuf::from(".") } else { d.to_path_buf() }); }
    let (tx, rx) = std::sync::mpsc::channel();
    req.events = Some(tx);
    let printer = std::thread::spawn(move || {
        for e in rx {
            match e {
                fr_core::script::Event::Log(m) => println!("{m}"),
                fr_core::script::Event::Progress(f, m) => eprintln!("[{:3.0}%] {m}", f * 100.0),
            }
        }
    });
    req.ask = Some(std::sync::Arc::new(|question: &str, default: Option<&str>| {
        eprint!("{question} {}", default.map_or("[y/N] ".to_owned(), |d| format!("[{d}] ")));
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() { return None; }
        let line = line.trim();
        match default {
            Some(d) => Some(if line.is_empty() { d.to_owned() } else { line.to_owned() }),
            None => matches!(line.to_ascii_lowercase().as_str(), "y" | "yes").then(String::new),
        }
    }));
    let result = fr_core::script::run(&mut session, &req);
    drop(req);
    let _ = printer.join();
    let outcome = match result {
        Ok(o) => o,
        Err(e) => { eprintln!("script failed: {e}"); return 1; }
    };
    if outcome.cancelled { eprintln!("cancelled"); return 1; }
    if !outcome.result.is_null() { println!("result: {}", outcome.result); }
    for (id, e) in &session.built.errors {
        eprintln!("timeline error in feature {id}: {e}");
    }
    if flag("--save") {
        let out = values("--save").into_iter().find(|n| n.ends_with(".ferr")).map(PathBuf::from).or(design.clone());
        match out {
            Some(path) => match session.save(&path) { Ok(_) => println!("saved {}", path.display()), Err(e) => { eprintln!("could not save: {e}"); return 1; } },
            None => { eprintln!("--save needs a design to save to"); return 2; }
        }
    }
    if flag("--strict") && !session.built.errors.is_empty() { return 3; }
    0
}
