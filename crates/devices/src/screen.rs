//! A device's live screen and input, while its pane is open.
//!
//! - Emulators: frames stream over the emulator's gRPC endpoint (tens of
//!   frames per second, only when the screen changes) and touches are sent
//!   in real time, so dragging and scrolling feel native.
//! - Phones (and emulators without gRPC): `screenrecord` streams H.264
//!   video from the phone's hardware encoder, sent only when the screen
//!   changes and decoded here by the system (see [`crate::h264`]), as
//!   Android Studio's mirroring does. Where that cannot run it streams raw
//!   frames instead, which costs the phone far more (it reads every frame
//!   back from its GPU, and a busy screen can outrun the cable); devices
//!   that cannot do either are polled with `screencap` (slow: full-size
//!   frames). Gestures are sent as `input tap`/`swipe` through one
//!   long-lived `adb shell` ([`InputShell`]), scrolling gathered into a
//!   swipe at a time so a slow phone never falls behind.

use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, EventEmitter, Keystroke, Task};
use ph_media::{Frame, PointerKind, SurfacePointer};

use crate::adb::{self, Adb};
use crate::emulator::{self, EmulatorControl};
use crate::input_shell::InputShell;

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
    fn bit_rate(self, wireless: bool) -> &'static str {
        match (self, wireless) {
            (Self::Low, false) => "3M",
            (Self::Low, true) => "1500K",
            (Self::Medium, false) => "8M",
            (Self::Medium, true) => "4M",
            (Self::High, false) => "12M",
            (Self::High, true) => "6M",
            (Self::Full, false) => "16M",
            (Self::Full, true) => "8M",
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
/// How long the stream must pause before the bytes held are taken as a
/// whole picture: the phone writes each picture at once.
const PICTURE_GAP: Duration = Duration::from_millis(10);
/// How often the screen's rotation is checked while video streams.
const ROTATION_CHECK: Duration = Duration::from_millis(1200);
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
    quality: StreamQuality,
    emulator: Option<EmulatorControl>,
    /// Display size in device pixels, for mapping frame pixels to touches.
    display: Option<(u32, u32)>,
    frame_size: (u32, u32),
    stop: Arc<AtomicBool>,
    /// The phone's `screenrecord`, killed when the pane closes (it waits
    /// for the next change of a still screen).
    recorder: Arc<Mutex<Option<Child>>>,
    shell: Option<InputShell>,
    press: Option<Press>,
    _stream: Option<Task<()>>,
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
            display: None,
            frame_size: (0, 0),
            stop: Arc::default(),
            recorder: Arc::default(),
            shell: None,
            press: None,
            _stream: None,
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

    fn start(&mut self, cx: &mut Context<Self>) {
        let (serial, adb, stop) = (self.serial.clone(), self.adb.clone(), self.stop.clone());
        let quality = self.quality;
        let recorder = self.recorder.clone();
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
                    None => {
                        let phone = Phone {
                            adb: &adb,
                            serial: &serial,
                            recorder: &recorder,
                            quality,
                        };
                        if !phone.stream(display, &frames, &stop) {
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
    /// before any frame was sent. Video first, raw frames if not.
    fn stream(&self, display: Option<(u32, u32)>, frames: &Latest, stop: &AtomicBool) -> bool {
        let wireless = adb::is_wireless(self.serial);
        if let Ok(decoder) = crate::h264::Decoder::new() {
            let mut quality = self.quality;
            let mut decoder = decoder;
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
                let rate = quality.bit_rate(wireless);
                match self.record_video(size, rate, turned, &mut decoder, frames, stop) {
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
                    // Not this device's (or this decoder's): raw frames.
                    Recorded::Unsupported => break,
                }
                if stop.load(Ordering::Relaxed) {
                    return true;
                }
            }
            tracing::info!("{}: no video stream, streaming raw frames", self.serial);
        }
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
        // Watches the screen's rotation; a turn ends this run.
        let done = AtomicBool::new(false);
        let ended = std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut waited = Duration::ZERO;
                while !done.load(Ordering::Relaxed) {
                    // Short naps, so the run's end is not held up.
                    std::thread::sleep(Duration::from_millis(100));
                    waited += Duration::from_millis(100);
                    if waited < ROTATION_CHECK || done.load(Ordering::Relaxed) {
                        continue;
                    }
                    waited = Duration::ZERO;
                    let now = rotation(self.adb, self.serial);
                    if now.is_some() && now != turned {
                        if let Some(mut child) =
                            self.recorder.lock().ok().and_then(|mut slot| slot.take())
                        {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        break;
                    }
                }
            });
            let ended = self.decode_video(decoder, &arrived, frames, stop);
            if let Some(mut child) = self.recorder.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = child.kill();
                let _ = child.wait();
            }
            done.store(true, Ordering::Relaxed);
            ended
        });
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

    /// Decodes the run's bytes as they arrive until it ends or the pane
    /// closes.
    fn decode_video(
        &self,
        decoder: &mut crate::h264::Decoder,
        arrived: &std::sync::mpsc::Receiver<Vec<u8>>,
        frames: &Latest,
        stop: &AtomicBool,
    ) -> Recorded {
        let mut assembler = crate::h264::Assembler::default();
        // Pictures shown, and decoding failures.
        let mut counts = (0usize, 0usize);
        let mut decode = |pictures: Vec<Vec<Vec<u8>>>, counts: &mut (usize, usize)| -> bool {
            for picture in pictures {
                match decoder.decode(&picture) {
                    Ok(Some(frame)) => {
                        counts.0 += 1;
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
            let pictures = match arrived.recv_timeout(PICTURE_GAP) {
                Ok(bytes) => assembler.push(&bytes),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => assembler.flush(),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    let pictures = assembler.flush();
                    decode(pictures, &mut counts);
                    break if counts.0 > 0 {
                        Recorded::Ended
                    } else {
                        Recorded::Unsupported
                    };
                }
            };
            if !decode(pictures, &mut counts) {
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
