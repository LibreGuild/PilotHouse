//! The Program Status Protocol (OSC 7501): a program in the terminal (Claude
//! Code, Codex, Antigravity…) writes its own state into its output: idle,
//! working, blocked (on a permission, a question or a sign-in), done or
//! error, with a line about it. A program first asks whether the terminal
//! understands (`OSC 7501 ; ?`) and reports only after the answer.
//! Spec: <https://www.superlogical.com/rex/docs/build/program-status>.
//!
//! Alacritty drops OSC codes it does not know, so the PTY's output is read
//! through [`Tapped`] first: it passes every byte on unchanged and picks the
//! reports out on the way.

use std::io;
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use base64::Engine as _;

/// What a terminal that understands the protocol answers to its probe.
pub const QUERY_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

const PREFIX: &[u8] = b"\x1b]7501;";
/// The longest report read; longer ones are dropped.
const MAX_BODY: usize = 4096;

/// A program's state, as it reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramState {
    Idle,
    Working,
    /// Waiting for the user (see [`ProgramStatus::kind`]).
    Blocked,
    Done,
    Error,
}

/// What blocks a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

/// The program's own report about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramStatus {
    pub state: ProgramState,
    pub kind: Option<BlockedKind>,
    /// Which program (`claude`, `codex`, …), when it says.
    pub app: Option<String>,
    /// Its session's name, when it says.
    pub title: Option<String>,
    /// A line about what it does or needs.
    pub msg: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// The program asks whether the terminal speaks the protocol.
    Query,
    /// A new state for the program itself (`None`: it cleared its state).
    Status(Option<ProgramStatus>),
}

/// Reads one sequence's body (between `OSC 7501;` and its end). Reports
/// about a program's children (with an `id`) are left out.
pub fn parse_body(body: &str) -> Option<Message> {
    if body.len() > MAX_BODY {
        return None;
    }
    if body.trim() == "?" {
        return Some(Message::Query);
    }
    let (mut state, mut kind, mut app, mut title, mut msg, mut id) =
        (None, None, None, None, None, None);
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "state" => state = Some(value.to_string()),
            "kind" => kind = Some(value.to_string()),
            "app" if value.len() <= 32 => app = Some(value.to_string()),
            "title" => title = Some(decode(value, 256)?),
            "msg" => msg = Some(decode(value, 2732)?),
            "id" => id = Some(value.to_string()),
            _ => {}
        }
    }
    if id.is_some() {
        return None;
    }
    let state = match state?.as_str() {
        "clear" => return Some(Message::Status(None)),
        "idle" => ProgramState::Idle,
        "working" => ProgramState::Working,
        "blocked" => ProgramState::Blocked,
        "done" => ProgramState::Done,
        "error" => ProgramState::Error,
        _ => return None,
    };
    let kind = match (state, kind.as_deref()) {
        (ProgramState::Blocked, Some("permission")) => Some(BlockedKind::Permission),
        (ProgramState::Blocked, Some("question")) => Some(BlockedKind::Question),
        (ProgramState::Blocked, Some("auth")) => Some(BlockedKind::Auth),
        _ => None,
    };
    let clean = |text: Option<String>| text.filter(|text| !text.trim().is_empty());
    Some(Message::Status(Some(ProgramStatus {
        state,
        kind,
        app: app.filter(|app| {
            app.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
        }),
        title: clean(title),
        msg: clean(msg),
    })))
}

/// Base64 text of at most `max` encoded bytes, without control characters.
fn decode(encoded: &str, max: usize) -> Option<String> {
    if encoded.len() > max {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(encoded.trim_end_matches('='))
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

/// Finds `OSC 7501` sequences in output that arrives in pieces.
#[derive(Default)]
pub struct Scanner {
    /// How much of the prefix matched so far.
    matched: usize,
    /// The body, while inside a sequence.
    body: Option<Vec<u8>>,
    /// The last byte inside a body was ESC (ST may follow).
    escape: bool,
}

impl Scanner {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Message>) {
        for &byte in bytes {
            if let Some(body) = self.body.as_mut() {
                let ends = byte == 0x07 || (self.escape && byte == b'\\');
                if ends {
                    if self.escape {
                        body.pop();
                    }
                    if let Some(message) = std::str::from_utf8(body).ok().and_then(parse_body) {
                        out.push(message);
                    }
                    self.body = None;
                    self.escape = false;
                    continue;
                }
                self.escape = byte == 0x1b;
                body.push(byte);
                if body.len() > MAX_BODY {
                    self.body = None;
                    self.escape = false;
                }
                continue;
            }
            if byte == PREFIX[self.matched] {
                self.matched += 1;
                if self.matched == PREFIX.len() {
                    self.matched = 0;
                    self.body = Some(Vec::new());
                }
            } else {
                self.matched = usize::from(byte == PREFIX[0]);
            }
        }
    }
}

/// A PTY whose output is scanned for program status on its way to the
/// terminal emulator. Reports go to `sender`.
pub(crate) struct Tapped<P: EventedReadWrite> {
    pty: P,
    tap: Tap<P::Reader>,
}

pub(crate) struct Tap<R> {
    /// The PTY's own reader, refreshed each time [`Tapped::reader`] hands
    /// the tap out (it lives in `pty`, which moves with the tap).
    inner: *mut R,
    scanner: Scanner,
    sender: smol::channel::Sender<Message>,
}

// The tap only points into the PTY it travels with, and is used by the one
// thread that owns them both.
unsafe impl<P: EventedReadWrite + Send> Send for Tapped<P> {}

impl<P: EventedReadWrite> Tapped<P> {
    pub(crate) fn new(pty: P, sender: smol::channel::Sender<Message>) -> Self {
        Self {
            pty,
            tap: Tap {
                inner: std::ptr::null_mut(),
                scanner: Scanner::default(),
                sender,
            },
        }
    }
}

impl<R: io::Read> io::Read for Tap<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: set by `Tapped::reader` from the PTY that owns the reader,
        // right before the event loop reads through it.
        let read = unsafe { (*self.inner).read(buf) }?;
        let mut messages = Vec::new();
        self.scanner.feed(&buf[..read], &mut messages);
        for message in messages {
            let _ = self.sender.try_send(message);
        }
        Ok(read)
    }
}

impl<P: EventedReadWrite> EventedReadWrite for Tapped<P> {
    type Reader = Tap<P::Reader>;
    type Writer = P::Writer;

    unsafe fn register(
        &mut self,
        poll: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        // SAFETY: the same contract as the wrapped PTY's.
        unsafe { self.pty.register(poll, interest, mode) }
    }

    fn reregister(
        &mut self,
        poll: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        self.pty.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<polling::Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        self.tap.inner = self.pty.reader();
        &mut self.tap
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.pty.writer()
    }
}

impl<P: EventedPty> EventedPty for Tapped<P> {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl<P: EventedReadWrite + OnResize> OnResize for Tapped<P> {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.pty.on_resize(window_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Message> {
        let mut scanner = Scanner::default();
        let mut out = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, &mut out);
        }
        out
    }

    #[test]
    fn finds_reports_split_across_reads() {
        let msg = base64::engine::general_purpose::STANDARD.encode("Allow Bash?");
        let report =
            format!("x\x1b]7501;state=blocked:kind=permission:app=claude:msg={msg}\x1b\\y");
        let (a, b) = report.as_bytes().split_at(12);
        let out = scan(&[b"\x1b]7501;?\x07", a, b, b"\x1b]7501;state=done\x07"]);
        assert_eq!(out[0], Message::Query);
        assert_eq!(
            out[1],
            Message::Status(Some(ProgramStatus {
                state: ProgramState::Blocked,
                kind: Some(BlockedKind::Permission),
                app: Some("claude".into()),
                title: None,
                msg: Some("Allow Bash?".into()),
            }))
        );
        assert!(
            matches!(&out[2], Message::Status(Some(status)) if status.state == ProgramState::Done)
        );
    }

    #[test]
    fn ignores_what_is_not_a_root_report() {
        assert_eq!(
            scan(&[b"\x1b]0;title\x07\x1b]7501;state=working:id=sub\x07"]),
            vec![]
        );
        assert_eq!(parse_body("state=nonsense"), None);
        assert_eq!(parse_body("state=clear"), Some(Message::Status(None)));
        let long = format!("\x1b]7501;state=working:msg={}\x07", "A".repeat(5000));
        assert_eq!(scan(&[long.as_bytes()]), vec![]);
    }
}
