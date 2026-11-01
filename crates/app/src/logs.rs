//! Where PilotHouse writes what happened: always stderr, and for the app a
//! log file in the data folder (`logs/pilothouse.log`, the previous runs'
//! kept as `.1` to `.3`), since a Windows app has no console to read. A
//! crash writes `logs/crash-<time>.log` with the panic and a backtrace (the
//! newest few are kept), so a report can say what went wrong.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

/// Logs of earlier runs kept beside the current one.
const OLD_LOGS: usize = 3;
/// Crash reports kept.
const CRASHES_KEPT: usize = 10;

fn logs_dir() -> PathBuf {
    ph_store::paths::data_dir().join("logs")
}

/// Sets up logging; `to_file` for the app (not the `ctl`, `mcp` and
/// `agy-acp` helpers, which run beside it). `RUST_LOG` overrides the level,
/// e.g. `RUST_LOG=ph_store=debug pilothouse`.
pub fn init(to_file: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,pilothouse=info,ph_=info"));
    let file = to_file
        .then(open_log)
        .flatten()
        .map(|file| fmt::layer().with_ansi(false).with_writer(Mutex::new(file)));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(file)
        .try_init();
}

/// A log grows to this, then starts afresh (the full one moved along), so
/// a session left running for weeks never fills the disk.
const LOG_LIMIT: u64 = 20 << 20;

/// The log file being written, and how much it holds.
struct Log {
    path: PathBuf,
    file: File,
    written: u64,
}

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.written > LOG_LIMIT {
            rotate(&self.path, OLD_LOGS);
            if let Ok(file) = File::create(&self.path) {
                self.file = file;
                self.written = 0;
            }
        }
        let written = self.file.write(bytes)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// A fresh `pilothouse.log`, the earlier ones moved along (`.1` is the
/// previous run, or the earlier part of a long one).
fn open_log() -> Option<Log> {
    let dir = logs_dir();
    ph_store::paths::ensure_private_dir(&dir).ok()?;
    let log = dir.join("pilothouse.log");
    rotate(&log, OLD_LOGS);
    Some(Log {
        file: File::create(&log).ok()?,
        path: log,
        written: 0,
    })
}

/// Shifts `path` to `path.1`, `path.1` to `path.2`, …, dropping the oldest.
fn rotate(path: &Path, keep: usize) {
    let numbered = |n: usize| PathBuf::from(format!("{}.{n}", path.display()));
    let _ = fs::remove_file(numbered(keep));
    for n in (1..keep).rev() {
        let _ = fs::rename(numbered(n), numbered(n + 1));
    }
    let _ = fs::rename(path, numbered(1));
}

/// On a panic, writes a crash report (what, where, a backtrace) before the
/// usual message.
pub fn install_crash_handler() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current();
        let report = format!(
            "PilotHouse {} panicked on thread {} (on the UI thread it closes; elsewhere it may have been caught).\n\n{info}\n\nBacktrace:\n{backtrace}\n",
            env!("CARGO_PKG_VERSION"),
            thread.name().unwrap_or("(unnamed)"),
        );
        if let Some(path) = write_crash(&report) {
            tracing::error!("crashed; report in {}", path.display());
        }
        previous(info);
    }));
}

fn write_crash(report: &str) -> Option<PathBuf> {
    let dir = logs_dir();
    ph_store::paths::ensure_private_dir(&dir).ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_millis())
        .unwrap_or_default();
    let path = dir.join(format!("crash-{stamp}.log"));
    fs::write(&path, report).ok()?;
    // The oldest reports go.
    let mut crashes: Vec<PathBuf> = fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("crash-"))
        })
        .collect();
    crashes.sort();
    let excess = crashes.len().saturating_sub(CRASHES_KEPT);
    for old in crashes.into_iter().take(excess) {
        let _ = fs::remove_file(old);
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_rotate_and_keep_a_few() {
        let dir = std::env::temp_dir().join(format!("ph-logs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("pilothouse.log");
        for run in 1..=5 {
            rotate(&log, 3);
            fs::write(&log, format!("run {run}")).unwrap();
        }
        let read = |name: &str| fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(read("pilothouse.log"), "run 5");
        assert_eq!(read("pilothouse.log.1"), "run 4");
        assert_eq!(read("pilothouse.log.3"), "run 2");
        assert!(
            !dir.join("pilothouse.log.4").exists(),
            "only three old logs"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
