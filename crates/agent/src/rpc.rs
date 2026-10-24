//! JSON-RPC over a child process's stdio, as ACP uses it.
//!
//! One message per line in each direction. Two background threads do the
//! blocking I/O (one reads stdout, one writes stdin) and a third drains stderr
//! into a short ring buffer, so a crashing agent's last words can be shown.
//! Responses are matched to requests by id; everything else the agent sends
//! (notifications and its own requests) is delivered as [`Incoming`].

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow};
use futures::channel::oneshot;
use serde_json::{Value, json};

/// Lines of stderr kept for error messages.
const STDERR_TAIL_LINES: usize = 40;

/// Something the agent sent that is not a response to us.
#[derive(Debug)]
pub enum Incoming {
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

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// A running agent process and its JSON-RPC plumbing.
pub struct RpcProcess {
    writer: smol::channel::Sender<String>,
    pending: Pending,
    next_id: AtomicU64,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    child: Mutex<Child>,
}

impl RpcProcess {
    /// Starts `program` in `cwd` with `env`, returning the process and the
    /// stream of messages it sends.
    pub fn spawn(
        program: &Path,
        args: &[String],
        cwd: &Path,
        env: &HashMap<String, String>,
    ) -> Result<(Arc<Self>, smol::channel::Receiver<Incoming>)> {
        let mut child = ph_env::command(program)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting {}", program.display()))?;

        let stdin = child.stdin.take().context("agent stdin")?;
        let stdout = child.stdout.take().context("agent stdout")?;
        let stderr = child.stderr.take().context("agent stderr")?;

        let (writer, write_queue) = smol::channel::unbounded::<String>();
        let (incoming_sender, incoming) = smol::channel::unbounded::<Incoming>();
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

        let read_pending = pending.clone();
        std::thread::Builder::new()
            .name("acp-read".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
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
                for (_, waiter) in read_pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .drain()
                {
                    let _ = waiter.send(Err(RpcError {
                        code: -32000,
                        message: "the agent process exited".into(),
                    }));
                }
                let _ = incoming_sender.send_blocking(Incoming::Closed);
            })?;

        let tail = stderr_tail.clone();
        std::thread::Builder::new()
            .name("acp-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
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
            next_id: AtomicU64::new(1),
            stderr_tail,
            child: Mutex::new(child),
        });
        Ok((process, incoming))
    }

    /// Sends a request; the future resolves with the agent's result.
    pub fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<Value>> + use<> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, sender);
        let sent =
            self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        async move {
            sent?;
            match receiver.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(err)) => Err(err.into()),
                Err(_) => Err(anyhow!("the agent process exited")),
            }
        }
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
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Sorts one message: our response, the agent's request, or a notification.
fn route(message: Value, pending: &Pending, incoming: &smol::channel::Sender<Incoming>) {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = message.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let _ = incoming.send_blocking(Incoming::Request { id, method, params });
        }
        (Some(method), None) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let _ = incoming.send_blocking(Incoming::Notification { method, params });
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
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string(),
                }),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = waiter.send(result);
        }
        (None, None) => tracing::warn!("agent sent a message with neither method nor id"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny "agent" in a shell script: answers the first request, then
    /// sends a notification and a request of its own.
    #[test]
    fn request_response_and_incoming_messages() {
        let answer = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let update = r#"{"jsonrpc":"2.0","method":"session/update","params":{"x":1}}"#;
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

        let result = smol::block_on(process.request("initialize", json!({}))).unwrap();
        assert_eq!(result["ok"], true);
        match smol::block_on(incoming.recv()).unwrap() {
            Incoming::Notification { method, .. } => assert_eq!(method, "session/update"),
            other => panic!("expected a notification, got {other:?}"),
        }
        match smol::block_on(incoming.recv()).unwrap() {
            Incoming::Request { id, method, .. } => {
                assert_eq!(id, json!("a"));
                assert_eq!(method, "session/request_permission");
            }
            other => panic!("expected a request, got {other:?}"),
        }
    }
}
