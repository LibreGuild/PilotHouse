//! Google Antigravity (`agy`) as an ACP agent.
//!
//! agy has no ACP mode, but its headless mode streams JSON:
//! `agy -p= --input-format stream-json --output-format stream-json` reads one
//! user message per line on stdin and prints one event per line (`init`,
//! `step_update`, `result`), keeping the conversation between turns. This
//! crate serves ACP on stdio and drives one such process per session,
//! translating between the two. PilotHouse runs it as `pilothouse agy-acp`.
//!
//! - A session is an agy conversation: the ACP session id is agy's
//!   conversation id, so `session/load` resumes with `--conversation`.
//! - Model, effort and mode are agy flags. Changing one restarts the process
//!   on the same conversation before the next turn.
//! - Headless agy cannot ask for permission: it refuses the action and lists
//!   it in the result. The bridge then asks through ACP and, when allowed,
//!   continues the conversation once with permissions skipped.
//! - Idle processes stop after a few minutes (each holds ~200 MB) and come
//!   back on the next turn.

mod agy;
mod options;
mod session;
mod translate;

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde_json::{Value, json};

use crate::agy::ModelList;
use crate::session::Session;

/// How long a session's agy process may sit idle before it is stopped.
const IDLE_STOP: Duration = Duration::from_secs(5 * 60);

/// Serves ACP on stdin/stdout until stdin closes.
pub fn run_stdio() -> Result<()> {
    let program = ph_env::which("agy").context("`agy` is not on your PATH")?;
    let bridge = Arc::new(Bridge {
        models: Arc::new(ModelList::new(program.clone())),
        program,
        out: Output::default(),
        sessions: Mutex::default(),
        pending: Mutex::default(),
        next_id: AtomicU64::new(1),
    });
    bridge.start_reaper();

    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(message) => bridge.dispatch(message),
            Err(err) => tracing::warn!("agy-acp: ignoring a message that is not JSON: {err}"),
        }
    }
    // The client is gone: stop every agy process (threads still running a
    // turn hold their sessions, so stop them explicitly).
    for session in bridge
        .sessions
        .lock()
        .unwrap()
        .drain()
        .map(|(_, session)| session)
    {
        session.stop();
    }
    Ok(())
}

/// A JSON-RPC error for the client.
struct RpcError {
    code: i64,
    message: String,
}

impl From<anyhow::Error> for RpcError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            code: -32603,
            message: format!("{err:#}"),
        }
    }
}

pub(crate) struct Bridge {
    program: PathBuf,
    out: Output,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// Our own requests to the client (permission prompts), by id.
    pending: Mutex<HashMap<u64, mpsc::Sender<Value>>>,
    next_id: AtomicU64,
    models: Arc<ModelList>,
}

impl Bridge {
    fn dispatch(self: &Arc<Self>, message: Value) {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method, id) {
            // Requests can block (a prompt runs a whole turn): one thread each.
            (Some(method), Some(id)) => {
                let (bridge, method) = (self.clone(), method.to_string());
                std::thread::spawn(move || {
                    let had_models = bridge.models.loaded().is_some();
                    let result = bridge.handle(&method, &params);
                    let opened = match (method.as_str(), &result) {
                        ("session/new", Ok(result)) => result.get("sessionId").cloned(),
                        ("session/load", Ok(_)) => params.get("sessionId").cloned(),
                        _ => None,
                    };
                    bridge.out.reply(id, result);
                    // A session that opened before the models were listed
                    // gets them now, after the client knows its id.
                    if let (Some(session_id), false) = (opened, had_models) {
                        bridge.send_models(&session_id);
                    }
                });
            }
            (Some("session/cancel"), None) => {
                if let Ok(session) = self.session(&params) {
                    session.cancel();
                }
            }
            (Some(_), None) => {}
            // A reply to one of our requests.
            (None, Some(id)) => {
                let sender = id
                    .as_u64()
                    .and_then(|id| self.pending.lock().unwrap().remove(&id));
                if let Some(sender) = sender {
                    let _ = sender.send(message);
                }
            }
            (None, None) => {}
        }
    }

    fn handle(self: &Arc<Self>, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => {
                self.list_models_in_background();
                Ok(json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": { "image": false, "audio": false, "embeddedContext": true },
                },
                "authMethods": [],
                "agentInfo": { "name": "pilothouse-agy", "version": env!("CARGO_PKG_VERSION") },
                }))
            }
            "authenticate" => Ok(json!({})),
            "session/new" => {
                let session = self.open_session(params, None)?;
                Ok(json!({ "sessionId": session.id, "configOptions": session.config_options() }))
            }
            "session/load" => {
                let id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let session = self.open_session(params, Some(id))?;
                Ok(json!({ "configOptions": session.config_options() }))
            }
            "session/prompt" => {
                let session = self.session(params)?;
                let prompt = params.get("prompt").unwrap_or(&Value::Null);
                Ok(session.prompt(self, prompt)?)
            }
            "session/set_config_option" => {
                let session = self.session(params)?;
                let config = params
                    .get("configId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let value = params
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                session.set_option(config, value)?;
                Ok(json!({ "configOptions": session.config_options() }))
            }
            _ => Err(RpcError {
                code: -32601,
                message: format!("`{method}` is not supported"),
            }),
        }
    }

    /// Starts agy for a new session, or resumes conversation `resume`.
    fn open_session(&self, params: &Value, resume: Option<&str>) -> Result<Arc<Session>> {
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .context("no working directory")?;
        let session = Session::start(self.program.clone(), cwd, resume, self.models.clone())?;
        let session = Arc::new(session);
        self.sessions
            .lock()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        Ok(session)
    }

    /// Lists models while agy starts.
    fn list_models_in_background(&self) {
        let models = self.models.clone();
        std::thread::spawn(move || {
            models.get();
        });
    }

    /// Waits for the model list and sends it to session `session_id`.
    fn send_models(&self, session_id: &Value) {
        self.models.get();
        let Ok(session) = self.session(&json!({ "sessionId": session_id })) else {
            return;
        };
        self.notify(
            "session/update",
            json!({
                "sessionId": session.id,
                "update": {
                    "sessionUpdate": "config_option_update",
                    "configOptions": session.config_options(),
                },
            }),
        );
    }

    fn session(&self, params: &Value) -> Result<Arc<Session>> {
        let id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .with_context(|| format!("unknown session `{id}`"))
    }

    /// Asks the client to allow what agy refused; true when allowed.
    pub(crate) fn ask_permission(&self, session_id: &str, tool_call: Value) -> Result<bool> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, sender);
        self.out.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/request_permission",
            "params": permission_params(session_id, tool_call),
        }));
        let reply = receiver.recv().context("the client closed")?;
        let chosen = reply
            .pointer("/result/outcome/optionId")
            .and_then(Value::as_str);
        Ok(chosen == Some("allow"))
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        self.out
            .send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Stops agy processes that have been idle for [`IDLE_STOP`].
    fn start_reaper(self: &Arc<Self>) {
        let bridge = Arc::downgrade(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let Some(bridge) = bridge.upgrade() else {
                    return;
                };
                let sessions: Vec<_> = bridge.sessions.lock().unwrap().values().cloned().collect();
                for session in sessions {
                    session.stop_if_idle(IDLE_STOP);
                }
            }
        });
    }
}

fn permission_params(session_id: &str, tool_call: Value) -> Value {
    json!({
        "sessionId": session_id,
        "toolCall": tool_call,
        "options": [
            { "optionId": "allow", "name": "Allow and continue", "kind": "allow_once" },
            { "optionId": "reject", "name": "Don't allow", "kind": "reject_once" },
        ],
    })
}

/// Stdout, shared by every thread; one JSON message per line.
#[derive(Default)]
struct Output(Mutex<()>);

impl Output {
    fn send(&self, message: &Value) {
        let _guard = self.0.lock().unwrap();
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{message}");
        let _ = stdout.flush();
    }

    fn reply(&self, id: Value, result: Result<Value, RpcError>) {
        self.send(&match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(err) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": err.code, "message": err.message },
            }),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol_schema::v1::RequestPermissionRequest;

    #[test]
    fn permission_prompts_are_valid_acp() {
        let tool_call =
            json!({ "toolCallId": "step-3", "title": "Write file a.txt", "status": "pending" });
        let params = permission_params("s1", tool_call);
        serde_json::from_value::<RequestPermissionRequest>(params).unwrap();
    }
}
