//! One running agent process for one project.
//!
//! All chat sessions of a project that use the same agent share a process
//! (agents such as the Claude adapter cost hundreds of MB each). The
//! connection performs the ACP handshake, then routes the agent's updates and
//! questions to the [`ChatThread`] that owns each ACP session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::v1::{
    ClientCapabilities, FileSystemCapabilities, Implementation, InitializeRequest,
    InitializeResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome,
};
use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, WeakEntity};
use serde_json::{Value, json};

use crate::agents::{AgentSpec, Availability};
use crate::rpc::{Incoming, RpcError, RpcProcess};
use crate::thread::ChatThread;
use crate::transcript::parse_update;

/// What a ready agent offers.
#[derive(Clone)]
pub struct Ready {
    pub process: Arc<RpcProcess>,
    /// The agent accepts files embedded in the prompt (else we send links).
    pub embedded_context: bool,
    /// The agent accepts images in prompts (screenshots, annotations).
    pub images: bool,
    /// The agent can resume an earlier session (`session/load`).
    pub load_session: bool,
    /// Ways to sign in that the agent handles itself (`authenticate`).
    pub sign_in: Vec<SignInMethod>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignInMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// Resolves once the handshake is done; cloneable so every thread can await it.
pub type ReadyTask = Shared<Task<Result<Ready, SharedString>>>;

pub struct AgentConnection {
    spec: &'static AgentSpec,
    ready: ReadyTask,
    process: Option<Arc<RpcProcess>>,
    /// ACP session id → the thread that owns it.
    threads: HashMap<String, WeakEntity<ChatThread>>,
    /// Set when the process exits; new prompts then restart the agent.
    exited: Option<SharedString>,
    _incoming: Option<Task<()>>,
}

impl AgentConnection {
    /// Starts `spec` in `cwd`. Never fails: problems surface when a thread
    /// awaits [`Self::ready`], as a message the chat can show.
    pub fn start(spec: &'static AgentSpec, cwd: PathBuf, cx: &mut Context<Self>) -> Self {
        let launched = match spec.availability() {
            Availability::Ready { program, args } => {
                RpcProcess::spawn(&program, &args, &cwd, ph_env::shell_env()).map_err(|err| {
                    SharedString::from(format!("Could not start {}: {err:#}", spec.name))
                })
            }
            Availability::Missing(reason) => Err(reason.into()),
        };

        let (process, ready, incoming) = match launched {
            Ok((process, incoming)) => {
                let handshake = initialize(process.clone(), spec);
                let ready = cx.background_spawn(handshake).shared();
                (Some(process), ready, Some(incoming))
            }
            Err(reason) => (None, Task::ready(Err(reason)).shared(), None),
        };

        let incoming_task = incoming.map(|incoming| {
            cx.spawn(async move |this, cx| {
                while let Ok(message) = incoming.recv().await {
                    let closed = matches!(message, Incoming::Closed);
                    if this
                        .update(cx, |this, cx| this.handle(message, cx))
                        .is_err()
                        || closed
                    {
                        break;
                    }
                }
            })
        });

        Self {
            spec,
            ready,
            process,
            threads: HashMap::new(),
            exited: None,
            _incoming: incoming_task,
        }
    }

    pub fn spec(&self) -> &'static AgentSpec {
        self.spec
    }

    pub fn ready(&self) -> ReadyTask {
        self.ready.clone()
    }

    pub fn has_exited(&self) -> bool {
        self.exited.is_some()
    }

    /// Runs an agent-handled sign-in (it may open the browser).
    pub fn sign_in(
        &self,
        method_id: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), SharedString>> {
        let ready = self.ready.clone();
        cx.background_spawn(async move {
            let ready = ready.await?;
            ready
                .process
                .request("authenticate", json!({ "methodId": method_id }))
                .await
                .map(|_| ())
                .map_err(|err| SharedString::from(format!("Sign-in failed: {err:#}")))
        })
    }

    pub fn register_thread(&mut self, acp_session_id: String, thread: WeakEntity<ChatThread>) {
        self.threads.insert(acp_session_id, thread);
    }

    fn thread(&self, acp_session_id: &str) -> Option<gpui_kit::Entity<ChatThread>> {
        self.threads
            .get(acp_session_id)
            .and_then(WeakEntity::upgrade)
    }

    fn handle(&mut self, message: Incoming, cx: &mut Context<Self>) {
        match message {
            Incoming::Notification { method, params } if method == "session/update" => {
                let Some((session_id, update)) = parse_update(&params) else {
                    return;
                };
                if let Some(thread) = self.thread(&session_id) {
                    thread.update(cx, |thread, cx| thread.apply_update(update, cx));
                }
            }
            Incoming::Notification { .. } => {}
            Incoming::Request { id, method, params } if method == "session/request_permission" => {
                self.request_permission(id, params, cx);
            }
            Incoming::Request { id, method, .. } => {
                // We advertise no fs/terminal capabilities, so agents should
                // not ask; answer politely if one does.
                if let Some(process) = &self.process {
                    let _ = process.respond(
                        id,
                        Err(RpcError {
                            code: -32601,
                            message: format!("PilotHouse does not support `{method}`"),
                        }),
                    );
                }
            }
            Incoming::Closed => {
                let tail = self
                    .process
                    .as_ref()
                    .map(|process| process.stderr_tail().join("\n"))
                    .unwrap_or_default();
                let message: SharedString = if tail.trim().is_empty() {
                    format!("{} stopped.", self.spec.name).into()
                } else {
                    format!("{} stopped:\n{tail}", self.spec.name).into()
                };
                self.exited = Some(message.clone());
                for thread in self.threads.values().filter_map(WeakEntity::upgrade) {
                    thread.update(cx, |thread, cx| thread.agent_exited(message.clone(), cx));
                }
            }
        }
    }

    fn request_permission(&mut self, id: Value, params: Value, cx: &mut Context<Self>) {
        let Some(process) = self.process.clone() else {
            return;
        };
        let request: RequestPermissionRequest = match serde_json::from_value(params) {
            Ok(request) => request,
            Err(err) => {
                let _ = process.respond(
                    id,
                    Err(RpcError {
                        code: -32602,
                        message: err.to_string(),
                    }),
                );
                return;
            }
        };
        let respond = move |choice: Option<String>, _: &mut App| {
            let outcome = match choice {
                Some(option_id) => {
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
                }
                None => RequestPermissionOutcome::Cancelled,
            };
            let response = serde_json::to_value(RequestPermissionResponse::new(outcome))
                .unwrap_or(Value::Null);
            let _ = process.respond(id, Ok(response));
        };
        match self.thread(&request.session_id.0) {
            Some(thread) => thread.update(cx, |thread, cx| {
                thread.request_permission(request, respond, cx)
            }),
            None => respond(None, cx),
        }
    }
}

/// The ACP handshake. We offer no file-system or terminal capabilities:
/// agents use their own tools, and PilotHouse watches the results.
async fn initialize(
    process: Arc<RpcProcess>,
    spec: &'static AgentSpec,
) -> Result<Ready, SharedString> {
    let request = InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            ClientCapabilities::new()
                .fs(FileSystemCapabilities::new()
                    .read_text_file(false)
                    .write_text_file(false))
                .terminal(false),
        )
        .client_info(Implementation::new("pilothouse", env!("CARGO_PKG_VERSION")));
    let params = serde_json::to_value(&request).unwrap_or_else(|_| json!({}));
    let result = process.request("initialize", params).await.map_err(|err| {
        let tail = process.stderr_tail().join("\n");
        SharedString::from(if tail.trim().is_empty() {
            format!("{} did not start: {err:#}", spec.name)
        } else {
            format!("{} did not start: {err:#}\n{tail}", spec.name)
        })
    })?;
    // Sign-in methods are read from raw JSON: the typed enum also holds
    // terminal-based methods we cannot run.
    let sign_in = result
        .get("authMethods")
        .and_then(Value::as_array)
        .map(|methods| {
            methods
                .iter()
                .filter(|method| {
                    method
                        .get("type")
                        .and_then(Value::as_str)
                        .is_none_or(|kind| kind == "agent")
                })
                .filter_map(|method| {
                    Some(SignInMethod {
                        id: method.get("id")?.as_str()?.to_string(),
                        name: method.get("name")?.as_str()?.to_string(),
                        description: method
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let response: InitializeResponse = serde_json::from_value(result).map_err(|err| {
        SharedString::from(format!("{} sent an unexpected handshake: {err}", spec.name))
    })?;
    let capabilities = response.agent_capabilities;
    Ok(Ready {
        process,
        embedded_context: capabilities.prompt_capabilities.embedded_context,
        images: capabilities.prompt_capabilities.image,
        load_session: capabilities.load_session,
        sign_in,
    })
}
