//! The Android emulator's gRPC controller: a live screen stream and
//! real-time touch and keys, as Android Studio's embedded emulator uses.
//!
//! Each running emulator writes a discovery file with its gRPC port and
//! access token. PilotHouse reads it, so nothing needs configuring.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use ph_media::Frame;

use crate::grpc::{GrpcClient, GrpcMessage, proto};

const SERVICE: &str = "/android.emulation.control.EmulatorController";

/// Where emulators announce themselves, per platform.
fn discovery_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(home) = &home {
        dirs.push(home.join("Library/Caches/TemporaryItems/avd/running"));
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        dirs.push(PathBuf::from(runtime).join("avd/running"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Temp/avd/running"));
    }
    dirs
}

/// The gRPC port and token of the emulator with console port `console_port`
/// (the number in its serial, `emulator-5554`). An emulator that was killed
/// leaves its file behind, and a new one on the same port writes another, so
/// the newest file wins.
pub fn discover(console_port: u16) -> Option<(u16, Option<String>)> {
    let mut newest: Option<(std::time::SystemTime, u16, Option<String>)> = None;
    for dir in discovery_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "ini") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some((port, token)) = parse_discovery(&text, console_port) else {
                continue;
            };
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            if newest.as_ref().is_none_or(|(time, _, _)| modified > *time) {
                newest = Some((modified, port, token));
            }
        }
    }
    newest.map(|(_, port, token)| (port, token))
}

/// The gRPC port and token in a discovery file, if it is for `console_port`.
fn parse_discovery(text: &str, console_port: u16) -> Option<(u16, Option<String>)> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|value| value.trim().to_string())
    };
    if value("port.serial")?.parse::<u16>().ok()? != console_port {
        return None;
    }
    Some((value("grpc.port")?.parse().ok()?, value("grpc.token")))
}

/// A connection to one emulator's controller.
#[derive(Clone)]
pub struct EmulatorControl {
    client: GrpcClient,
    /// Input calls, sent one at a time in order. Separate calls could be
    /// handled out of order, and a lift that overtakes a move leaves the
    /// finger down.
    input: smol::channel::Sender<(&'static str, Vec<u8>)>,
    /// Closes the connection when the last copy is dropped.
    _closer: std::sync::Arc<Closer>,
}

struct Closer(GrpcClient);

impl Drop for Closer {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// How long a just-booted emulator may turn the controller away (its access
/// keys are set up a few seconds after Android reports it has booted).
const SETTLE_ATTEMPTS: u32 = 8;
const SETTLE_PAUSE: std::time::Duration = std::time::Duration::from_secs(1);

impl EmulatorControl {
    /// Connects like [`Self::connect`], trying again for a few seconds when
    /// the emulator has an endpoint but refuses (it is still settling). One
    /// without an endpoint fails at once.
    pub fn connect_patiently(serial: &str) -> Result<Self> {
        let mut attempt = 1;
        loop {
            match Self::connect(serial) {
                Ok(control) => return Ok(control),
                Err(err) if attempt < SETTLE_ATTEMPTS && Self::has_endpoint(serial) => {
                    tracing::debug!("emulator gRPC not ready (attempt {attempt}): {err:#}");
                    std::thread::sleep(SETTLE_PAUSE);
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn has_endpoint(serial: &str) -> bool {
        serial
            .strip_prefix("emulator-")
            .and_then(|port| port.parse().ok())
            .and_then(discover)
            .is_some()
    }

    /// Connects to the emulator behind `serial` (`emulator-5554`).
    pub fn connect(serial: &str) -> Result<Self> {
        let console_port: u16 = serial
            .strip_prefix("emulator-")
            .and_then(|port| port.parse().ok())
            .context("not an emulator serial")?;
        let (port, token) =
            discover(console_port).context("the emulator did not publish a gRPC endpoint")?;
        let client = GrpcClient::connect(port, token)?;
        let (input, queue) = smol::channel::unbounded::<(&'static str, Vec<u8>)>();
        let input_client = client.clone();
        std::thread::Builder::new()
            .name(format!("{serial}-input"))
            .spawn(move || {
                while let Ok((method, message)) = queue.recv_blocking() {
                    if let Err(err) = input_client.unary(&format!("{SERVICE}/{method}"), &message) {
                        tracing::debug!("emulator {method}: {err:#}");
                    }
                }
            })?;
        let control = Self {
            _closer: std::sync::Arc::new(Closer(client.clone())),
            client,
            input,
        };
        // Fail early if the endpoint refuses us (e.g. an access policy).
        control
            .screenshot(64, 64)
            .context("the emulator's gRPC endpoint refused PilotHouse")?;
        Ok(control)
    }

    /// Closes the connection now, even while other copies exist (a screen
    /// stream waiting for the next frame ends at once).
    pub fn close(&self) {
        self.client.shutdown();
    }

    /// One frame, scaled by the emulator to fit `max_width`×`max_height`.
    pub fn screenshot(&self, max_width: u32, max_height: u32) -> Result<Frame> {
        let reply = self.client.unary(
            &format!("{SERVICE}/getScreenshot"),
            &image_format(max_width, max_height),
        )?;
        decode_image(reply.bytes()).context("unexpected screenshot reply")
    }

    /// Frames as the screen changes, until the receiver is dropped.
    pub fn stream(
        &self,
        max_width: u32,
        max_height: u32,
    ) -> Result<smol::channel::Receiver<Option<GrpcMessage>>> {
        self.client.start(
            &format!("{SERVICE}/streamScreenshot"),
            &image_format(max_width, max_height),
        )
    }

    /// A touch at device pixels; `down` false lifts the finger.
    pub fn touch(&self, x: i32, y: i32, down: bool) {
        let mut touch = Vec::new();
        proto::int(&mut touch, 1, x as i64);
        proto::int(&mut touch, 2, y as i64);
        proto::int(&mut touch, 4, if down { 1024 } else { 0 });
        // NEVER_EXPIRE: the finger stays down until we lift it.
        proto::int(&mut touch, 7, 1);
        let mut event = Vec::new();
        proto::bytes(&mut event, 1, &touch);
        let _ = self.input.try_send(("sendTouch", event));
    }

    /// A key by its W3C name ("Enter", "GoBack", "GoHome", "AppSwitch"…).
    pub fn key(&self, key: &str) {
        let mut event = Vec::new();
        proto::int(&mut event, 2, 2); // keypress: down then up
        proto::bytes(&mut event, 4, key.as_bytes());
        let _ = self.input.try_send(("sendKey", event));
    }

    pub fn text(&self, text: &str) {
        let mut event = Vec::new();
        proto::bytes(&mut event, 5, text.as_bytes());
        let _ = self.input.try_send(("sendKey", event));
    }
}

/// `ImageFormat { format: RGB888, width, height }`: three bytes a pixel
/// means a quarter less to send and copy than RGBA.
fn image_format(width: u32, height: u32) -> Vec<u8> {
    let mut message = Vec::new();
    proto::int(&mut message, 1, 2);
    proto::int(&mut message, 3, width as i64);
    proto::int(&mut message, 4, height as i64);
    message
}

/// `Image { format { width, height }, image }` (RGB888) to a frame,
/// converted straight to the GPU's layout in one pass.
pub fn decode_image(message: &[u8]) -> Option<Frame> {
    let (mut width, mut height, mut pixels) = (0, 0, None);
    for (field, value) in proto::fields(message) {
        match (field, value) {
            (1, proto::Value::Bytes(format)) => {
                for (field, value) in proto::fields(format) {
                    match (field, value) {
                        (3, proto::Value::Int(value)) => width = value as u32,
                        (4, proto::Value::Int(value)) => height = value as u32,
                        _ => {}
                    }
                }
            }
            (4, proto::Value::Bytes(bytes)) => pixels = Some(bytes),
            _ => {}
        }
    }
    let pixels = pixels?;
    let size = (width as usize) * (height as usize);
    // RGB888 as asked; RGBA if an older emulator ignores the format.
    match pixels.len() {
        len if width > 0 && len == size * 3 => Some(Frame::from_rgb_slice(width, height, pixels)),
        len if width > 0 && len == size * 4 => Some(Frame::from_rgba_slice(width, height, pixels)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_files_match_the_console_port() {
        let file = "port.serial=5556\ngrpc.port=8556\ngrpc.token=abc\n";
        assert_eq!(
            parse_discovery(file, 5556),
            Some((8556, Some("abc".into())))
        );
        assert_eq!(parse_discovery(file, 5554), None);
        assert_eq!(parse_discovery("port.serial=5554\n", 5554), None, "no gRPC");
    }

    /// Needs a running emulator: `cargo test -p ph-devices -- --ignored`.
    #[test]
    #[ignore]
    fn streams_frames_from_a_running_emulator() {
        let started = std::time::Instant::now();
        let control = EmulatorControl::connect("emulator-5554").unwrap();
        eprintln!("connect {:?}", started.elapsed());
        let started = std::time::Instant::now();
        let frame = control.screenshot(540, 1200).unwrap();
        eprintln!(
            "screenshot {}x{} in {:?}",
            frame.width,
            frame.height,
            started.elapsed()
        );
        let frames = control.stream(540, 1200).unwrap();
        // Scroll the home screen so frames change.
        std::thread::spawn(|| {
            for _ in 0..3 {
                let _ = std::process::Command::new("adb")
                    .args([
                        "-s",
                        "emulator-5554",
                        "shell",
                        "input swipe 540 1600 540 600 600",
                    ])
                    .status();
            }
        });
        let start = std::time::Instant::now();
        let mut count = 0;
        while start.elapsed().as_secs() < 3 {
            match frames.recv_blocking() {
                Ok(Some(message)) => {
                    assert!(decode_image(message.bytes()).is_some());
                    count += 1;
                }
                _ => break,
            }
        }
        eprintln!("{count} frames in 3 s");
        assert!(count > 0);
    }
}
