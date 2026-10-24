//! JSON-RPC over a language server's stdin/stdout, with `Content-Length`
//! framing. A reader thread answers the server's own requests (e.g.
//! `workspace/configuration`) and routes replies and notifications.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow};
use serde_json::{Value, json};

type Pending = Arc<Mutex<HashMap<i64, smol::channel::Sender<Result<Value>>>>>;

pub struct Transport {
    stdin: Arc<Mutex<ChildStdin>>,
    child: Mutex<Child>,
    next_id: AtomicI64,
    pending: Pending,
    /// Server notifications: `(method, params)`.
    pub notifications: smol::channel::Receiver<(String, Value)>,
}

impl Transport {
    pub fn spawn(program: &Path, args: &[&str], cwd: &Path) -> Result<Arc<Self>> {
        let mut child = ph_env::command(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not start {}", program.display()))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().context("no stdin")?));
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let pending: Pending = Arc::default();
        let (sender, notifications) = smol::channel::unbounded();

        let (reader_pending, reader_stdin) = (pending.clone(), stdin.clone());
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(message) = read_message(&mut reader) {
                route(message, &reader_pending, &reader_stdin, &sender);
            }
            // The server is gone: fail what is still waiting.
            for (_, waiter) in reader_pending.lock().unwrap().drain() {
                let _ = waiter.try_send(Err(anyhow!("the language server stopped")));
            }
        });
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                tracing::debug!("language server: {line}");
            }
        });
        Ok(Arc::new(Self {
            stdin,
            child: Mutex::new(child),
            next_id: AtomicI64::new(1),
            pending,
            notifications,
        }))
    }

    /// Sends a request now and returns where its reply will arrive, so
    /// callers can queue requests in order and wait elsewhere.
    pub fn request_channel(
        &self,
        method: &str,
        params: Value,
    ) -> Result<smol::channel::Receiver<Result<Value>>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = smol::channel::bounded(1);
        self.pending.lock().unwrap().insert(id, sender);
        if let Err(err) = write(
            &self.stdin,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        ) {
            self.pending.lock().unwrap().remove(&id);
            return Err(err);
        }
        Ok(receiver)
    }

    /// Sends a request; the reply arrives through the returned future.
    pub fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value>> + use<> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = smol::channel::bounded(1);
        self.pending.lock().unwrap().insert(id, sender);
        let sent = write(
            &self.stdin,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        );
        let pending = self.pending.clone();
        async move {
            if let Err(err) = sent {
                pending.lock().unwrap().remove(&id);
                return Err(err);
            }
            receiver
                .recv()
                .await
                .map_err(|_| anyhow!("the language server stopped"))?
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        write(
            &self.stdin,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
    }

    pub fn kill(&self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.kill();
    }
}

fn write(stdin: &Mutex<ChildStdin>, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message)?;
    let mut stdin = stdin.lock().unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
    stdin.write_all(&body)?;
    stdin.flush()?;
    Ok(())
}

/// Reads one framed message; `None` at end of stream.
fn read_message(reader: &mut impl std::io::BufRead) -> Option<Value> {
    loop {
        let mut length = None;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).ok()? == 0 {
                return None;
            }
            let header = header.trim();
            if header.is_empty() {
                break;
            }
            if let Some(value) = header.strip_prefix("Content-Length:") {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let Some(length) = length else { continue };
        let mut body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
        match serde_json::from_slice(&body) {
            Ok(message) => return Some(message),
            Err(err) => tracing::warn!("bad message from a language server: {err}"),
        }
    }
}

fn route(
    message: Value,
    pending: &Pending,
    stdin: &Mutex<ChildStdin>,
    notifications: &smol::channel::Sender<(String, Value)>,
) {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    match (method, message.get("id").cloned()) {
        // A reply to us.
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            if let Some(waiter) = pending.lock().unwrap().remove(&id) {
                let result = match message.get("error") {
                    Some(error) => Err(anyhow!(
                        "{}",
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("language server error")
                    )),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiter.try_send(result);
            }
        }
        // The server asks us something: answer with defaults.
        (Some(method), Some(id)) => {
            let result = match method.as_str() {
                "workspace/configuration" => {
                    let items = message
                        .pointer("/params/items")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    Value::Array(vec![Value::Null; items])
                }
                "workspace/workspaceFolders" => Value::Null,
                _ => Value::Null,
            };
            let _ = write(
                stdin,
                &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            );
        }
        (Some(method), None) => {
            let _ = notifications.try_send((
                method,
                message.get("params").cloned().unwrap_or(Value::Null),
            ));
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_framed_messages() {
        let one = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let two = r#"{"jsonrpc":"2.0","method":"x","params":{}}"#;
        let stream = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{one}Content-Length: {}\r\n\r\n{two}",
            one.len(),
            two.len()
        );
        let mut reader = std::io::Cursor::new(stream.into_bytes());
        assert_eq!(read_message(&mut reader).unwrap()["id"], 1);
        assert_eq!(read_message(&mut reader).unwrap()["method"], "x");
        assert!(read_message(&mut reader).is_none());
    }
}
