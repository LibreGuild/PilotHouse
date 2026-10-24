//! Running `git`. Every call blocks, so callers use a background thread.

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};

/// Settings that stop a repository's own config from running programs when
/// PilotHouse reads it (fsmonitor hooks, external diff and text converters).
/// Opening a project should never execute code from it.
const SAFE_CONFIG: &[&str] = &["-c", "core.fsmonitor=false", "-c", "diff.external="];

/// Runs `git args…` in `root` and returns stdout.
pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = command(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("running git (is it installed?)")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Like [`git`], with `input` on stdin.
pub fn git_with_input(root: &Path, args: &[&str], input: &str) -> Result<String> {
    let mut child = command(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running git (is it installed?)")?;
    child
        .stdin
        .take()
        .context("git stdin")?
        .write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A `git` command in `root` with the safe settings. Network commands
/// (fetch, pull, push) never wait on a terminal password prompt; credential
/// helpers still work.
pub fn command(root: &Path) -> std::process::Command {
    let mut command = ph_env::command("git");
    command
        .args(SAFE_CONFIG)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}
