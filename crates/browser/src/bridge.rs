//! The app's end of the Chrome extension's channel: a local endpoint
//! ([`ph_store::paths::chrome_bridge_path`]: an owner-only Unix socket, or a
//! named pipe only this user can open) where the native messaging host
//! (`pilothouse chrome-host`) connects. Each line is one JSON object. What
//! arrives is handed to the UI thread as [`BridgeEvent`]s; the writer sends
//! the app's messages back.

use std::io::{BufReader, Write};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;

use crate::extension::read_line;

/// Most one message from the extension may be (a full-page screenshot).
const MAX_MESSAGE: usize = 64 << 20;

pub(crate) type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

pub(crate) enum BridgeEvent {
    /// A host connected (Chrome started it for the extension).
    Opened { id: u64, writer: Writer },
    /// One message from the extension: a JSON object.
    Message { id: u64, message: Value },
    /// The host went away (Chrome closed it, or quit).
    Closed { id: u64 },
}

/// Starts listening; events arrive on the returned channel.
pub(crate) fn listen() -> Result<smol::channel::Receiver<BridgeEvent>> {
    let endpoint = ph_store::paths::chrome_bridge_path();
    if let Some(parent) = endpoint.parent() {
        ph_store::paths::ensure_private_dir(parent)?;
    }
    let mut listener = ph_control::transport::Listener::bind(&endpoint)?;
    let (sender, receiver) = smol::channel::unbounded();
    std::thread::Builder::new()
        .name("chrome-bridge".into())
        .spawn(move || {
            let mut next = 0u64;
            loop {
                let connection = match listener.accept() {
                    Ok(connection) => connection,
                    Err(err) => {
                        tracing::warn!("chrome bridge: accept failed: {err}");
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        continue;
                    }
                };
                next += 1;
                let id = next;
                let ph_control::transport::Connection { reader, writer } = connection;
                if sender
                    .send_blocking(BridgeEvent::Opened {
                        id,
                        writer: Arc::new(Mutex::new(writer)),
                    })
                    .is_err()
                {
                    return;
                }
                let sender = sender.clone();
                let _ = std::thread::Builder::new()
                    .name("chrome-bridge-conn".into())
                    .spawn(move || {
                        let mut reader = BufReader::with_capacity(1 << 20, reader);
                        while let Ok(Some(line)) = read_line(&mut reader, MAX_MESSAGE) {
                            let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                                continue;
                            };
                            if !message.is_object() {
                                continue;
                            }
                            if sender
                                .send_blocking(BridgeEvent::Message { id, message })
                                .is_err()
                            {
                                return;
                            }
                        }
                        let _ = sender.send_blocking(BridgeEvent::Closed { id });
                    });
            }
        })?;
    tracing::info!("chrome bridge listening at {}", endpoint.display());
    Ok(receiver)
}

/// Sends one message (a JSON object's text) to the extension.
pub(crate) fn send(writer: &Writer, text: String) -> std::io::Result<()> {
    let mut writer = writer
        .lock()
        .map_err(|_| std::io::Error::other("chrome bridge writer poisoned"))?;
    writer.write_all(text.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}
