//! A project's language servers: started when a file needs one, fed the
//! open documents, and asked for hover, definitions and the rest.
//!
//! Each server has an outbox processed in order, so a document is always
//! opened before it is changed and a request sees the latest text.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{AppContext as _, Context, EventEmitter, SharedString, Task};
use lsp_types::{Diagnostic, PublishDiagnosticsParams};
use serde_json::{Value, json};

use crate::client::Server;
use crate::servers::{self, ServerSpec};

#[derive(Debug, Clone, PartialEq)]
pub enum LspEvent {
    /// New diagnostics for this file.
    Diagnostics(PathBuf),
    /// A server started, failed or stopped.
    Status,
}

/// What a server is doing, for the status bar.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    Starting,
    Ready,
    Failed(SharedString),
}

enum Outgoing {
    Notify(&'static str, Value),
    Request(
        &'static str,
        Value,
        smol::channel::Sender<Result<smol::channel::Receiver<Result<Value>>>>,
    ),
}

struct Slot {
    spec: &'static ServerSpec,
    outbox: smol::channel::Sender<Outgoing>,
    status: ServerStatus,
    _task: Task<()>,
}

struct Document {
    server: &'static str,
    version: i32,
}

pub struct LspManager {
    root: PathBuf,
    servers: HashMap<&'static str, Slot>,
    documents: HashMap<PathBuf, Document>,
    diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
}

impl EventEmitter<LspEvent> for LspManager {}

impl LspManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            servers: HashMap::new(),
            documents: HashMap::new(),
            diagnostics: HashMap::new(),
        }
    }

    /// The servers and their state, for the status bar.
    pub fn status(&self) -> Vec<(&'static str, ServerStatus)> {
        let mut list: Vec<_> = self
            .servers
            .values()
            .map(|slot| (slot.spec.name, slot.status.clone()))
            .collect();
        list.sort_by_key(|(name, _)| *name);
        list
    }

    /// The server started for `path`'s language and how it is doing. Reads
    /// no files, so the status bar can ask on every frame (finding an
    /// installed server searches the PATH).
    pub fn server_for(&self, path: &Path) -> Option<(&'static str, ServerStatus)> {
        servers::candidates(path)
            .find_map(|spec| self.servers.get(spec.name))
            .map(|slot| (slot.spec.name, slot.status.clone()))
    }

    /// The server for `path` (started on first use), or `None` if none is
    /// installed for its language.
    fn slot(&mut self, path: &Path, cx: &mut Context<Self>) -> Option<&Slot> {
        let (spec, program) = servers::find(path)?;
        if !self.servers.contains_key(spec.name) {
            let (outbox, inbox) = smol::channel::unbounded::<Outgoing>();
            let root = self.root.clone();
            let start =
                cx.background_spawn(async move { Server::start(spec, &program, &root).await });
            let task = cx.spawn(async move |this, cx| {
                let server = match start.await {
                    Ok(server) => Arc::new(server),
                    Err(err) => {
                        tracing::warn!("{} did not start: {err:#}", spec.name);
                        _ = this.update(cx, |this, cx| {
                            if let Some(slot) = this.servers.get_mut(spec.name) {
                                slot.status = ServerStatus::Failed(format!("{err:#}").into());
                            }
                            cx.emit(LspEvent::Status);
                        });
                        return;
                    }
                };
                _ = this.update(cx, |this, cx| {
                    if let Some(slot) = this.servers.get_mut(spec.name) {
                        slot.status = ServerStatus::Ready;
                    }
                    cx.emit(LspEvent::Status);
                });
                // Diagnostics arrive as notifications.
                let notifications = server.transport.notifications.clone();
                let listen = cx.spawn({
                    let this = this.clone();
                    async move |cx| {
                        while let Ok((method, params)) = notifications.recv().await {
                            if method != "textDocument/publishDiagnostics" {
                                continue;
                            }
                            let Ok(params) =
                                serde_json::from_value::<PublishDiagnosticsParams>(params)
                            else {
                                continue;
                            };
                            let Some(path) = crate::uri::to_path(&params.uri) else {
                                continue;
                            };
                            if this
                                .update(cx, |this, cx| {
                                    this.diagnostics.insert(path.clone(), params.diagnostics);
                                    cx.emit(LspEvent::Diagnostics(path));
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                });
                // The outbox, strictly in order.
                let work = cx.background_spawn(async move {
                    while let Ok(item) = inbox.recv().await {
                        match item {
                            Outgoing::Notify(method, params) => {
                                let _ = server.notify(method, params);
                            }
                            Outgoing::Request(method, params, reply) => {
                                let _ = reply
                                    .try_send(server.transport.request_channel(method, params));
                            }
                        }
                    }
                    server.shutdown();
                });
                futures::future::join(listen, work).await;
            });
            self.servers.insert(
                spec.name,
                Slot {
                    spec,
                    outbox,
                    status: ServerStatus::Starting,
                    _task: task,
                },
            );
            cx.emit(LspEvent::Status);
        }
        self.servers
            .get(spec.name)
            .filter(|slot| !matches!(slot.status, ServerStatus::Failed(_)))
    }

    /// Opens `path` with `text` on its server (once per file).
    pub fn open(&mut self, path: &Path, text: &str, cx: &mut Context<Self>) {
        if self.documents.contains_key(path) {
            return self.change(path, text, cx);
        }
        let Some(slot) = self.slot(path, cx) else {
            return;
        };
        let (name, language, outbox) = (slot.spec.name, slot.spec.language, slot.outbox.clone());
        let Some(uri) = crate::uri::from_path(path) else {
            return;
        };
        let _ = outbox.try_send(Outgoing::Notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": language, "version": 1, "text": text } }),
        ));
        self.documents.insert(
            path.to_path_buf(),
            Document {
                server: name,
                version: 1,
            },
        );
    }

    /// Sends the whole new text of an open file.
    pub fn change(&mut self, path: &Path, text: &str, _cx: &mut Context<Self>) {
        let Some(document) = self.documents.get_mut(path) else {
            return;
        };
        document.version += 1;
        let (server, version) = (document.server, document.version);
        let (Some(slot), Some(uri)) = (self.servers.get(server), crate::uri::from_path(path))
        else {
            return;
        };
        let _ = slot.outbox.try_send(Outgoing::Notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": version }, "contentChanges": [{ "text": text }] }),
        ));
    }

    pub fn saved(&mut self, path: &Path) {
        let Some(document) = self.documents.get(path) else {
            return;
        };
        let (Some(slot), Some(uri)) = (
            self.servers.get(document.server),
            crate::uri::from_path(path),
        ) else {
            return;
        };
        let _ = slot.outbox.try_send(Outgoing::Notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri } }),
        ));
    }

    pub fn close(&mut self, path: &Path) {
        let Some(document) = self.documents.remove(path) else {
            return;
        };
        self.diagnostics.remove(path);
        let (Some(slot), Some(uri)) = (
            self.servers.get(document.server),
            crate::uri::from_path(path),
        ) else {
            return;
        };
        let _ = slot.outbox.try_send(Outgoing::Notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        ));
    }

    /// Whether `path` is open on a server (so requests can be answered).
    pub fn is_open(&self, path: &Path) -> bool {
        self.documents.contains_key(path)
    }

    pub fn diagnostics(&self, path: &Path) -> &[Diagnostic] {
        self.diagnostics
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Sends request `R` about `path` to its server.
    pub fn request<R>(
        &mut self,
        path: &Path,
        params: R::Params,
        cx: &mut Context<Self>,
    ) -> Task<Result<R::Result>>
    where
        R: lsp_types::request::Request,
        R::Result: Send + 'static,
    {
        let Some(server) = self.documents.get(path).map(|document| document.server) else {
            return Task::ready(Err(anyhow!("no language server for this file")));
        };
        let Some(slot) = self.servers.get(server) else {
            return Task::ready(Err(anyhow!("the language server is not running")));
        };
        let params = match serde_json::to_value(params) {
            Ok(params) => params,
            Err(err) => return Task::ready(Err(err.into())),
        };
        let (reply, sent) = smol::channel::bounded(1);
        let queued = slot
            .outbox
            .try_send(Outgoing::Request(R::METHOD, params, reply));
        cx.background_spawn(async move {
            queued.map_err(|_| anyhow!("the language server stopped"))?;
            let receiver = sent.recv().await.context("the language server stopped")??;
            let value = receiver
                .recv()
                .await
                .context("the language server stopped")??;
            Ok(serde_json::from_value(value)?)
        })
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        // Closing the outboxes ends each server's loop, which shuts it down.
        for slot in self.servers.values() {
            slot.outbox.close();
        }
    }
}
