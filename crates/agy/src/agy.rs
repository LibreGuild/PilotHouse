//! One running `agy` process in headless stream-json mode.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use anyhow::{Context as _, Result};
use serde_json::{Value, json};

use crate::options::{Model, parse_models};

/// Lines of stderr kept to explain a failure.
const STDERR_LINES: usize = 20;

pub struct Agy {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
    /// stdout events, one per line; closes when agy exits.
    pub events: mpsc::Receiver<Value>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// The flags it was started with (to tell when a restart is needed).
    pub flags: Vec<String>,
}

impl Agy {
    /// Starts agy in `cwd`, resuming `conversation` when given.
    pub fn spawn(
        program: &Path,
        cwd: &Path,
        conversation: Option<&str>,
        flags: Vec<String>,
    ) -> Result<Self> {
        let mut command = ph_env::command(program);
        command
            .args([
                "-p=",
                "--input-format=stream-json",
                "--output-format=stream-json",
            ])
            .args(conversation.map(|id| format!("--conversation={id}")))
            .args(&flags)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("could not start agy")?;
        let stdin = child.stdin.take().context("agy has no stdin")?;
        let stdout = child.stdout.take().context("agy has no stdout")?;
        let stderr_pipe = child.stderr.take().context("agy has no stderr")?;

        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match serde_json::from_str::<Value>(&line) {
                    Ok(event) => {
                        if sender.send(event).is_err() {
                            return;
                        }
                    }
                    Err(_) => tracing::debug!("agy: {line}"),
                }
            }
        });
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
                let mut tail = tail.lock().unwrap();
                if tail.len() == STDERR_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });

        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            stdin,
            events,
            stderr,
            flags,
        })
    }

    /// Sends one user turn.
    pub fn send(&mut self, content: Value) -> Result<()> {
        let message = json!({
            "event": "user",
            "message": { "role": "user", "content": content },
        });
        writeln!(self.stdin, "{message}")
            .and_then(|_| self.stdin.flush())
            .context("agy stopped reading")
    }

    /// A handle that can stop the process from another thread.
    pub fn killer(&self) -> Killer {
        Killer(self.child.clone())
    }

    /// Why agy stopped, from its last stderr lines.
    pub fn failure(&self) -> String {
        let tail = self.stderr.lock().unwrap();
        // Model and API failures come as `AGY_ERROR: {json}` or `error: …`.
        let reason = tail
            .iter()
            .rev()
            .find(|line| line.starts_with("AGY_ERROR") || line.starts_with("error:"))
            .or(tail.back());
        match reason {
            Some(line) => format!("agy stopped: {line}"),
            None => "agy stopped unexpectedly".into(),
        }
    }
}

impl Drop for Agy {
    fn drop(&mut self) {
        self.killer().kill();
    }
}

#[derive(Clone)]
pub struct Killer(Arc<Mutex<Child>>);

impl Killer {
    pub fn kill(&self) {
        let mut child = self.0.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// `agy models`, listed once per bridge. Listing takes seconds, so it starts
/// in the background and only what needs the list waits for it.
pub struct ModelList {
    program: PathBuf,
    list: OnceLock<Vec<Model>>,
}

impl ModelList {
    pub fn new(program: PathBuf) -> Self {
        Self {
            program,
            list: OnceLock::new(),
        }
    }

    /// The models, listing them first if needed (blocks).
    pub fn get(&self) -> &[Model] {
        self.list.get_or_init(|| list_models(&self.program))
    }

    /// The models if they are listed already.
    pub fn loaded(&self) -> Option<&[Model]> {
        self.list.get().map(Vec::as_slice)
    }
}

/// `agy models`, grouped; empty if agy cannot list them (not signed in).
fn list_models(program: &Path) -> Vec<Model> {
    match ph_env::command(program)
        .arg("models")
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => {
            parse_models(&String::from_utf8_lossy(&output.stdout))
        }
        Ok(output) => {
            tracing::warn!(
                "agy models failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            Vec::new()
        }
        Err(err) => {
            tracing::warn!("agy models failed: {err}");
            Vec::new()
        }
    }
}
