//! A minimal Chrome DevTools Protocol client.
//!
//! Two transports carry it:
//!
//! - **Chrome's debugging pipe** ([`Cdp::connect`], the Browser pane's
//!   headless Chrome): messages are JSON, each ended by a NUL byte. A reader
//!   thread blocks on the pipe (no polling, so an idle browser costs no CPU).
//! - **The Chrome extension** ([`Cdp::with_sink`], agents' Chrome): messages
//!   go out through a sink, and the extension bridge hands what comes back to
//!   the [`CdpInbox`].
//!
//! Writes go through the sink from any thread. The connection is to the
//! browser; page commands carry the page's `sessionId` (see
//! [`Cdp::for_session`]).

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

type Replies = Arc<Mutex<HashMap<u64, smol::channel::Sender<Result<Value>>>>>;

/// Sends one message (a JSON object's text) on the transport.
pub type Sink = Arc<dyn Fn(String) -> std::io::Result<()> + Send + Sync>;

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
    sink: Sink,
    replies: Replies,
    next_id: Arc<AtomicU64>,
    session: Option<Arc<str>>,
}

/// Where a transport delivers what the browser sends: answers go to the
/// calls waiting for them, events to the event channel.
pub struct CdpInbox {
    replies: Replies,
    events: smol::channel::Sender<CdpEvent>,
}

impl CdpInbox {
    /// Routes one message. False once nobody listens for events any more.
    pub fn deliver(&self, message: Value) -> bool {
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            let Some(reply) = self.replies.lock().expect("cdp replies").remove(&id) else {
                return true;
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
            return true;
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return true;
        };
        let event = CdpEvent {
            method: method.to_string(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
            session: message
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        self.events.send_blocking(event).is_ok()
    }

    /// The transport closed: everything still waiting fails, so callers do
    /// not hang, and the event channel ends.
    pub fn close(&self) {
        for (_, reply) in self.replies.lock().expect("cdp replies").drain() {
            let _ = reply.try_send(Err(anyhow!("the browser closed")));
        }
        self.events.close();
    }
}

impl Cdp {
    /// Starts talking over Chrome's pipe. Events arrive on the returned
    /// channel, which closes when Chrome exits.
    pub fn connect(
        reader: std::io::PipeReader,
        writer: std::io::PipeWriter,
    ) -> Result<(Self, smol::channel::Receiver<CdpEvent>)> {
        let writer = Mutex::new(writer);
        let sink: Sink = Arc::new(move |text: String| {
            let mut bytes = text.into_bytes();
            bytes.push(0);
            writer
                .lock()
                .map_err(|_| std::io::Error::other("cdp writer poisoned"))?
                .write_all(&bytes)
        });
        let (cdp, inbox, events) = Self::with_sink(sink);
        std::thread::Builder::new()
            .name("cdp-reader".into())
            .spawn(move || read_loop(reader, inbox))?;
        Ok((cdp, events))
    }

    /// A client that sends through `sink`; the transport delivers what comes
    /// back to the returned inbox.
    pub fn with_sink(sink: Sink) -> (Self, CdpInbox, smol::channel::Receiver<CdpEvent>) {
        let replies: Replies = Arc::default();
        let (events, receiver) = smol::channel::unbounded();
        (
            Self {
                sink,
                replies: replies.clone(),
                next_id: Arc::new(AtomicU64::new(1)),
                session: None,
            },
            CdpInbox { replies, events },
            receiver,
        )
    }

    /// The same connection, sending to one target's session.
    pub fn for_session(&self, session: &str) -> Self {
        Self {
            session: Some(session.into()),
            ..self.clone()
        }
    }

    /// The same connection, at the browser rather than a page.
    pub fn browser(&self) -> Self {
        Self {
            session: None,
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
        (self.sink)(message.to_string()).map_err(|err| {
            self.replies.lock().expect("cdp replies").remove(&id);
            anyhow!("sending {method}: {err}")
        })?;
        Ok(id)
    }
}

fn read_loop(reader: std::io::PipeReader, inbox: CdpInbox) {
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
        if !inbox.deliver(message) {
            break;
        }
    }
    inbox.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sink_carries_calls_and_the_inbox_answers_them() {
        let sent: Arc<Mutex<Vec<Value>>> = Arc::default();
        let log = sent.clone();
        let sink: Sink = Arc::new(move |text| {
            log.lock()
                .unwrap()
                .push(serde_json::from_str(&text).unwrap());
            Ok(())
        });
        let (cdp, inbox, events) = Cdp::with_sink(sink);
        let page = cdp.for_session("42");
        let call =
            smol::spawn(async move { page.call("Page.navigate", json!({ "url": "x" })).await });
        // Wait until the call went out, then answer it.
        let id = loop {
            if let Some(message) = sent.lock().unwrap().first().cloned() {
                assert_eq!(message["sessionId"], "42");
                assert_eq!(message["method"], "Page.navigate");
                break message["id"].as_u64().unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(inbox.deliver(json!({ "id": id, "result": { "frameId": "f" } })));
        assert_eq!(smol::block_on(call).unwrap()["frameId"], "f");
        assert!(
            inbox.deliver(
                json!({ "method": "Page.loadEventFired", "params": {}, "sessionId": "42" })
            )
        );
        let event = smol::block_on(events.recv()).unwrap();
        assert_eq!(
            (event.method.as_str(), event.session.as_deref()),
            ("Page.loadEventFired", Some("42"))
        );
        // Closing fails what still waits.
        let waiting = smol::spawn({
            let cdp = cdp.clone();
            async move { cdp.call("Runtime.evaluate", json!({})).await }
        });
        while sent.lock().unwrap().len() < 2 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        inbox.close();
        assert!(smol::block_on(waiting).is_err());
    }
}
