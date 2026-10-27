//! Tasks: a branch checked out in its own worktree, opened as a workspace,
//! so an agent can work on it without touching the main checkout.
//!
//! *New task…* (a workspace's menu, ⌘K) names the task; PilotHouse makes
//! branch `task/<name>` from the folder's HEAD in a worktree in the data
//! folder, prepares it as the repository's `pilothouse.toml` says, and opens
//! it. *Finish task…* removes the worktree and the workspace (the branch
//! stays, or goes too once merged). Which workspaces are tasks is kept in
//! the `workspace.tasks` setting.
//!
//! ```toml
//! # pilothouse.toml, checked in at the repository's root
//! [worktree]
//! setup = "npm install"          # run in a terminal of the new task
//! copy = [".env", ".env.local"]  # files copied from the main checkout
//! share = ["node_modules"]       # folders linked to the main checkout's
//! ```
//!
//! The setup command and terminals see `PILOTHOUSE_PORT`, a port of the
//! task's own (4100, 4110, …) for its dev server.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use ph_store::Store;
use ph_store::projects::ProjectId;
use serde::{Deserialize, Serialize};

const KEY: &str = "workspace.tasks";
/// The first task's port; each next one is ten higher.
const FIRST_PORT: u16 = 4100;

/// A workspace that is a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Task {
    pub project: ProjectId,
    /// The checkout it branched from.
    pub root: PathBuf,
    pub branch: String,
    /// What it branched from (a branch name, or a commit).
    pub base: String,
    /// The commit it started at.
    #[serde(default)]
    pub start: String,
    pub path: PathBuf,
    pub port: u16,
    /// Files copied and folders linked in (relative), taken away again
    /// when it finishes.
    #[serde(default)]
    pub extras: Vec<PathBuf>,
}

/// What `pilothouse.toml` says about new worktrees.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
pub(crate) struct WorktreeConfig {
    #[serde(default)]
    pub setup: Option<String>,
    #[serde(default)]
    pub copy: Vec<String>,
    #[serde(default)]
    pub share: Vec<String>,
}

#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    worktree: WorktreeConfig,
}

pub(crate) fn all(store: &Store) -> Vec<Task> {
    store.setting(KEY)
}

pub(crate) fn of(store: &Store, project: ProjectId) -> Option<Task> {
    all(store).into_iter().find(|task| task.project == project)
}

pub(crate) fn remember(store: &Store, task: Task) -> Result<()> {
    let mut tasks = all(store);
    tasks.retain(|known| known.project != task.project);
    tasks.push(task);
    store.set_setting(KEY, &tasks)
}

pub(crate) fn forget(store: &Store, project: ProjectId) -> Result<()> {
    let mut tasks = all(store);
    tasks.retain(|known| known.project != project);
    store.set_setting(KEY, &tasks)
}

/// The lowest task port not taken.
pub(crate) fn free_port(store: &Store) -> u16 {
    let taken: Vec<u16> = all(store).iter().map(|task| task.port).collect();
    (0..)
        .map(|n| FIRST_PORT + n * 10)
        .find(|port| !taken.contains(port))
        .unwrap_or(FIRST_PORT)
}

/// `pilothouse.toml` of the repository at `root`, if it has one.
pub(crate) fn config(root: &Path) -> Result<WorktreeConfig> {
    let file = root.join("pilothouse.toml");
    if !file.is_file() {
        return Ok(WorktreeConfig::default());
    }
    let text = std::fs::read_to_string(&file)?;
    let parsed: ConfigFile =
        toml::from_str(&text).with_context(|| format!("reading {}", file.display()))?;
    Ok(parsed.worktree)
}

/// A branch-safe form of a task's name: "Fix login bug" → `fix-login-bug`.
pub(crate) fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').chars().take(48).collect()
}

/// Makes the task's branch and worktree, with the configured files copied
/// and folders linked. Blocking: run off the UI thread.
pub(crate) struct Created {
    pub path: PathBuf,
    pub branch: String,
    pub base: String,
    pub start: String,
    pub extras: Vec<PathBuf>,
}

pub(crate) fn create(root: &Path, name: &str, worktrees: &Path) -> Result<Created> {
    let slug = slug(name);
    if slug.is_empty() {
        bail!("Give the task a name with letters or digits.");
    }
    let branch = format!("task/{slug}");
    if ph_git::git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
    {
        bail!("A branch {branch} already exists.");
    }
    let base = ph_git::git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|head| head.trim().to_string())
        .ok()
        .filter(|head| !head.is_empty() && head != "HEAD")
        .map_or_else(
            || ph_git::git(root, &["rev-parse", "HEAD"]).map(|sha| sha.trim().to_string()),
            Ok,
        )?;
    let repo = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let path = worktrees.join(format!("{repo}-{slug}"));
    if path.exists() {
        bail!("{} already exists.", path.display());
    }
    // A broken pilothouse.toml stops it before anything is made.
    let config = config(root)?;
    std::fs::create_dir_all(worktrees)?;
    let start = ph_git::git(root, &["rev-parse", "HEAD"])?
        .trim()
        .to_string();
    ph_git::worktree::add_branch(root, &path, &branch, &start)?;
    // Half a task is no task: should the files not come along, the
    // worktree and branch go again, so a retry can make them.
    let extras = match bring_extras(&config, root, &path) {
        Ok(extras) => extras,
        Err(err) => {
            let _ = ph_git::worktree::remove(root, &path, true);
            let _ = ph_git::git(root, &["branch", "-D", "--", &branch]);
            return Err(err);
        }
    };
    Ok(Created {
        path,
        branch,
        base,
        start,
        extras,
    })
}

/// Copies and links into the new worktree `path` what `config` names.
fn bring_extras(config: &WorktreeConfig, root: &Path, path: &Path) -> Result<Vec<PathBuf>> {
    let mut extras = Vec::new();
    for file in &config.copy {
        let Some(relative) = inside(file) else {
            continue;
        };
        let from = root.join(&relative);
        if from.is_file() {
            if let Some(parent) = path.join(&relative).parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&from, path.join(&relative))?;
            extras.push(relative);
        }
    }
    for folder in &config.share {
        let Some(relative) = inside(folder) else {
            continue;
        };
        let from = root.join(&relative);
        let to = path.join(&relative);
        if from.is_dir() && !to.exists() {
            link(&from, &to)?;
            extras.push(relative);
        }
    }
    Ok(extras)
}

/// A path from `pilothouse.toml` when it stays inside the repository.
fn inside(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let ok = path
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    ok.then(|| path.to_path_buf())
}

/// Removes a file, or a link (never what it points to).
fn remove_link_or_file(path: &Path) -> Result<()> {
    // A directory link on Windows goes with `remove_dir` (its target stays).
    #[cfg(windows)]
    if std::fs::metadata(path).is_ok_and(|meta| meta.is_dir()) {
        return Ok(std::fs::remove_dir(path)?);
    }
    Ok(std::fs::remove_file(path)?)
}

#[cfg(unix)]
fn link(from: &Path, to: &Path) -> Result<()> {
    Ok(std::os::unix::fs::symlink(from, to)?)
}

#[cfg(windows)]
fn link(from: &Path, to: &Path) -> Result<()> {
    Ok(std::os::windows::fs::symlink_dir(from, to)?)
}

/// Removes the task's worktree, and its branch when `delete_branch` and it
/// is merged (Git refuses otherwise). Blocking.
pub(crate) fn finish(task: &Task, delete_branch: bool) -> Result<()> {
    const UNCOMMITTED: &str =
        "The task has changes that are not committed. Commit or discard them first.";
    // What PilotHouse put in can go: links, and copies still as they were
    // (one edited in the task is the user's).
    let ours: Vec<&PathBuf> = task
        .extras
        .iter()
        .filter(|extra| {
            let (here, original) = (task.path.join(extra), task.root.join(extra));
            let is_link =
                std::fs::symlink_metadata(&here).is_ok_and(|meta| meta.file_type().is_symlink());
            is_link
                || matches!((std::fs::read(&here), std::fs::read(&original)), (Ok(a), Ok(b)) if a == b)
        })
        .collect();
    // Nothing is touched while anything else would make Git refuse: the
    // task stays usable as it is.
    let status = ph_git::git(&task.path, &["status", "--porcelain"]).context(UNCOMMITTED)?;
    let others = status.lines().any(|line| {
        let changed = line.get(3..).unwrap_or_default().trim_matches('"');
        let changed = Path::new(changed.trim_end_matches('/'));
        !ours.iter().any(|extra| extra.as_path() == changed)
    });
    if others {
        bail!(UNCOMMITTED);
    }
    for extra in ours {
        remove_link_or_file(&task.path.join(extra))?;
    }
    ph_git::worktree::remove(&task.root, &task.path, false).context(UNCOMMITTED)?;
    if delete_branch {
        ph_git::git(&task.root, &["branch", "-d", "--", &task.branch])
            .context("The worktree is gone, but the branch is not merged, so it stays.")?;
    }
    Ok(())
}

/// Whether the task's branch moved on from where it started and is now
/// part of its base. Blocking.
pub(crate) fn is_merged(task: &Task) -> bool {
    let Ok(tip) = ph_git::git(
        &task.root,
        &["rev-parse", "--verify", "--quiet", &task.branch],
    ) else {
        return false;
    };
    tip.trim() != task.start
        && ph_git::git(
            &task.root,
            &["merge-base", "--is-ancestor", &task.branch, &task.base],
        )
        .is_ok()
}

/// The command typed into the task's setup terminal: `command` with the
/// task's port in `PILOTHOUSE_PORT`.
pub(crate) fn setup_command(command: &str, port: u16) -> String {
    if !cfg!(windows) {
        return format!("export PILOTHOUSE_PORT={port}; {command}");
    }
    // Windows: in the shell terminals start (PowerShell, else Command Prompt).
    let (shell, _) = ph_env::default_shell();
    let cmd = shell
        .file_stem()
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd"));
    if cmd {
        format!("set PILOTHOUSE_PORT={port}&& {command}")
    } else {
        format!("$env:PILOTHOUSE_PORT={port}; {command}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_branches() {
        assert_eq!(slug("Fix the login bug!"), "fix-the-login-bug");
        assert_eq!(slug("  ##  "), "");
        assert_eq!(inside("../secret"), None);
        assert_eq!(inside(".env"), Some(PathBuf::from(".env")));
    }

    #[test]
    fn reads_the_config() {
        let parsed: ConfigFile = toml::from_str(
            "[worktree]\nsetup = \"npm install\"\ncopy = [\".env\"]\nshare = [\"node_modules\"]\n",
        )
        .unwrap();
        assert_eq!(parsed.worktree.setup.as_deref(), Some("npm install"));
        assert_eq!(parsed.worktree.share, vec!["node_modules"]);
        let empty: ConfigFile = toml::from_str("").unwrap();
        assert_eq!(empty.worktree, WorktreeConfig::default());
    }
}
