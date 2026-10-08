//! `ferrender mcp`: a Model Context Protocol server on stdin/stdout, in the
//! shape of the Blender MCP: look at the scene, inspect an object, take a
//! screenshot, and run commands. It drives the running app when there is
//! one, and otherwise works on a document of its own with no window.

use std::io::Write;

use fr_core::{Session, api};
use serde_json::{Value, json};

struct Backend {
    port: u16,
    mode: Mode,
    session: Session,
}

#[derive(Debug, PartialEq)]
enum Mode { Undecided, Headless, Live(Option<String>) }

impl Backend {
    fn new(port: u16, headless: bool) -> Self {
        Self { port, mode: if headless { Mode::Headless } else { Mode::Undecided }, session: Session::default() }
    }

    fn run(&mut self, cmd: &Value) -> Result<Value, String> {
        self.run_with(cmd, crate::bridge::call)
    }

    fn run_with(&mut self, cmd: &Value, connect: impl FnOnce(u16, &Value, Option<&str>) -> Result<crate::bridge::CallReply, crate::bridge::CallError>) -> Result<Value, String> {
        if self.mode == Mode::Headless { return api::execute(&mut self.session, cmd, None); }
        let expected = match &self.mode { Mode::Live(id) => id.as_deref(), _ => None };
        match connect(self.port, cmd, expected) {
            Ok(reply) => {
                self.mode = Mode::Live(Some(reply.instance));
                reply.result
            }
            Err(e) if self.mode == Mode::Undecided && e.app_absent() => {
                // Only an initial absence, before connecting or sending anything,
                // selects a headless document. This choice lasts for the session.
                self.mode = Mode::Headless;
                api::execute(&mut self.session, cmd, None)
            }
            Err(e) => {
                if !matches!(self.mode, Mode::Live(Some(_))) {
                    self.mode = Mode::Live(e.instance);
                }
                let outcome = if e.may_have_executed { " The command outcome is unknown; inspect the app before retrying." } else { " No command was sent." };
                Err(format!("Ferrender app connection failed: {}.{outcome} The MCP session has not switched to another document. Restart the MCP client to select a new backend, or use --headless explicitly.", e.source))
            }
        }
    }
}

fn tools() -> Value {
    json!([
        {
            "name": "get_scene_info",
            "description": "Get the Ferrender document: active component, component tree, units, parameters, the feature timeline (sketches, primitives, extrudes, revolves, with any errors) and the bodies with their sizes and volumes. Call this first; never assume the document is empty.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "get_object_info",
            "description": "Get the details of one component, construction plane, sketch, feature or body by id. Sketch plane axes and body topology are reported in world coordinates. For a sketch this lists its points, entities, constraints, remaining degrees of freedom, and the closed profiles that can be extruded or revolved.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}
        },
        {
            "name": "get_viewport_screenshot",
            "description": "Render the model to an image to check what was built. view: iso, top, front, right, back, left, bottom, or current (the app's camera).",
            "inputSchema": {"type": "object", "properties": {"view": {"type": "string"}, "width": {"type": "integer"}, "height": {"type": "integer"}}}
        },
        {
            "name": "get_reference",
            "description": "The full list of Ferrender commands with their arguments. Read this once before building anything.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "execute_ferrender_commands",
            "description": "Run modelling commands in order; each is one undo step and execution stops at the first error. Commands are JSON objects with an \"op\": create_component, activate_component, move_component, create_plane, create_sketch, add_geometry, add_constraint, set_dimension, move, trim, mirror, offset, fillet, chamfer, project, delete, extrude, revolve, primitive, pattern, fillet_edges, chamfer_edges, shell, transform, combine, edit_feature, delete_feature, rollback, set_parameter, set_visible, import_stl, export_stl, export_step, save, open, undo, redo. Call get_reference for the arguments of each. Results list only the bodies a command changed. Inside a command, \"$last_sketch\", \"$last_feature\" and \"$last_body\" stand for the newest of each.",
            "inputSchema": {"type": "object", "properties": {"commands": {"type": "array", "items": {"type": "object"}, "description": "Command objects, each with an \"op\"."}}, "required": ["commands"]}
        }
    ])
}

fn call(b: &mut Backend, name: &str, args: &Value) -> Result<Value, String> {
    let text = |v: Value| json!([{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap()}]);
    match name {
        "get_scene_info" => {
            let mut v = b.run(&json!({"op": "get_scene_info"}))?;
            v["running_in"] = json!(if matches!(b.mode, Mode::Live(_)) { "the Ferrender app (changes appear live)" } else { "headless mode (separate document); use save and export_stl to write files" });
            Ok(text(v))
        }
        "get_object_info" => Ok(text(b.run(&json!({"op": "get_object_info", "id": args["id"]}))?)),
        "get_viewport_screenshot" => {
            let mut cmd = args.clone();
            if !cmd.is_object() {
                cmd = json!({});
            }
            cmd["op"] = json!("get_viewport_screenshot");
            let v = b.run(&cmd)?;
            Ok(json!([{"type": "image", "data": v["png_base64"], "mimeType": "image/png"}]))
        }
        "get_reference" => Ok(json!([{"type": "text", "text": api::REFERENCE}])),
        "execute_ferrender_commands" => Ok(text(b.run(&json!({"op": "batch", "commands": args["commands"]}))?)),
        other => Err(format!("unknown tool '{other}'")),
    }
}

pub fn serve(port: u16, headless: bool) {
    let mut b = Backend::new(port, headless);
    let stdout = std::io::stdout();
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    loop {
        let line = match crate::bridge::read_frame(&mut input, crate::bridge::MAX_REQUEST_BYTES) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => { eprintln!("MCP input rejected: {e}"); break; }
        };
        let Ok(msg) = serde_json::from_slice::<Value>(&line) else { continue };
        let id = msg["id"].clone();
        let result = match msg["method"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "ferrender", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Ferrender is a parametric CAD program. Read get_reference once, start with get_scene_info, build with execute_ferrender_commands, and check the result with get_viewport_screenshot.",
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => Ok(match call(&mut b, msg["params"]["name"].as_str().unwrap_or(""), &msg["params"]["arguments"]) {
                Ok(content) => json!({"content": content}),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            }),
            m if id.is_null() || m.starts_with("notifications/") => continue,
            m => Err(format!("method not found: {m}")),
        };
        let reply = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": e}}),
        };
        let mut out = stdout.lock();
        if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{CallError, CallReply};
    use std::io::{Error, ErrorKind};

    fn failure(kind: ErrorKind, connected: bool, may_have_executed: bool) -> CallError {
        CallError { source: Error::new(kind, "test transport failure"), connected, instance: connected.then(|| "window-a".to_owned()), may_have_executed }
    }

    #[test]
    fn initial_absence_selects_and_pins_headless() {
        let mut b = Backend::new(0, false);
        let cmd = json!({"op": "get_scene_info"});
        b.run_with(&cmd, |_, _, _| Err(failure(ErrorKind::NotFound, false, false))).unwrap();
        assert_eq!(b.mode, Mode::Headless);
        b.run_with(&cmd, |_, _, _| panic!("must not switch to an app that opened later")).unwrap();
    }

    #[test]
    fn live_session_never_falls_back_after_disconnect() {
        let mut b = Backend::new(0, false);
        let cmd = json!({"op": "get_scene_info"});
        b.run_with(&cmd, |_, _, expected| {
            assert!(expected.is_none());
            Ok(CallReply { instance: "window-a".to_owned(), result: Ok(json!({})) })
        }).unwrap();
        let error = b.run_with(&cmd, |_, _, expected| {
            assert_eq!(expected, Some("window-a"));
            Err(failure(ErrorKind::ConnectionRefused, false, false))
        }).unwrap_err();
        assert!(error.contains("No command was sent"));
        assert_eq!(b.mode, Mode::Live(Some("window-a".to_owned())));
    }

    #[test]
    fn connected_and_uncertain_failures_pin_live_without_replaying() {
        for (kind, connected, sent) in [(ErrorKind::TimedOut, true, true), (ErrorKind::InvalidData, true, false), (ErrorKind::PermissionDenied, false, false)] {
            let mut b = Backend::new(0, false);
            let error = b.run_with(&json!({"op": "new"}), |_, _, _| Err(failure(kind, connected, sent))).unwrap_err();
            assert!(matches!(b.mode, Mode::Live(_)));
            assert_eq!(error.contains("outcome is unknown"), sent);
            assert!(b.run_with(&json!({"op": "new"}), |_, _, _| Err(failure(ErrorKind::NotFound, false, false))).is_err());
            assert!(matches!(b.mode, Mode::Live(_)));
        }
    }
}
