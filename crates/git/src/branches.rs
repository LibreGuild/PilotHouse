//! Branches: list, switch, create, delete, and syncing with the remote.

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
            "--format=%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(committerdate:unix)%00%(contents:subject)",
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
    Some(Branch {
        remote: full.starts_with("refs/remotes/"),
        name,
        current: fields.next()? == "*",
        upstream: fields
            .next()
            .filter(|up| !up.is_empty())
            .map(str::to_string),
        updated: fields.next()?.parse().unwrap_or(0),
        subject: fields.next().unwrap_or_default().to_string(),
    })
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
}
