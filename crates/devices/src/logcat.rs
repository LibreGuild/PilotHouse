//! Live logcat for the device in use, parsed into columns and filtered
//! with a small query language, after Android Studio's:
//!
//! - words match the tag or the message (all of them, any case); `-word`
//!   hides lines with it;
//! - `tag:Name` keeps a tag (part of it), `-tag:Name` hides one;
//! - `package:com.example` (or `app:`) keeps that app's processes, and
//!   `package:mine` those of the open project's apps;
//! - `pid:1234` keeps one process;
//! - `level:warn` keeps that level and above;
//! - `is:crash` keeps crashes (an uncaught exception, a native crash, an
//!   ANR), `is:stacktrace` the lines of stack traces.
//!
//! Lines are told to belong to an app by its process: the names of the
//! processes running when the log opens, and of each one ActivityManager
//! starts or ends after (and before, as far back as the log goes).

use std::collections::{HashMap, HashSet, VecDeque};
use std::process::{Child, Stdio};
use std::rc::Rc;

use gpui_kit::{AppContext as _, Context, EventEmitter, Task};

use crate::adb::Adb;

/// Lines kept in memory.
const KEEP: usize = 20_000;
/// History shown when logcat opens.
const BACKLOG: &str = "1000";
const BACKLOG_LINES: u64 = 1000;
/// Lines of one crash kept for its report: the exception and its causes.
const CRASH_LINES: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
    /// `F` (fatal) and `A` (assert).
    Fatal,
}

impl Priority {
    pub const ALL: [Priority; 6] = [
        Priority::Verbose,
        Priority::Debug,
        Priority::Info,
        Priority::Warn,
        Priority::Error,
        Priority::Fatal,
    ];

    pub fn letter(self) -> &'static str {
        match self {
            Priority::Verbose => "V",
            Priority::Debug => "D",
            Priority::Info => "I",
            Priority::Warn => "W",
            Priority::Error => "E",
            Priority::Fatal => "F",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Priority::Verbose => "Verbose",
            Priority::Debug => "Debug",
            Priority::Info => "Info",
            Priority::Warn => "Warning",
            Priority::Error => "Error",
            Priority::Fatal => "Fatal",
        }
    }

    /// `warn`, `W`, `warning`… as typed after `level:`.
    pub fn from_word(word: &str) -> Option<Self> {
        let word = word.to_ascii_lowercase();
        Self::ALL.into_iter().find(|priority| {
            let name = priority.name().to_ascii_lowercase();
            word == priority.letter().to_ascii_lowercase()
                || (word.len() >= 2 && name.starts_with(&word))
                || (*priority == Priority::Fatal && word == "assert")
        })
    }

    fn from_letter(letter: &str) -> Option<Self> {
        Some(match letter {
            "V" => Priority::Verbose,
            "D" => Priority::Debug,
            "I" => Priority::Info,
            "W" => Priority::Warn,
            "E" => Priority::Error,
            "F" | "A" => Priority::Fatal,
            _ => return None,
        })
    }
}

/// One `threadtime` line: `10-24 10:00:00.123  1234  1250 E Tag: message`.
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    /// `10:00:00.123` (the date is left out).
    pub time: String,
    pub pid: u32,
    pub tid: u32,
    pub priority: Priority,
    pub tag: String,
    pub message: String,
    /// The line as logcat printed it, for chat and copying.
    pub text: String,
    /// The tag and message in lowercase, as filters match them (made once,
    /// not at every redraw of thousands of lines).
    tag_lower: String,
    message_lower: String,
}

impl LogLine {
    /// Parses a line; one in another shape is kept whole as an Info message.
    /// Terminal colour codes and other control characters (some devices'
    /// logcat and some adb setups add them) are dropped first.
    pub fn parse(text: &str) -> Self {
        let cleaned = plain(text);
        let text = cleaned.as_str();
        Self::parse_threadtime(text).unwrap_or_else(|| Self {
            time: String::new(),
            pid: 0,
            tid: 0,
            priority: Priority::Info,
            tag: String::new(),
            message: text.to_string(),
            text: text.to_string(),
            tag_lower: String::new(),
            message_lower: text.to_lowercase(),
        })
    }

    fn parse_threadtime(text: &str) -> Option<Self> {
        let mut rest = text;
        let mut field = || {
            rest = rest.trim_start();
            let end = rest.find(char::is_whitespace)?;
            let (value, tail) = rest.split_at(end);
            rest = tail;
            Some(value)
        };
        let (_date, time, pid, tid, letter) = (field()?, field()?, field()?, field()?, field()?);
        let priority = Priority::from_letter(letter)?;
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let (tag, message) = rest
            .split_once(": ")
            .unwrap_or((rest.trim_end_matches(':'), ""));
        Some(Self {
            time: time.to_string(),
            pid: pid.parse().ok()?,
            tid: tid.parse().ok()?,
            priority,
            tag: tag.trim().to_string(),
            message: message.to_string(),
            text: text.to_string(),
            tag_lower: tag.trim().to_lowercase(),
            message_lower: message.to_lowercase(),
        })
    }
}

/// `text` without ANSI escape sequences (`ESC [ … letter`), a byte-order
/// mark or other control characters (tabs become spaces).
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // CSI: ESC [ parameters final-letter; other escapes: one char.
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() || c == '~' {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\t' => out.push(' '),
            '\u{feff}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A parsed filter: see the module docs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// Lowercase words, all of which must appear.
    pub words: Vec<String>,
    hidden_words: Vec<String>,
    tags: Vec<String>,
    hidden_tags: Vec<String>,
    /// The apps kept, as typed ([`MINE`] for the open project's).
    pub packages: Vec<String>,
    pids: Vec<u32>,
    /// The lowest level kept, when one is asked for.
    pub level: Option<Priority>,
    crash: bool,
    stack_trace: bool,
}

/// `package:mine`: the apps of the open project.
pub const MINE: &str = "mine";

impl Query {
    pub fn parse(input: &str) -> Self {
        let mut query = Self::default();
        for token in input.split_whitespace() {
            let lower = token.to_lowercase();
            if let Some(tag) = lower.strip_prefix("tag:") {
                query.tags.push(tag.to_string());
            } else if let Some(tag) = lower.strip_prefix("-tag:") {
                query.hidden_tags.push(tag.to_string());
            } else if let Some(package) = token
                .strip_prefix("package:")
                .or_else(|| token.strip_prefix("app:"))
            {
                if !package.is_empty() {
                    query.packages.push(package.to_string());
                }
            } else if let Some(pid) = lower.strip_prefix("pid:").and_then(|pid| pid.parse().ok()) {
                query.pids.push(pid);
            } else if let Some(level) = lower.strip_prefix("level:") {
                // A level still being typed keeps every line.
                query.level = Priority::from_word(level).or(query.level);
            } else if lower == "is:crash" {
                query.crash = true;
            } else if lower == "is:stacktrace" {
                query.stack_trace = true;
            } else if let Some(word) = lower.strip_prefix('-').filter(|word| !word.is_empty()) {
                query.hidden_words.push(word.to_string());
            } else {
                query.words.push(lower);
            }
        }
        query
    }

    /// Whether the filter asks for the open project's apps.
    pub fn wants_mine(&self) -> bool {
        self.packages.iter().any(|package| package == MINE)
    }

    /// Whether `line` passes. `process` is the name of the process that
    /// wrote it, when known, and `mine` the open project's application ids.
    pub fn matches(&self, line: &LogLine, process: Option<&str>, mine: &[String]) -> bool {
        if self.level.is_some_and(|level| line.priority < level) {
            return false;
        }
        if !self.packages.is_empty() {
            let Some(process) = process else {
                return false;
            };
            let kept = self.packages.iter().any(|package| {
                if package == MINE {
                    mine.iter().any(|id| is_process_of(process, id, true))
                } else {
                    is_process_of(process, package, false)
                }
            });
            if !kept {
                return false;
            }
        }
        if !self.pids.is_empty() && !self.pids.contains(&line.pid) {
            return false;
        }
        if (self.crash && !is_crash(line)) || (self.stack_trace && !is_stack_trace(line)) {
            return false;
        }
        let (tag, message) = (&line.tag_lower, &line.message_lower);
        if !self.tags.is_empty() && !self.tags.iter().any(|wanted| tag.contains(wanted)) {
            return false;
        }
        if self.hidden_tags.iter().any(|hidden| tag.contains(hidden)) {
            return false;
        }
        if self
            .hidden_words
            .iter()
            .any(|word| tag.contains(word) || message.contains(word))
        {
            return false;
        }
        self.words
            .iter()
            .all(|word| tag.contains(word) || message.contains(word))
    }
}

/// Whether the process named `process` belongs to the app `package`: the
/// app's main process or one of its others (`com.app:sync`). With
/// `variants`, also a build of it whose id has a suffix (`com.app.debug`),
/// for ids read from the build files, where the suffix is not known.
pub(crate) fn is_process_of(process: &str, package: &str, variants: bool) -> bool {
    match process.strip_prefix(package) {
        Some("") => true,
        Some(rest) => rest.starts_with(':') || (variants && rest.starts_with('.')),
        None => false,
    }
}

/// A line of a crash: an uncaught exception, a native crash or an ANR.
pub fn is_crash(line: &LogLine) -> bool {
    match line.tag.as_str() {
        "AndroidRuntime" => line.priority >= Priority::Error,
        "DEBUG" | "libc" => line.priority == Priority::Fatal,
        "ActivityManager" => line.message.starts_with("ANR in "),
        _ => false,
    }
}

fn is_stack_trace(line: &LogLine) -> bool {
    let message = line.message.trim_start();
    (message.starts_with("at ") && message.contains('('))
        || message.starts_with("Caused by:")
        || (message.starts_with("... ") && message.ends_with(" more"))
}

/// The first line of an uncaught exception: `FATAL EXCEPTION: main`.
fn starts_crash(line: &LogLine) -> bool {
    line.tag == "AndroidRuntime" && line.message.starts_with("FATAL EXCEPTION")
}

/// An app crashed: what its uncaught exception printed.
#[derive(Debug, Clone, PartialEq)]
pub struct Crash {
    /// The process that crashed, when the log said (`com.example.app`).
    pub process: Option<String>,
    pub pid: u32,
    /// The lines as logcat printed them, the exception and its stack.
    pub lines: Vec<String>,
}

impl Crash {
    /// The crash as text for the chat: which app, and what its exception
    /// printed.
    pub fn report(&self) -> String {
        let name = self.process.as_deref().unwrap_or("The app");
        format!(
            "{name} crashed on the device:\n```\n{}\n```",
            self.lines.join("\n")
        )
    }

    /// The exception: `java.lang.IllegalStateException: boom`.
    pub fn summary(&self) -> String {
        self.lines
            .iter()
            .filter_map(|line| line.split_once("AndroidRuntime: ").map(|(_, rest)| rest))
            .find(|message| {
                !message.starts_with("FATAL EXCEPTION")
                    && !message.starts_with("Process: ")
                    && !message.trim().is_empty()
            })
            .unwrap_or("The app crashed")
            .trim()
            .to_string()
    }
}

pub enum LogcatEvent {
    /// An app crashed with an uncaught exception.
    Crashed(Crash),
}

/// A crash being read: its lines come one by one.
struct OpenCrash {
    pid: u32,
    process: Option<String>,
    lines: Vec<String>,
    /// A line of it came since the lines last paused.
    growing: bool,
}

/// The lines a filter passes, as [`Logcat::matching`] returns them. A view
/// that draws them keeps `epoch` and `dropped` to tell what changed since:
/// another `epoch` is another list, a larger `dropped` is that many lines
/// gone from the front, and the rest is new lines at the end.
pub struct Shown {
    pub epoch: u64,
    pub dropped: u64,
    pub lines: Vec<Rc<LogLine>>,
}

pub struct Logcat {
    pub lines: VecDeque<Rc<LogLine>>,
    /// Lines ever pushed: the oldest kept is number `pushed - lines.len()`.
    pushed: u64,
    /// The lines the last filter passed, kept up to date as lines come.
    matched: Option<Matched>,
    /// Counts the lists made for [`Shown::epoch`].
    epochs: u64,
    /// The process each pid is, as far as known.
    processes: HashMap<u32, Rc<str>>,
    /// Pids that wrote lines before their process was known: learning one
    /// of them changes what a filter by app passes.
    unnamed: HashSet<u32>,
    /// Counts up when lines already here may now belong to an app.
    named: u64,
    crash: Option<OpenCrash>,
    /// Crashes read to their end, not told of yet.
    crashed: Vec<Crash>,
    /// Lines that were in the log before it opened are still coming: a
    /// crash among them is an old one, and no news.
    backlog: bool,
    child: Option<Child>,
    _reader: Option<Task<()>>,
    /// Ends `adb logcat` when PilotHouse quits (views are not dropped then).
    _quit: Option<gpui_kit::Subscription>,
}

impl EventEmitter<LogcatEvent> for Logcat {}

impl Drop for Logcat {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Logcat {
    pub fn new(adb: &Adb, serial: &str, cx: &mut Context<Self>) -> Self {
        let mut logcat = Self::empty();
        logcat._quit = Some(cx.on_app_quit(|logcat, _| {
            if let Some(mut child) = logcat.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            async {}
        }));
        let child = adb
            .command(Some(serial))
            .args(["logcat", "-v", "threadtime", "-T", BACKLOG])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!("starting logcat failed: {err}");
                return logcat;
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        logcat.child = Some(child);
        // The processes running now: their lines belong to their apps from
        // the first one, without waiting for ActivityManager to name them.
        let (names_adb, names_serial) = (adb.clone(), serial.to_string());
        cx.spawn(async move |this, cx| {
            let running = cx
                .background_spawn(async move { names_adb.processes(&names_serial) })
                .await;
            let _ = this.update(cx, |logcat, cx| {
                for (pid, name) in running {
                    logcat.name_process(pid, &name);
                }
                cx.notify();
            });
        })
        .detach();
        let (lines, receiver) = smol::channel::unbounded::<String>();
        std::thread::Builder::new()
            .name("logcat".into())
            .spawn(move || {
                for line in ph_env::lossy_lines(stdout) {
                    if lines.send_blocking(line).is_err() {
                        return;
                    }
                }
            })
            .ok();
        logcat._reader = Some(cx.spawn(async move |this, cx| {
            while let Ok(first) = receiver.recv().await {
                // Take everything already waiting, then redraw once.
                let mut batch = vec![first];
                while let Ok(line) = receiver.try_recv() {
                    batch.push(line);
                }
                let done = this.update(cx, |logcat, cx| {
                    for text in batch {
                        if !text.starts_with("---------") {
                            logcat.push(LogLine::parse(&text));
                        }
                    }
                    logcat.tell_crashes(cx);
                    cx.notify();
                });
                if done.is_err() {
                    return;
                }
                // Batch bursts: at most ~10 redraws a second.
                let waited = cx
                    .background_executor()
                    .timer(std::time::Duration::from_millis(100));
                waited.await;
                // A pause after the first lines: the log's past has been
                // read, and what comes now happens now.
                if receiver.is_empty() {
                    let _ = this.update(cx, |logcat, cx| {
                        logcat.caught_up();
                        logcat.tell_crashes(cx);
                    });
                }
            }
        }));
        logcat
    }

    fn empty() -> Self {
        Self {
            lines: VecDeque::new(),
            pushed: 0,
            matched: None,
            epochs: 0,
            processes: HashMap::new(),
            unnamed: HashSet::new(),
            named: 0,
            crash: None,
            crashed: Vec::new(),
            backlog: true,
            child: None,
            _reader: None,
            _quit: None,
        }
    }

    /// The lines have paused. The first time, the log's past has been
    /// read; a crash whose lines have stopped coming is told of.
    fn caught_up(&mut self) {
        if self.backlog {
            self.backlog = false;
            self.crash = None;
        } else if let Some(crash) = &mut self.crash {
            // Its lines may come in two parts with a pause between: it is
            // over when a whole pause brought none.
            if std::mem::take(&mut crash.growing) {
                return;
            }
            self.close_crash();
        }
    }

    /// Tells of the crashes whose reports are complete.
    fn tell_crashes(&mut self, cx: &mut Context<Self>) {
        for crash in std::mem::take(&mut self.crashed) {
            cx.emit(LogcatEvent::Crashed(crash));
        }
    }

    /// Learns that `pid` is the process `name`.
    fn name_process(&mut self, pid: u32, name: &str) {
        if self
            .processes
            .get(&pid)
            .is_some_and(|known| &**known == name)
        {
            return;
        }
        self.processes.insert(pid, name.into());
        // Its earlier lines are its app's too.
        if self.unnamed.remove(&pid) {
            self.named += 1;
        }
    }

    /// The process that wrote `line`, when known.
    pub fn process_of(&self, line: &LogLine) -> Option<&str> {
        self.processes.get(&line.pid).map(|name| &**name)
    }

    fn push(&mut self, line: LogLine) {
        if let Some((pid, name)) = named_process(&line) {
            let name = name.to_string();
            self.name_process(pid, &name);
        }
        if !self.processes.contains_key(&line.pid) {
            self.unnamed.insert(line.pid);
        }
        // On a busy phone the lines never pause: its past is then over once
        // as many lines have come as were asked of it.
        if self.backlog && self.pushed >= BACKLOG_LINES {
            self.backlog = false;
            self.crash = None;
        }
        self.follow_crash(&line);
        if self.lines.len() == KEEP {
            self.lines.pop_front();
        }
        self.lines.push_back(Rc::new(line));
        self.pushed += 1;
    }

    /// Gathers the lines of an uncaught exception as they come, and tells
    /// of the crash once they have all come.
    fn follow_crash(&mut self, line: &LogLine) {
        if starts_crash(line) {
            self.close_crash();
            self.crash = Some(OpenCrash {
                pid: line.pid,
                process: self.process_of(line).map(str::to_string),
                lines: vec![line.text.clone()],
                growing: true,
            });
            return;
        }
        let Some(crash) = &mut self.crash else {
            return;
        };
        // Other processes write between its lines.
        if line.pid != crash.pid {
            return;
        }
        // Its own next line under another tag is the end of the report.
        if line.tag != "AndroidRuntime" {
            self.close_crash();
            return;
        }
        crash.growing = true;
        if let Some(process) = line
            .message
            .strip_prefix("Process: ")
            .and_then(|rest| rest.split(',').next())
        {
            crash.process = Some(process.trim().to_string());
        }
        if crash.lines.len() < CRASH_LINES {
            crash.lines.push(line.text.clone());
        }
    }

    /// The crash being read is complete: it is told of, unless it is one
    /// from the log's past.
    fn close_crash(&mut self) {
        let Some(crash) = self.crash.take() else {
            return;
        };
        if self.backlog {
            return;
        }
        self.crashed.push(Crash {
            process: crash.process,
            pid: crash.pid,
            lines: crash.lines,
        });
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.lines.clear();
        self.matched = None;
        cx.notify();
    }

    /// Lines passing `query`, oldest first; `mine` are the open project's
    /// application ids. Only lines that came since the last call are
    /// filtered (with the same filter); the rest is remembered.
    pub fn matching(&mut self, query: &Query, mine: &[String]) -> Shown {
        let first = self.pushed - self.lines.len() as u64;
        let by_app = !query.packages.is_empty();
        let same = self.matched.as_ref().is_some_and(|matched| {
            matched.query == *query
                && matched.upto >= first
                && (!by_app || (matched.named == self.named && matched.mine == mine))
        });
        if !same {
            self.epochs += 1;
            self.matched = Some(Matched {
                epoch: self.epochs,
                query: query.clone(),
                mine: mine.to_vec(),
                named: self.named,
                upto: first,
                dropped: 0,
                lines: VecDeque::new(),
            });
        }
        let Self {
            lines,
            matched,
            pushed,
            processes,
            ..
        } = self;
        let matched = matched.as_mut().expect("made above");
        let new = (matched.upto - first) as usize;
        for (at, line) in lines.iter().enumerate().skip(new) {
            let process = processes.get(&line.pid).map(|name| &**name);
            if query.matches(line, process, mine) {
                matched.lines.push_back((first + at as u64, line.clone()));
            }
        }
        matched.upto = *pushed;
        // Lines dropped from the log go from the result too.
        while matched
            .lines
            .front()
            .is_some_and(|(number, _)| *number < first)
        {
            matched.lines.pop_front();
            matched.dropped += 1;
        }
        Shown {
            epoch: matched.epoch,
            dropped: matched.dropped,
            lines: matched.lines.iter().map(|(_, line)| line.clone()).collect(),
        }
    }

    /// The newest `limit` lines at `min` or above, oldest first.
    pub fn recent(&self, min: Priority, limit: usize) -> Vec<&LogLine> {
        let mut lines: Vec<&LogLine> = self
            .lines
            .iter()
            .rev()
            .filter(|line| line.priority >= min)
            .take(limit)
            .map(|line| line.as_ref())
            .collect();
        lines.reverse();
        lines
    }
}

/// The lines a filter passed: by their number, up to line `upto`.
struct Matched {
    epoch: u64,
    query: Query,
    mine: Vec<String>,
    /// [`Logcat::named`] when it was made: a process named since may have
    /// lines here that the filter left out.
    named: u64,
    upto: u64,
    /// How many of its lines have gone from the front.
    dropped: u64,
    lines: VecDeque<(u64, Rc<LogLine>)>,
}

/// A process ActivityManager names in `line`, starting or ending it, and a
/// crashing one named by its own report:
///
/// - `Start proc 12345:com.example/u0a190 for activity …`
/// - `Killing 12345:com.example/u0a190 (adj 900): remove task`
/// - `Process com.example (pid 12345) has died: fg  TOP`
/// - `Process: com.example, PID: 12345` (AndroidRuntime)
fn named_process(line: &LogLine) -> Option<(u32, &str)> {
    let message = line.message.as_str();
    if let Some(rest) = message
        .strip_prefix("Start proc ")
        .or_else(|| message.strip_prefix("Killing "))
    {
        let (pid, rest) = rest.split_once(':')?;
        let name = rest.split(['/', ' ']).next()?;
        return Some((pid.parse().ok()?, name));
    }
    if let Some(rest) = message.strip_prefix("Process: ") {
        let (name, pid) = rest.split_once(", PID: ")?;
        return Some((pid.trim().parse().ok()?, name.trim()));
    }
    let rest = message.strip_prefix("Process ")?;
    let (name, rest) = rest.split_once(" (pid ")?;
    let (pid, rest) = rest.split_once(')')?;
    if !rest.trim_start().starts_with("has died") {
        return None;
    }
    Some((pid.parse().ok()?, name))
}

/// What an agent asks of a `logcat -d` dump.
#[derive(Debug, Default)]
pub struct DumpFilter<'a> {
    /// Only lines containing this text, any case.
    pub text: Option<&'a str>,
    /// The lowest level kept.
    pub level: Option<Priority>,
    /// Only this app's lines, with its processes running now.
    pub package: Option<(&'a str, HashSet<u32>)>,
}

/// Filters a `logcat -d` dump for agents: the last `limit` lines passing.
pub fn filter_dump(dump: &str, filter: &DumpFilter, limit: usize) -> String {
    let text = filter.text.map(str::to_lowercase);
    let lines: Vec<LogLine> = dump
        .lines()
        .filter(|line| !line.starts_with("---------"))
        .map(LogLine::parse)
        .collect();
    // The app's processes: those running, and those the dump itself names
    // (one that crashed is not running any more).
    let pids = filter.package.as_ref().map(|(package, running)| {
        let mut pids = running.clone();
        pids.extend(
            lines
                .iter()
                .filter_map(named_process)
                .filter(|(_, name)| is_process_of(name, package, false))
                .map(|(pid, _)| pid),
        );
        pids
    });
    let kept: Vec<&str> = lines
        .iter()
        .filter(|line| filter.level.is_none_or(|level| line.priority >= level))
        .filter(|line| pids.as_ref().is_none_or(|pids| pids.contains(&line.pid)))
        .filter(|line| {
            text.as_ref()
                .is_none_or(|text| line.text.to_lowercase().contains(text))
        })
        .map(|line| line.text.as_str())
        .collect();
    kept[kept.len().saturating_sub(limit)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> LogLine {
        LogLine::parse(text)
    }

    #[test]
    fn colour_codes_and_control_characters_are_dropped() {
        let line = LogLine::parse(
            "\u{feff}\u{1b}[38;5;196m10-24 10:00:00.123  1234  1250 E Tag: boom\u{1b}[0m\r",
        );
        assert_eq!(line.time, "10:00:00.123");
        assert_eq!(line.tag, "Tag");
        assert_eq!(line.message, "boom");
    }

    const DUMP: &str = "--------- beginning of main\n\
        10-24 10:00:00.100   500   520 I ActivityManager: Start proc 1234:com.example.app/u0a190 for activity\n\
        10-24 10:00:00.123  1234  1250 I Example: hello\n\
        10-24 10:00:00.150   900   901 W Other: slow\n\
        10-24 10:00:00.200  1234  1250 E AndroidRuntime: FATAL EXCEPTION: main\n";

    #[test]
    fn a_dump_is_filtered_for_agents() {
        let errors = DumpFilter {
            level: Some(Priority::Error),
            ..Default::default()
        };
        assert_eq!(
            filter_dump(DUMP, &errors, 10),
            "10-24 10:00:00.200  1234  1250 E AndroidRuntime: FATAL EXCEPTION: main"
        );
        let text = DumpFilter {
            text: Some("OTHER"),
            ..Default::default()
        };
        assert!(filter_dump(DUMP, &text, 10).ends_with("W Other: slow"));
        // An app's lines, though it is not running any more: the dump names
        // its process.
        let app = DumpFilter {
            package: Some(("com.example.app", HashSet::new())),
            ..Default::default()
        };
        let kept = filter_dump(DUMP, &app, 10);
        assert_eq!(kept.lines().count(), 2);
        assert!(kept.contains("Example: hello") && kept.contains("FATAL EXCEPTION"));
        // The newest lines when there are more than asked for.
        assert!(filter_dump(DUMP, &DumpFilter::default(), 1).contains("FATAL EXCEPTION"));
    }

    #[test]
    fn parses_columns() {
        let line = LogLine::parse("10-24 10:00:00.200  1234  1250 E Android Runtime: FATAL: main");
        assert_eq!(line.time, "10:00:00.200");
        assert_eq!((line.pid, line.tid), (1234, 1250));
        assert_eq!(line.priority, Priority::Error);
        assert_eq!(line.tag, "Android Runtime");
        assert_eq!(line.message, "FATAL: main");
        let odd = LogLine::parse("not a log line");
        assert_eq!(odd.message, "not a log line");
        assert_eq!(
            LogLine::parse("10-24 10:00:00.200  1 2 A libc: abort").priority,
            Priority::Fatal
        );
    }

    #[test]
    fn queries_match_tags_words_and_processes() {
        let line = line("10-24 10:00:00.200  1234  1250 W OkHttp: slow request to api");
        let passes = |query: &str| Query::parse(query).matches(&line, None, &[]);
        assert!(passes("slow API"));
        assert!(passes("tag:okhttp"));
        assert!(!passes("-tag:okhttp"));
        assert!(!passes("tag:okhttp missing"));
        assert!(passes("pid:1234"));
        assert!(!passes("pid:9"));
        // Hidden words, and levels by name or letter.
        assert!(!passes("-slow"));
        assert!(passes("-fast"));
        assert!(passes("level:warn") && passes("level:W") && passes("level:info"));
        assert!(!passes("level:error"));
        assert!(passes("level:"), "a level still being typed hides nothing");
    }

    #[test]
    fn queries_keep_an_apps_processes() {
        let line = line("10-24 10:00:00.200  1234  1250 I Example: hello");
        let app = Query::parse("package:com.example");
        assert_eq!(app.packages, ["com.example"]);
        assert!(!app.matches(&line, None, &[]), "a process not known yet");
        assert!(app.matches(&line, Some("com.example"), &[]));
        assert!(app.matches(&line, Some("com.example:sync"), &[]));
        assert!(!app.matches(&line, Some("com.example.debug"), &[]));
        assert!(!app.matches(&line, Some("com.examples"), &[]));
        // The open project's apps, whatever suffix the build gave their id.
        let mine = Query::parse("package:mine");
        assert!(mine.wants_mine());
        let ids = ["com.example".to_string()];
        assert!(mine.matches(&line, Some("com.example.debug"), &ids));
        assert!(mine.matches(&line, Some("com.example:sync"), &ids));
        assert!(!mine.matches(&line, Some("com.other"), &ids));
        assert!(!mine.matches(&line, Some("com.example"), &[]));
    }

    #[test]
    fn crashes_and_stack_traces_are_told() {
        let crash = Query::parse("is:crash");
        let trace = Query::parse("is:stacktrace");
        let fatal = line("10-24 10:00:00.200  1234  1250 E AndroidRuntime: FATAL EXCEPTION: main");
        let frame = line(
            "10-24 10:00:00.200  1234  1250 E AndroidRuntime: \tat com.example.Main.onCreate(Main.kt:42)",
        );
        let cause = line("10-24 10:00:00.200  1 2 E Tag: Caused by: java.io.IOException: x");
        let plain = line("10-24 10:00:00.200  1 2 E Tag: look at this (now)");
        assert!(crash.matches(&fatal, None, &[]) && crash.matches(&frame, None, &[]));
        assert!(!crash.matches(&cause, None, &[]));
        assert!(trace.matches(&frame, None, &[]) && trace.matches(&cause, None, &[]));
        assert!(!trace.matches(&fatal, None, &[]) && !trace.matches(&plain, None, &[]));
        let anr = line("10-24 10:00:00.200  500  520 E ActivityManager: ANR in com.example");
        assert!(crash.matches(&anr, None, &[]));
    }

    #[test]
    fn processes_are_named_as_they_start_and_end() {
        let named = |message: &str| {
            let line = line(&format!("10-24 10:00:00.200  500  520 I Tag: {message}"));
            named_process(&line).map(|(pid, name)| (pid, name.to_string()))
        };
        assert_eq!(
            named("Start proc 4321:com.example.app/u0a190 for activity"),
            Some((4321, "com.example.app".into()))
        );
        assert_eq!(
            named("Killing 4321:com.example.app/u0a190 (adj 900): remove task"),
            Some((4321, "com.example.app".into()))
        );
        assert_eq!(
            named("Process com.example.app (pid 4321) has died: fg  TOP"),
            Some((4321, "com.example.app".into()))
        );
        assert_eq!(
            named("Process: com.example.app, PID: 4321"),
            Some((4321, "com.example.app".into()))
        );
        assert_eq!(named("Process com.example.app (pid 4321) is fine"), None);
        assert_eq!(named("nothing of the kind"), None);
    }

    #[test]
    fn the_lines_of_an_app_named_late_are_found() {
        let mut logcat = Logcat::empty();
        let push = |logcat: &mut Logcat, text: &str| logcat.push(line(text));
        let query = Query::parse("package:com.example");
        push(
            &mut logcat,
            "10-24 10:00:00.100  1234  1250 I Example: early",
        );
        let shown = logcat.matching(&query, &[]);
        assert!(shown.lines.is_empty());
        // Named now: the list is made again, with the early line in it.
        logcat.name_process(1234, "com.example");
        push(
            &mut logcat,
            "10-24 10:00:00.200  1234  1250 I Example: late",
        );
        let again = logcat.matching(&query, &[]);
        assert_ne!(again.epoch, shown.epoch);
        assert_eq!(again.lines.len(), 2);
        // More lines come to the same list.
        push(
            &mut logcat,
            "10-24 10:00:00.300  1234  1250 I Example: more",
        );
        let more = logcat.matching(&query, &[]);
        assert_eq!((more.epoch, more.dropped), (again.epoch, 0));
        assert_eq!(more.lines.len(), 3);
    }

    #[test]
    fn a_crash_is_gathered_until_its_lines_stop() {
        let mut logcat = Logcat::empty();
        let runtime = |logcat: &mut Logcat, message: &str| {
            logcat.push(line(&format!(
                "10-24 10:00:00.200  4321  4321 E AndroidRuntime: {message}"
            )))
        };
        // One in the log's past is no news.
        runtime(&mut logcat, "FATAL EXCEPTION: main");
        logcat.caught_up();
        assert!(logcat.crashed.is_empty() && logcat.crash.is_none());
        // One that happens now: its lines, another process between them.
        runtime(&mut logcat, "FATAL EXCEPTION: main");
        runtime(&mut logcat, "Process: com.example.app, PID: 4321");
        logcat.push(line("10-24 10:00:00.201  900  901 I Other: unrelated"));
        runtime(&mut logcat, "java.lang.IllegalStateException: boom");
        // A pause while it still grows is not its end; the next one is.
        logcat.caught_up();
        assert!(logcat.crashed.is_empty());
        runtime(&mut logcat, "\tat com.example.Main.run(Main.kt:9)");
        logcat.caught_up();
        assert!(logcat.crashed.is_empty());
        logcat.caught_up();
        let crash = logcat.crashed.pop().expect("told once its lines stopped");
        assert_eq!(crash.process.as_deref(), Some("com.example.app"));
        assert_eq!(crash.lines.len(), 4);
        assert_eq!(crash.summary(), "java.lang.IllegalStateException: boom");
        // Its own next line under another tag ends a report at once.
        runtime(&mut logcat, "FATAL EXCEPTION: main");
        logcat.push(line(
            "10-24 10:00:00.300  4321  4321 I Process: Sending signal. PID: 4321 SIG: 9",
        ));
        assert_eq!(logcat.crashed.len(), 1);
    }

    #[test]
    fn a_crash_is_summed_up_by_its_exception() {
        let crash = Crash {
            process: Some("com.example".into()),
            pid: 1,
            lines: vec![
                "10-24 10:00:00.200  1  2 E AndroidRuntime: FATAL EXCEPTION: main".into(),
                "10-24 10:00:00.200  1  2 E AndroidRuntime: Process: com.example, PID: 1".into(),
                "10-24 10:00:00.200  1  2 E AndroidRuntime: java.lang.IllegalStateException: boom"
                    .into(),
                "10-24 10:00:00.200  1  2 E AndroidRuntime: \tat com.example.Main.run(Main.kt:9)"
                    .into(),
            ],
        };
        assert_eq!(crash.summary(), "java.lang.IllegalStateException: boom");
    }
}
