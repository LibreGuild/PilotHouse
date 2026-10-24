//! Extra working trees: a PR checked out for review, or a branch for an
//! agent working in parallel, without touching the main checkout.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::check_name;
use crate::run::git;

#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
}

/// Bytes a checkout of HEAD takes (the tracked files' sizes): what one
/// more worktree costs on disk.
pub fn checkout_size(root: &Path) -> Result<u64> {
    let out = git(root, &["ls-tree", "-r", "-l", "--full-tree", "HEAD"])?;
    Ok(out
        .lines()
        .filter_map(|line| line.split_whitespace().nth(3))
        .filter_map(|size| size.parse::<u64>().ok())
        .sum())
}

/// Where PilotHouse puts extra working trees of the repository at `root`
/// when it is on a mapped network drive: in the share, so Git on the server
/// can write them and agents here can reach them, in a folder of `owner`'s
/// (an install's id) so people sharing the drive keep theirs apart. `None`
/// for a repository on this computer (they go in the data folder).
pub fn home(root: &Path, owner: &str) -> Option<PathBuf> {
    let remote = ph_remote::remote_of(root)?;
    Some(share_home(&remote.mapping, owner))
}

/// [`home`] for every repository under `mapping`.
pub fn share_home(mapping: &ph_remote::Mapping, owner: &str) -> PathBuf {
    PathBuf::from(&mapping.local)
        .join(".pilothouse")
        .join(owner)
        .join("worktrees")
}

/// Checks out `commit` (detached) at `path`.
pub fn add_detached(root: &Path, path: &Path, commit: &str) -> Result<()> {
    check_name("commit", commit)?;
    let path = path.to_string_lossy();
    git(root, &["worktree", "add", "--detach", "--", &path, commit]).map(|_| ())
}

/// Creates branch `branch` at `start` and checks it out at `path`.
pub fn add_branch(root: &Path, path: &Path, branch: &str, start: &str) -> Result<()> {
    check_name("branch", branch)?;
    check_name("start point", start)?;
    let path = path.to_string_lossy();
    git(root, &["worktree", "add", "-b", branch, "--", &path, start]).map(|_| ())
}

/// Removes the working tree at `path` (`force`: even with changes in it).
pub fn remove(root: &Path, path: &Path, force: bool) -> Result<()> {
    let path = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", &path]);
    git(root, &args)?;
    git(root, &["worktree", "prune"]).map(|_| ())
}

pub fn list(root: &Path) -> Result<Vec<Worktree>> {
    let out = git(root, &["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    let mut current: Option<Worktree> = None;
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            list.extend(current.take());
            current = Some(Worktree {
                path: crate::local_path(path),
                head: String::new(),
                branch: None,
            });
        } else if let Some(worktree) = &mut current {
            if let Some(head) = line.strip_prefix("HEAD ") {
                worktree.head = head.to_string();
            } else if let Some(branch) = line.strip_prefix("branch ") {
                worktree.branch = Some(branch.trim_start_matches("refs/heads/").to_string());
            }
        }
    }
    list.extend(current);
    Ok(list)
}

/// Fetches `refspec` from `remote` (e.g. `pull/12/head:refs/pilothouse/pr/12`).
pub fn fetch_ref(root: &Path, remote: &str, refspec: &str) -> Result<()> {
    check_name("remote", remote)?;
    check_name("refspec", refspec)?;
    git(
        root,
        &["fetch", "--quiet", "--force", "--", remote, refspec],
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_size_adds_up_tracked_files() {
        let root = crate::testing::temp_repo("checkout-size");
        assert_eq!(checkout_size(&root).unwrap(), "# Test\n".len() as u64);
    }
    use crate::testing::temp_repo;

    #[test]
    fn adds_lists_and_removes() {
        let root = temp_repo("worktree");
        let tree = root.with_extension("wt");
        let _ = std::fs::remove_dir_all(&tree);
        add_branch(&root, &tree, "agent/try", "HEAD").unwrap();
        assert!(tree.join("README.md").exists());
        let trees = list(&root).unwrap();
        assert!(
            trees
                .iter()
                .any(|t| t.branch.as_deref() == Some("agent/try"))
        );
        remove(&root, &tree, true).unwrap();
        assert!(!tree.exists());
        assert_eq!(list(&root).unwrap().len(), 1);
    }
}
