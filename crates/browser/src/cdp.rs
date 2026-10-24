//! A minimal Chrome DevTools Protocol client over Chrome's debugging pipe.
//!
//! Messages are JSON, each ended by a NUL byte. A reader thread blocks on
//! the pipe (no polling, so an idle browser costs no CPU) and writes go
//! through a mutex from any thread. The connection is to the browser; page
//! commands carry the page's `sessionId` (see [`Cdp::for_session`]).

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

type Replies = Arc<Mutex<HashMap<u64, smol::channel::Sender<Result<Value>>>>>;

/// The longest a command may take.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// A DevTools event: `Page.loadEventFired`, `Runtime.consoleAPICalled`…
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    /// The page session it came from; `None` for browser-level events.
    pub session: Option<String>,
}

#[derive(Clone)]
pub struct Cdp {
    writer: Arc<Mutex<std::io::PipeWriter>>,
    replies: Replies,
    next_id: Arc<AtomicU64>,
    session: Option<Arc<str>>,
}

impl Cdp {
    /// Starts talking over Chrome's pipe. Events arrive on the returned
    /// channel, which closes when Chrome exits.
    pub fn connect(
        reader: std::io::PipeReader,
        writer: std::io::PipeWriter,
    ) -> Result<(Self, smol::channel::Receiver<CdpEvent>)> {
        let replies: Replies = Arc::default();
        let (events, receiver) = smol::channel::unbounded();
        let thread_replies = replies.clone();
        std::thread::Builder::new()
            .name("cdp-reader".into())
            .spawn(move || read_loop(reader, thread_replies, events))?;
        Ok((
            Self {
                writer: Arc::new(Mutex::new(writer)),
                replies,
                next_id: Arc::new(AtomicU64::new(1)),
                session: None,
            },
            receiver,
        ))
    }

    /// The same connection, sending to one target's session.
    pub fn for_session(&self, session: &str) -> Self {
        Self {
            session: Some(session.into()),
            ..self.clone()
        }
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Sends a command and waits for its result, for at most [`CALL_TIMEOUT`]:
    /// a page that never settles (a script awaiting a promise that never
    /// resolves) must not hold up the agent asking.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let (reply, answer) = smol::channel::bounded(1);
        let id = self.send(method, params, Some(reply))?;
        let answer = async { Some(answer.recv().await) };
        let late = async {
            smol::Timer::after(CALL_TIMEOUT).await;
            None
        };
        match smol::future::or(answer, late).await {
            Some(answer) => answer.map_err(|_| anyhow!("the browser closed"))?,
            None => {
                self.replies.lock().expect("cdp replies").remove(&id);
                Err(anyhow!(
                    "the browser did not answer {method} within {} seconds",
                    CALL_TIMEOUT.as_secs()
                ))
            }
        }
    }

    /// Sends a command without waiting (input events, frame acks).
    pub fn fire(&self, method: &str, params: Value) {
        if let Err(err) = self.send(method, params, None) {
            tracing::debug!("cdp {method}: {err:#}");
        }
    }

    fn send(
        &self,
        method: &str,
        params: Value,
        reply: Option<smol::channel::Sender<Result<Value>>>,
    ) -> Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(reply) = reply {
            self.replies.lock().expect("cdp replies").insert(id, reply);
        }
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = &self.session {
            message["sessionId"] = json!(session.as_ref());
        }
        let mut bytes = message.to_string().into_bytes();
        bytes.push(0);
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("cdp writer poisoned"))?;
        writer.write_all(&bytes).map_err(|err| {
            self.replies.lock().expect("cdp replies").remove(&id);
            anyhow!("sending {method}: {err}")
        })?;
        Ok(id)
    }
}

fn read_loop(
    reader: std::io::PipeReader,
    replies: Replies,
    events: smol::channel::Sender<CdpEvent>,
) {
    let mut reader = BufReader::with_capacity(1 << 20, reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(0, &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        let Ok(message) = serde_json::from_slice::<Value>(&buffer) else {
            continue;
        };
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            let Some(reply) = replies.lock().expect("cdp replies").remove(&id) else {
                continue;
            };
            let result = match message.get("error") {
                Some(error) => Err(anyhow!(
                    "{}",
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("DevTools error")
                )),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = reply.try_send(result);
        } else if let Some(method) = message.get("method").and_then(Value::as_str) {
            let event = CdpEvent {
                method: method.to_string(),
                params: message.get("params").cloned().unwrap_or(Value::Null),
                session: message
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            };
            if events.send_blocking(event).is_err() {
                break;
            }
        }
    }
    // Fail everything still waiting, so callers do not hang.
    for (_, reply) in replies.lock().expect("cdp replies").drain() {
        let _ = reply.try_send(Err(anyhow!("the browser closed")));
    }
}
