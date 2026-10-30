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
    /// Program status reports (OSC 7501) found in the output.
    statuses: smol::channel::Receiver<crate::program_status::Message>,
    title: SharedString,
    /// The shell's name, as the user picked it (`zsh`, `PowerShell 7`).
    shell: SharedString,
    /// Where it started.
    cwd: std::path::PathBuf,
    columns: usize,
    rows: usize,
}

/// What an agent running in the terminal is doing, as its tab shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBadge {
    Working,
    /// It waits for the user.
    NeedsYou,
    /// It finished a turn the user has not looked at.
    Done,
}

#[derive(Debug, Clone)]
pub enum TerminalEvent {
    TitleChanged,
    /// The program in it reported a new state (OSC 7501).
    ProgramStatus,
    Exited,
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: EventLoopSender,
    title: SharedString,
    /// The shell started (see [`Terminal::shell`]).
    shell: SharedString,
    /// Where the shell started.
    cwd: std::path::PathBuf,
    /// A fixed name (a run configuration's), shown instead of the shell's title.
    label: Option<SharedString>,
    /// An agent's state, when an agent runs in it.
    badge: Option<AgentBadge>,
    /// What the program in it says about itself (OSC 7501), if it does.
    program: Option<crate::program_status::ProgramStatus>,
    _statuses: Task<()>,
    exited: bool,
    /// How the program ended, once it has (`Some(0)`: it succeeded).
    exit_code: Option<i32>,
    columns: usize,
    rows: usize,
    _events: Task<()>,
    /// When the program last wrote to the screen.
    last_output: Option<std::time::Instant>,
    /// Types a command once the shell has shown its prompt.
    _type_when_ready: Option<Task<()>>,
}

impl EventEmitter<TerminalEvent> for Terminal {}

impl Terminal {
    /// Starts the user's login shell in `cwd`. Done before creating the
    /// terminal so a failure can be shown instead of an empty one. A folder
    /// on a mapped network drive gets a shell on the server, in the same
    /// folder there (through `ssh`, which may ask for a password).
    pub fn start(cwd: std::path::PathBuf) -> anyhow::Result<StartedShell> {
        Self::start_with(cwd, None)
    }

    /// [`Self::start`] with `shell` (one of [`ph_env::shells`]) instead of
    /// the default. A folder on a network drive always gets the server's.
    pub fn start_with(
        cwd: std::path::PathBuf,
        shell: Option<&ph_env::Shell>,
    ) -> anyhow::Result<StartedShell> {
        if let Some(remote) = ph_remote::remote_of(&cwd) {
            let (program, args) = ph_remote::ssh::shell(&remote.mapping, &remote.path);
            let mut shell = Self::start_program(&program, args, home_or(cwd))?;
            shell.title = remote.mapping.host.clone().into();
            shell.shell = remote.mapping.host.clone().into();
            return Ok(shell);
        }
        match shell {
            Some(shell) => {
                let mut started =
                    Self::start_program(&shell.program.to_string_lossy(), shell.args.clone(), cwd)?;
                started.shell = shell.name.clone().into();
                Ok(started)
            }
            None => {
                let (shell, args) = ph_env::default_shell();
                Self::start_program(&shell.to_string_lossy(), args, cwd)
            }
        }
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
        // BSD `ls` and friends on macOS colour their output only when asked.
        extra_env
            .entry("CLICOLOR".into())
            .or_insert_with(|| "1".into());

        let options = tty::Options {
            shell: Some(tty::Shell::new(program.to_string(), args)),
            working_directory: Some(cwd.clone()),
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
        let (status_sender, statuses) = smol::channel::bounded(64);
        let pty = crate::program_status::Tapped::new(pty, status_sender);

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
        let title: SharedString = title.into();
        Ok(StartedShell {
            term,
            pty,
            events,
            statuses,
            shell: title.clone(),
            title,
            cwd,
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
        let statuses = shell.statuses;
        let status_task = cx.spawn(async move |this, cx| {
            while let Ok(message) = statuses.recv().await {
                if this
                    .update(cx, |this, cx| this.handle_status(message, cx))
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
            shell: shell.shell,
            cwd: shell.cwd,
            label: None,
            badge: None,
            program: None,
            _statuses: status_task,
            exited: false,
            exit_code: None,
            columns: shell.columns,
            rows: shell.rows,
            _events: events,
            last_output: None,
            _type_when_ready: None,
        }
    }

    /// The shell this terminal started (its name, not what it runs now).
    pub fn shell(&self) -> &SharedString {
        &self.shell
    }

    pub fn title(&self) -> &SharedString {
        self.label.as_ref().unwrap_or(&self.title)
    }

    pub fn label(&self) -> Option<&SharedString> {
        self.label.as_ref()
    }

    /// The title the program in it set, whatever its tab is named.
    pub fn program_title(&self) -> &SharedString {
        &self.title
    }

    pub fn set_label(&mut self, label: impl Into<SharedString>) {
        self.label = Some(label.into());
    }

    pub fn badge(&self) -> Option<AgentBadge> {
        self.badge
    }

    /// What the program running in it says it is doing (OSC 7501).
    pub fn program_status(&self) -> Option<&crate::program_status::ProgramStatus> {
        self.program.as_ref()
    }

    /// The user looked at it: a finished program's news is seen.
    pub fn acknowledge(&mut self, cx: &mut Context<Self>) {
        if self.badge == Some(AgentBadge::Done) {
            self.set_badge(None, cx);
        }
    }

    fn handle_status(&mut self, message: crate::program_status::Message, cx: &mut Context<Self>) {
        use crate::program_status::{Message, ProgramState};
        match message {
            // Saying yes is what makes a program report at all.
            Message::Query => self.write(crate::program_status::QUERY_REPLY.to_vec()),
            Message::Status(status) => {
                let badge = status.as_ref().and_then(|status| match status.state {
                    ProgramState::Working => Some(AgentBadge::Working),
                    ProgramState::Blocked => Some(AgentBadge::NeedsYou),
                    ProgramState::Done => Some(AgentBadge::Done),
                    ProgramState::Idle | ProgramState::Error => None,
                });
                // Done stays only as news: a program that sits idle after
                // reporting it keeps the dot until the user looks.
                let keep_done = self.badge == Some(AgentBadge::Done)
                    && status
                        .as_ref()
                        .is_some_and(|status| status.state == ProgramState::Idle);
                if !keep_done {
                    self.set_badge(badge, cx);
                }
                self.program = status;
                cx.emit(TerminalEvent::ProgramStatus);
                cx.notify();
            }
        }
    }

    /// Shows what the agent in it is doing on its tab.
    pub fn set_badge(&mut self, badge: Option<AgentBadge>, cx: &mut Context<Self>) {
        if self.badge != badge {
            self.badge = badge;
            cx.emit(TerminalEvent::TitleChanged);
            cx.notify();
        }
    }

    /// Types `command` and presses Enter, as if the user had.
    pub fn run_command(&self, command: &str) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.write(format!("{command}\r").into_bytes());
    }

    /// Types `command` once the shell is ready for it: it has drawn
    /// something (its prompt) and gone quiet. A shell still starting up
    /// (PowerShell loading its profile on a slow computer) can drop what is
    /// typed before; after a few seconds it is typed anyway.
    pub fn run_command_when_ready(&mut self, command: String, cx: &mut Context<Self>) {
        const QUIET: std::time::Duration = std::time::Duration::from_millis(250);
        const GIVE_UP: std::time::Duration = std::time::Duration::from_secs(8);
        let started = std::time::Instant::now();
        self._type_when_ready = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let Ok(ready) = this.update(cx, |terminal, _| {
                    terminal.exited
                        || started.elapsed() > GIVE_UP
                        || terminal
                            .last_output
                            .is_some_and(|last| last.elapsed() >= QUIET)
                }) else {
                    return;
                };
                if ready {
                    _ = this.update(cx, |terminal, _| {
                        if !terminal.exited {
                            terminal.run_command(&command);
                        }
                    });
                    return;
                }
            }
        }));
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
                // A program asks for a colour (OSC 4/10/11): it gets the one
                // it is drawn with, its own if it set one.
                Event::ColorRequest(index, format) => {
                    use gpui_kit::component::ActiveTheme as _;
                    let set = self.term.lock().colors()[index];
                    let color = set.unwrap_or_else(|| {
                        crate::palette::Palette::from_theme(cx.theme()).query(index)
                    });
                    self.write(format(color).into_bytes());
                }
                Event::ClipboardStore(_, text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text))
                }
                // The exit code comes first, then the end of the screen.
                Event::ChildExit(status) => {
                    self.exit_code = status.code();
                    self.mark_exited(cx);
                }
                Event::Exit => self.mark_exited(cx),
                Event::Wakeup => self.last_output = Some(std::time::Instant::now()),
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
        bytes.extend_from_slice(pasted(text, bracketed).as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.write(bytes);
    }

    /// The text of grid line `line`, one character per column (wide
    /// characters' second halves as spaces), so a column indexes it.
    pub(crate) fn line_text(&self, line: alacritty_terminal::index::Line) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        if line < alacritty_terminal::index::Line(-(grid.history_size() as i32))
            || line.0 >= term.screen_lines() as i32
        {
            return String::new();
        }
        let row = &grid[line];
        (0..term.columns())
            .map(|column| {
                let cell = &row[alacritty_terminal::index::Column(column)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    ' '
                } else {
                    cell.c
                }
            })
            .collect()
    }

    /// The selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// The folder the shell started in.
    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    /// The history and the screen as text, at most the last `max_lines`
    /// lines, without trailing blanks: what is kept when PilotHouse quits.
    pub fn history_text(&self, max_lines: usize) -> String {
        use alacritty_terminal::index::{Column, Line};
        let term = self.term.lock();
        let grid = term.grid();
        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        let mut lines: Vec<String> = (top..=bottom)
            .map(|line| {
                let row = &grid[Line(line)];
                (0..term.columns())
                    .filter_map(|column| {
                        let cell = &row[Column(column)];
                        (!cell.flags.contains(Flags::WIDE_CHAR_SPACER)).then_some(cell.c)
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    }

    /// Shows text kept from an earlier run above the new shell's output,
    /// dimmed, with a line saying when it is from.
    pub(crate) fn show_restored(&self, text: &str, note: &str) {
        let mut bytes = b"\x1b[2m".to_vec();
        for line in text.lines() {
            // Only text: whatever escapes it held are not replayed.
            let clean: String = line.chars().filter(|c| !c.is_control()).collect();
            bytes.extend_from_slice(clean.as_bytes());
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(
            format!("\x1b[0m\x1b[2m\u{2500}\u{2500} {note} \u{2500}\u{2500}\x1b[0m\r\n").as_bytes(),
        );
        let mut parser: alacritty_terminal::vte::ansi::Processor =
            alacritty_terminal::vte::ansi::Processor::new();
        parser.advance(&mut *self.term.lock(), &bytes);
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

/// `text` as typed by a paste: line breaks as Enter, and, while the
/// program reads a bracketed paste, without ESC, which could end the paste
/// early (`ESC [201~`) and run what follows as typed commands.
fn pasted(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    match bracketed {
        true => text.replace('\x1b', ""),
        false => text,
    }
}

#[cfg(test)]
mod paste_tests {
    use super::pasted;

    #[test]
    fn a_paste_cannot_end_bracketed_mode() {
        let text = "echo hi\x1b[201~rm -rf ~\n";
        assert_eq!(pasted(text, true), "echo hi[201~rm -rf ~\r");
        assert_eq!(pasted("a\r\nb", false), "a\rb");
    }
}
