//! Running `git`. Every call blocks, so callers use a background thread.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context as _, Result, bail};

/// Settings that stop a repository's own config from running programs when
/// PilotHouse reads it (fsmonitor hooks, external diff). Text converters
/// are per driver, so every call that shows content passes `--no-textconv`
/// and `--no-ext-diff` (see the test below). Opening a project should never
/// execute code from it.
const SAFE_CONFIG: &[&str] = &["-c", "core.fsmonitor=false", "-c", "diff.external="];

/// Put first in the arguments of a call naming one file: the path is that
/// name, never a pattern (discarding a file named `*.rs` touches only it).
/// Not for every call: `git stash pop` fails with it.
pub(crate) const LITERAL: &str = "--literal-pathspecs";

/// Runs `git args…` in `root` and returns stdout.
pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = command(root);
    command.args(args);
    let output = ready(command)
        .stdin(Stdio::null())
        .output()
        .context("running git (is it installed?)")?;
    if !output.status.success() {
        bail!("{}", failure(root, &args.join(" "), &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Like [`git`], with environment variables `env` set (credentials for one
/// call, which never reach the command line or a config file).
pub fn git_env(root: &Path, args: &[&str], env: &[(String, String)]) -> Result<String> {
    let mut command = command(root);
    command.args(args);
    command.envs(env.iter().map(|(key, value)| (key, value)));
    let output = ready(command)
        .stdin(Stdio::null())
        .output()
        .context("running git (is it installed?)")?;
    if !output.status.success() {
        bail!("{}", failure(root, args.first().unwrap_or(&""), &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Like [`git`], with `input` on stdin.
pub fn git_with_input(root: &Path, args: &[&str], input: &str) -> Result<String> {
    let mut command = command(root);
    command.args(args);
    let mut child = ready(command)
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
        bail!("{}", failure(root, args.first().unwrap_or(&""), &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `command`, made to run on the server when its folder is on a mapped
/// network drive; as it is otherwise. Call it once arguments and variables
/// are set, then set stdin and stdout on what it returns.
pub(crate) fn ready(command: Command) -> Command {
    ph_remote::route(&command).unwrap_or(command)
}

/// Why `git what` failed: what SSH said in words, when it was SSH.
pub(crate) fn failure(root: &Path, what: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    ph_remote::remote_of(root)
        .and_then(|remote| {
            ph_remote::explain_failure(output.status.code(), &stderr, &remote.mapping.host)
        })
        .unwrap_or_else(|| format!("git {what} failed: {}", stderr.trim()))
}

/// A path git printed for the repository at `root`, as this computer
/// reaches it: git on a network drive's server prints the server's paths.
/// Only that drive's mapping reads them (a local repository's paths are
/// never touched).
pub fn local_path(root: &Path, text: &str) -> PathBuf {
    let text = text.trim();
    ph_remote::remote_of(root)
        .and_then(|remote| ph_remote::to_local(&remote.mapping, text))
        .unwrap_or_else(|| PathBuf::from(text))
}

/// The empty file for git options that take one, where git runs.
pub fn null_device(root: &Path) -> &'static str {
    if cfg!(windows) && ph_remote::remote_of(root).is_none() {
        "NUL"
    } else {
        "/dev/null"
    }
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

#[cfg(all(test, unix))]
mod tests {
    use crate::testing::temp_repo;

    /// A repository whose config and attributes point every diff at a
    /// program: reading it must not run that program.
    #[test]
    fn reading_a_repository_runs_none_of_its_programs() {
        let root = temp_repo("programs");
        let marker = root.join("ran");
        let program = format!("sh -c 'touch {}' --", marker.display());
        let run = |args: &[&str]| crate::git(&root, args).unwrap();
        run(&["config", "diff.evil.textconv", &program]);
        run(&["config", "diff.evil.command", &program]);
        run(&["config", "diff.external", &program]);
        run(&["config", "core.fsmonitor", &program]);
        std::fs::write(root.join(".gitattributes"), "* diff=evil\n").unwrap();
        std::fs::write(root.join("README.md"), "# Test\nchanged\n").unwrap();
        run(&["add", ".gitattributes"]);
        run(&["commit", "-q", "-m", "attributes"]);

        crate::status::status(&root).unwrap();
        crate::status::working_changes(&root).unwrap();
        crate::blame::blame(&root, "README.md").unwrap();
        crate::stash::push(&root, "work").unwrap();
        crate::stash::show(&root, 0).unwrap();
        crate::stash::pop(&root, 0).unwrap();
        run(&["add", "README.md"]);
        crate::status::staged_diff(&root).unwrap();
        let head = run(&["rev-parse", "HEAD"]);
        crate::parallel::changes_since(&root, head.trim()).unwrap();
        let commits = crate::log::log(&root, 2, 0, false).unwrap();
        crate::log::changes(&root, &commits[0]).unwrap();

        assert!(!marker.exists(), "a repository program ran");
        let _ = std::fs::remove_dir_all(&root);
    }
}
