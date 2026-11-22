//! A device's live screen and input, while its pane is open.
//!
//! - Emulators: frames stream over the emulator's gRPC endpoint (tens of
//!   frames per second, only when the screen changes) and touches are sent
//!   in real time, so dragging and scrolling feel native.
//! - Phones (and emulators without gRPC): scrcpy's server
//!   ([`crate::scrcpy`]) streams H.264 video from the phone's hardware
//!   encoder, sent only when the screen changes and decoded here by the
//!   system (see [`crate::h264`]), and takes touches, keys and text as they
//!   come, as Android Studio's mirroring does. A phone it will not start on
//!   is recorded with `screenrecord` instead (the same video; gestures are
//!   then sent on release as `input tap`/`swipe` through one long-lived
//!   `adb shell`, [`InputShell`], scrolling gathered into a swipe at a
//!   time). A phone that streams no video at all is shown in snapshots
//!   (`screencap`, a few a second), which leaves it usable.
//! - Where this computer has no H.264 decoder (Linux), `screenrecord`
//!   streams raw frames. That costs the phone far more: it reads every
//!   frame back from its GPU, and scrolling on it stutters meanwhile.

use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, EventEmitter, Keystroke, Task};
use ph_media::{Frame, PointerKind, SurfacePointer};

use crate::adb::{self, Adb};
use crate::emulator::{self, EmulatorControl};
use crate::input_shell::InputShell;
use crate::scrcpy::{self, Control, Touch};

/// Largest frame requested from an emulator (it scales to fit). Enough to
/// fill a tall pane on a Retina screen; full resolution would double the
/// pixels to copy for no visible gain. Agents' screenshots use full size.
const STREAM_SIZE: (u32, u32) = (640, 1280);
/// Phones on Wi-Fi stream smaller frames: raw pixels over a wireless link
/// would otherwise arrive a few a second.
const WIRELESS_STREAM_SIZE: (u32, u32) = (400, 880);
/// H.264 is small on the wire, so the picture can be sharper.
const VIDEO_SIZE: (u32, u32) = (720, 1280);

/// How sharp a phone's stream is. A sharper picture costs the phone more
/// (its encoder, and the cable): a slow phone streams more smoothly lower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamQuality {
    /// 480p: for slow phones and Wi-Fi.
    Low,
    /// 720p.
    #[default]
    Medium,
    /// 1080p.
    High,
    /// The display's own size.
    Full,
}

impl StreamQuality {
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Full];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low (480p)",
            Self::Medium => "Medium (720p)",
            Self::High => "High (1080p)",
            Self::Full => "Full (the phone's own)",
        }
    }

    /// Its name in settings.
    pub fn key(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Full => "full",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|quality| quality.key() == key)
            .unwrap_or_default()
    }

    /// The largest video (portrait), or none for the display's own size.
    fn video_fit(self) -> Option<(u32, u32)> {
        match self {
            Self::Low => Some((480, 854)),
            Self::Medium => Some(VIDEO_SIZE),
            Self::High => Some((1080, 1920)),
            Self::Full => None,
        }
    }

    /// The video's bit rate (bits per second), over USB or Wi-Fi.
    fn bit_rate(self, wireless: bool) -> u32 {
        let megabits = match self {
            Self::Low => 3,
            Self::Medium => 8,
            Self::High => 12,
            Self::Full => 16,
        };
        megabits * 1_000_000 / if wireless { 2 } else { 1 }
    }

    /// What is asked of scrcpy's server.
    fn video_options(self, wireless: bool) -> scrcpy::VideoOptions {
        scrcpy::VideoOptions {
            max_size: self.video_fit().map_or(0, |(_, long)| long),
            bit_rate: self.bit_rate(wireless),
            max_fps: MAX_FPS,
        }
    }

    /// Raw frames (when video cannot stream) are scaled by this.
    fn frame_scale(self) -> f32 {
        match self {
            Self::Low => 0.75,
            Self::Medium => 1.0,
            Self::High | Self::Full => 1.5,
        }
    }
}
/// Pictures a second asked of the phone at most: what the pane draws
/// twice over, and half of what a 120 Hz screen would have encoded.
const MAX_FPS: u32 = 60;
/// Wheel movement (frame pixels) that is one notch of a mouse wheel to
/// Android.
const SCROLL_NOTCH: f32 = 40.0;
/// How often a phone that streams no video is asked for a snapshot: often
/// enough to follow what one does on it, seldom enough to leave it usable.
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(250);
/// How long the stream must pause before the bytes held are taken as a
/// whole picture: the phone writes each picture at once.
const PICTURE_GAP: Duration = Duration::from_millis(10);
/// How long the stream is waited on while nothing of a picture is held:
/// long enough that a still screen costs next to nothing, short enough to
/// notice the pane closing.
const IDLE_WAIT: Duration = Duration::from_millis(250);
/// How long the picture must look turned (see [`looks_turned`]) before the
/// phone is asked whether it is, and then asked again while the look
/// lasts: an answer may come before the phone has finished turning. After
/// these, a look that lasts is the app's own (dark, with black edges).
const TURN_ASKS: [Duration; 3] = [
    Duration::from_millis(400),
    Duration::from_secs(2),
    Duration::from_secs(6),
];
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
    /// The phone's clipboard, after a Copy or Cut asked of it.
    Clipboard(String),
}

/// How a screen reaches the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    /// Not known yet.
    Connecting,
    /// Video: an emulator's own stream, scrcpy's or `screenrecord`'s.
    Video,
    /// Raw frames: this computer has no H.264 decoder. Heavy for the phone.
    RawFrames,
    /// Snapshots: the device streams no video.
    Snapshots,
}

impl StreamMode {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Video,
            2 => Self::RawFrames,
            3 => Self::Snapshots,
            _ => Self::Connecting,
        }
    }
}

/// A phone's scrcpy server while it streams.
struct Live {
    control: Control,
    server: Arc<scrcpy::Server>,
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
    quality: StreamQuality,
    emulator: Option<EmulatorControl>,
    /// The emulator's wheel steps, as one touch drag at a time.
    touch_scroll: Option<TouchScroller>,
    /// Display size in device pixels, for mapping frame pixels to touches.
    display: Option<(u32, u32)>,
    frame_size: (u32, u32),
    stop: Arc<AtomicBool>,
    /// The phone's `screenrecord`, killed when the pane closes (it waits
    /// for the next change of a still screen).
    recorder: Arc<Mutex<Option<Child>>>,
    /// The phone's scrcpy server while it streams: input goes through it.
    live: Arc<Mutex<Option<Live>>>,
    /// How the screen streams now (a [`StreamMode`]).
    mode: Arc<std::sync::atomic::AtomicU8>,
    shell: Option<InputShell>,
    press: Option<Press>,
    _stream: Option<Task<()>>,
    _clipboard: Option<Task<()>>,
    /// Stops the phone's recording when PilotHouse quits (views are not
    /// dropped then): left running, it keeps the phone busy.
    _quit: Option<gpui_kit::Subscription>,
}

struct Press {
    from: (f32, f32),
    to: (f32, f32),
    started: Instant,
}

impl EventEmitter<ScreenEvent> for DeviceScreen {}

impl Drop for DeviceScreen {
    fn drop(&mut self) {
        self.shut_down();
    }
}

impl DeviceScreen {
    /// Ends the stream and its processes.
    fn shut_down(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblocks a stream waiting for the next frame of a still screen.
        if let Some(control) = &self.emulator {
            control.close();
        }
        if let Some(mut child) = self.recorder.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Ends the video its thread waits on, and the server with it.
        if let Some(live) = self.live.lock().ok().and_then(|mut slot| slot.take()) {
            live.server.end();
        }
        self.shell = None;
    }
}

impl DeviceScreen {
    pub fn new(serial: String, adb: Adb, quality: StreamQuality, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            serial,
            adb,
            quality,
            emulator: None,
            touch_scroll: None,
            display: None,
            frame_size: (0, 0),
            stop: Arc::default(),
            recorder: Arc::default(),
            live: Arc::default(),
            mode: Arc::default(),
            shell: None,
            press: None,
            _stream: None,
            _clipboard: None,
            _quit: None,
        };
        screen._quit = Some(cx.on_app_quit(|screen, _| {
            screen.shut_down();
            async {}
        }));
        screen.start(cx);
        screen
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn is_emulator_stream(&self) -> bool {
        self.emulator.is_some()
    }

    /// How the screen reaches the pane now.
    pub fn mode(&self) -> StreamMode {
        StreamMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    /// The phone's scrcpy server's controls, while it streams.
    fn control(&self) -> Option<Control> {
        let live = self.live.lock().ok()?;
        live.as_ref().map(|live| live.control.clone())
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let (serial, adb, stop) = (self.serial.clone(), self.adb.clone(), self.stop.clone());
        let quality = self.quality;
        let recorder = self.recorder.clone();
        let (live, mode) = (self.live.clone(), self.mode.clone());
        let (frames, ready) = Latest::new();
        // The phone's clipboard comes from its server's thread.
        let (copied, clipboard) = smol::channel::bounded::<String>(4);
        self._clipboard = Some(cx.spawn(async move |this, cx| {
            while let Ok(text) = clipboard.recv().await {
                if this
                    .update(cx, |_, cx| cx.emit(ScreenEvent::Clipboard(text)))
                    .is_err()
                {
                    return;
                }
            }
        }));
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
                screen.touch_scroll = None;
                screen.display = display;
            });
            std::thread::Builder::new()
                .name(format!("screen-{serial}"))
                .spawn(move || match control {
                    Some(control) => {
                        mode.store(StreamMode::Video as u8, Ordering::Relaxed);
                        stream_emulator(&control, &frames, &stop)
                    }
                    None => {
                        let phone = Phone {
                            adb: &adb,
                            serial: &serial,
                            recorder: &recorder,
                            live: &live,
                            mode: &mode,
                            copied,
                            quality,
                        };
                        if !phone.stream(display, &frames, &stop) {
                            mode.store(StreamMode::Snapshots as u8, Ordering::Relaxed);
                            poll_adb(&adb, &serial, &frames, &stop);
                        }
                    }
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
    /// The display's size as the screen is turned now, and how many of its
    /// pixels a frame pixel is.
    fn device_size(&self) -> Option<((u32, u32), f32)> {
        let (fw, fh) = self.frame_size;
        let (dw, dh) = self.display.filter(|_| fw > 0 && fh > 0)?;
        // The display size is reported in portrait; frames rotate with the screen.
        let (dw, dh) = if (fw > fh) != (dw > dh) {
            (dh, dw)
        } else {
            (dw, dh)
        };
        Some(((dw, dh), dh as f32 / fh as f32))
    }

    fn to_device(&self, x: f32, y: f32) -> (i32, i32) {
        match self.device_size() {
            Some(((dw, _), scale)) => {
                let scale_x = dw as f32 / self.frame_size.0 as f32;
                ((x * scale_x) as i32, (y * scale) as i32)
            }
            None => (x as i32, y as i32),
        }
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
                    let control = control.clone();
                    self.touch_scroll
                        .get_or_insert_with(|| TouchScroller::new(control))
                        .scroll((x, y), dy);
                }
            }
            return;
        }
        // A phone with scrcpy's server takes touches as they come, in the
        // video's own pixels.
        if let Some(control) = self.control() {
            let (at, size) = ((pointer.x as i32, pointer.y as i32), self.frame_size);
            match pointer.kind {
                PointerKind::Down => control.touch(Touch::Down, at, size),
                PointerKind::Move => control.touch(Touch::Move, at, size),
                PointerKind::Up => control.touch(Touch::Up, at, size),
                // The fingers going up show what is below, as a wheel
                // turned towards oneself does: Android counts that as down.
                PointerKind::Scroll { dx, dy } => {
                    control.scroll(at, size, (dx / SCROLL_NOTCH, -dy / SCROLL_NOTCH))
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
                let (display, scale) = self.device_size().unwrap_or((self.frame_size, 1.0));
                self.with_shell(|shell| shell.scroll((x, y), dy * scale, display));
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
            (Some((_, code)), None) => match self.control() {
                // Back also wakes a screen that is off.
                Some(control) if code == 4 => control.back(),
                Some(control) => control.key(code),
                None => self.shell_command(&format!("input keyevent {code}")),
            },
            (None, _) => {
                let Some(text) = keystroke.key_char.clone().filter(|text| !text.is_empty()) else {
                    return;
                };
                self.type_text(&text);
            }
        }
    }

    /// Types `text` as keys would.
    pub fn type_text(&mut self, text: &str) {
        match (&self.emulator, self.control()) {
            (Some(control), _) => control.text(text),
            // Android has keys for ASCII; anything else goes through the
            // clipboard, which takes every script.
            (None, Some(control)) if text.is_ascii() => control.text(text),
            (None, Some(control)) => control.paste(text),
            (None, None) => self.shell_command(&adb::input_text_command(text)),
        }
    }

    /// Pastes `text` into the device: through its clipboard where scrcpy's
    /// server runs (text of any length and script, at once), else typed.
    pub fn paste(&mut self, text: &str) {
        match self.control() {
            Some(control) => control.paste(text),
            None => self.type_text(text),
        }
    }

    /// Copies (or cuts) what is selected on the phone: its clipboard comes
    /// back as [`ScreenEvent::Clipboard`]. Only with scrcpy's server.
    pub fn copy(&mut self, cut: bool) {
        if let Some(control) = self.control() {
            control.copy(cut);
        }
    }

    pub fn system_key(&mut self, key: SystemKey) {
        let (name, code) = match key {
            SystemKey::Back => ("GoBack", 4),
            SystemKey::Home => ("GoHome", 3),
            SystemKey::Recents => ("AppSwitch", 187),
        };
        match (&self.emulator, self.control()) {
            (Some(control), _) => control.key(name),
            (None, Some(control)) => match key {
                SystemKey::Back => control.back(),
                SystemKey::Home => control.key(scrcpy::KEY_HOME),
                SystemKey::Recents => control.key(scrcpy::KEY_APP_SWITCH),
            },
            (None, None) => self.shell_command(&format!("input keyevent {code}")),
        }
    }

    /// Runs a command in the device's long-lived shell, starting it if needed.
    fn shell_command(&mut self, command: &str) {
        self.with_shell(|shell| shell.send(command));
    }

    /// Uses the device's input shell, starting it if needed, and once more
    /// with a new one if it died (the device reconnected).
    fn with_shell(&mut self, mut action: impl FnMut(&InputShell) -> anyhow::Result<()>) {
        for _ in 0..2 {
            if self.shell.is_none() {
                match InputShell::start(&self.adb, &self.serial) {
                    Ok(shell) => self.shell = Some(shell),
                    Err(err) => {
                        tracing::warn!("adb shell failed: {err:#}");
                        return;
                    }
                }
            }
            let shell = self.shell.as_ref().expect("started above");
            if action(shell).is_ok() {
                return;
            }
            self.shell = None;
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

/// A phone's screen through `adb`.
struct Phone<'a> {
    adb: &'a Adb,
    serial: &'a str,
    recorder: &'a Mutex<Option<Child>>,
    live: &'a Mutex<Option<Live>>,
    mode: &'a std::sync::atomic::AtomicU8,
    /// Where the phone's clipboard goes when it was asked for.
    copied: smol::channel::Sender<String>,
    quality: StreamQuality,
}

/// How `screenrecord` ended.
enum Recorded {
    /// The pane closed.
    Stopped,
    /// It sent frames, then stopped (a time limit, the screen turned off).
    Ended,
    /// It never sent a frame: this device cannot stream raw frames.
    Unsupported,
}

impl Phone<'_> {
    /// Streams frames until the pane closes; `false` when the device cannot,
    /// before any frame was sent. Video from scrcpy's server first, from
    /// `screenrecord` if not; raw frames only where this computer cannot
    /// decode video.
    fn stream(&self, display: Option<(u32, u32)>, frames: &Latest, stop: &AtomicBool) -> bool {
        let wireless = adb::is_wireless(self.serial);
        if let Ok(decoder) = crate::h264::Decoder::new() {
            let mut quality = self.quality;
            let mut decoder = decoder;
            let mut streamed = false;
            loop {
                match self.mirror(wireless, &mut decoder, frames, stop) {
                    Recorded::Stopped => return true,
                    Recorded::Ended => streamed = true,
                    // It worked before: the phone went away or is restarting.
                    Recorded::Unsupported if streamed => {}
                    Recorded::Unsupported => break,
                }
                if stop.load(Ordering::Relaxed) {
                    return true;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            tracing::info!("{}: no screen server, recording the screen", self.serial);
            let mut streamed = false;
            loop {
                // The video takes the screen's current shape: asked for
                // portrait while the phone is sideways, it would be boxed
                // in black.
                let turned = rotation(self.adb, self.serial);
                let natural = display.unwrap_or(VIDEO_SIZE);
                let shape = if turned.is_some_and(|turn| turn % 2 == 1) {
                    (natural.1, natural.0)
                } else {
                    natural
                };
                let size = match quality.video_fit() {
                    Some(fit) => video_size(shape, fit),
                    None => video_size(shape, shape),
                };
                let rate = quality.bit_rate(wireless).to_string();
                match self.record_video(size, &rate, turned, &mut decoder, frames, stop) {
                    Recorded::Stopped => return true,
                    Recorded::Ended => streamed = true,
                    Recorded::Unsupported if streamed => {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    // Some encoders cannot do the whole display: 1080p.
                    Recorded::Unsupported if quality == StreamQuality::Full => {
                        tracing::info!("{}: full size refused, streaming 1080p", self.serial);
                        quality = StreamQuality::High;
                    }
                    // No video from this device: snapshots. Raw frames
                    // would show more, and leave the phone hard to use.
                    Recorded::Unsupported => {
                        tracing::info!("{}: no video stream, taking snapshots", self.serial);
                        return false;
                    }
                }
                if stop.load(Ordering::Relaxed) {
                    return true;
                }
            }
        }
        tracing::info!(
            "{}: no video decoder here, streaming raw frames",
            self.serial
        );
        let fit = if wireless {
            WIRELESS_STREAM_SIZE
        } else {
            STREAM_SIZE
        };
        let scale = self.quality.frame_scale();
        let fit = ((fit.0 as f32 * scale) as u32, (fit.1 as f32 * scale) as u32);
        let size = stream_size(display.unwrap_or(STREAM_SIZE), fit);
        let mut streamed = false;
        loop {
            match self.record(size, frames, stop) {
                Recorded::Stopped => return true,
                Recorded::Ended => streamed = true,
                Recorded::Unsupported if streamed => {
                    // It worked before: the device went away or is asleep.
                    std::thread::sleep(Duration::from_secs(1));
                }
                Recorded::Unsupported => return false,
            }
            if stop.load(Ordering::Relaxed) {
                return true;
            }
        }
    }

    /// One run of scrcpy's server, its video decoded as it arrives, its
    /// controls there for the pane meanwhile.
    fn mirror(
        &self,
        wireless: bool,
        decoder: &mut crate::h264::Decoder,
        frames: &Latest,
        stop: &AtomicBool,
    ) -> Recorded {
        let copied = self.copied.clone();
        let started = scrcpy::start(
            self.adb,
            self.serial,
            self.quality.video_options(wireless),
            move |text| {
                let _ = copied.try_send(text);
            },
        );
        let (mut video, control, server) = match started {
            Ok(started) => started,
            Err(err) => {
                tracing::info!("{}: no screen server: {err:#}", self.serial);
                return Recorded::Unsupported;
            }
        };
        let server = Arc::new(server);
        if let Ok(mut live) = self.live.lock() {
            *live = Some(Live {
                control,
                server: server.clone(),
            });
        }
        // Pictures shown, and decoding failures.
        let (mut shown, mut failed) = (0usize, 0usize);
        let ended = loop {
            if stop.load(Ordering::Relaxed) {
                break Recorded::Stopped;
            }
            let data = match video.next() {
                // The size is in the parameter sets that follow.
                Ok(scrcpy::Packet::Session { .. }) => continue,
                Ok(scrcpy::Packet::Config(data) | scrcpy::Packet::Picture(data)) => data,
                Err(_) if shown > 0 => break Recorded::Ended,
                Err(_) => {
                    tracing::info!("{}: the screen server said: {}", self.serial, server.said());
                    break Recorded::Unsupported;
                }
            };
            match decoder.decode(&crate::h264::split_nals(&data)) {
                Ok(Some(frame)) => {
                    shown += 1;
                    self.mode.store(StreamMode::Video as u8, Ordering::Relaxed);
                    if !frames.put(Ok(frame)) {
                        break Recorded::Stopped;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    failed += 1;
                    tracing::info!("decoding the screen: {err:#}");
                    // A decoder that never makes a picture is not one to keep.
                    if shown == 0 && failed >= 3 {
                        break Recorded::Unsupported;
                    }
                }
            }
        };
        // Its controls go with it: input falls back to the shell meanwhile.
        if let Ok(mut live) = self.live.lock() {
            *live = None;
        }
        drop(server);
        if stop.load(Ordering::Relaxed) {
            return Recorded::Stopped;
        }
        ended
    }

    /// One `screenrecord --output-format=h264` run, decoded as it arrives.
    /// It ends (to start again) when the screen turns.
    fn record_video(
        &self,
        (width, height): (u32, u32),
        bit_rate: &str,
        turned: Option<u8>,
        decoder: &mut crate::h264::Decoder,
        frames: &Latest,
        stop: &AtomicBool,
    ) -> Recorded {
        let size = format!("{width}x{height}");
        let child = self
            .adb
            .command(Some(self.serial))
            .args([
                "exec-out",
                "screenrecord",
                "--output-format=h264",
                "--size",
                &size,
                "--bit-rate",
                bit_rate,
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                tracing::info!("screenrecord failed to start: {err}");
                return Recorded::Unsupported;
            }
        };
        let mut stdout = child.stdout.take().expect("piped stdout");
        if let Ok(mut slot) = self.recorder.lock() {
            *slot = Some(child);
        }
        // Bytes are read on their own thread, so a pause in them can be
        // noticed: it ends the picture held (see `PICTURE_GAP`).
        let (chunks, arrived) = std::sync::mpsc::sync_channel::<Vec<u8>>(256);
        let reader = std::thread::Builder::new()
            .name("screen-video".into())
            .spawn(move || {
                let mut buffer = vec![0u8; 256 * 1024];
                loop {
                    match std::io::Read::read(&mut stdout, &mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if chunks.send(buffer[..read].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        let ended = self.decode_video(decoder, turned, &arrived, frames, stop);
        if let Some(mut child) = self.recorder.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(arrived);
        if let Ok(reader) = reader {
            let _ = reader.join();
        }
        if stop.load(Ordering::Relaxed) {
            return Recorded::Stopped;
        }
        ended
    }

    /// Decodes the run's bytes as they arrive until it ends, the screen
    /// turns (the run is then ended, to start again in the new shape) or
    /// the pane closes.
    ///
    /// A turn is seen in the picture, not asked of the phone: asking means
    /// `dumpsys display`, a dump made inside Android's system process, and
    /// one every second made the phone itself stutter while it was
    /// mirrored. The phone is asked only once the picture looks turned.
    fn decode_video(
        &self,
        decoder: &mut crate::h264::Decoder,
        turned: Option<u8>,
        arrived: &std::sync::mpsc::Receiver<Vec<u8>>,
        frames: &Latest,
        stop: &AtomicBool,
    ) -> Recorded {
        let mut assembler = crate::h264::Assembler::default();
        // Pictures shown, and decoding failures.
        let mut counts = (0usize, 0usize);
        let mut turn = TurnWatch::default();
        let mut decode = |pictures: Vec<Vec<Vec<u8>>>,
                          counts: &mut (usize, usize),
                          turn: &mut TurnWatch|
         -> bool {
            for picture in pictures {
                match decoder.decode(&picture) {
                    Ok(Some(frame)) => {
                        counts.0 += 1;
                        self.mode.store(StreamMode::Video as u8, Ordering::Relaxed);
                        turn.saw(looks_turned(frame.width, frame.height, frame.bgra()));
                        if !frames.put(Ok(frame)) {
                            return false;
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        counts.1 += 1;
                        tracing::info!("decoding the screen: {err:#}");
                    }
                }
            }
            true
        };
        loop {
            if stop.load(Ordering::Relaxed) {
                break Recorded::Stopped;
            }
            // A decoder that never makes a picture is not one to keep.
            if counts.0 == 0 && counts.1 >= 3 {
                break Recorded::Unsupported;
            }
            if turn.ask() {
                let now = rotation(self.adb, self.serial);
                if now.is_some() && now != turned {
                    break Recorded::Ended;
                }
            }
            // With part of a picture held, a short pause ends it; with
            // nothing held there is nothing to end, and the wait is long.
            let wait = if assembler.is_empty() {
                IDLE_WAIT
            } else {
                PICTURE_GAP
            };
            let pictures = match arrived.recv_timeout(wait) {
                Ok(bytes) => assembler.push(&bytes),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => assembler.flush(),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    let pictures = assembler.flush();
                    decode(pictures, &mut counts, &mut turn);
                    break if counts.0 > 0 {
                        Recorded::Ended
                    } else {
                        Recorded::Unsupported
                    };
                }
            };
            if !decode(pictures, &mut counts, &mut turn) {
                break Recorded::Stopped;
            }
        }
    }

    /// One `screenrecord --output-format=frames` run: each frame is a
    /// little-endian header (length of the rest, width, height, stride,
    /// pixel format) and RGB888 pixels.
    fn record(&self, (width, height): (u32, u32), frames: &Latest, stop: &AtomicBool) -> Recorded {
        let size = format!("{width}x{height}");
        let child = self
            .adb
            .command(Some(self.serial))
            .args([
                "exec-out",
                "screenrecord",
                "--output-format=frames",
                "--size",
                &size,
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                tracing::info!("screenrecord failed to start: {err}");
                return Recorded::Unsupported;
            }
        };
        let mut stdout = child.stdout.take().expect("piped stdout");
        if let Ok(mut slot) = self.recorder.lock() {
            *slot = Some(child);
        }
        let mut sent = 0usize;
        let mut buffer = Vec::new();
        let ended = loop {
            if stop.load(Ordering::Relaxed) {
                break Recorded::Stopped;
            }
            let Some(frame) = read_frame(&mut stdout, &mut buffer) else {
                break if sent > 0 {
                    Recorded::Ended
                } else {
                    Recorded::Unsupported
                };
            };
            sent += 1;
            self.mode
                .store(StreamMode::RawFrames as u8, Ordering::Relaxed);
            if !frames.put(Ok(frame)) {
                break Recorded::Stopped;
            }
        };
        if let Some(mut child) = self.recorder.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        if stop.load(Ordering::Relaxed) {
            return Recorded::Stopped;
        }
        ended
    }
}

/// Whether the phone should be asked if its screen turned, from how the
/// pictures look.
#[derive(Default)]
struct TurnWatch {
    /// Since when the pictures have looked turned.
    since: Option<Instant>,
    /// How many times the phone was asked about this look.
    asked: usize,
}

impl TurnWatch {
    /// A picture arrived, looking turned or not.
    fn saw(&mut self, turned: bool) {
        if !turned {
            *self = Self::default();
        } else if self.since.is_none() {
            self.since = Some(Instant::now());
        }
    }

    /// Whether to ask the phone now: the look has lasted to the next of
    /// [`TURN_ASKS`].
    fn ask(&mut self) -> bool {
        let due = TURN_ASKS
            .get(self.asked)
            .zip(self.since)
            .is_some_and(|(after, since)| since.elapsed() >= *after);
        if due {
            self.asked += 1;
        }
        due
    }
}

/// Whether a picture of the stream looks like the screen turned since the
/// recording began: `screenrecord` keeps the video's shape and fits the
/// turned screen inside it, so a portrait video then has black bands above
/// and below a wide picture (a landscape one, left and right of a tall
/// picture). Bands that are black while the middle is not; a wholly black
/// screen says nothing.
fn looks_turned(width: u32, height: u32, bgra: &[u8]) -> bool {
    let (width, height) = (width as usize, height as usize);
    if bgra.len() < width * height * 4 || width.min(height) < 64 {
        return false;
    }
    // Along the long side: where the bands end and the picture begins.
    let (long, short) = (width.max(height), width.min(height));
    let band = (long - short * short / long) / 2;
    if band < 16 {
        return false;
    }
    let pixel = |along: usize, across: usize| {
        let (x, y) = if height > width {
            (across, along)
        } else {
            (along, across)
        };
        let at = (y * width + x) * 4;
        bgra[at].max(bgra[at + 1]).max(bgra[at + 2])
    };
    // Nine places across the short side, away from its edges.
    let across = (1..=9).map(|n| short * n / 10);
    let bands_black = [band / 4, band / 2, band * 3 / 4]
        .into_iter()
        .flat_map(|depth| [depth, long - 1 - depth])
        .all(|along| across.clone().all(|across| pixel(along, across) <= 24));
    if !bands_black {
        return false;
    }
    let middle = long - 2 * band;
    (1..=5)
        .map(|n| band + middle * n / 6)
        .any(|along| across.clone().any(|across| pixel(along, across) > 48))
}

/// The screen's rotation from its natural orientation, in quarter turns.
fn rotation(adb: &Adb, serial: &str) -> Option<u8> {
    let text = adb
        .shell(
            serial,
            "dumpsys display 2>/dev/null | grep -m1 -o 'mCurrentOrientation=[0-9]'",
        )
        .ok()?;
    text.trim()
        .strip_prefix("mCurrentOrientation=")?
        .parse()
        .ok()
        .filter(|turn: &u8| *turn < 4)
}

/// The video size: as [`stream_size`], in multiples of 16 as hardware
/// encoders want.
fn video_size(display: (u32, u32), fit: (u32, u32)) -> (u32, u32) {
    let (width, height) = stream_size(display, fit);
    ((width & !15).max(16), (height & !15).max(16))
}

/// The streamed size: the display scaled to fit `fit` (either way round),
/// in even numbers as video sizes must be.
fn stream_size((width, height): (u32, u32), fit: (u32, u32)) -> (u32, u32) {
    let (max_w, max_h) = if width > height { (fit.1, fit.0) } else { fit };
    let scale = (max_w as f32 / width as f32)
        .min(max_h as f32 / height as f32)
        .min(1.0);
    let even = |value: u32| ((value as f32 * scale) as u32).max(2) & !1;
    (even(width), even(height))
}

/// One frame of `screenrecord --output-format=frames`; `None` at the end of
/// the stream or on anything unexpected.
fn read_frame(input: &mut impl std::io::Read, buffer: &mut Vec<u8>) -> Option<Frame> {
    let mut length = [0u8; 4];
    input.read_exact(&mut length).ok()?;
    let length = u32::from_le_bytes(length) as usize;
    // A header of four words, and no frame bigger than 8K.
    if !(16..=16 + 7680 * 4320 * 3).contains(&length) {
        return None;
    }
    buffer.resize(length, 0);
    input.read_exact(buffer).ok()?;
    parse_frame(buffer)
}

/// Width, height, stride and pixel format, then the pixels.
fn parse_frame(packet: &[u8]) -> Option<Frame> {
    let word = |at: usize| Some(u32::from_le_bytes(packet.get(at..at + 4)?.try_into().ok()?));
    let (width, height, stride) = (word(0)?, word(4)?, word(8)? as usize);
    let row = width as usize * 3;
    let pixels = packet.get(16..)?;
    if width == 0 || height == 0 || stride < row || pixels.len() < stride * height as usize {
        return None;
    }
    if stride == row {
        return Some(Frame::from_rgb_slice(
            width,
            height,
            &pixels[..row * height as usize],
        ));
    }
    let rows: Vec<u8> = pixels
        .chunks(stride)
        .take(height as usize)
        .flat_map(|line| &line[..row])
        .copied()
        .collect();
    Some(Frame::from_rgb_slice(width, height, &rows))
}

/// Polls `screencap` a few times a second: a grab is the whole screen read
/// back and sent, and as fast as the device answers would leave it hard to
/// use. Raw pixels are faster on emulators and fast cables; PNG is used if
/// a raw grab is slow.
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
        std::thread::sleep(SNAPSHOT_INTERVAL.saturating_sub(started.elapsed()));
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

/// Wheel steps on an emulator as short touch drags, one at a time on one
/// thread: steps that come while a drag runs are added up into the next
/// (a trackpad sends dozens a second; drags sent side by side read as
/// stray taps).
/// Where the next drag starts, and how far it goes.
type NextDrag = Arc<Mutex<Option<((i32, i32), i32)>>>;

struct TouchScroller {
    pending: NextDrag,
    wake: smol::channel::Sender<()>,
}

impl TouchScroller {
    fn new(control: EmulatorControl) -> Self {
        let pending: NextDrag = Arc::default();
        let (wake, woken) = smol::channel::bounded::<()>(1);
        let next = pending.clone();
        // Ends with the screen (its sender goes).
        std::thread::spawn(move || {
            while woken.recv_blocking().is_ok() {
                let Some(((x, y), dy)) = next.lock().ok().and_then(|mut next| next.take()) else {
                    continue;
                };
                // At most about half a screen per drag.
                let distance = (dy * 3).clamp(-900, 900);
                let steps = 6;
                control.touch(x, y, true);
                for step in 1..=steps {
                    std::thread::sleep(Duration::from_millis(12));
                    control.touch(x, y - distance * step / steps, true);
                }
                control.touch(x, y - distance, false);
            }
        });
        Self { pending, wake }
    }

    fn scroll(&self, at: (i32, i32), dy: i32) {
        if let Ok(mut pending) = self.pending.lock() {
            let total = pending.map_or(0, |(_, sum)| sum) + dy;
            *pending = Some((at, total));
        }
        let _ = self.wake.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_quality_streams_a_size_video_can_be() {
        let tablet = (1200, 1920);
        let sizes: Vec<_> = StreamQuality::ALL
            .into_iter()
            .map(|quality| video_size(tablet, quality.video_fit().unwrap_or(tablet)))
            .collect();
        assert_eq!(sizes, [(480, 768), (720, 1152), (1072, 1728), (1200, 1920)]);
        assert_eq!(StreamQuality::from_key("high"), StreamQuality::High);
        assert_eq!(StreamQuality::from_key(""), StreamQuality::Medium);
    }

    /// A `width` x `height` picture, black but for `lit` (x, y) places.
    fn picture(width: usize, height: usize, lit: impl Fn(usize, usize) -> bool) -> Vec<u8> {
        let mut bgra = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                if lit(x, y) {
                    bgra[(y * width + x) * 4..][..3].fill(200);
                }
            }
        }
        bgra
    }

    #[test]
    fn a_turned_screen_is_seen_in_the_picture() {
        // Portrait video, the screen now sideways: a wide picture in the
        // middle (rows 60..100 of 160), black above and below.
        let turned = picture(80, 160, |_, y| (60..100).contains(&y));
        assert!(looks_turned(80, 160, &turned));
        // Landscape video, the screen now upright.
        let upright = picture(160, 80, |x, _| (60..100).contains(&x));
        assert!(looks_turned(160, 80, &upright));
        // The screen as recorded, a black screen, and an app that is dark
        // only at its top do not look turned.
        assert!(!looks_turned(80, 160, &picture(80, 160, |_, _| true)));
        assert!(!looks_turned(80, 160, &picture(80, 160, |_, _| false)));
        assert!(!looks_turned(80, 160, &picture(80, 160, |_, y| y > 40)));
        // Too small or cut short to judge.
        assert!(!looks_turned(
            40,
            80,
            &picture(40, 80, |_, y| (30..50).contains(&y))
        ));
        assert!(!looks_turned(80, 160, &turned[..100]));
    }

    #[test]
    fn the_phone_is_asked_a_few_times_per_turned_look() {
        let mut watch = TurnWatch::default();
        watch.saw(true);
        assert!(!watch.ask(), "not before the look has lasted");
        watch.since = Some(Instant::now() - TURN_ASKS[0]);
        assert!(watch.ask());
        watch.saw(true);
        assert!(!watch.ask(), "asked a moment ago");
        watch.since = Some(Instant::now() - TURN_ASKS[2]);
        assert!(watch.ask() && watch.ask(), "again while the look lasts");
        assert!(!watch.ask(), "then no more: the app looks like that");
        watch.saw(false);
        watch.saw(true);
        watch.since = Some(Instant::now() - TURN_ASKS[0]);
        assert!(watch.ask(), "a new look is asked about");
    }

    #[test]
    fn stream_size_fits_and_stays_even() {
        assert_eq!(stream_size((1080, 2400), STREAM_SIZE), (576, 1280));
        assert_eq!(stream_size((2400, 1080), STREAM_SIZE), (1280, 576));
        assert_eq!(stream_size((480, 800), STREAM_SIZE), (480, 800));
        assert_eq!(stream_size((1081, 2401), STREAM_SIZE), (576, 1280));
        assert_eq!(stream_size((1080, 2400), WIRELESS_STREAM_SIZE), (396, 880));
    }

    #[test]
    fn reads_frames_until_the_stream_ends() {
        let (width, height) = (2u32, 2u32);
        let mut stream = Vec::new();
        for _ in 0..2 {
            stream.extend((16 + width * height * 3).to_le_bytes());
            for word in [width, height, width * 3, 3] {
                stream.extend(word.to_le_bytes());
            }
            stream.extend([10u8, 20, 30].repeat(4));
        }
        let mut input = std::io::Cursor::new(stream);
        let mut buffer = Vec::new();
        for _ in 0..2 {
            let frame = read_frame(&mut input, &mut buffer).unwrap();
            assert_eq!((frame.width, frame.height), (2, 2));
            assert_eq!(&frame.bgra()[..4], &[30, 20, 10, 255]);
        }
        assert!(read_frame(&mut input, &mut buffer).is_none());
    }
}
