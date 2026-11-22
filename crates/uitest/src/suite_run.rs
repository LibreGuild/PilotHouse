//! Running a suite: its command, through the shell, in its folder, with
//! what it prints kept and the results it writes read ([`crate::suite_read`]).
//!
//! Everything here blocks until the command ends: it is run on a background
//! thread, told to stop through [`Stop`].

use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::suite::{Case, Outcome, Suite, SuiteResult};

/// What a command printed that is kept for reading its results (a runner
/// that prints them, like `cargo test`): the rest of a longer run is left
/// out from the middle.
const OUTPUT_MAX: usize = 8 * 1024 * 1024;
/// The end of the output kept with the result, for the user to read.
const OUTPUT_KEPT: usize = 200 * 1024;

/// Stops a run from another thread, and says what it has printed so far.
#[derive(Clone, Default)]
pub struct Stop {
    stopped: Arc<AtomicBool>,
    /// The id of the command's process (and of its group, on Unix).
    process: Arc<Mutex<Option<u32>>>,
    output: Arc<Mutex<String>>,
}

impl Stop {
    /// Ends the command and everything it started.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        let Some(id) = self.process.lock().ok().and_then(|process| *process) else {
            return;
        };
        kill_tree(id, false);
        // What does not end when asked is ended: a runner waiting on a
        // child of its own, a build that catches the signal.
        let process = self.process.clone();
        std::thread::spawn(move || {
            std::thread::sleep(STOP_GRACE);
            // Still the same command: not one started since.
            if process.lock().is_ok_and(|now| *now == Some(id)) {
                kill_tree(id, true);
            }
        });
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// The last `lines` lines printed so far.
    pub fn tail(&self, lines: usize) -> String {
        let Ok(output) = self.output.lock() else {
            return String::new();
        };
        let start = output
            .rmatch_indices('\n')
            .nth(lines)
            .map_or(0, |(at, _)| at + 1);
        output[start..].to_string()
    }
}

/// How long a command asked to stop has to end by itself.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Ends process `id` and its children: a test runner's own children (the
/// test binaries, a browser) would otherwise run on. Asked to end, or with
/// `force` ended.
fn kill_tree(id: u32, force: bool) {
    if cfg!(windows) {
        let _ = ph_env::command("taskkill")
            .args(["/PID", &id.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    } else {
        // The command leads a process group of its own (see `spawn`).
        let signal = if force { "-KILL" } else { "-TERM" };
        let _ = std::process::Command::new("/bin/kill")
            .args([signal, &format!("-{id}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Starts `line` through the shell in `cwd`, its errors in the same stream
/// as its output (a reader needs them in the order they were printed).
fn spawn(line: &str, cwd: &Path, environment: &[(&'static str, String)]) -> std::io::Result<Child> {
    let mut command;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command = ph_env::command("cmd");
        // As typed: `cmd` reads its own quotes.
        command.raw_arg(format!("/S /C \"({line}) 2>&1\""));
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt as _;
        command = ph_env::command("/bin/sh");
        command.args(["-c", &format!("exec 2>&1; {line}")]);
        // A group of its own, so that stopping it stops what it started.
        command.process_group(0);
        // No signal blocked: a child keeps the blocked signals of the thread
        // that starts it, and the app's background threads block most. A
        // runner that could not be told to end (or to end its own children:
        // a dev server a test started) would hang, where in a terminal it
        // does not.
        // SAFETY: between fork and exec, only calls that are safe there.
        unsafe {
            command.pre_exec(|| {
                let mut none: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut none);
                libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
                Ok(())
            });
        }
        // As the user's shell has it: the tools on their `PATH`.
        command.envs(ph_env::shell_env());
    }
    command
        .current_dir(cwd)
        .envs(environment.iter().map(|(name, value)| (*name, value)))
        // Plain text: colour codes are noise in a result.
        .env("NO_COLOR", "1")
        .env("CARGO_TERM_COLOR", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

/// Runs `suite` (or only `only` of it), with `dir` for the results it
/// writes. `before` is what the last full run found: a run of a part
/// replaces only the cases it ran.
pub fn run(
    suite: &Suite,
    only: &[Case],
    before: Option<&SuiteResult>,
    dir: &Path,
    stop: &Stop,
) -> SuiteResult {
    let started_at = crate::run::now_millis();
    let started = Instant::now();
    let results = dir.join("results");
    let _ = std::fs::create_dir_all(dir);
    // What an earlier run wrote is not this run's.
    let _ = std::fs::remove_file(&results);
    let only_refs: Vec<&Case> = only.iter().collect();
    let line = suite.command_line(&only_refs, &results);
    let failed_to_start = |error: String| SuiteResult {
        started_at,
        millis: 0,
        exit: None,
        stopped: false,
        cases: Vec::new(),
        output: error,
        partial: false,
    };
    let mut child = match spawn(&line, &suite.cwd, &suite.environment(&results)) {
        Ok(child) => child,
        Err(err) => return failed_to_start(format!("Could not start `{line}`: {err}")),
    };
    if let Ok(mut process) = stop.process.lock() {
        *process = Some(child.id());
    }
    if let Ok(mut output) = stop.output.lock() {
        output.clear();
    }
    // Asked to stop while it started.
    if stop.is_stopped() {
        kill_tree(child.id(), true);
    }
    let mut all = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let mut buffer = [0u8; 16 * 1024];
        // Bytes of a character cut by the end of a read.
        let mut pending: Vec<u8> = Vec::new();
        while let Ok(read) = pipe.read(&mut buffer) {
            if read == 0 {
                break;
            }
            pending.extend_from_slice(&buffer[..read]);
            let whole = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(err) if err.error_len().is_none() => err.valid_up_to(),
                // Not text at all: taken as it is.
                Err(_) => pending.len(),
            };
            let text = plain(&String::from_utf8_lossy(&pending[..whole]));
            pending.drain(..whole);
            if all.len() + text.len() <= OUTPUT_MAX {
                all.push_str(&text);
            }
            if let Ok(mut output) = stop.output.lock() {
                output.push_str(&text);
                // Only its end is shown while it runs.
                if output.len() > 2 * OUTPUT_KEPT {
                    let cut = floor_boundary(&output, output.len() - OUTPUT_KEPT);
                    output.drain(..cut);
                }
            }
        }
    }
    let exit = child.wait().ok().and_then(|status| status.code());
    if let Ok(mut process) = stop.process.lock() {
        *process = None;
    }
    let stopped = stop.is_stopped();
    let ran = if stopped {
        Vec::new()
    } else {
        crate::suite_read::read(suite, &all, &results)
    };
    let partial = !only.is_empty();
    let cases = match before {
        Some(before) if partial => merge(&before.cases, ran),
        _ => ran,
    };
    let kept = floor_boundary(&all, all.len().saturating_sub(OUTPUT_KEPT));
    SuiteResult {
        started_at,
        millis: started.elapsed().as_millis() as u64,
        exit,
        stopped,
        cases,
        output: all[kept..].to_string(),
        partial,
    }
}

/// The cases of the run before, with those just run in place of their old
/// selves (and any new ones at the end).
fn merge(before: &[Case], ran: Vec<Case>) -> Vec<Case> {
    let mut cases = before.to_vec();
    for case in ran {
        match cases
            .iter_mut()
            .find(|old| old.group == case.group && old.name == case.name)
        {
            Some(old) => *old = case,
            None => cases.push(case),
        }
    }
    cases
}

/// The start of the character at or before byte `at`.
fn floor_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// `text` without the codes a terminal reads (colours, cursor moves), and
/// with what a progress line rewrote left as it ended.
fn plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                // `ESC [ … letter`.
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // `ESC ] … BEL` (a title, a link).
                Some(']') => {
                    for c in chars.by_ref() {
                        if c == '\u{7}' {
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            },
            '\r' if chars.peek() == Some(&'\n') => {}
            // A line a progress bar draws over: its start is dropped.
            '\r' => {
                let start = plain.rfind('\n').map_or(0, |at| at + 1);
                plain.truncate(start);
            }
            c => plain.push(c),
        }
    }
    plain
}

/// How a result reads in a line: `12 passed, 1 failed`, `Could not run`.
pub fn summary(result: &SuiteResult) -> String {
    if result.stopped {
        return "Stopped".into();
    }
    let (passed, failed, skipped) = (
        result.count(Outcome::Passed),
        result.count(Outcome::Failed),
        result.count(Outcome::Skipped),
    );
    if result.cases.is_empty() {
        return match result.exit {
            Some(0) => "Passed".into(),
            Some(code) => format!("Failed (exit {code})"),
            None => "Could not run".into(),
        };
    }
    let mut parts = Vec::new();
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    parts.push(format!("{passed} passed"));
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
    }
    let mut summary = parts.join(", ");
    if result.broke() {
        summary.push_str(match result.exit {
            Some(_) => ", and the command failed",
            None => ", and the command did not end",
        });
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tests that run a command are written for a POSIX shell.
    #[cfg(unix)]
    fn other(command: &str, cwd: &Path) -> Suite {
        Suite {
            id: "x".into(),
            name: "x".into(),
            kind: crate::suite::Kind::Other,
            command: command.into(),
            cwd: cwd.into(),
            custom: true,
        }
    }

    #[test]
    fn terminal_codes_are_not_part_of_what_was_printed() {
        assert_eq!(plain("\u{1b}[32mok\u{1b}[0m\r\n"), "ok\n");
        assert_eq!(plain("a\nBuilding 1/3\rBuilding 3/3\rdone\n"), "a\ndone\n");
        assert_eq!(plain("\u{1b}]0;title\u{7}text"), "text");
    }

    #[cfg(unix)]
    #[test]
    fn a_command_passes_or_fails_by_how_it_ends() {
        let dir = std::env::temp_dir().join(format!("ph-suite-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ok = run(
            &other("echo out; echo err >&2", &dir),
            &[],
            None,
            &dir,
            &Stop::default(),
        );
        assert_eq!(ok.exit, Some(0));
        assert!(ok.passed());
        assert_eq!(ok.output, "out\nerr\n", "errors are in the same stream");
        assert_eq!(summary(&ok), "Passed");
        let failed = run(
            &other("echo no; exit 3", &dir),
            &[],
            None,
            &dir,
            &Stop::default(),
        );
        assert!(failed.broke());
        assert_eq!(summary(&failed), "Failed (exit 3)");
        let missing = run(
            &other("surely-no-such-program-here", &dir),
            &[],
            None,
            &dir,
            &Stop::default(),
        );
        assert!(!missing.passed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_run_is_stopped_with_what_it_started() {
        let dir = std::env::temp_dir().join(format!("ph-suite-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stop = Stop::default();
        let stopper = stop.clone();
        let waiter = std::thread::spawn(move || {
            // Once it has said it runs.
            for _ in 0..200 {
                if stopper.tail(5).contains("started") {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            stopper.stop();
        });
        let started = Instant::now();
        // A child of the shell, which a kill of the shell alone would leave.
        let result = run(
            &other("echo started; sleep 30; echo done", &dir),
            &[],
            None,
            &dir,
            &stop,
        );
        waiter.join().unwrap();
        assert!(result.stopped && !result.passed());
        assert!(
            started.elapsed().as_secs() < 20,
            "it did not wait for the sleep"
        );
        assert_eq!(summary(&result), "Stopped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_run_again_replaces_only_its_cases() {
        let case = |name: &str, outcome| Case {
            group: "g".into(),
            name: name.into(),
            outcome,
            millis: None,
            message: None,
            file: None,
            line: None,
        };
        let before = [case("a", Outcome::Passed), case("b", Outcome::Failed)];
        let merged = merge(
            &before,
            vec![case("b", Outcome::Passed), case("c", Outcome::Passed)],
        );
        let seen: Vec<(&str, Outcome)> = merged
            .iter()
            .map(|case| (case.name.as_str(), case.outcome))
            .collect();
        assert_eq!(
            seen,
            [
                ("a", Outcome::Passed),
                ("b", Outcome::Passed),
                ("c", Outcome::Passed)
            ]
        );
    }

    #[test]
    fn a_result_reads_in_a_line() {
        let case = |outcome| Case {
            group: "g".into(),
            name: "n".into(),
            outcome,
            millis: None,
            message: None,
            file: None,
            line: None,
        };
        let mut result = SuiteResult {
            started_at: 0,
            millis: 0,
            exit: Some(1),
            stopped: false,
            cases: vec![
                case(Outcome::Passed),
                case(Outcome::Failed),
                case(Outcome::Skipped),
            ],
            output: String::new(),
            partial: false,
        };
        assert_eq!(summary(&result), "1 failed, 1 passed, 1 skipped");
        // Every test passed, and yet the command failed (a later crate did
        // not build).
        result.cases.truncate(1);
        assert_eq!(summary(&result), "1 passed, and the command failed");
        result.cases.clear();
        result.exit = None;
        assert_eq!(summary(&result), "Could not run");
    }
}
