//! Stashes: put work aside and bring it back.

use std::path::Path;

use anyhow::Result;

use crate::run::git;

#[derive(Debug, Clone, PartialEq)]
pub struct Stash {
    pub index: usize,
    /// Its commit, to act on this stash even if others were added since.
    pub hash: String,
    pub message: String,
    /// Unix time it was made.
    pub created: i64,
}

pub fn list(root: &Path) -> Result<Vec<Stash>> {
    let out = git(root, &["stash", "list", "--format=%gd%x00%H%x00%ct%x00%gs"])?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let name = fields.next()?;
            let index = name
                .strip_prefix("stash@{")?
                .strip_suffix('}')?
                .parse()
                .ok()?;
            Some(Stash {
                index,
                hash: fields.next()?.to_string(),
                created: fields.next()?.parse().unwrap_or(0),
                message: fields.next().unwrap_or_default().to_string(),
            })
        })
        .collect())
}

/// Stashes all changes, new files included.
pub fn push(root: &Path, message: &str) -> Result<()> {
    let mut args = vec!["stash", "push", "--include-untracked"];
    if !message.trim().is_empty() {
        args.extend(["-m", message]);
    }
    git(root, &args).map(|_| ())
}

/// `stash@{index}`, once it is still the stash `hash` names (an agent or
/// a terminal may have stashed since the list was read; empty: unchecked).
fn checked(root: &Path, index: usize, hash: &str) -> Result<String> {
    let reference = reference(index);
    if !hash.is_empty() && git(root, &["rev-parse", &reference])?.trim() != hash {
        anyhow::bail!("the stashes changed meanwhile: look again and retry");
    }
    Ok(reference)
}

pub fn apply(root: &Path, index: usize, hash: &str) -> Result<()> {
    git(root, &["stash", "apply", &checked(root, index, hash)?]).map(|_| ())
}

pub fn pop(root: &Path, index: usize, hash: &str) -> Result<()> {
    git(root, &["stash", "pop", &checked(root, index, hash)?]).map(|_| ())
}

pub fn drop(root: &Path, index: usize, hash: &str) -> Result<()> {
    git(root, &["stash", "drop", &checked(root, index, hash)?]).map(|_| ())
}

/// The stash's changes as a patch.
pub fn show(root: &Path, index: usize) -> Result<String> {
    git(
        root,
        &[
            "stash",
            "show",
            "-p",
            "--include-untracked",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            &reference(index),
        ],
    )
}

fn reference(index: usize) -> String {
    format!("stash@{{{index}}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    #[test]
    fn push_list_show_pop() {
        let root = temp_repo("stash");
        std::fs::write(root.join("README.md"), "# Changed\n").unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        push(&root, "work in progress").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            "# Test\n"
        );
        let stashes = list(&root).unwrap();
        assert_eq!(stashes.len(), 1);
        assert!(stashes[0].message.contains("work in progress"));
        assert!(show(&root, 0).unwrap().contains("# Changed"));
        // A stash that is not the one listed any more is left alone.
        assert!(drop(&root, 0, "0000000000000000000000000000000000000000").is_err());
        pop(&root, 0, &stashes[0].hash).unwrap();
        assert!(root.join("new.txt").exists());
        assert!(list(&root).unwrap().is_empty());
    }
}
