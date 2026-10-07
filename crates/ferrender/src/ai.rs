//! The built-in assistant: Claude drives the document through the same
//! command API as the MCP server. The conversation runs on a worker
//! thread; each command it wants is executed on the UI thread.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use fr_core::api;
use serde_json::{Value, json};

use crate::app::App;

pub enum Event {
    Text(String),
    /// A command to run, and where to send its result.
    Tool(Value, Sender<Result<Value, String>>),
    /// The turn ended; carries the conversation so far for follow-ups.
    Done(Vec<Value>),
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Who {
    User,
    Assistant,
    Action,
    Error,
}

#[derive(Default)]
pub struct Assistant {
    pub open: bool,
    pub input: String,
    pub key_input: String,
    pub log: Vec<(Who, String)>,
    history: Vec<Value>,
    rx: Option<Receiver<Event>>,
}

const SYSTEM: &str = "You are the modelling assistant built into Ferrender, a parametric CAD program used to design parts for 3D printing. Build what the user asks for by calling the `ferrender` tool with commands; the user watches the model change as you work.\n\nWork the way a careful CAD user does: put sizes the user might change into named parameters, sketch on a plane, add dimensions, then extrude or revolve. After building, read back the scene for errors and take a screenshot to check the shape before saying it is done. Keep your replies to the user short: what you built and anything they should know, such as an assumption you made about a size.\n\n";

fn describe(cmd: &Value) -> String {
    let one = |c: &Value| c["op"].as_str().unwrap_or("?").replace('_', " ");
    match cmd["commands"].as_array() {
        Some(list) => list.iter().map(one).collect::<Vec<_>>().join(", "),
        None => one(cmd),
    }
}

/// One request to the Messages API.
fn request(key: &str, model: &str, messages: &[Value]) -> Result<Value, String> {
    let body = json!({
        "model": model,
        "max_tokens": 16000,
        "system": format!("{SYSTEM}{}", api::REFERENCE),
        // Requests the safety classifiers decline are retried on Anthropic's recommended fallback model.
        "fallbacks": "default",
        "tools": [{
            "name": "ferrender",
            "description": "Run Ferrender commands in order and get each one's result. Stops at the first error and reports which command failed.",
            "input_schema": {"type": "object", "properties": {"commands": {"type": "array", "items": {"type": "object"}, "description": "Command objects, each with an \"op\", as described in the system prompt."}}, "required": ["commands"]}
        }],
        "messages": messages,
    });
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(600))).http_status_as_error(false).build().into();
    let mut resp = agent
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "server-side-fallback-2026-07-01")
        .header("content-type", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|e| format!("Couldn't reach the Claude API: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| format!("Couldn't read the reply: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|_| format!("The Claude API returned status {status} with a reply that is not JSON."))?;
    if status != 200 {
        let msg = v["error"]["message"].as_str().unwrap_or("no details given");
        return Err(match status {
            401 => "The Claude API rejected the API key. Check it in the assistant panel.".to_owned(),
            429 => format!("The Claude API is rate limiting this key: {msg}"),
            s if s >= 500 => format!("The Claude API had a server error ({s}): {msg}"),
            s => format!("The Claude API refused the request ({s}): {msg}"),
        });
    }
    Ok(v)
}

/// Runs one user turn to completion, executing tool calls through `tx`.
fn converse(key: String, model: String, mut messages: Vec<Value>, tx: &Sender<Event>, wake: &dyn Fn()) -> Result<Vec<Value>, String> {
    let send = |e: Event| {
        let _ = tx.send(e);
        wake();
    };
    for _ in 0..40 {
        let reply = request(&key, &model, &messages)?;
        let content = reply["content"].as_array().cloned().unwrap_or_default();
        let stop = reply["stop_reason"].as_str().unwrap_or("");
        if stop == "refusal" {
            return Err("Claude declined this request.".into());
        }
        // The reply goes back unchanged, thinking blocks included.
        messages.push(json!({"role": "assistant", "content": content}));
        let mut results = Vec::new();
        for block in &content {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(t) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                        send(Event::Text(t.trim().to_owned()));
                    }
                }
                Some("tool_use") => {
                    let (back, wait) = channel();
                    send(Event::Tool(block["input"].clone(), back));
                    let result = wait.recv_timeout(Duration::from_secs(180)).unwrap_or(Err("Ferrender did not answer.".into()));
                    let mut out = json!({"type": "tool_result", "tool_use_id": block["id"]});
                    match result {
                        Ok(v) => {
                            // Screenshots go back as images, everything else as JSON text.
                            let mut blocks = Vec::new();
                            let mut rest = v.clone();
                            for item in rest.as_array_mut().into_iter().flatten() {
                                if let Some(png) = item["png_base64"].as_str() {
                                    blocks.push(json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": png}}));
                                    *item = json!({"screenshot": "attached"});
                                }
                            }
                            blocks.insert(0, json!({"type": "text", "text": rest.to_string()}));
                            out["content"] = json!(blocks);
                        }
                        Err(e) => {
                            out["content"] = json!(e);
                            out["is_error"] = json!(true);
                        }
                    }
                    results.push(out);
                }
                _ => {}
            }
        }
        match stop {
            "tool_use" => messages.push(json!({"role": "user", "content": results})),
            // A paused server turn resumes when the conversation is sent back as it is.
            "pause_turn" => {}
            "max_tokens" => return Err("Claude's reply was cut off for length. Ask it to continue.".into()),
            _ => return Ok(messages),
        }
    }
    Err("Stopped after 40 steps without finishing.".into())
}

impl Assistant {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    pub fn clear(&mut self) {
        if !self.busy() {
            self.log.clear();
            self.history.clear();
        }
    }

    pub fn send(&mut self, app: &App, prompt: String) {
        let Some(key) = app.config.api_key() else {
            self.log.push((Who::Error, "Add an Anthropic API key first.".into()));
            return;
        };
        let scene = api::execute(&mut fr_core::Session::new(app.session.doc.clone()), &json!({"op": "get_scene_info"}), None).unwrap_or_default();
        let editing = app.sketch().map_or(String::new(), |(id, _)| format!(" The user is currently editing sketch {id}."));
        let mut messages = self.history.clone();
        messages.push(json!({"role": "user", "content": format!("{prompt}\n\n<current_scene>{scene}</current_scene>{editing}")}));
        self.log.push((Who::User, prompt));
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let (model, ctx) = (app.config.model().to_owned(), app.ctx.clone());
        std::thread::spawn(move || {
            let wake = || ctx.request_repaint();
            let end = match converse(key, model, messages, &tx, &wake) {
                Ok(m) => Event::Done(m),
                Err(e) => Event::Error(e),
            };
            let _ = tx.send(end);
            wake();
        });
    }

    /// Handles what the worker has sent since the last frame.
    pub fn poll(&mut self, app: &mut App) {
        let Some(rx) = &self.rx else { return };
        let mut finished = false;
        while let Ok(e) = rx.try_recv() {
            match e {
                Event::Text(t) => self.log.push((Who::Assistant, t)),
                Event::Tool(input, back) => {
                    self.log.push((Who::Action, describe(&input)));
                    let _ = back.send(app.execute(&json!({"op": "batch", "commands": input["commands"]})));
                }
                Event::Done(history) => {
                    self.history = history;
                    finished = true;
                }
                Event::Error(e) => {
                    self.log.push((Who::Error, e));
                    finished = true;
                }
            }
        }
        if finished {
            self.rx = None;
        }
    }
}
