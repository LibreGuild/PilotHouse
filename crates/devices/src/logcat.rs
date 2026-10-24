//! Live logcat for the device on screen.

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader};
use std::process::{Child, Stdio};

use gpui_kit::{Context, Task};

use crate::adb::Adb;

/// Lines kept in memory.
const KEEP: usize = 5000;
/// History shown when logcat opens.
const BACKLOG: &str = "500";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub priority: Priority,
    pub text: String,
}

/// The priority letter of a `threadtime` line:
/// `10-24 10:00:00.123  1234  1250 E Tag: message`.
pub fn priority(line: &str) -> Priority {
    match line.split_whitespace().nth(4) {
        Some("E") | Some("F") | Some("A") => Priority::Error,
        Some("W") => Priority::Warn,
        Some("I") => Priority::Info,
        Some("D") => Priority::Debug,
        _ => Priority::Verbose,
    }
}

pub struct Logcat {
    pub lines: VecDeque<LogLine>,
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
                        if text.starts_with("---------") {
                            continue;
                        }
                        if logcat.lines.len() == KEEP {
                            logcat.lines.pop_front();
                        }
                        logcat.lines.push_back(LogLine {
                            priority: priority(&text),
                            text,
                        });
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

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.lines.clear();
        cx.notify();
    }

    /// The last `limit` lines matching `filter` (case-insensitive), oldest first.
    pub fn matching(&self, filter: &str, errors_only: bool, limit: usize) -> Vec<&LogLine> {
        let filter = filter.to_lowercase();
        let mut lines: Vec<&LogLine> = self
            .lines
            .iter()
            .rev()
            .filter(|line| !errors_only || line.priority >= Priority::Error)
            .filter(|line| filter.is_empty() || line.text.to_lowercase().contains(&filter))
            .take(limit)
            .collect();
        lines.reverse();
        lines
    }
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
}
