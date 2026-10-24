//! One shell on a PTY, emulated by `alacritty_terminal`: the screen and the
//! process, with no window. [`crate::TerminalView`] draws it and turns input
//! into bytes, so a shell keeps running while nothing shows it, and a new
//! view (in another window) picks it up as it is.
//!
//! The PTY is read on alacritty's own I/O thread. It wakes us through
//! [`Listener`]; bursts of wake-ups are coalesced into one notify, so a
//! command printing thousands of lines costs a handful of frames, not
//! thousands.

use std::borrow::Cow;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use gpui_kit::{ClipboardItem, Context, EventEmitter, SharedString, Task};

/// Lines kept above the screen for scrolling back.
const SCROLLBACK_LINES: usize = 10_000;

/// Forwards alacritty's events to the UI thread.
#[derive(Clone)]
pub(crate) struct Listener(smol::channel::Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.try_send(event);
    }
}

/// A shell that is running but not yet wrapped in a [`Terminal`].
pub struct StartedShell {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: EventLoopSender,
    events: smol::channel::Receiver<Event>,
    title: SharedString,
    columns: usize,
    rows: usize,
}

#[derive(Debug, Clone)]
pub enum TerminalEvent {
    TitleChanged,
    Exited,
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: EventLoopSender,
    title: SharedString,
    /// A fixed name (a run configuration's), shown instead of the shell's title.
    label: Option<SharedString>,
    exited: bool,
    /// How the program ended, once it has (`Some(0)`: it succeeded).
    exit_code: Option<i32>,
    columns: usize,
    rows: usize,
    _events: Task<()>,
}

impl EventEmitter<TerminalEvent> for Terminal {}

impl Terminal {
    /// Starts the user's login shell in `cwd`. Done before creating the
    /// terminal so a failure can be shown instead of an empty one. A folder
    /// on a mapped network drive gets a shell on the server, in the same
    /// folder there (through `ssh`, which may ask for a password).
    pub fn start(cwd: std::path::PathBuf) -> anyhow::Result<StartedShell> {
        if let Some(remote) = ph_remote::remote_of(&cwd) {
            let (program, args) = ph_remote::ssh::shell(&remote.mapping, &remote.path);
            let mut shell = Self::start_program(&program, args, home_or(cwd))?;
            shell.title = remote.mapping.host.clone().into();
            return Ok(shell);
        }
        let (shell, args) = ph_env::default_shell();
        Self::start_program(&shell.to_string_lossy(), args, cwd)
    }

    /// Starts `program` with `args` in `cwd` on a PTY (a shell, `ssh`,
    /// `ssh-keygen`). The terminal is named after the program.
    pub fn start_program(
        program: &str,
        args: Vec<String>,
        cwd: std::path::PathBuf,
    ) -> anyhow::Result<StartedShell> {
        let shell = std::path::PathBuf::from(program);
        let mut extra_env = ph_env::shell_env().clone();
        extra_env.insert("TERM".into(), "xterm-256color".into());
        extra_env.insert("COLORTERM".into(), "truecolor".into());
        extra_env.insert("TERM_PROGRAM".into(), "PilotHouse".into());

        let options = tty::Options {
            shell: Some(tty::Shell::new(program.to_string(), args)),
            working_directory: Some(cwd),
            drain_on_exit: true,
            env: extra_env,
            #[cfg(target_os = "windows")]
            escape_args: true,
        };
        let (columns, rows) = (80, 24);
        let size = WindowSize {
            num_lines: rows as u16,
            num_cols: columns as u16,
            cell_width: 8,
            cell_height: 16,
        };
        let pty = tty::new(&options, size, 0)?;

        let (sender, events) = smol::channel::unbounded();
        let listener = Listener(sender);
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &TermSize::new(columns, rows),
            listener.clone(),
        )));
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let pty = event_loop.channel();
        event_loop.spawn();

        let title = shell
            .file_stem()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "shell".into());
        Ok(StartedShell {
            term,
            pty,
            events,
            title: title.into(),
            columns,
            rows,
        })
    }

    pub fn new(shell: StartedShell, cx: &mut Context<Self>) -> Self {
        let receiver = shell.events;
        let events = cx.spawn(async move |this, cx| {
            while let Ok(first) = receiver.recv().await {
                // Drain the burst, then redraw once.
                let mut batch = vec![first];
                while let Ok(event) = receiver.try_recv() {
                    batch.push(event);
                }
                if this
                    .update(cx, |this, cx| this.handle_events(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            term: shell.term,
            pty: shell.pty,
            title: shell.title,
            label: None,
            exited: false,
            exit_code: None,
            columns: shell.columns,
            rows: shell.rows,
            _events: events,
        }
    }

    pub fn title(&self) -> &SharedString {
        self.label.as_ref().unwrap_or(&self.title)
    }

    pub fn label(&self) -> Option<&SharedString> {
        self.label.as_ref()
    }

    pub fn set_label(&mut self, label: impl Into<SharedString>) {
        self.label = Some(label.into());
    }

    /// Types `command` and presses Enter, as if the user had.
    pub fn run_command(&self, command: &str) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.write(format!("{command}\r").into_bytes());
    }

    /// Ctrl-C: stops whatever runs in the foreground.
    pub fn interrupt(&self) {
        self.write(vec![0x03]);
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// The program's exit code, once it has ended (`None` before, or when
    /// it was ended by a signal).
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    fn mark_exited(&mut self, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.exited, true) {
            cx.emit(TerminalEvent::Exited);
        }
    }

    fn handle_events(&mut self, events: Vec<Event>, cx: &mut Context<Self>) {
        for event in events {
            match event {
                Event::Title(title) => {
                    self.title = title.into();
                    cx.emit(TerminalEvent::TitleChanged);
                }
                Event::PtyWrite(text) => self.write(text.into_bytes()),
                Event::ClipboardStore(_, text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text))
                }
                // The exit code comes first, then the end of the screen.
                Event::ChildExit(status) => {
                    self.exit_code = status.code();
                    self.mark_exited(cx);
                }
                Event::Exit => self.mark_exited(cx),
                _ => {}
            }
        }
        cx.notify();
    }

    /// Sends `bytes` to the shell, as typed.
    pub(crate) fn write(&self, bytes: Vec<u8>) {
        let _ = self.pty.send(Msg::Input(Cow::Owned(bytes)));
    }

    /// The emulated screen, for drawing and selecting.
    pub(crate) fn term(&self) -> &Arc<FairMutex<Term<Listener>>> {
        &self.term
    }

    /// The grid's columns and rows.
    pub(crate) fn size(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }

    /// Resizes the grid and the PTY (cells are `cell` pixels wide and high).
    pub(crate) fn resize(&mut self, columns: usize, rows: usize, cell: (u16, u16)) {
        if (columns, rows) == (self.columns, self.rows) {
            return;
        }
        self.columns = columns;
        self.rows = rows;
        self.term.lock().resize(TermSize::new(columns, rows));
        let _ = self.pty.send(Msg::Resize(WindowSize {
            num_lines: rows as u16,
            num_cols: columns as u16,
            cell_width: cell.0,
            cell_height: cell.1,
        }));
    }

    /// Pastes `text`, bracketed when the program asked for that.
    pub(crate) fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let mut bytes = Vec::new();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.write(bytes);
    }

    /// The selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// The lines on screen now, without trailing blanks.
    pub fn visible_text(&self) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let mut lines = vec![String::new(); term.screen_lines()];
        for indexed in content.display_iter {
            if let Some(line) = lines.get_mut((indexed.point.line.0 + offset) as usize)
                && !indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER)
            {
                line.push(indexed.cell.c);
            }
        }
        let lines: Vec<String> = lines
            .into_iter()
            .map(|line| line.trim_end().to_string())
            .collect();
        let end = lines
            .iter()
            .rposition(|line| !line.is_empty())
            .map_or(0, |ix| ix + 1);
        lines[..end].join("\n")
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.pty.send(Msg::Shutdown);
    }
}

/// The home folder for a program that only needs some folder to start in
/// (`ssh` to a server), else `fallback`.
fn home_or(fallback: std::path::PathBuf) -> std::path::PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(std::path::PathBuf::from)
        .filter(|home| home.is_dir())
        .unwrap_or(fallback)
}
