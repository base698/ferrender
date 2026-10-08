mod ai;
mod app;
mod bridge;
mod build_info;
mod config;
mod construction;
mod primitives;
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
       ferrender --version

Opens a design (.ferr) or imports a mesh (.stl).

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
