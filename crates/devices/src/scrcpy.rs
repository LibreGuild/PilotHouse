//! A phone's screen and input through [scrcpy]'s server, the way scrcpy
//! itself and Android Studio's mirroring work: a small program is sent to
//! the phone and started there with `app_process`. It has the phone's
//! hardware encoder compress the screen into H.264, at a size and frame
//! rate asked of it, and puts touches, keys and text into Android's input
//! as they come, so a drag is followed while the finger is down, a long
//! press and a fling are what they are on the phone, and text in any
//! script can be pasted. A turned screen is a new video in the same
//! stream; nothing has to ask the phone for it.
//!
//! The server is scrcpy's own, unchanged (`assets/scrcpy-server-v…`,
//! Apache-2.0, see `NOTICE`): this file is only a client for it. Its
//! protocol is private to scrcpy and changes between versions, so the
//! client and the server here are one version, and a new server needs this
//! file read against scrcpy's `doc/develop.md` and its tests
//! (`app/tests/test_control_msg_serialize.c`) again.
//!
//! Two connections go through one `adb forward`ed port: the video (a
//! header, then packets of one picture each), and the controls (messages
//! to the phone; the phone's clipboard back).
//!
//! [scrcpy]: https://github.com/Genymobile/scrcpy

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::process::{Child, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use crate::adb::Adb;

/// scrcpy's server, as released.
const SERVER: &[u8] = include_bytes!("../assets/scrcpy-server-v5.0.1");
/// Its version, which it checks against the one it is started with.
const VERSION: &str = "5.0.1";
/// Where it is put on the phone: a name of its own, so a scrcpy of another
/// version used on the same phone neither replaces it nor is replaced.
const DEVICE_PATH: &str = "/data/local/tmp/pilothouse-scrcpy-server.jar";
/// `"h264"` as the codec id the video starts with.
const H264: u32 = 0x6832_3634;
/// How long the server may take to listen: it is a Java program started on
/// the phone, about a second on a fast one.
const CONNECT_TRIES: usize = 100;
const CONNECT_PAUSE: Duration = Duration::from_millis(100);
/// How long the phone is given to paste before the next message.
const PASTE_SETTLES: Duration = Duration::from_millis(150);

/// What is asked of the phone's encoder.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VideoOptions {
    /// The longer side of the video at most (0: the display's own size).
    pub max_size: u32,
    /// Bits per second.
    pub bit_rate: u32,
    /// Pictures per second at most: a 120 Hz screen is not encoded 120
    /// times a second for a pane drawn 30 times.
    pub max_fps: u32,
}

/// One packet of the video.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Packet {
    /// A new video starts (at first, and when the screen turns), this big.
    Session { width: u32, height: u32 },
    /// The encoder's parameter sets, before the first picture of a video.
    Config(Vec<u8>),
    /// One picture, as Annex B NAL units.
    Picture(Vec<u8>),
}

/// The video connection.
pub(crate) struct Video {
    socket: TcpStream,
}

impl Video {
    /// The next packet; an error when the stream ends.
    pub(crate) fn next(&mut self) -> std::io::Result<Packet> {
        let mut header = [0u8; 12];
        self.socket.read_exact(&mut header)?;
        let word = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().expect("4 bytes"));
        // The first bit tells a session from a media packet.
        if header[0] & 0x80 != 0 {
            return Ok(Packet::Session {
                width: word(4),
                height: word(8),
            });
        }
        let config = header[0] & 0x40 != 0;
        let size = word(8) as usize;
        // No picture is this big; a wrong size would otherwise be waited
        // for, for ever.
        if size > 64 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "a video packet of an impossible size",
            ));
        }
        let mut data = vec![0u8; size];
        self.socket.read_exact(&mut data)?;
        Ok(if config {
            Packet::Config(data)
        } else {
            Packet::Picture(data)
        })
    }
}

/// A touch as Android names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Touch {
    Down = 0,
    Up = 1,
    Move = 2,
}

/// The controls of a running server: messages are written in order on a
/// thread of their own, so sending one never waits for the phone.
#[derive(Clone)]
pub(crate) struct Control {
    messages: Sender<Vec<u8>>,
}

/// The pointer every touch from here is: one finger. (scrcpy's
/// `SC_POINTER_ID_GENERIC_FINGER`.)
const FINGER: u64 = u64::MAX - 1;
/// Android key codes used here.
pub(crate) const KEY_HOME: u32 = 3;
pub(crate) const KEY_APP_SWITCH: u32 = 187;

impl Control {
    fn send(&self, message: Vec<u8>) {
        // The server is gone: the screen says so.
        let _ = self.messages.send(message);
    }

    /// A finger at `(x, y)` in a video `size` big. A touch meant for a
    /// video of another size (the screen has turned since) is ignored by
    /// the server.
    pub(crate) fn touch(&self, touch: Touch, (x, y): (i32, i32), size: (u32, u32)) {
        self.send(touch_message(touch, (x, y), size));
    }

    /// The wheel turned at `(x, y)` by `(dx, dy)` notches.
    pub(crate) fn scroll(&self, at: (i32, i32), size: (u32, u32), (dx, dy): (f32, f32)) {
        self.send(scroll_message(at, size, (dx, dy)));
    }

    /// Presses and releases an Android key.
    pub(crate) fn key(&self, keycode: u32) {
        self.send(key_message(false, keycode));
        self.send(key_message(true, keycode));
    }

    /// Types `text`, where Android has keys for its characters.
    pub(crate) fn text(&self, text: &str) {
        self.send(text_message(text));
    }

    /// Back, or the screen on when it is off.
    pub(crate) fn back(&self) {
        self.send(vec![BACK_OR_SCREEN_ON, 0]);
        self.send(vec![BACK_OR_SCREEN_ON, 1]);
    }

    /// Puts `text` in the phone's clipboard and pastes it: text in any
    /// script, which keys cannot always type.
    pub(crate) fn paste(&self, text: &str) {
        self.send(set_clipboard_message(text, true));
    }

    /// Asks for the phone's clipboard after its Copy (or Cut) key: it
    /// comes back to whoever listens to the server (see [`start`]).
    pub(crate) fn copy(&self, cut: bool) {
        self.send(vec![GET_CLIPBOARD, if cut { 2 } else { 1 }]);
    }
}

// Message types (scrcpy's `sc_control_msg_type`).
const INJECT_KEYCODE: u8 = 0;
const INJECT_TEXT: u8 = 1;
const INJECT_TOUCH_EVENT: u8 = 2;
const INJECT_SCROLL_EVENT: u8 = 3;
const BACK_OR_SCREEN_ON: u8 = 4;
const GET_CLIPBOARD: u8 = 8;
const SET_CLIPBOARD: u8 = 9;
/// The longest text one message types.
const TEXT_MAX: usize = 300;
/// The longest text a clipboard message carries (the message is 256 KiB at
/// most).
const CLIPBOARD_MAX: usize = (1 << 18) - 14;

fn key_message(up: bool, keycode: u32) -> Vec<u8> {
    let mut message = vec![INJECT_KEYCODE, u8::from(up)];
    message.extend(keycode.to_be_bytes());
    // No repeat, no modifiers.
    message.extend([0u8; 8]);
    message
}

fn text_message(text: &str) -> Vec<u8> {
    let text = truncated(text, TEXT_MAX);
    let mut message = vec![INJECT_TEXT];
    message.extend((text.len() as u32).to_be_bytes());
    message.extend(text.as_bytes());
    message
}

fn position(message: &mut Vec<u8>, (x, y): (i32, i32), (width, height): (u32, u32)) {
    message.extend(x.to_be_bytes());
    message.extend(y.to_be_bytes());
    message.extend((width as u16).to_be_bytes());
    message.extend((height as u16).to_be_bytes());
}

fn touch_message(touch: Touch, at: (i32, i32), size: (u32, u32)) -> Vec<u8> {
    let mut message = vec![INJECT_TOUCH_EVENT, touch as u8];
    message.extend(FINGER.to_be_bytes());
    position(&mut message, at, size);
    // Pressure as 16-bit fixed point: full while the finger is down.
    let pressure: u16 = if touch == Touch::Up { 0 } else { 0xffff };
    message.extend(pressure.to_be_bytes());
    // A finger has no buttons.
    message.extend([0u8; 8]);
    message
}

fn scroll_message(at: (i32, i32), size: (u32, u32), (dx, dy): (f32, f32)) -> Vec<u8> {
    // Each way from -16 to 16 notches, as signed 16-bit fixed point.
    let fixed = |notches: f32| {
        let scaled = (notches.clamp(-16.0, 16.0) / 16.0 * 32768.0) as i32;
        (scaled.clamp(-0x8000, 0x7fff) as i16).to_be_bytes()
    };
    let mut message = vec![INJECT_SCROLL_EVENT];
    position(&mut message, at, size);
    message.extend(fixed(dx));
    message.extend(fixed(dy));
    message.extend([0u8; 4]);
    message
}

fn set_clipboard_message(text: &str, paste: bool) -> Vec<u8> {
    let text = truncated(text, CLIPBOARD_MAX);
    let mut message = vec![SET_CLIPBOARD];
    // No sequence number: no acknowledgement is waited for.
    message.extend([0u8; 8]);
    message.push(u8::from(paste));
    message.extend((text.len() as u32).to_be_bytes());
    message.extend(text.as_bytes());
    message
}

/// `text` cut to `max` bytes at most, between characters.
fn truncated(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One message from the phone: its clipboard's text, or nothing to do
/// anything about. `None` at the end of the connection or on a message
/// not understood (there is no telling where the next one would begin).
fn read_device_message(socket: &mut impl Read) -> Option<Option<String>> {
    let mut kind = [0u8; 1];
    socket.read_exact(&mut kind).ok()?;
    let mut read = |length: usize| {
        let mut bytes = vec![0u8; length];
        socket.read_exact(&mut bytes).ok().map(|()| bytes)
    };
    match kind[0] {
        // The clipboard: a length, then the text.
        0 => {
            let length = u32::from_be_bytes(read(4)?.try_into().ok()?) as usize;
            if length > 1 << 18 {
                return None;
            }
            Some(Some(String::from_utf8_lossy(&read(length)?).into_owned()))
        }
        // A clipboard set was done (none here asks to be told).
        1 => read(8).map(|_| None),
        // What a keyboard made of the phone's own would be told.
        2 => {
            let header = read(4)?;
            read(usize::from(u16::from_be_bytes([header[2], header[3]]))).map(|_| None)
        }
        _ => None,
    }
}

/// A running server and the way to end it.
pub(crate) struct Server {
    process: Mutex<Child>,
    adb: Adb,
    serial: String,
    port: u16,
    /// The video connection again, to end a read that waits on it.
    video: TcpStream,
    /// What the server printed last: why it stopped, when it did.
    said: Arc<Mutex<String>>,
    ended: std::sync::atomic::AtomicBool,
}

impl Server {
    /// Ends the server and what was made for it, once: the video (which
    /// ends the thread reading it, and the server on the phone, as when
    /// scrcpy's own window closes), the `adb shell` it was started through,
    /// and the forwarded port. Done at once rather than left to a thread:
    /// when the app quits there is no later, and the port would stay
    /// forwarded for as long as adb runs.
    pub(crate) fn end(&self) {
        if self.ended.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let _ = self.video.shutdown(Shutdown::Both);
        if let Ok(mut process) = self.process.lock() {
            let _ = process.kill();
            let _ = process.wait();
        }
        let _ = self.adb.run(
            Some(&self.serial),
            &["forward", "--remove", &format!("tcp:{}", self.port)],
        );
    }

    /// What the server printed last.
    pub(crate) fn said(&self) -> String {
        self.said
            .lock()
            .map(|said| said.clone())
            .unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.end();
    }
}

/// Sends the server to the phone `serial` and starts it: its video, its
/// controls, and the server itself (dropped, it ends). `clipboard` is
/// called with the phone's clipboard when it is asked for
/// ([`Control::copy`]).
pub(crate) fn start(
    adb: &Adb,
    serial: &str,
    options: VideoOptions,
    clipboard: impl Fn(String) + Send + 'static,
) -> Result<(Video, Control, Server)> {
    push_server(adb, serial)?;
    // Told apart from another mirror of the same phone by a number (31
    // bits, as the server reads it).
    let id = session_id();
    let port: u16 = adb
        .text(
            Some(serial),
            &[
                "forward",
                "tcp:0",
                &format!("localabstract:scrcpy_{id:08x}"),
            ],
        )?
        .trim()
        .parse()
        .context("adb did not say which port it forwards")?;
    let unforward = || {
        let _ = adb.run(
            Some(serial),
            &["forward", "--remove", &format!("tcp:{port}")],
        );
    };
    let process = adb
        .command(Some(serial))
        .args([
            "shell",
            &format!("CLASSPATH={DEVICE_PATH}"),
            "app_process",
            "/",
            "com.genymobile.scrcpy.Server",
            VERSION,
            &format!("scid={id:08x}"),
            "log_level=info",
            "audio=false",
            // The phone listens and this computer connects: nothing has to
            // listen here.
            "tunnel_forward=true",
            // Nothing of the phone's settings is changed, so nothing is to
            // be put back by a second process.
            "cleanup=false",
            "send_device_meta=false",
            // The clipboards meet only when asked (Copy, Paste).
            "clipboard_autosync=false",
            &format!("max_size={}", options.max_size),
            &format!("video_bit_rate={}", options.bit_rate),
            &format!("max_fps={}", options.max_fps),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut process = match process {
        Ok(process) => process,
        Err(err) => {
            unforward();
            return Err(err).context("starting the screen server");
        }
    };
    let said: Arc<Mutex<String>> = Arc::default();
    if let Some(output) = process.stdout.take() {
        let said = said.clone();
        std::thread::Builder::new()
            .name("scrcpy-log".into())
            .spawn(move || {
                for line in ph_env::lossy_lines(output) {
                    tracing::info!("screen server: {line}");
                    if let Ok(mut said) = said.lock() {
                        *said = line;
                    }
                }
            })
            .ok();
    }
    let connected = connect(port, &mut process, &said);
    let (video, control) = match connected {
        Ok(sockets) => sockets,
        Err(err) => {
            let _ = process.kill();
            let _ = process.wait();
            unforward();
            return Err(err);
        }
    };
    // The server runs from memory now: its file is not left on the phone
    // (scrcpy's own clean-up does the same).
    let (tidy_adb, tidy_serial) = (adb.clone(), serial.to_string());
    std::thread::spawn(move || {
        let _ = tidy_adb.shell(&tidy_serial, &format!("rm -f {DEVICE_PATH}"));
    });
    let again = match video.try_clone() {
        Ok(again) => again,
        Err(err) => {
            let _ = process.kill();
            let _ = process.wait();
            unforward();
            return Err(err).context("the video connection");
        }
    };
    let server = Server {
        process: Mutex::new(process),
        adb: adb.clone(),
        serial: serial.to_string(),
        port,
        video: again,
        said,
        ended: Default::default(),
    };
    let (messages, queued) = channel::<Vec<u8>>();
    let mut writer = control.try_clone().context("the control connection")?;
    std::thread::Builder::new()
        .name("scrcpy-control".into())
        .spawn(move || {
            // Ends with the last `Control` (its sender goes), or the server.
            while let Ok(message) = queued.recv() {
                if writer.write_all(&message).is_err() {
                    break;
                }
                // The phone pastes a moment after its clipboard is set: a
                // second paste sent meanwhile would be pasted twice, in
                // place of the first.
                if message.first() == Some(&SET_CLIPBOARD) {
                    std::thread::sleep(PASTE_SETTLES);
                }
            }
            let _ = writer.shutdown(Shutdown::Both);
        })
        .context("the control thread")?;
    let mut reader = control;
    std::thread::Builder::new()
        .name("scrcpy-clipboard".into())
        .spawn(move || {
            while let Some(message) = read_device_message(&mut reader) {
                if let Some(text) = message {
                    clipboard(text);
                }
            }
        })
        .context("the clipboard thread")?;
    Ok((Video { socket: video }, Control { messages }, server))
}

/// Puts the server on the phone, through a pipe: no file of it is made on
/// this computer, where another program could put its own in its place.
fn push_server(adb: &Adb, serial: &str) -> Result<()> {
    let mut push = adb
        .command(Some(serial))
        .args(["exec-in", &format!("cat > {DEVICE_PATH}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("sending the screen server")?;
    let sent = push
        .stdin
        .take()
        .context("no pipe to adb")
        .and_then(|mut pipe| Ok(pipe.write_all(SERVER)?));
    let output = push.wait_with_output()?;
    sent.context("sending the screen server")?;
    if !output.status.success() {
        bail!(
            "sending the screen server: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// A number for one run of the server, different each time.
fn session_id() -> u32 {
    use std::hash::{BuildHasher as _, Hasher as _};
    // The standard library's randomly keyed hasher: no dependency for one
    // number that only has to differ from the last.
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (random as u32) & 0x7fff_ffff
}

/// Connects the video, then the controls, once the server listens.
fn connect(port: u16, process: &mut Child, said: &Mutex<String>) -> Result<(TcpStream, TcpStream)> {
    let mut video = None;
    for _ in 0..CONNECT_TRIES {
        if let Ok(Some(_)) = process.try_wait() {
            let said = said.lock().map(|said| said.clone()).unwrap_or_default();
            bail!("the screen server stopped: {said}");
        }
        // adb takes the connection whether or not the server listens yet:
        // a byte from the server says that it does.
        if let Ok(mut socket) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
            let mut byte = [1u8; 1];
            if socket.read_exact(&mut byte).is_ok() && byte[0] == 0 {
                video = Some(socket);
                break;
            }
        }
        std::thread::sleep(CONNECT_PAUSE);
    }
    let mut video = video.context("the screen server did not answer")?;
    let control = TcpStream::connect(("127.0.0.1", port)).context("the control connection")?;
    // A touch is a few bytes that must not wait for more to go with them.
    let _ = control.set_nodelay(true);
    let mut codec = [0u8; 4];
    video
        .read_exact(&mut codec)
        .context("the screen server sent no video")?;
    if u32::from_be_bytes(codec) != H264 {
        bail!("the screen server sent no H.264 video");
    }
    // From here the video is waited for as long as the screen is still.
    let _ = video.set_read_timeout(None);
    Ok((video, control))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The expected bytes are those of scrcpy's own tests
    // (`app/tests/test_control_msg_serialize.c`), with a finger for its
    // pointer and no buttons.

    #[test]
    fn a_key_is_its_action_and_code() {
        assert_eq!(
            key_message(true, 66),
            [0, 1, 0, 0, 0, 0x42, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(key_message(false, 66)[1], 0);
    }

    #[test]
    fn text_is_its_length_and_bytes() {
        let mut expected = vec![1, 0, 0, 0, 0x0d];
        expected.extend(b"hello, world!");
        assert_eq!(text_message("hello, world!"), expected);
        // No more than one message holds, cut between characters.
        let long = "é".repeat(200);
        let message = text_message(&long);
        assert_eq!(message.len(), 5 + 300);
        assert_eq!(&message[1..5], [0, 0, 0x01, 0x2c]);
        assert!(std::str::from_utf8(&message[5..]).is_ok());
    }

    #[test]
    fn a_touch_is_a_finger_at_a_place_in_the_video() {
        let message = touch_message(Touch::Down, (100, 200), (1080, 1920));
        #[rustfmt::skip]
        let expected = [
            2, 0,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
            0, 0, 0, 0x64, 0, 0, 0, 0xc8,
            0x04, 0x38, 0x07, 0x80,
            0xff, 0xff,
            0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(message, expected);
        let up = touch_message(Touch::Up, (100, 200), (1080, 1920));
        assert_eq!((up[1], &up[22..24]), (1, &[0u8, 0][..]));
        assert_eq!(touch_message(Touch::Move, (1, 2), (3, 4))[1], 2);
    }

    #[test]
    fn a_scroll_is_notches_in_fixed_point() {
        let message = scroll_message((260, 1026), (1080, 1920), (16.0, -16.0));
        #[rustfmt::skip]
        let expected = [
            3,
            0, 0, 0x01, 0x04, 0, 0, 0x04, 0x02,
            0x04, 0x38, 0x07, 0x80,
            0x7f, 0xff,
            0x80, 0x00,
            0, 0, 0, 0,
        ];
        assert_eq!(message, expected);
        // More than the most is the most.
        let far = scroll_message((0, 0), (1, 1), (0.0, 400.0));
        assert_eq!(&far[15..17], [0x7f, 0xff]);
    }

    #[test]
    fn a_paste_sets_the_clipboard_and_presses_paste() {
        let message = set_clipboard_message("hé", true);
        let mut expected = vec![9, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 3];
        expected.extend("hé".as_bytes());
        assert_eq!(message, expected);
    }

    #[test]
    fn the_phones_messages_are_read_one_by_one() {
        let mut bytes = vec![0, 0, 0, 0, 3];
        bytes.extend(b"abc");
        // An acknowledgement, and a report for a keyboard: nothing to do.
        bytes.extend([1, 0, 0, 0, 0, 0, 0, 0, 7]);
        bytes.extend([2, 0, 1, 0, 2, 9, 9]);
        bytes.extend([0, 0, 0, 0, 0]);
        let mut input = std::io::Cursor::new(bytes);
        assert_eq!(read_device_message(&mut input), Some(Some("abc".into())));
        assert_eq!(read_device_message(&mut input), Some(None));
        assert_eq!(read_device_message(&mut input), Some(None));
        assert_eq!(read_device_message(&mut input), Some(Some(String::new())));
        assert_eq!(read_device_message(&mut input), None, "the end");
        // A kind not known ends the reading.
        assert_eq!(
            read_device_message(&mut std::io::Cursor::new(vec![9, 1, 2])),
            None
        );
    }

    #[test]
    fn video_packets_are_told_apart_by_their_first_bits() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let writer = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            // A session 576 by 1280, the parameter sets, a key picture.
            socket
                .write_all(&[0x80, 0, 0, 0, 0, 0, 0x02, 0x40, 0, 0, 0x05, 0])
                .unwrap();
            socket
                .write_all(&[0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 7, 8])
                .unwrap();
            socket
                .write_all(&[0x20, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 3, 1, 2, 3])
                .unwrap();
        });
        let mut video = Video {
            socket: TcpStream::connect(("127.0.0.1", port)).unwrap(),
        };
        assert_eq!(
            video.next().unwrap(),
            Packet::Session {
                width: 576,
                height: 1280
            }
        );
        assert_eq!(video.next().unwrap(), Packet::Config(vec![7, 8]));
        assert_eq!(video.next().unwrap(), Packet::Picture(vec![1, 2, 3]));
        writer.join().unwrap();
        assert!(video.next().is_err(), "the end of the stream");
    }

    #[test]
    fn the_server_is_the_version_it_is_started_as() {
        // The file is a jar (a zip) named for the version asked of it.
        assert_eq!(&SERVER[..2], b"PK");
        assert!(SERVER.len() > 100_000);
        assert!(VERSION.split('.').all(|part| part.parse::<u32>().is_ok()));
    }

    #[test]
    fn session_ids_fit_in_31_bits() {
        assert!((0..50).all(|_| session_id() <= 0x7fff_ffff));
    }
}
