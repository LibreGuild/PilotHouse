//! Short, one-off writing jobs for an agent (commit messages, PR
//! descriptions, changelogs, merging a conflict), run through the agent
//! CLI's own non-interactive mode instead of a chat session.
//!
//! The prompt goes on stdin where the CLI reads it (Claude Code, Codex,
//! Gemini CLI), so long diffs and any characters are safe on every platform.
//! `agy` takes it only as an argument (its stdin mode needs stream-json), so
//! callers cut every part of a prompt: the largest, a pull request's, stays
//! near 23 KB, inside Linux's 128 KB for one argument and Windows' 32,767
//! characters for a whole command line.

use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use gpui_kit::{App, AppContext as _, Task};
use ph_store::Store;

use crate::agents::{AGENTS, AgentSpec, Availability};

/// Longest a one-off job may take.
const TIMEOUT: Duration = Duration::from_secs(180);

/// Writes text with an agent: `(prompt, working folder)`.
pub type Writer = Rc<dyn Fn(String, PathBuf, &mut App) -> Task<Result<String>>>;

/// A [`Writer`] using the user's default agent (else the first one
/// installed), chosen when each job starts.
pub fn writer(store: Store) -> Writer {
    Rc::new(move |prompt, cwd, cx| {
        let spec = pick_agent(&store);
        // A thread of its own: the job waits on the agent for minutes, and
        // the shared executor's threads must stay free for file reads, Git
        // and previews.
        let (done, answer) = futures::channel::oneshot::channel();
        let started = std::thread::Builder::new()
            .name("one-off-agent".into())
            .spawn(move || {
                let result = spec
                    .context("No coding agent is installed to write this.")
                    .and_then(|spec| ask(spec, &prompt, &cwd));
                let _ = done.send(result);
            });
        cx.background_spawn(async move {
            started.context("could not start the job")?;
            answer.await.context("the job stopped")?
        })
    })
}

fn pick_agent(store: &Store) -> Option<&'static AgentSpec> {
    let preferred: String = store.setting(crate::chats::DEFAULT_AGENT_KEY);
    crate::agents::find(&preferred)
        .into_iter()
        .chain(AGENTS.iter())
        .find(|spec| ph_env::which(spec.cli).is_some())
}

/// Runs `prompt` through `spec`'s CLI in `cwd` and returns its answer.
pub fn ask(spec: &AgentSpec, prompt: &str, cwd: &Path) -> Result<String> {
    let Availability::Ready { .. } = spec.availability() else {
        bail!("{} is not installed.", spec.name);
    };
    let program = ph_env::which(spec.cli).context("agent CLI not found")?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let job = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // In PilotHouse's own private folder: in a shared temp folder another
    // user could put the file there first and write the answer.
    let folder = ph_store::paths::data_dir().join("oneshot");
    ph_store::paths::ensure_private_dir(&folder).context("making a folder for the answer")?;
    let last_message = folder.join(format!("{}-{job}.txt", std::process::id()));
    let _ = std::fs::remove_file(&last_message);
    let (args, stdin): (Vec<String>, Option<&str>) = match spec.id {
        "claude" => (
            vec!["-p".into(), "--output-format".into(), "text".into()],
            Some(prompt),
        ),
        "codex" => (
            vec![
                "exec".into(),
                "--skip-git-repo-check".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--output-last-message".into(),
                last_message.to_string_lossy().into_owned(),
                "-".into(),
            ],
            Some(prompt),
        ),
        "gemini" => (
            vec!["-p".into(), "Do what the text above asks.".into()],
            Some(prompt),
        ),
        "antigravity" => (vec![format!("-p={prompt}")], None),
        other => bail!("{other} cannot run one-off jobs"),
    };
    if !cwd.is_dir() {
        bail!(
            "the folder it works in, {}, no longer exists",
            cwd.display()
        );
    }
    let mut child = ph_env::command(&program)
        .args(&args)
        .current_dir(cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start {}", spec.name))?;
    // Fed from a thread, so a CLI that answers before reading everything
    // cannot deadlock us on a full pipe.
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let text = text.to_string();
        std::thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let stdout = child.stdout.take().context("no stdout")?;
    let stderr = child.stderr.take().context("no stderr")?;
    // Read as it comes, so what arrived is there even if the pipe never
    // closes (see `collect` below).
    let reader = |mut pipe: Box<dyn Read + Send>| {
        let read: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let into = read.clone();
        let thread = std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n @ 1..) = pipe.read(&mut chunk) {
                if let Ok(mut read) = into.lock() {
                    read.extend_from_slice(&chunk[..n]);
                }
            }
        });
        (thread, read)
    };
    let out = reader(Box::new(stdout));
    let err = reader(Box::new(stderr));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{} took too long (over {} s)", spec.name, TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // A grandchild the CLI left running can hold its output open: what was
    // read by then is the answer, without waiting for it.
    let collect = |(thread, read): (
        std::thread::JoinHandle<()>,
        std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    )| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let bytes = read.lock().map(|read| read.clone()).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let stdout = collect(out);
    let stderr = collect(err);
    let answer = if spec.id == "codex" {
        let text = std::fs::read_to_string(&last_message).unwrap_or_default();
        let _ = std::fs::remove_file(&last_message);
        text
    } else {
        stdout
    };
    if !status.success() || answer.trim().is_empty() {
        let reason = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no answer");
        bail!("{} could not answer: {reason}", spec.name);
    }
    Ok(answer.trim().to_string())
}
