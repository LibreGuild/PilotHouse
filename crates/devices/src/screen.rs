//! A device's live screen and input, while its pane is open.
//!
//! - Emulators: frames stream over the emulator's gRPC endpoint (tens of
//!   frames per second, only when the screen changes) and touches are sent
//!   in real time, so dragging and scrolling feel native.
//! - Phones (and emulators without gRPC): frames are polled with
//!   `screencap`, and gestures are sent as `input tap`/`swipe` through one
//!   long-lived `adb shell`, so each event costs no process start.

use std::io::Write as _;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, EventEmitter, Keystroke, Task};
use ph_media::{Frame, PointerKind, SurfacePointer};

use crate::adb::{self, Adb};
use crate::emulator::{self, EmulatorControl};

/// Largest frame requested from an emulator (it scales to fit). Enough to
/// fill a tall pane on a Retina screen; full resolution would double the
/// pixels to copy for no visible gain. Agents' screenshots use full size.
const STREAM_SIZE: (u32, u32) = (640, 1280);
/// Redraws for a moving screen are capped at about 30 a second: smooth to
/// the eye, and each one repaints the window.
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// A press shorter than this that does not move is a tap.
const LONG_PRESS: Duration = Duration::from_millis(500);
/// Movement (device pixels) below which a press counts as not moving.
const SLOP: f32 = 12.0;

pub enum ScreenEvent {
    Frame(Frame),
    /// The source failed (device gone, endpoint refused).
    Failed(String),
}

/// Phone keys shown as buttons.
#[derive(Debug, Clone, Copy)]
pub enum SystemKey {
    Back,
    Home,
    Recents,
}

pub struct DeviceScreen {
    serial: String,
    adb: Adb,
    emulator: Option<EmulatorControl>,
    /// Display size in device pixels, for mapping frame pixels to touches.
    display: Option<(u32, u32)>,
    frame_size: (u32, u32),
    stop: Arc<AtomicBool>,
    shell: Option<(Child, ChildStdin)>,
    press: Option<Press>,
    _stream: Option<Task<()>>,
}

struct Press {
    from: (f32, f32),
    to: (f32, f32),
    started: Instant,
}

impl EventEmitter<ScreenEvent> for DeviceScreen {}

impl Drop for DeviceScreen {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblocks a stream waiting for the next frame of a still screen.
        if let Some(control) = &self.emulator {
            control.close();
        }
        if let Some((mut child, _)) = self.shell.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl DeviceScreen {
    pub fn new(serial: String, adb: Adb, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            serial,
            adb,
            emulator: None,
            display: None,
            frame_size: (0, 0),
            stop: Arc::default(),
            shell: None,
            press: None,
            _stream: None,
        };
        screen.start(cx);
        screen
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn is_emulator_stream(&self) -> bool {
        self.emulator.is_some()
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let (serial, adb, stop) = (self.serial.clone(), self.adb.clone(), self.stop.clone());
        let (frames, ready) = Latest::new();
        self._stream = Some(cx.spawn(async move |this, cx| {
            // Connect and learn the display size off the UI thread.
            let setup_serial = serial.clone();
            let setup_adb = adb.clone();
            let (control, display) = cx
                .background_spawn(async move {
                    let control = setup_serial
                        .starts_with("emulator-")
                        .then(|| EmulatorControl::connect_patiently(&setup_serial))
                        .and_then(|result| {
                            result
                                .map_err(|err| {
                                    tracing::info!("emulator gRPC unavailable, using adb: {err:#}")
                                })
                                .ok()
                        });
                    (control, display_size(&setup_adb, &setup_serial))
                })
                .await;
            let _ = this.update(cx, |screen, _| {
                screen.emulator = control.clone();
                screen.display = display;
            });
            std::thread::Builder::new()
                .name(format!("screen-{serial}"))
                .spawn(move || match control {
                    Some(control) => stream_emulator(&control, &frames, &stop),
                    None => poll_adb(&adb, &serial, &frames, &stop),
                })
                .ok();
            let mut shown = Instant::now() - MIN_FRAME_INTERVAL;
            while ready.signal.recv().await.is_ok() {
                // At most ~30 redraws a second; the newest frame always wins,
                // so the end of an animation is never lost.
                let wait = MIN_FRAME_INTERVAL.saturating_sub(shown.elapsed());
                if !wait.is_zero() {
                    cx.background_executor().timer(wait).await;
                }
                let Some(frame) = ready.take() else { continue };
                shown = Instant::now();
                let event = match frame {
                    Ok(frame) => ScreenEvent::Frame(frame),
                    Err(message) => ScreenEvent::Failed(message),
                };
                if this
                    .update(cx, |screen, cx| {
                        if let ScreenEvent::Frame(frame) = &event {
                            screen.frame_size = (frame.width, frame.height);
                        }
                        cx.emit(event)
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    /// Frame pixels to device pixels.
    fn to_device(&self, x: f32, y: f32) -> (i32, i32) {
        let (fw, fh) = self.frame_size;
        let Some((dw, dh)) = self.display.filter(|_| fw > 0 && fh > 0) else {
            return (x as i32, y as i32);
        };
        // The display size is reported in portrait; frames rotate with the screen.
        let (dw, dh) = if (fw > fh) != (dw > dh) {
            (dh, dw)
        } else {
            (dw, dh)
        };
        (
            (x * dw as f32 / fw as f32) as i32,
            (y * dh as f32 / fh as f32) as i32,
        )
    }

    pub fn pointer(&mut self, pointer: SurfacePointer) {
        let (x, y) = self.to_device(pointer.x, pointer.y);
        if let Some(control) = &self.emulator {
            match pointer.kind {
                PointerKind::Down => control.touch(x, y, true),
                PointerKind::Move => control.touch(x, y, true),
                PointerKind::Up => control.touch(x, y, false),
                PointerKind::Scroll { dy, .. } => {
                    let (_, dy) = self.to_device(0.0, dy);
                    scroll_with_touch(control.clone(), (x, y), dy);
                }
            }
            return;
        }
        let at = (x as f32, y as f32);
        match pointer.kind {
            PointerKind::Down => {
                self.press = Some(Press {
                    from: at,
                    to: at,
                    started: Instant::now(),
                })
            }
            PointerKind::Move => {
                if let Some(press) = &mut self.press {
                    press.to = at;
                }
            }
            PointerKind::Up => {
                if let Some(press) = self.press.take() {
                    let held = press.started.elapsed();
                    let moved = ((press.to.0 - press.from.0).powi(2)
                        + (press.to.1 - press.from.1).powi(2))
                    .sqrt();
                    let (fx, fy) = (press.from.0 as i32, press.from.1 as i32);
                    let command = if moved < SLOP && held < LONG_PRESS {
                        format!("input tap {fx} {fy}")
                    } else {
                        let ms = held.as_millis().clamp(100, 3000);
                        format!(
                            "input swipe {fx} {fy} {} {} {ms}",
                            press.to.0 as i32, press.to.1 as i32
                        )
                    };
                    self.shell_command(&command);
                }
            }
            PointerKind::Scroll { dy, .. } => {
                let (_, dy) = self.to_device(0.0, dy);
                let to = (y - dy * 3).max(0);
                self.shell_command(&format!("input swipe {x} {y} {x} {to} 150"));
            }
        }
    }

    pub fn key(&mut self, keystroke: &Keystroke) {
        let named = match keystroke.key.as_str() {
            "enter" => Some(("Enter", 66)),
            "backspace" => Some(("Backspace", 67)),
            "delete" => Some(("Delete", 112)),
            "tab" => Some(("Tab", 61)),
            "escape" => Some(("GoBack", 4)),
            "left" => Some(("ArrowLeft", 21)),
            "right" => Some(("ArrowRight", 22)),
            "up" => Some(("ArrowUp", 19)),
            "down" => Some(("ArrowDown", 20)),
            "home" => Some(("Home", 122)),
            "end" => Some(("End", 123)),
            _ => None,
        };
        match (named, &self.emulator) {
            (Some((key, _)), Some(control)) => control.key(key),
            (Some((_, code)), None) => self.shell_command(&format!("input keyevent {code}")),
            (None, _) => {
                let Some(text) = keystroke.key_char.clone().filter(|text| !text.is_empty()) else {
                    return;
                };
                self.type_text(&text);
            }
        }
    }

    pub fn type_text(&mut self, text: &str) {
        match &self.emulator {
            Some(control) => control.text(text),
            None => self.shell_command(&adb::input_text_command(text)),
        }
    }

    pub fn system_key(&mut self, key: SystemKey) {
        let (name, code) = match key {
            SystemKey::Back => ("GoBack", 4),
            SystemKey::Home => ("GoHome", 3),
            SystemKey::Recents => ("AppSwitch", 187),
        };
        match &self.emulator {
            Some(control) => control.key(name),
            None => self.shell_command(&format!("input keyevent {code}")),
        }
    }

    /// Runs a command in the device's long-lived shell, starting it if needed.
    fn shell_command(&mut self, command: &str) {
        for _ in 0..2 {
            if self.shell.is_none() {
                let child = self
                    .adb
                    .command(Some(&self.serial))
                    .arg("shell")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                match child {
                    Ok(mut child) => {
                        let stdin = child.stdin.take().expect("piped stdin");
                        self.shell = Some((child, stdin));
                    }
                    Err(err) => {
                        tracing::warn!("adb shell failed: {err}");
                        return;
                    }
                }
            }
            let (_, stdin) = self.shell.as_mut().expect("started above");
            if writeln!(stdin, "{command}")
                .and_then(|_| stdin.flush())
                .is_ok()
            {
                return;
            }
            // The shell died (device reconnected): start a new one and retry.
            if let Some((mut child, _)) = self.shell.take() {
                let _ = child.kill();
            }
        }
    }
}

fn display_size(adb: &Adb, serial: &str) -> Option<(u32, u32)> {
    let text = adb.shell(serial, "wm size").ok()?;
    // "Override size" wins over "Physical size" when set.
    let line = text
        .lines()
        .find(|line| line.starts_with("Override size"))
        .or_else(|| text.lines().find(|line| line.starts_with("Physical size")))?;
    let (w, h) = line.rsplit(' ').next()?.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

fn stream_emulator(control: &EmulatorControl, frames: &Latest, stop: &AtomicBool) {
    stream_frames(control, frames, stop);
    // However the stream ended, release the connection and its threads.
    control.close();
}

fn stream_frames(control: &EmulatorControl, frames: &Latest, stop: &AtomicBool) {
    // The stream only sends on changes, so show the current screen first.
    if let Ok(frame) = control.screenshot(STREAM_SIZE.0, STREAM_SIZE.1)
        && !frames.put(Ok(frame))
    {
        return;
    }
    let stream = match control.stream(STREAM_SIZE.0, STREAM_SIZE.1) {
        Ok(stream) => stream,
        Err(err) => {
            frames.put(Err(format!("{err:#}")));
            return;
        }
    };
    while !stop.load(Ordering::Relaxed) {
        match stream.recv_blocking() {
            Ok(Some(message)) => {
                if let Some(frame) = emulator::decode_image(message.bytes()) {
                    // Drop frames the UI has not drawn yet rather than queue them.
                    if !frames.put(Ok(frame)) {
                        return;
                    }
                }
            }
            _ => {
                frames.put(Err("The emulator stopped streaming.".into()));
                return;
            }
        }
    }
}

/// Polls `screencap` as fast as the device answers. Raw pixels are faster on
/// emulators and fast cables; PNG is used if a raw grab is slow.
fn poll_adb(adb: &Adb, serial: &str, frames: &Latest, stop: &AtomicBool) {
    let mut raw = true;
    let mut failures = 0;
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        match adb.screencap(serial, raw) {
            Ok(frame) => {
                failures = 0;
                if raw && started.elapsed() > Duration::from_millis(600) {
                    raw = false;
                }
                if !frames.put(Ok(frame)) {
                    return;
                }
            }
            Err(err) => {
                failures += 1;
                if failures == 3 {
                    frames.put(Err(format!("{err:#}")));
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(40).saturating_sub(started.elapsed()));
    }
}

/// The newest frame (or error) from a source thread, waiting for the UI.
/// A new frame replaces one not yet shown, so a busy UI neither queues
/// memory nor ends up showing an old picture.
#[derive(Clone)]
struct Latest {
    slot: Arc<std::sync::Mutex<Option<Result<Frame, String>>>>,
    signal: smol::channel::Sender<()>,
}

struct Ready {
    slot: Arc<std::sync::Mutex<Option<Result<Frame, String>>>>,
    signal: smol::channel::Receiver<()>,
}

impl Latest {
    fn new() -> (Self, Ready) {
        let slot: Arc<std::sync::Mutex<Option<Result<Frame, String>>>> = Arc::default();
        let (signal, receiver) = smol::channel::bounded(1);
        (
            Self {
                slot: slot.clone(),
                signal,
            },
            Ready {
                slot,
                signal: receiver,
            },
        )
    }

    /// Offers a frame; `false` once nobody is listening any more.
    fn put(&self, frame: Result<Frame, String>) -> bool {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(frame);
        }
        !matches!(
            self.signal.try_send(()),
            Err(smol::channel::TrySendError::Closed(_))
        )
    }
}

impl Ready {
    fn take(&self) -> Option<Result<Frame, String>> {
        self.slot.lock().ok()?.take()
    }
}

/// A scroll wheel step as a short touch drag.
fn scroll_with_touch(control: EmulatorControl, (x, y): (i32, i32), dy: i32) {
    std::thread::spawn(move || {
        let steps = 6;
        control.touch(x, y, true);
        for step in 1..=steps {
            std::thread::sleep(Duration::from_millis(12));
            control.touch(x, y - dy * 3 * step / steps, true);
        }
        control.touch(x, y - dy * 3, false);
    });
}
