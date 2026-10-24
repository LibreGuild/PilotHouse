//! Committing what is staged.

use std::path::Path;

use anyhow::{Result, bail};

use crate::run::{git, git_with_input};

/// Commits the index with `message` (read from stdin, so any text is safe)
/// and returns the new commit's short id.
pub fn commit(root: &Path, message: &str, amend: bool) -> Result<String> {
    if message.trim().is_empty() {
        bail!("the commit message is empty");
    }
    let mut args = vec!["commit", "--quiet", "--file=-"];
    if amend {
        args.push("--amend");
    }
    git_with_input(root, &args, message)?;
    Ok(git(root, &["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string())
}

/// The last commit's full message (for amending).
pub fn last_message(root: &Path) -> Result<String> {
    Ok(git(root, &["log", "-1", "--format=%B"])?
        .trim_end()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    #[test]
    fn commits_and_amends() {
        let root = temp_repo("commit");
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "a.txt"]).unwrap();
        let id = commit(&root, "Add a\n\n- with `quotes` and \"more\"", false).unwrap();
        assert!(!id.is_empty());
        assert!(last_message(&root).unwrap().starts_with("Add a"));
        commit(&root, "Add a (amended)", true).unwrap();
        assert_eq!(last_message(&root).unwrap(), "Add a (amended)");
        assert!(commit(&root, "  ", false).is_err());
    }
}
