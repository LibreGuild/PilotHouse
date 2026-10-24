//! Stashes: put work aside and bring it back.

use std::path::Path;

use anyhow::Result;

use crate::run::git;

#[derive(Debug, Clone, PartialEq)]
pub struct Stash {
    pub index: usize,
    pub message: String,
    /// Unix time it was made.
    pub created: i64,
}

pub fn list(root: &Path) -> Result<Vec<Stash>> {
    let out = git(root, &["stash", "list", "--format=%gd%x00%ct%x00%gs"])?;
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

pub fn apply(root: &Path, index: usize) -> Result<()> {
    git(root, &["stash", "apply", &reference(index)]).map(|_| ())
}

pub fn pop(root: &Path, index: usize) -> Result<()> {
    git(root, &["stash", "pop", &reference(index)]).map(|_| ())
}

pub fn drop(root: &Path, index: usize) -> Result<()> {
    git(root, &["stash", "drop", &reference(index)]).map(|_| ())
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
        pop(&root, 0).unwrap();
        assert!(root.join("new.txt").exists());
        assert!(list(&root).unwrap().is_empty());
    }
}
