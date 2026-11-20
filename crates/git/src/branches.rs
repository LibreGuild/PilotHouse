//! Branches: list, switch, create, rename, merge, delete, and syncing with
//! the remote.

use std::path::Path;

use anyhow::Result;

use crate::check_name;
use crate::run::git;

#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    /// `main`, or `origin/main` for a remote branch.
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
    /// Commits it has that its upstream has not, and the other way round.
    pub ahead: usize,
    pub behind: usize,
    /// Unix time of the tip commit.
    pub updated: i64,
    pub subject: String,
}

/// Local branches, then remote ones, most recently updated first.
pub fn branches(root: &Path) -> Result<Vec<Branch>> {
    let out = git(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(upstream:track)%00%(committerdate:unix)%00%(contents:subject)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut list: Vec<Branch> = out.lines().filter_map(parse_branch).collect();
    list.sort_by_key(|branch| branch.remote);
    Ok(list)
}

fn parse_branch(line: &str) -> Option<Branch> {
    let mut fields = line.split('\0');
    let full = fields.next()?;
    let name = fields.next()?.to_string();
    // `refs/remotes/origin/HEAD` points at a branch already listed.
    if full.ends_with("/HEAD") {
        return None;
    }
    let current = fields.next()? == "*";
    let upstream = fields
        .next()
        .filter(|up| !up.is_empty())
        .map(str::to_string);
    // `[ahead 2, behind 1]`, `[gone]`, or nothing when they are level.
    let track = fields.next()?;
    Some(Branch {
        remote: full.starts_with("refs/remotes/"),
        name,
        current,
        upstream,
        ahead: count(track, "ahead "),
        behind: count(track, "behind "),
        updated: fields.next()?.parse().unwrap_or(0),
        subject: fields.next().unwrap_or_default().to_string(),
    })
}

/// The number after `word` in a branch's tracking note (0 without it).
fn count(track: &str, word: &str) -> usize {
    track
        .split_once(word)
        .map(|(_, rest)| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0)
}

/// Switches to `name`. A remote branch (`origin/x`) gets a local tracking
/// branch `x`, or switches to `x` if it exists already.
pub fn switch(root: &Path, name: &str, remote: bool) -> Result<()> {
    check_name("branch", name)?;
    if remote {
        let local = name.split_once('/').map(|(_, rest)| rest).unwrap_or(name);
        let exists = git(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{local}"),
            ],
        )
        .is_ok();
        if exists {
            return git(root, &["switch", local]).map(|_| ());
        }
        return git(root, &["switch", "--track", name]).map(|_| ());
    }
    git(root, &["switch", name]).map(|_| ())
}

/// Creates `name` at `start` (HEAD by default) and switches to it.
pub fn create(root: &Path, name: &str, start: Option<&str>) -> Result<()> {
    check_name("branch", name)?;
    git(root, &["check-ref-format", "--branch", name])?;
    let mut args = vec!["switch", "-c", name];
    if let Some(start) = start {
        check_name("start point", start)?;
        args.push(start);
    }
    git(root, &args).map(|_| ())
}

/// Deletes a local branch; `force` also deletes unmerged work.
pub fn delete(root: &Path, name: &str, force: bool) -> Result<()> {
    check_name("branch", name)?;
    git(
        root,
        &["branch", if force { "-D" } else { "-d" }, "--", name],
    )
    .map(|_| ())
}

/// Renames the local branch `from` to `to`.
pub fn rename(root: &Path, from: &str, to: &str) -> Result<()> {
    check_name("branch", from)?;
    check_name("branch", to)?;
    git(root, &["check-ref-format", "--branch", to])?;
    git(root, &["branch", "-m", "--", from, to]).map(|_| ())
}

/// Whether every commit of `name` is in the branch checked out: deleting it
/// loses nothing.
pub fn is_merged(root: &Path, name: &str) -> bool {
    check_name("branch", name).is_ok()
        && git(root, &["merge-base", "--is-ancestor", name, "HEAD"]).is_ok()
}

/// Merges `name` into the branch checked out. With conflicts Git stops in
/// the merge, for the user to resolve them (the Git view shows them).
pub fn merge(root: &Path, name: &str) -> Result<()> {
    check_name("branch", name)?;
    git(root, &["merge", "--no-edit", name]).map(|_| ())
}

pub fn fetch(root: &Path) -> Result<()> {
    git(root, &["fetch", "--all", "--prune", "--quiet"]).map(|_| ())
}

/// Fast-forwards to the upstream; refuses to create a merge commit.
pub fn pull(root: &Path) -> Result<()> {
    git(root, &["pull", "--ff-only", "--quiet"]).map(|_| ())
}

/// Pushes the current branch, setting its upstream on the first push.
pub fn push(root: &Path, has_upstream: bool) -> Result<()> {
    if has_upstream {
        git(root, &["push", "--quiet"]).map(|_| ())
    } else {
        git(root, &["push", "--quiet", "-u", "origin", "HEAD"]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    #[test]
    fn create_switch_and_delete() {
        let root = temp_repo("branches");
        create(&root, "feature/x", None).unwrap();
        let list = branches(&root).unwrap();
        let current: Vec<_> = list
            .iter()
            .filter(|b| b.current)
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(current, ["feature/x"]);
        assert!(
            list.iter()
                .any(|b| b.name == "main" && !b.remote && b.subject == "init")
        );
        switch(&root, "main", false).unwrap();
        delete(&root, "feature/x", false).unwrap();
        assert_eq!(branches(&root).unwrap().len(), 1);
        assert!(create(&root, "-bad", None).is_err());
        assert!(create(&root, "bad..name", None).is_err());
    }

    #[test]
    fn rename_merge_and_what_deleting_would_lose() {
        let root = temp_repo("branches-more");
        create(&root, "feature/x", Some("main")).unwrap();
        std::fs::write(root.join("x.txt"), "x").unwrap();
        git(&root, &["add", "x.txt"]).unwrap();
        git(&root, &["commit", "-q", "-m", "add x"]).unwrap();
        switch(&root, "main", false).unwrap();
        // Its commit is on it alone: deleting it would lose the commit.
        assert!(!is_merged(&root, "feature/x"));
        assert!(delete(&root, "feature/x", false).is_err());

        rename(&root, "feature/x", "feature/y").unwrap();
        assert!(rename(&root, "feature/y", "bad..name").is_err());
        let names: Vec<String> = branches(&root)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert!(
            names.contains(&"feature/y".to_string()) && !names.contains(&"feature/x".to_string())
        );

        merge(&root, "feature/y").unwrap();
        assert!(root.join("x.txt").exists());
        assert!(is_merged(&root, "feature/y"));
        delete(&root, "feature/y", false).unwrap();
    }

    #[test]
    fn a_tracking_note_is_counted() {
        assert_eq!(count("[ahead 2, behind 13]", "ahead "), 2);
        assert_eq!(count("[ahead 2, behind 13]", "behind "), 13);
        assert_eq!(count("[gone]", "ahead "), 0);
        assert_eq!(count("", "behind "), 0);
    }
}
