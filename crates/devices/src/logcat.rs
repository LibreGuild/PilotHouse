//! Live logcat for the device on screen, parsed into columns and filtered
//! with a small query language:
//!
//! - words match the tag or the message (all of them, any case);
//! - `tag:Name` keeps a tag (part of it), `-tag:Name` hides one;
//! - `package:com.example` (or `app:`) keeps that app's processes;
//! - `pid:1234` keeps one process.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead as _, BufReader};
use std::process::{Child, Stdio};
use std::rc::Rc;

use gpui_kit::{AppContext as _, Context, Task};

use crate::adb::Adb;

/// Lines kept in memory.
const KEEP: usize = 20_000;
/// History shown when logcat opens.
const BACKLOG: &str = "1000";

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
    pub fn parse(text: &str) -> Self {
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

/// The priority of a `threadtime` line.
pub fn priority(line: &str) -> Priority {
    LogLine::parse(line).priority
}

/// A parsed filter: see the module docs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// Lowercase words, all of which must appear.
    pub words: Vec<String>,
    tags: Vec<String>,
    hidden_tags: Vec<String>,
    pub package: Option<String>,
    pids: Vec<u32>,
}

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
                query.package = Some(package.to_string());
            } else if let Some(pid) = lower.strip_prefix("pid:").and_then(|pid| pid.parse().ok()) {
                query.pids.push(pid);
            } else {
                query.words.push(lower);
            }
        }
        query
    }

    /// Whether `line` passes; `package_pids` are the processes of the
    /// query's package.
    pub fn matches(&self, line: &LogLine, package_pids: Option<&HashSet<u32>>) -> bool {
        if self.package.is_some() && !package_pids.is_some_and(|pids| pids.contains(&line.pid)) {
            return false;
        }
        if !self.pids.is_empty() && !self.pids.contains(&line.pid) {
            return false;
        }
        let tag = &line.tag_lower;
        if !self.tags.is_empty() && !self.tags.iter().any(|wanted| tag.contains(wanted)) {
            return false;
        }
        if self.hidden_tags.iter().any(|hidden| tag.contains(hidden)) {
            return false;
        }
        if self.words.is_empty() {
            return true;
        }
        let message = &line.message_lower;
        self.words
            .iter()
            .all(|word| tag.contains(word) || message.contains(word))
    }
}

pub struct Logcat {
    pub lines: VecDeque<Rc<LogLine>>,
    /// Lines ever pushed: the oldest kept is number `pushed - lines.len()`.
    pushed: u64,
    /// The lines the last filter passed, kept up to date as lines come.
    matched: Option<Matched>,
    /// The app the filter names now; a lookup started for another one (a
    /// name still being typed) is dropped.
    asking: Option<String>,
    /// Processes of each app asked about, from `pidof` and the
    /// "Start proc" lines that follow.
    package_pids: HashMap<String, HashSet<u32>>,
    adb: Adb,
    serial: String,
    child: Option<Child>,
    _reader: Option<Task<()>>,
}

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
        let mut logcat = Self {
            lines: VecDeque::new(),
            pushed: 0,
            matched: None,
            asking: None,
            package_pids: HashMap::new(),
            adb: adb.clone(),
            serial: serial.to_string(),
            child: None,
            _reader: None,
        };
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
        let (lines, receiver) = smol::channel::unbounded::<String>();
        std::thread::Builder::new()
            .name("logcat".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
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
                    cx.notify();
                });
                if done.is_err() {
                    return;
                }
                // Batch bursts: at most ~10 redraws a second.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
        }));
        logcat
    }

    fn push(&mut self, line: LogLine) {
        if let Some((pid, package)) = started_process(&line)
            && let Some(pids) = self.package_pids.get_mut(package)
        {
            pids.insert(pid);
        }
        if self.lines.len() == KEEP {
            self.lines.pop_front();
        }
        self.lines.push_back(Rc::new(line));
        self.pushed += 1;
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.lines.clear();
        self.matched = None;
        cx.notify();
    }

    /// The processes of `package` known so far; asks the device the first
    /// time (the answer arrives with a redraw).
    pub fn package_pids(&mut self, package: &str, cx: &mut Context<Self>) -> &HashSet<u32> {
        self.asking = Some(package.to_string());
        if !self.package_pids.contains_key(package) {
            self.package_pids
                .insert(package.to_string(), HashSet::new());
            let (adb, serial) = (self.adb.clone(), self.serial.clone());
            let (name, asked) = (package.to_string(), package.to_string());
            cx.spawn(async move |this, cx| {
                // While a name is typed, only the one it stops at is asked.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                let still = this.update(cx, |logcat, _| {
                    let still = logcat.asking.as_deref() == Some(name.as_str());
                    if !still {
                        logcat.package_pids.remove(&name);
                    }
                    still
                });
                if !still.unwrap_or(false) {
                    return;
                }
                let pids = cx
                    .background_spawn(async move { adb.pids_of(&serial, &asked) })
                    .await;
                let _ = this.update(cx, |logcat, cx| {
                    if let Some(known) = logcat.package_pids.get_mut(&name) {
                        known.extend(pids);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        &self.package_pids[package]
    }

    /// Lines passing `query` at `min` priority or above, oldest first.
    /// Only lines that came since the last call are filtered (with the
    /// same filter); the rest is remembered.
    pub fn matching(
        &mut self,
        query: &Query,
        min: Priority,
        cx: &mut Context<Self>,
    ) -> Vec<Rc<LogLine>> {
        let pids = match query.package.clone() {
            Some(package) => Some(self.package_pids(&package, cx).clone()),
            None => {
                self.asking = None;
                None
            }
        };
        let pid_count = pids.as_ref().map_or(0, HashSet::len);
        let first = self.pushed - self.lines.len() as u64;
        let same = self.matched.as_ref().is_some_and(|matched| {
            matched.query == *query
                && matched.min == min
                && matched.package_pids == pid_count
                && matched.upto >= first
        });
        if !same {
            self.matched = Some(Matched {
                query: query.clone(),
                min,
                package_pids: pid_count,
                upto: first,
                lines: VecDeque::new(),
            });
        }
        let Self {
            lines,
            matched,
            pushed,
            ..
        } = self;
        let Some(matched) = matched.as_mut() else {
            return Vec::new();
        };
        let new = (matched.upto - first) as usize;
        for (at, line) in lines.iter().enumerate().skip(new) {
            if line.priority >= min && query.matches(line, pids.as_ref()) {
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
        }
        matched.lines.iter().map(|(_, line)| line.clone()).collect()
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
    query: Query,
    min: Priority,
    /// How many processes the app had: more found means another result.
    package_pids: usize,
    upto: u64,
    lines: VecDeque<(u64, Rc<LogLine>)>,
}

/// "Start proc 12345:com.example/u0a190 for activity …" from ActivityManager.
fn started_process(line: &LogLine) -> Option<(u32, &str)> {
    let rest = line.message.strip_prefix("Start proc ")?;
    let (pid, rest) = rest.split_once(':')?;
    let package = rest.split(['/', ' ']).next()?;
    Some((pid.parse().ok()?, package))
}

/// Filters a `logcat -d` dump for agents.
pub fn filter_dump(dump: &str, filter: Option<&str>, errors_only: bool, limit: usize) -> String {
    let filter = filter.map(str::to_lowercase);
    let lines: Vec<&str> = dump
        .lines()
        .filter(|line| !line.starts_with("---------"))
        .filter(|line| !errors_only || priority(line) >= Priority::Error)
        .filter(|line| {
            filter
                .as_ref()
                .is_none_or(|filter| line.to_lowercase().contains(filter))
        })
        .collect();
    lines[lines.len().saturating_sub(limit)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_priorities_and_filters() {
        let dump = "--------- beginning of main\n10-24 10:00:00.123  1234  1250 I ActivityManager: Start proc\n10-24 10:00:00.200  1234  1250 E AndroidRuntime: FATAL EXCEPTION: main\n";
        assert_eq!(priority(dump.lines().nth(2).unwrap()), Priority::Error);
        assert_eq!(
            filter_dump(dump, None, true, 10),
            "10-24 10:00:00.200  1234  1250 E AndroidRuntime: FATAL EXCEPTION: main"
        );
        assert!(filter_dump(dump, Some("activitymanager"), false, 10).contains("Start proc"));
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
        let line = LogLine::parse("10-24 10:00:00.200  1234  1250 W OkHttp: slow request to api");
        assert!(Query::parse("slow API").matches(&line, None));
        assert!(Query::parse("tag:okhttp").matches(&line, None));
        assert!(!Query::parse("-tag:okhttp").matches(&line, None));
        assert!(!Query::parse("tag:okhttp missing").matches(&line, None));
        assert!(Query::parse("pid:1234").matches(&line, None));
        assert!(!Query::parse("pid:9").matches(&line, None));
        let app = Query::parse("package:com.example");
        assert_eq!(app.package.as_deref(), Some("com.example"));
        assert!(!app.matches(&line, None));
        assert!(app.matches(&line, Some(&HashSet::from([1234]))));
    }

    #[test]
    fn notices_started_processes() {
        let line = LogLine::parse(
            "10-24 10:00:00.200  500  520 I ActivityManager: Start proc 4321:com.example.app/u0a190 for activity",
        );
        assert_eq!(started_process(&line), Some((4321, "com.example.app")));
    }
}
