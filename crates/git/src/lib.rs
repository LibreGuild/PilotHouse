//! Git for PilotHouse, over the `git` command line.
//!
//! The CLI is on every developer machine, honors the user's config and
//! credential helpers, and keeps PilotHouse free of a libgit2 dependency.
//! Every function blocks: call them from a background thread.
//!
//! - [`status`]: working tree, index, branch and upstream; staging hunks.
//! - [`diff`]: unified diffs as files and hunks.
//! - [`branches`]: list, switch, create, delete; fetch, pull, push.
//! - [`commit`]: commit and amend.
//! - [`stash`], [`log`] (history with graph lanes), [`blame`].
//! - [`conflicts`]: parse and resolve merge conflicts.
//! - [`remote`]: remotes and the hosting repo they point at.
//! - [`worktree`]: extra working trees (PR reviews, parallel agents).
//! - [`checkpoint`]: snapshots before agent turns, and restoring them.
//! - [`parallel`]: comparing and keeping agents' work from parallel worktrees.
//!
//! In a folder on a mapped network drive, Git runs on the server over SSH
//! ([`ph_remote`]); paths it prints are turned back into this computer's.

pub mod blame;
pub mod branches;
pub mod checkpoint;
pub mod commit;
pub mod conflicts;
pub mod diff;
pub mod log;
pub mod parallel;
pub mod remote;
mod run;
pub mod stash;
pub mod status;
#[cfg(test)]
mod testing;
pub mod worktree;

pub use diff::{FileChange, Hunk, parse_diff};
pub use run::{git, git_env};
pub(crate) use run::{local_path, null_device};
pub use status::{
    ApplyMode, Change, RepoStatus, WorkingChanges, apply, current_branch, is_repository,
    stage_path, staged_diff, status, unstage_path, working_changes,
};

/// Refuses a branch or ref name git would read as an option, for callers
/// outside this crate.
pub fn check_ref(name: &str) -> anyhow::Result<()> {
    check_name("name", name)
}

/// Refuses names that git would read as options (`-x`) or that hold
/// control characters, before they reach the command line.
pub(crate) fn check_name(kind: &str, name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.starts_with('-') || name.chars().any(char::is_control) {
        anyhow::bail!("not a valid {kind} name: `{name}`");
    }
    Ok(())
}
