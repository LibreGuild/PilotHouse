//! JSON-RPC over a child process's stdio, as ACP uses it.
//!
//! One message per line in each direction. Two background threads do the
//! blocking I/O (one reads stdout, one writes stdin) and a third drains stderr
//! into a short ring buffer, so a crashing agent's last words can be shown.
//! Responses are matched to requests by id; everything else the agent sends
//! (notifications and its own requests) is delivered as [`Incoming`].
//!
//! The reader's thread also reads each `session/update` into its type, and
//! everything it routes goes through one queue of limited length
//! ([`IncomingMessages`]), answers to our requests included: whoever takes
//! the messages sees them, and whoever asked gets an answer, in the order
//! the agent sent them, and an agent that sends faster than they are taken
//! waits on its pipe.

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use anyhow::{Context as _, Result, anyhow};
use futures::channel::oneshot;
use serde_json::{Value, json};

/// Lines of stderr kept for error messages.
const STDERR_TAIL_LINES: usize = 40;

/// Messages read from the agent and not taken yet. Past this the reader's
/// thread waits for room, and the agent for its pipe: a flood is held back
/// there, not kept in the app's memory.
const QUEUED_MESSAGES: usize = 1024;

/// Something the agent sent that is not a response to us.
#[derive(Debug)]
pub enum Incoming {
    /// A `session/update`, read into its type already (one this build does
    /// not know is left out).
    Update(Box<crate::transcript::Update>),
    Notification {
        method: String,
        params: Value,
    },
    /// The agent asks us something; answer with [`RpcProcess::respond`].
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// The process exited or closed stdout.
    Closed,
}

/// An error the agent returned for one of our requests.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// A JSON-RPC error's message, with the reason the agent put in its `data`
/// (Cline says only "Internal error" there, and why in `data.details`).
fn error_message(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    let data = error.get("data");
    let details = data
        .and_then(Value::as_str)
        .or_else(|| {
            let data = data?;
            ["details", "message", "error"]
                .iter()
                .find_map(|key| data.get(key).and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|details| !details.is_empty() && !message.contains(details));
    match details {
        Some(details) => format!("{message}: {details}"),
        None => message.to_string(),
    }
}

/// The agent's answer to one request.
type Answer = Result<Value, RpcError>;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

/// What the reader's thread puts in the queue.
enum Routed {
    Message(Incoming),
    /// The answer to a request of ours, kept in its place among the
    /// agent's messages: whoever asked gets it only once the messages
    /// before it were taken (a turn's answer must not overtake the last
    /// of what the turn said).
    Answer(oneshot::Sender<Answer>, Answer),
}

/// What the agent sends, in the order it sent it.
pub struct IncomingMessages {
    queue: smol::channel::Receiver<Routed>,
}

impl IncomingMessages {
    /// The next message, waited for; `None` when the process is gone and
    /// nothing is left. Answers to our requests go to whoever asked on the
    /// way.
    pub async fn next(&self) -> Option<Incoming> {
        loop {
            match self.queue.recv().await.ok()? {
                Routed::Message(message) => return Some(message),
                Routed::Answer(waiter, answer) => {
                    let _ = waiter.send(answer);
                    // Whoever asked acts on it before the next message
                    // is taken.
                    smol::future::yield_now().await;
                }
            }
        }
    }

    /// The next message when it is here already. `None` also right after
    /// an answer went to whoever asked: they act on it before what the
    /// agent sent next is taken, so the caller lets others run before it
    /// asks again.
    pub fn next_now(&self) -> Option<Incoming> {
        match self.queue.try_recv().ok()? {
            Routed::Message(message) => Some(message),
            Routed::Answer(waiter, answer) => {
                let _ = waiter.send(answer);
                None
            }
        }
    }
}

impl Drop for IncomingMessages {
    fn drop(&mut self) {
        // Nobody takes messages any more. An answer that comes later fails
        // its request as it cannot be queued; those queued already are
        // handed over here, or their requests would wait for ever (and
        // keep the process they hold).
        self.queue.close();
        while let Ok(routed) = self.queue.try_recv() {
            if let Routed::Answer(waiter, answer) = routed {
                let _ = waiter.send(answer);
            }
        }
    }
}

/// Every agent process started and not let go of yet, for
/// [`stop_all_before_exit`].
static RUNNING: Mutex<Vec<Weak<RpcProcess>>> = Mutex::new(Vec::new());

/// Ends every agent still running, for when the app quits: nothing is
/// dropped then, so [`RpcProcess`]'s own `Drop` never runs, and an agent
/// that does not end by itself when its input closes would stay behind
/// with all it started.
pub fn stop_all_before_exit() {
    let running = std::mem::take(&mut *RUNNING.lock().unwrap_or_else(|e| e.into_inner()));
    for process in running.iter().filter_map(Weak::upgrade) {
        process.end(true);
    }
}

/// A running agent process and its JSON-RPC plumbing.
pub struct RpcProcess {
    writer: smol::channel::Sender<String>,
    pending: Pending,
    /// The agent's output ended: no answer will come (set with `pending`
    /// locked, so a request either sees it or is failed with the rest).
    closed: Arc<std::sync::atomic::AtomicBool>,
    next_id: AtomicU64,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// Taken when the process is stopped.
    child: Mutex<Option<Child>>,
}

impl RpcProcess {
    /// Starts `program` in `cwd` with `env`, returning the process and the
    /// stream of messages it sends.
    pub fn spawn(
        program: &Path,
        args: &[String],
        cwd: &Path,
        env: &HashMap<String, String>,
    ) -> Result<(Arc<Self>, IncomingMessages)> {
        // Said plainly: the system reports a missing folder as the program
        // missing ("npx: No such file or directory").
        if !cwd.is_dir() {
            anyhow::bail!(
                "the folder it works in, {}, no longer exists",
                cwd.display()
            );
        }
        let mut command = ph_env::command(program);
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // A group of its own, so stopping it stops what it started too
        // (`npx` starts the adapter, which starts the agent's program).
        own_group(&mut command);
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", program.display()))?;

        let stdin = child.stdin.take().context("agent stdin")?;
        let stdout = child.stdout.take().context("agent stdout")?;
        let stderr = child.stderr.take().context("agent stderr")?;

        let (writer, write_queue) = smol::channel::unbounded::<String>();
        let (incoming_sender, incoming) = smol::channel::bounded::<Routed>(QUEUED_MESSAGES);
        let pending: Pending = Arc::default();
        let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::default();

        std::thread::Builder::new()
            .name("acp-write".into())
            .spawn(move || {
                let mut stdin = stdin;
                while let Ok(line) = write_queue.recv_blocking() {
                    if stdin
                        .write_all(line.as_bytes())
                        .and_then(|_| stdin.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;

        let closed: Arc<std::sync::atomic::AtomicBool> = Arc::default();
        let read_closed = closed.clone();
        let read_pending = pending.clone();
        std::thread::Builder::new()
            .name("acp-read".into())
            .spawn(move || {
                for line in ph_env::lossy_lines(stdout) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(&line) {
                        Ok(message) => route(message, &read_pending, &incoming_sender),
                        Err(err) => {
                            // Log a prefix only: a broken agent can send megabytes.
                            let preview: String = line.chars().take(200).collect();
                            tracing::warn!("agent sent invalid JSON ({err}): {preview}")
                        }
                    }
                }
                // Fail requests still waiting, so callers are not stuck forever.
                let mut waiting = read_pending.lock().unwrap_or_else(|e| e.into_inner());
                read_closed.store(true, Ordering::Relaxed);
                let waiters: Vec<_> = waiting.drain().map(|(_, waiter)| waiter).collect();
                drop(waiting);
                // That it ended is said first, and in its place: after all
                // it sent, and before the requests fail (which says less).
                // Whoever stops taking messages there lets go of the queue,
                // and a request whose answer goes with it fails as well.
                let _ = incoming_sender.send_blocking(Routed::Message(Incoming::Closed));
                for waiter in waiters {
                    let failed = Err(RpcError {
                        code: -32000,
                        message: "the agent process exited".into(),
                    });
                    let _ = incoming_sender.send_blocking(Routed::Answer(waiter, failed));
                }
            })?;

        let tail = stderr_tail.clone();
        std::thread::Builder::new()
            .name("acp-stderr".into())
            .spawn(move || {
                for line in ph_env::lossy_lines(stderr) {
                    tracing::debug!(target: "ph_agent::stderr", "{line}");
                    let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                    if tail.len() == STDERR_TAIL_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            })?;

        let process = Arc::new(Self {
            writer,
            pending,
            closed,
            next_id: AtomicU64::new(1),
            stderr_tail,
            child: Mutex::new(Some(child)),
        });
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        running.retain(|known| known.strong_count() > 0);
        running.push(Arc::downgrade(&process));
        drop(running);
        Ok((process, IncomingMessages { queue: incoming }))
    }

    /// Sends a request; the future resolves with the agent's result.
    pub fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<Value>> + use<> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if self.closed.load(Ordering::Relaxed) {
                drop(pending);
                return futures::future::Either::Left(std::future::ready(Err(anyhow!(
                    "the agent process exited"
                ))));
            }
            pending.insert(id, sender);
        }
        let sent =
            self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        futures::future::Either::Right(async move {
            sent?;
            match receiver.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(err)) => Err(err.into()),
                Err(_) => Err(anyhow!("the agent process exited")),
            }
        })
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Answers a request the agent made.
    pub fn respond(&self, id: Value, result: std::result::Result<Value, RpcError>) -> Result<()> {
        let message = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": error.code, "message": error.message },
            }),
        };
        self.send(message)
    }

    /// The last lines the agent wrote to stderr, oldest first.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    fn send(&self, message: Value) -> Result<()> {
        let mut line = serde_json::to_string(&message)?;
        line.push('\n');
        self.writer
            .send_blocking(line)
            .map_err(|_| anyhow!("the agent process exited"))
    }
}

impl Drop for RpcProcess {
    fn drop(&mut self) {
        // Agents are children of the app; never leave them running.
        self.stop();
    }
}

impl RpcProcess {
    /// Ends the agent and everything it started, now (its output then
    /// closes, and waiting requests fail).
    pub fn stop(&self) {
        self.end(false);
    }

    /// [`Self::stop`]; `exiting` when the app is about to be gone, and
    /// nothing may be left to a thread that would be gone with it.
    fn end(&self, exiting: bool) {
        let Some(child) = self.child.lock().ok().and_then(|mut child| child.take()) else {
            return;
        };
        #[cfg(unix)]
        {
            let _ = exiting;
            kill_tree(child);
        }
        // It can take a while: on a thread of its own, not whichever one
        // let the agent go. When the app quits, `taskkill` is only started:
        // it is a program of its own and ends the tree without the app.
        #[cfg(windows)]
        if exiting {
            let _ = taskkill(child.id()).spawn();
        } else {
            std::thread::spawn(move || kill_tree(child));
        }
    }
}

/// Makes `command` start a process group of its own, so that
/// [`kill_tree`] ends what the program started too.
pub(crate) fn own_group(command: &mut std::process::Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(not(unix))]
    let _ = command;
}

/// Ends `child` (started with [`own_group`]) and everything it started,
/// and waits until it is gone. On Windows that takes a moment.
pub(crate) fn kill_tree(mut child: Child) {
    // The whole group: its children too.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    // The whole tree. `taskkill /T` finds it from the process at its top,
    // so that one is killed only after (or the tree is left running: an
    // agent's adapter, its MCP server, a copy of PilotHouse that keeps its
    // program file open).
    #[cfg(windows)]
    let _ = taskkill(child.id()).status();
    let _ = child.kill();
    let _ = child.wait();
}

/// `taskkill` for the process `pid` and all it started.
#[cfg(windows)]
fn taskkill(pid: u32) -> std::process::Command {
    let mut command = ph_env::command("taskkill");
    command
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// `message[key]`, taken out of it, not copied: an update can carry a whole
/// file or a picture.
fn taken(message: &mut Value, key: &str) -> Value {
    message.get_mut(key).map(Value::take).unwrap_or(Value::Null)
}

/// Sorts one message: our response, the agent's request, or a notification.
/// Each goes into the queue, waiting while it is full.
fn route(mut message: Value, pending: &Pending, incoming: &smol::channel::Sender<Routed>) {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = message.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            let params = taken(&mut message, "params");
            let _ =
                incoming.send_blocking(Routed::Message(Incoming::Request { id, method, params }));
        }
        // Read into its type here, on the reader's thread: the UI's only
        // applies it.
        (Some(method), None) if method == "session/update" => {
            let params = taken(&mut message, "params");
            if let Some(update) = crate::transcript::parse_update(params) {
                let _ = incoming.send_blocking(Routed::Message(Incoming::Update(Box::new(update))));
            }
        }
        (Some(method), None) => {
            let params = taken(&mut message, "params");
            let _ =
                incoming.send_blocking(Routed::Message(Incoming::Notification { method, params }));
        }
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let Some(waiter) = pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id)
            else {
                return;
            };
            let result = match message.get("error") {
                Some(error) => Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                    message: error_message(error),
                }),
                None => Ok(taken(&mut message, "result")),
            };
            // Nobody takes messages any more: the request fails as its
            // waiter goes with what could not be queued.
            let _ = incoming.send_blocking(Routed::Answer(waiter, result));
        }
        (None, None) => tracing::warn!("agent sent a message with neither method nor id"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_carry_the_agents_reason() {
        let error = serde_json::json!({ "code": -32603, "message": "Internal error",
            "data": { "details": "cline requires re-authentication." } });
        assert_eq!(
            error_message(&error),
            "Internal error: cline requires re-authentication."
        );
        let plain = serde_json::json!({ "code": -32000, "message": "Authentication required" });
        assert_eq!(error_message(&plain), "Authentication required");
        let repeated = serde_json::json!({ "message": "Bad input", "data": "Bad input" });
        assert_eq!(error_message(&repeated), "Bad input");
    }

    const UPDATE: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#;

    /// An answer is handed to whoever asked in its place among what the
    /// agent sent: not before the messages ahead of it were taken, and
    /// before those after it are.
    #[test]
    fn answers_keep_their_place_among_the_messages() {
        let (sender, queue) = smol::channel::bounded(8);
        let incoming = IncomingMessages { queue };
        let pending: Pending = Arc::default();
        let (waiter, mut asked) = oneshot::channel();
        pending.lock().unwrap().insert(1, waiter);
        let update: Value = serde_json::from_str(UPDATE).unwrap();
        route(update.clone(), &pending, &sender);
        route(
            json!({ "jsonrpc": "2.0", "id": 1, "result": { "ok": true } }),
            &pending,
            &sender,
        );
        route(update, &pending, &sender);
        // An update this build does not know is left out, here already.
        route(
            json!({ "method": "session/update", "params": { "x": 1 } }),
            &pending,
            &sender,
        );

        assert!(
            matches!(incoming.next_now(), Some(Incoming::Update(update)) if update.session_id == "s1")
        );
        assert!(matches!(asked.try_recv(), Ok(None)), "answered too early");
        // The answer: handed over, and nothing more is taken in this go.
        assert!(incoming.next_now().is_none());
        assert_eq!(asked.try_recv().unwrap().unwrap().unwrap()["ok"], true);
        assert!(matches!(incoming.next_now(), Some(Incoming::Update(_))));
        assert!(incoming.next_now().is_none());

        // Nobody takes messages any more: an answer queued already still
        // reaches its request, and one that comes later fails it.
        let (waiter, mut queued) = oneshot::channel();
        let (late_waiter, mut late) = oneshot::channel();
        let mut waiting = pending.lock().unwrap();
        waiting.insert(2, waiter);
        waiting.insert(3, late_waiter);
        drop(waiting);
        route(json!({ "id": 2, "result": 7 }), &pending, &sender);
        drop(incoming);
        assert_eq!(queued.try_recv().unwrap().unwrap().unwrap(), 7);
        route(json!({ "id": 3, "result": 8 }), &pending, &sender);
        assert!(late.try_recv().is_err());
    }

    /// A tiny "agent" in a shell script: answers the first request, then
    /// sends a notification and a request of its own.
    #[test]
    fn request_response_and_incoming_messages() {
        let answer = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let update = UPDATE;
        let ask = r#"{"jsonrpc":"2.0","id":"a","method":"session/request_permission","params":{}}"#;
        let (program, args) = if cfg!(windows) {
            let script = format!(
                "$null = [Console]::In.ReadLine(); Write-Output '{answer}'; Write-Output '{update}'; Write-Output '{ask}'; $null = [Console]::In.ReadLine()"
            );
            (
                ph_env::which("powershell").expect("powershell"),
                vec!["-NoProfile".to_string(), "-Command".to_string(), script],
            )
        } else {
            let script =
                format!("read line; echo '{answer}'; echo '{update}'; echo '{ask}'; read line");
            (
                std::path::PathBuf::from("/bin/sh"),
                vec!["-c".to_string(), script],
            )
        };
        let env: HashMap<String, String> = std::env::vars().collect();
        let (process, incoming) =
            RpcProcess::spawn(&program, &args, &std::env::temp_dir(), &env).unwrap();

        // The answer comes with the messages: it is handed over on the way
        // to the first of them.
        let request = process.request("initialize", json!({}));
        let (result, first) = smol::block_on(futures::future::join(request, incoming.next()));
        assert_eq!(result.unwrap()["ok"], true);
        match first.unwrap() {
            Incoming::Update(update) => assert_eq!(update.session_id, "s1"),
            other => panic!("expected an update, got {other:?}"),
        }
        match smol::block_on(incoming.next()).unwrap() {
            Incoming::Request { id, method, .. } => {
                assert_eq!(id, json!("a"));
                assert_eq!(method, "session/request_permission");
            }
            other => panic!("expected a request, got {other:?}"),
        }
    }
}
