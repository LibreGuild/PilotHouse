//! History: commits with the lanes of a graph, and one commit's changes.

use std::path::Path;

use anyhow::Result;

use crate::diff::{FileChange, parse_diff};
use crate::run::git;

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Unix time.
    pub time: i64,
    pub subject: String,
    /// Branch and tag names pointing here (`HEAD -> main`, `tag: v1`).
    pub refs: Vec<String>,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }
}

/// How one commit's row of the graph is drawn. Lanes are columns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GraphRow {
    /// The commit's own lane (where its dot goes).
    pub lane: usize,
    /// Whether a child comes down into the dot in its own lane.
    pub from_above: bool,
    /// Other lanes that end at this commit (branches that forked here).
    pub merging_in: Vec<usize>,
    /// Lanes passing by without touching this commit.
    pub passing: Vec<usize>,
    /// Lanes leaving the dot toward its parents.
    pub to_parents: Vec<usize>,
    /// Columns this row needs.
    pub width: usize,
}

const FIELD: char = '\u{1f}';
const RECORD: char = '\u{1e}';

/// Up to `limit` commits after skipping `skip`, newest first. `all` includes
/// every branch and tag, otherwise only HEAD's history.
pub fn log(root: &Path, limit: usize, skip: usize, all: bool) -> Result<Vec<Commit>> {
    let limit = format!("--max-count={limit}");
    let skip = format!("--skip={skip}");
    let mut args = vec![
        "log",
        "--topo-order",
        "--no-color",
        "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D%x1e",
        &limit,
        &skip,
    ];
    if all {
        args.extend(["--branches", "--remotes", "--tags", "HEAD"]);
    }
    // An empty repository has no history yet.
    let Ok(out) = git(root, &args) else {
        return Ok(Vec::new());
    };
    Ok(parse_log(&out))
}

fn parse_log(out: &str) -> Vec<Commit> {
    out.split(RECORD)
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').split(FIELD);
            let id = fields.next().filter(|id| !id.is_empty())?.to_string();
            Some(Commit {
                id,
                parents: fields
                    .next()?
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
                author: fields.next()?.to_string(),
                email: fields.next()?.to_string(),
                time: fields.next()?.parse().unwrap_or(0),
                subject: fields.next()?.to_string(),
                refs: fields
                    .next()
                    .unwrap_or_default()
                    .split(", ")
                    .filter(|name| !name.trim().is_empty())
                    .map(|name| name.trim().to_string())
                    .collect(),
            })
        })
        .collect()
}

/// Lays out graph lanes for commits in topological order (newest first).
pub fn graph(commits: &[Commit]) -> Vec<GraphRow> {
    // Each lane holds the commit it expects next, or nothing.
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let before = lanes.clone();
        let found = before
            .iter()
            .position(|lane| *lane == Some(commit.id.as_str()));
        let lane = found.unwrap_or_else(|| free_lane(&mut lanes));
        let mut row = GraphRow {
            lane,
            from_above: found.is_some(),
            ..GraphRow::default()
        };
        for (ix, expected) in before.iter().enumerate() {
            match expected {
                Some(id) if ix != lane && *id == commit.id => {
                    row.merging_in.push(ix);
                    lanes[ix] = None;
                }
                Some(_) if ix != lane => row.passing.push(ix),
                _ => {}
            }
        }
        lanes[lane] = None;
        for (ix, parent) in commit.parents.iter().enumerate() {
            let existing = lanes.iter().position(|lane| *lane == Some(parent.as_str()));
            let target = match existing {
                Some(target) => target,
                // The first parent continues in this lane when it can.
                None if ix == 0 => lane,
                None => free_lane(&mut lanes),
            };
            lanes[target] = Some(parent.as_str());
            row.to_parents.push(target);
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        row.width = before.len().max(lanes.len()).max(lane + 1);
        rows.push(row);
    }
    rows
}

fn free_lane(lanes: &mut Vec<Option<&str>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(ix) => ix,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

/// What `commit` changed (against its first parent).
pub fn changes(root: &Path, commit: &Commit) -> Result<Vec<FileChange>> {
    crate::check_name("commit", &commit.id)?;
    let diff = match commit.parents.first() {
        Some(parent) => git(
            root,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                parent,
                &commit.id,
            ],
        )?,
        None => git(
            root,
            &[
                "show",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--format=",
                &commit.id,
            ],
        )?,
    };
    Ok(parse_diff(&diff))
}

/// The newest tag reachable from HEAD, for "changes since the last release".
pub fn last_tag(root: &Path) -> Option<String> {
    git(root, &["describe", "--tags", "--abbrev=0"])
        .ok()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
}

/// One line per commit in `range` (`v1.0..HEAD`), for changelogs.
pub fn summaries(root: &Path, range: &str) -> Result<String> {
    crate::check_name("range", range)?;
    git(
        root,
        &[
            "log",
            "--no-color",
            "--no-merges",
            "--format=- %s (%h, %an)",
            range,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            email: String::new(),
            time: 0,
            subject: String::new(),
            refs: Vec::new(),
        }
    }

    /// d (merge of c and b) → c → a, and b → a: a diamond.
    #[test]
    fn lays_out_a_merge() {
        let commits = [
            commit("d", &["c", "b"]),
            commit("c", &["a"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ];
        let rows = graph(&commits);
        assert_eq!(
            rows[0],
            GraphRow {
                lane: 0,
                from_above: false,
                merging_in: vec![],
                passing: vec![],
                to_parents: vec![0, 1],
                width: 2
            }
        );
        assert_eq!(rows[1].lane, 0);
        assert_eq!(rows[1].passing, vec![1]);
        assert_eq!(rows[2].lane, 1);
        // b's parent a is already expected in lane 0: the lanes join.
        assert_eq!(rows[2].to_parents, vec![0]);
        assert_eq!(
            rows[3],
            GraphRow {
                lane: 0,
                from_above: true,
                merging_in: vec![],
                passing: vec![],
                to_parents: vec![],
                width: 1
            }
        );
    }

    #[test]
    fn reads_history_and_changes() {
        let root = temp_repo("log");
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "a.txt"]).unwrap();
        git(&root, &["commit", "-q", "-m", "Add a"]).unwrap();
        git(&root, &["tag", "v1"]).unwrap();
        let commits = log(&root, 10, 0, true).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "Add a");
        assert!(commits[0].refs.iter().any(|name| name == "tag: v1"));
        let files = changes(&root, &commits[0]).unwrap();
        assert_eq!(files[0].path, "a.txt");
        let root_files = changes(&root, &commits[1]).unwrap();
        assert_eq!(root_files[0].path, "README.md");
        assert_eq!(last_tag(&root).as_deref(), Some("v1"));
        assert!(summaries(&root, "HEAD~1..HEAD").unwrap().contains("Add a"));
    }
}
