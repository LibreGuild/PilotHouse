use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, AsyncApp, Global, Task};
use serde_json::Value;

use crate::protocol::{Request, Response};
use crate::transport::{Connection, Listener};

type SyncHandler = dyn Fn(Value, &mut App) -> Result<Value>;
type AsyncHandler = dyn Fn(Value, &mut App) -> Task<Result<Value>>;
/// Told of each request from outside that succeeded: method and params.
pub type Observer = Rc<dyn Fn(&str, &Value, &mut App)>;

/// A method handler. Quick ones answer at once; slow ones (a browser page
/// load, an adb command) return a task so the UI keeps running meanwhile.
#[derive(Clone)]
enum Handler {
    Sync(Rc<SyncHandler>),
    Async(Rc<AsyncHandler>),
}

/// A request from a socket thread, waiting for the UI thread to answer it.
struct Pending {
    request: Request,
    reply: smol::channel::Sender<Response>,
}

/// Method handlers, stored as a GPUI global so any crate can register some.
#[derive(Default)]
pub struct ControlServer {
    handlers: HashMap<&'static str, Handler>,
    /// Watches what agents do (a UI test being recorded).
    observer: Option<Observer>,
}

impl Global for ControlServer {}

impl ControlServer {
    /// Registers `handler` for `method`, replacing any previous one.
    ///
    /// Handlers run on the UI thread and must stay quick: a slow handler
    /// freezes the window while it runs.
    pub fn handle(
        method: &'static str,
        handler: impl Fn(Value, &mut App) -> Result<Value> + 'static,
        cx: &mut App,
    ) {
        cx.default_global::<Self>()
            .handlers
            .insert(method, Handler::Sync(Rc::new(handler)));
    }

    /// Registers a handler that answers later. It starts on the UI thread and
    /// returns a task; the caller waits for it without blocking the window.
    pub fn handle_async(
        method: &'static str,
        handler: impl Fn(Value, &mut App) -> Task<Result<Value>> + 'static,
        cx: &mut App,
    ) {
        cx.default_global::<Self>()
            .handlers
            .insert(method, Handler::Async(Rc::new(handler)));
    }

    /// Calls `method` in-process, as a client would (the UI test runner
    /// replays steps this way). Observers are not told.
    pub fn call(method: &str, params: Value, cx: &mut App) -> Task<Result<Value>> {
        let request = Request {
            id: 0,
            method: method.to_string(),
            params,
        };
        match dispatch(request, cx) {
            Dispatched::Ready(result) => Task::ready(result),
            Dispatched::Later(task) => task,
        }
    }

    /// Sets (or with `None` clears) the one observer of successful requests
    /// from clients.
    pub fn set_observer(observer: Option<Observer>, cx: &mut App) {
        cx.default_global::<Self>().observer = observer;
    }

    /// Starts listening on `endpoint` (see [`crate::transport`]). Connection
    /// I/O happens on background threads; every request is answered on the
    /// UI thread.
    pub fn start(endpoint: &Path, cx: &mut App) -> Result<()> {
        if let Some(parent) = endpoint.parent() {
            ph_store::paths::ensure_private_dir(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let listener = Listener::bind(endpoint)?;

        let (sender, receiver) = smol::channel::unbounded::<Pending>();
        let path = endpoint.to_path_buf();
        std::thread::Builder::new()
            .name("control-accept".into())
            .spawn(move || accept_loop(listener, sender, path))?;

        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(pending) = receiver.recv().await {
                let id = pending.request.id;
                let (method, params) = (
                    pending.request.method.clone(),
                    pending.request.params.clone(),
                );
                match cx.update(|cx| dispatch(pending.request, cx)) {
                    Dispatched::Ready(result) => {
                        if result.is_ok() {
                            cx.update(|cx| observe(&method, &params, cx));
                        }
                        let _ = pending.reply.send(Response::from_result(id, result)).await;
                    }
                    Dispatched::Later(task) => {
                        cx.spawn(async move |cx: &mut AsyncApp| {
                            let result = task.await;
                            if result.is_ok() {
                                cx.update(|cx| observe(&method, &params, cx));
                            }
                            let _ = pending.reply.send(Response::from_result(id, result)).await;
                        })
                        .detach();
                    }
                }
            }
        })
        .detach();

        tracing::info!("control endpoint listening at {}", endpoint.display());
        Ok(())
    }
}

fn observe(method: &str, params: &Value, cx: &mut App) {
    let observer = cx
        .try_global::<ControlServer>()
        .and_then(|server| server.observer.clone());
    if let Some(observer) = observer {
        observer(method, params, cx);
    }
}

enum Dispatched {
    Ready(Result<Value>),
    Later(Task<Result<Value>>),
}

fn dispatch(request: Request, cx: &mut App) -> Dispatched {
    let handler = cx
        .try_global::<ControlServer>()
        .and_then(|server| server.handlers.get(request.method.as_str()).cloned());
    match handler {
        Some(Handler::Sync(handler)) => Dispatched::Ready(handler(request.params, cx)),
        Some(Handler::Async(handler)) => Dispatched::Later(handler(request.params, cx)),
        None => Dispatched::Ready(Err(anyhow!("unknown method `{}`", request.method))),
    }
}

fn accept_loop(mut listener: Listener, sender: smol::channel::Sender<Pending>, path: PathBuf) {
    loop {
        let connection = match listener.accept() {
            Ok(connection) => connection,
            Err(err) => {
                tracing::warn!("control accept failed on {}: {err}", path.display());
                // Avoid a hot loop if accepting keeps failing.
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        };
        let sender = sender.clone();
        let _ = std::thread::Builder::new()
            .name("control-conn".into())
            .spawn(move || serve_connection(connection, sender));
    }
}

/// Reads requests line by line and writes each response back in order.
fn serve_connection(connection: Connection, sender: smol::channel::Sender<Pending>) {
    let Connection { reader, mut writer } = connection;
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let (reply, answer) = smol::channel::bounded(1);
                if sender.send_blocking(Pending { request, reply }).is_err() {
                    break; // The app is shutting down.
                }
                match answer.recv_blocking() {
                    Ok(response) => response,
                    Err(_) => break,
                }
            }
            Err(err) => Response::from_result(0, Err(anyhow!("malformed request: {err}"))),
        };
        let Ok(mut text) = serde_json::to_string(&response) else {
            break;
        };
        text.push('\n');
        if writer
            .write_all(text.as_bytes())
            .and_then(|_| writer.flush())
            .is_err()
        {
            break;
        }
    }
}
