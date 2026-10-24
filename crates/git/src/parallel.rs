//! Comparing and keeping the work of agents that ran side by side, each in
//! a worktree started from the same snapshot (`base`).
//!
//! A worktree's changes are read through a checkpoint of it, so new files
//! count too, and its own index and HEAD are never touched.

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::{checkpoint, git};

/// How much a worktree changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// `" 3 files changed, 10 insertions(+), 2 deletions(-)"`.
fn parse_shortstat(text: &str) -> DiffStat {
    let mut stat = DiffStat::default();
    for part in text.split(',') {
        let mut words = part.split_whitespace();
        let (Some(number), Some(word)) = (words.next(), words.next()) else {
            continue;
        };
        let Ok(number) = number.parse() else { continue };
        if word.starts_with("file") {
            stat.files = number;
        } else if word.starts_with("insertion") {
            stat.insertions = number;
        } else if word.starts_with("deletion") {
            stat.deletions = number;
        }
    }
    stat
}

/// The commit holding the worktree's current state.
fn snapshot(worktree: &Path) -> Result<String> {
    checkpoint::create(worktree, "Parallel run result")?.context("not a Git worktree")
}

pub fn changes_since(worktree: &Path, base: &str) -> Result<DiffStat> {
    crate::check_name("commit", base)?;
    let now = snapshot(worktree)?;
    let out = git(
        worktree,
        &[
            "diff",
            "--shortstat",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            base,
            &now,
        ],
    )?;
    Ok(parse_shortstat(&out))
}

/// The worktree's changes as a patch (binary files included).
pub fn patch_since(worktree: &Path, base: &str) -> Result<String> {
    crate::check_name("commit", base)?;
    let now = snapshot(worktree)?;
    git(
        worktree,
        &[
            "diff",
            "--binary",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            base,
            &now,
        ],
    )
}

/// How applying a patch went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Every change applied to the working tree.
    Clean,
    /// The project had moved on: changes were merged in, conflicting ones
    /// with conflict markers to resolve (and the result is staged).
    WithConflicts,
}

/// Applies `patch` to the working tree at `root`. A clean apply touches only
/// the files; when that fails, a three-way apply merges what it can.
pub fn apply_patch(root: &Path, patch: &str) -> Result<Applied> {
    if patch.trim().is_empty() {
        return Ok(Applied::Clean);
    }
    if crate::run::git_with_input(root, &["apply", "--whitespace=nowarn", "-"], patch).is_ok() {
        return Ok(Applied::Clean);
    }
    crate::run::git_with_input(
        root,
        &["apply", "--3way", "--whitespace=nowarn", "-"],
        patch,
    )
    .map(|_| Applied::WithConflicts)
    .or_else(|err| {
        // `--3way` leaves conflict markers and still reports failure.
        let conflicted = git(root, &["diff", "--name-only", "--diff-filter=U"])?;
        if conflicted.trim().is_empty() {
            Err(err)
        } else {
            Ok(Applied::WithConflicts)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    #[test]
    fn shortstat_lines_are_read() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 10 insertions(+), 2 deletions(-)\n"),
            DiffStat {
                files: 3,
                insertions: 10,
                deletions: 2
            }
        );
        assert_eq!(
            parse_shortstat(" 1 file changed, 1 deletion(-)"),
            DiffStat {
                files: 1,
                insertions: 0,
                deletions: 1
            }
        );
        assert_eq!(parse_shortstat(""), DiffStat::default());
    }

    #[test]
    fn a_worktrees_work_comes_back_into_the_project() {
        let root = temp_repo("parallel");
        let base = checkpoint::create(&root, "Parallel run").unwrap().unwrap();
        let lane = root.with_file_name(format!(
            "{}-lane",
            root.file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&lane);
        crate::worktree::add_detached(&root, &lane, &base).unwrap();

        // The agent edits a file and adds one.
        std::fs::write(lane.join("README.md"), "# Test\nMore.\n").unwrap();
        std::fs::write(lane.join("cache.rs"), "pub struct Cache;\n").unwrap();
        let stat = changes_since(&lane, &base).unwrap();
        assert_eq!(
            stat,
            DiffStat {
                files: 2,
                insertions: 2,
                deletions: 0
            }
        );

        let patch = patch_since(&lane, &base).unwrap();
        assert_eq!(apply_patch(&root, &patch).unwrap(), Applied::Clean);
        assert_eq!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            "# Test\nMore.\n"
        );
        assert!(root.join("cache.rs").exists());
        assert_eq!(apply_patch(&root, "").unwrap(), Applied::Clean);

        crate::worktree::remove(&root, &lane, true).unwrap();
    }
}
