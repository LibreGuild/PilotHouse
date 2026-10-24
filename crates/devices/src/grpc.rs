//! A minimal gRPC client over HTTP/2 cleartext, for the emulator's local
//! controller endpoint.
//!
//! Only what the emulator needs: unary and server-streaming calls with small
//! requests. Requests use uncompressed HPACK literals; response headers are
//! never decoded (a call that ends without a message is reported as failed),
//! so no HPACK state is kept. One reader thread blocks on the socket; frames
//! are written under a mutex. Received data is credited back right away, so
//! a long screen stream never stalls on flow control.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow};

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const DATA: u8 = 0x0;
const HEADERS: u8 = 0x1;
const RST_STREAM: u8 = 0x3;
const SETTINGS: u8 = 0x4;
const PING: u8 = 0x6;
const GOAWAY: u8 = 0x7;
const WINDOW_UPDATE: u8 = 0x8;
const END_STREAM: u8 = 0x1;
const ACK: u8 = 0x1;
const END_HEADERS: u8 = 0x4;
/// Largest flow-control window HTTP/2 allows.
const MAX_WINDOW: u32 = 0x7fff_ffff;
/// Received bytes are credited back in batches of this size.
const CREDIT_BATCH: u32 = 8 << 20;

/// Largest gRPC message accepted (a full-resolution RGBA frame is ~10 MB).
const MAX_MESSAGE: usize = 64 << 20;

/// One received gRPC message. It keeps the 5-byte gRPC prefix in its buffer,
/// so a whole message can be handed over without copying its bytes.
pub struct GrpcMessage(Vec<u8>);

impl GrpcMessage {
    pub fn bytes(&self) -> &[u8] {
        &self.0[5..]
    }
}

/// Messages of one call, then `None` when it ends.
type StreamSender = smol::channel::Sender<Option<GrpcMessage>>;

struct Call {
    sender: StreamSender,
    buffer: Vec<u8>,
    /// Bytes received on this stream and not yet credited back.
    uncredited: u32,
}

#[derive(Clone)]
pub struct GrpcClient {
    writer: Arc<Mutex<TcpStream>>,
    calls: Arc<Mutex<HashMap<u32, Call>>>,
    next_stream: Arc<AtomicU32>,
    /// Bytes received on the connection and not yet credited back.
    uncredited: Arc<AtomicU32>,
    authority: String,
    token: Option<String>,
}

impl GrpcClient {
    pub fn connect(port: u16, token: Option<String>) -> Result<Self> {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .with_context(|| format!("connecting to the emulator on port {port}"))?;
        stream.set_nodelay(true)?;
        stream.write_all(PREFACE)?;
        // Initial stream window: the maximum, so big frames flow freely.
        let mut settings = Vec::new();
        settings.extend(0x4u16.to_be_bytes());
        settings.extend(MAX_WINDOW.to_be_bytes());
        write_frame(&mut stream, SETTINGS, 0, 0, &settings)?;
        write_frame(
            &mut stream,
            WINDOW_UPDATE,
            0,
            0,
            &(MAX_WINDOW - 65_535).to_be_bytes(),
        )?;

        let reader = stream.try_clone()?;
        let client = Self {
            writer: Arc::new(Mutex::new(stream)),
            calls: Arc::default(),
            next_stream: Arc::new(AtomicU32::new(1)),
            uncredited: Arc::default(),
            authority: format!("127.0.0.1:{port}"),
            token,
        };
        let thread_client = client.clone();
        std::thread::Builder::new()
            .name("emulator-grpc".into())
            .spawn(move || thread_client.read_loop(reader))?;
        Ok(client)
    }

    /// Starts a call; its messages arrive on the returned channel.
    pub fn start(
        &self,
        path: &str,
        message: &[u8],
    ) -> Result<smol::channel::Receiver<Option<GrpcMessage>>> {
        let (sender, receiver) = smol::channel::unbounded();
        let id = self.next_stream.fetch_add(2, Ordering::Relaxed);
        self.calls.lock().expect("grpc calls").insert(
            id,
            Call {
                sender,
                buffer: Vec::new(),
                uncredited: 0,
            },
        );
        let mut headers = Vec::new();
        for (name, value) in [
            (":method", "POST"),
            (":scheme", "http"),
            (":path", path),
            (":authority", &self.authority),
            ("content-type", "application/grpc"),
            ("te", "trailers"),
        ] {
            literal_header(&mut headers, name, value);
        }
        if let Some(token) = &self.token {
            literal_header(&mut headers, "authorization", &format!("Bearer {token}"));
        }
        let mut body = vec![0];
        body.extend((message.len() as u32).to_be_bytes());
        body.extend(message);

        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("grpc writer poisoned"))?;
        write_frame(&mut writer, HEADERS, END_HEADERS, id, &headers)?;
        write_frame(&mut writer, DATA, END_STREAM, id, &body)?;
        Ok(receiver)
    }

    /// A unary call: waits for its one reply (half a minute at most).
    pub fn unary(&self, path: &str, message: &[u8]) -> Result<GrpcMessage> {
        let replies = self.start(path, message)?;
        let reply = smol::block_on(smol::future::or(
            async { Some(replies.recv().await) },
            async {
                smol::Timer::after(std::time::Duration::from_secs(30)).await;
                None
            },
        ));
        match reply {
            Some(Ok(Some(reply))) => Ok(reply),
            Some(_) => Err(anyhow!("the emulator refused {path}")),
            None => Err(anyhow!("the emulator did not answer {path}")),
        }
    }

    fn read_loop(&self, mut stream: TcpStream) {
        // One buffer for every frame's payload, reused.
        let mut payload = Vec::new();
        let result: Result<()> = (|| loop {
            let mut header = [0u8; 9];
            stream.read_exact(&mut header)?;
            let length = u32::from_be_bytes([0, header[0], header[1], header[2]]) as usize;
            let (kind, flags) = (header[3], header[4]);
            let id = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) & MAX_WINDOW;
            payload.resize(length, 0);
            stream.read_exact(&mut payload)?;
            match kind {
                DATA => self.receive_data(id, flags, &payload)?,
                HEADERS if flags & END_STREAM != 0 => self.finish(id),
                RST_STREAM => self.finish(id),
                SETTINGS if flags & ACK == 0 => self.write(SETTINGS, ACK, 0, &[])?,
                PING if flags & ACK == 0 => self.write(PING, ACK, 0, &payload)?,
                GOAWAY => return Ok(()),
                _ => {}
            }
        })();
        if let Err(err) = result {
            tracing::debug!("emulator gRPC connection closed: {err:#}");
        }
        for (_, call) in self.calls.lock().expect("grpc calls").drain() {
            let _ = call.sender.try_send(None);
        }
    }

    fn receive_data(&self, id: u32, flags: u8, payload: &[u8]) -> Result<()> {
        let mut padding = 0;
        let mut data = payload;
        if flags & 0x8 != 0 {
            padding = *data.first().unwrap_or(&0) as usize;
            data = data.get(1..).unwrap_or_default();
        }
        let data = &data[..data.len().saturating_sub(padding)];
        let received = payload.len() as u32;
        let connection = self.uncredited.fetch_add(received, Ordering::Relaxed) + received;
        if connection >= CREDIT_BATCH {
            self.uncredited.store(0, Ordering::Relaxed);
            self.write(WINDOW_UPDATE, 0, 0, &connection.to_be_bytes())?;
        }
        let mut calls = self.calls.lock().expect("grpc calls");
        if let Some(call) = calls.get_mut(&id) {
            call.uncredited += received;
            if call.uncredited >= CREDIT_BATCH && flags & END_STREAM == 0 {
                let credit = std::mem::take(&mut call.uncredited);
                self.write(WINDOW_UPDATE, 0, id, &credit.to_be_bytes())?;
            }
            call.buffer.extend_from_slice(data);
            // Each gRPC message: compressed flag, u32 length, bytes.
            while call.buffer.len() >= 5 {
                let length = u32::from_be_bytes([
                    call.buffer[1],
                    call.buffer[2],
                    call.buffer[3],
                    call.buffer[4],
                ]) as usize;
                if length > MAX_MESSAGE {
                    return Err(anyhow!("the emulator sent a {length}-byte message"));
                }
                let end = 5 + length;
                if call.buffer.len() < end {
                    // Grow once to the full size instead of doubling as data arrives.
                    call.buffer.reserve_exact(end - call.buffer.len());
                    break;
                }
                // Hand the buffer over whole; only bytes of a following message move.
                let message = if call.buffer.len() == end {
                    std::mem::take(&mut call.buffer)
                } else {
                    let rest = call.buffer.split_off(end);
                    std::mem::replace(&mut call.buffer, rest)
                };
                if call.sender.try_send(Some(GrpcMessage(message))).is_err() {
                    break;
                }
            }
        }
        drop(calls);
        if flags & END_STREAM != 0 {
            self.finish(id);
        }
        Ok(())
    }

    /// Closes the connection: the reader thread ends and every open call
    /// gets its end, so nothing waits on a dead emulator.
    pub fn shutdown(&self) {
        if let Ok(writer) = self.writer.lock() {
            let _ = writer.shutdown(std::net::Shutdown::Both);
        }
    }

    fn finish(&self, id: u32) {
        if let Some(call) = self.calls.lock().expect("grpc calls").remove(&id) {
            let _ = call.sender.try_send(None);
        }
    }

    fn write(&self, kind: u8, flags: u8, id: u32, payload: &[u8]) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("grpc writer poisoned"))?;
        write_frame(&mut writer, kind, flags, id, payload)
    }
}

fn write_frame(stream: &mut TcpStream, kind: u8, flags: u8, id: u32, payload: &[u8]) -> Result<()> {
    let length = (payload.len() as u32).to_be_bytes();
    let mut frame = Vec::with_capacity(9 + payload.len());
    frame.extend(&length[1..]);
    frame.push(kind);
    frame.push(flags);
    frame.extend(id.to_be_bytes());
    frame.extend(payload);
    stream.write_all(&frame)?;
    Ok(())
}

/// HPACK "literal header field without indexing, new name", no Huffman.
fn literal_header(out: &mut Vec<u8>, name: &str, value: &str) {
    out.push(0);
    hpack_string(out, name);
    hpack_string(out, value);
}

fn hpack_string(out: &mut Vec<u8>, text: &str) {
    // A 7-bit prefixed integer for the length.
    let mut length = text.len();
    if length < 127 {
        out.push(length as u8);
    } else {
        out.push(127);
        length -= 127;
        while length >= 128 {
            out.push((length % 128 + 128) as u8);
            length /= 128;
        }
        out.push(length as u8);
    }
    out.extend(text.as_bytes());
}

/// Protocol Buffers encoding helpers for the few messages PilotHouse sends.
pub mod proto {
    pub fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    /// An integer field (proto3 skips zero values).
    pub fn int(out: &mut Vec<u8>, field: u32, value: i64) {
        if value != 0 {
            varint(out, u64::from(field << 3));
            varint(out, value as u64);
        }
    }

    pub fn bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
        varint(out, u64::from(field << 3 | 2));
        varint(out, value.len() as u64);
        out.extend(value);
    }

    /// A decoded field: number and either an integer or bytes.
    pub enum Value<'a> {
        Int(u64),
        Bytes(&'a [u8]),
    }

    /// Splits a message into its fields (skipping fixed-width ones).
    pub fn fields(mut data: &[u8]) -> Vec<(u32, Value<'_>)> {
        let mut fields = Vec::new();
        while !data.is_empty() {
            let Some(key) = read_varint(&mut data) else {
                break;
            };
            let field = (key >> 3) as u32;
            match key & 7 {
                0 => match read_varint(&mut data) {
                    Some(value) => fields.push((field, Value::Int(value))),
                    None => break,
                },
                1 => data = data.get(8..).unwrap_or_default(),
                2 => {
                    let Some(length) = read_varint(&mut data) else {
                        break;
                    };
                    let Some(value) = data.get(..length as usize) else {
                        break;
                    };
                    fields.push((field, Value::Bytes(value)));
                    data = &data[length as usize..];
                }
                5 => data = data.get(4..).unwrap_or_default(),
                _ => break,
            }
        }
        fields
    }

    fn read_varint(data: &mut &[u8]) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = data.split_first()?;
            *data = rest;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::proto;

    #[test]
    fn protobuf_round_trip() {
        let mut inner = Vec::new();
        proto::int(&mut inner, 3, 540);
        proto::int(&mut inner, 4, 1200);
        let mut message = Vec::new();
        proto::bytes(&mut message, 1, &inner);
        proto::bytes(&mut message, 4, &[9, 9, 9]);
        proto::int(&mut message, 5, 7);
        let fields = proto::fields(&message);
        assert_eq!(fields.len(), 3);
        let proto::Value::Bytes(format) = fields[0].1 else {
            panic!()
        };
        let sizes: Vec<u64> = proto::fields(format)
            .into_iter()
            .filter_map(|(_, value)| match value {
                proto::Value::Int(value) => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![540, 1200]);
        assert!(matches!(fields[1].1, proto::Value::Bytes(&[9, 9, 9])));
    }
}
