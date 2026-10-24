//! Who last changed each line, from `git blame --porcelain`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::run::git;

#[derive(Debug, Clone, PartialEq)]
pub struct BlameLine {
    /// Short commit id; all zeros for lines not committed yet.
    pub commit: String,
    pub author: String,
    /// Unix time.
    pub time: i64,
    pub summary: String,
}

impl BlameLine {
    pub fn uncommitted(&self) -> bool {
        self.commit.chars().all(|c| c == '0')
    }
}

/// One entry per line of `path` (relative to `root`), as in the working tree.
pub fn blame(root: &Path, path: &str) -> Result<Vec<BlameLine>> {
    crate::check_name("path", path)?;
    let out = git(root, &["blame", "--porcelain", "--no-textconv", "--", path])?;
    Ok(parse_blame(&out))
}

#[derive(Default, Clone)]
struct Info {
    author: String,
    time: i64,
    summary: String,
}

fn parse_blame(out: &str) -> Vec<BlameLine> {
    let mut known: HashMap<String, Info> = HashMap::new();
    let mut lines = Vec::new();
    let mut current: Option<String> = None;
    for line in out.lines() {
        if line.starts_with('\t') {
            // The line's content ends its entry.
            if let Some(commit) = current.take() {
                let info = known.get(&commit).cloned().unwrap_or_default();
                lines.push(BlameLine {
                    commit: commit[..commit.len().min(8)].to_string(),
                    author: info.author,
                    time: info.time,
                    summary: info.summary,
                });
            }
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default();
        if key.len() == 40 && key.chars().all(|c| c.is_ascii_hexdigit()) {
            known.entry(key.to_string()).or_default();
            current = Some(key.to_string());
            continue;
        }
        let Some(info) = current.as_ref().and_then(|commit| known.get_mut(commit)) else {
            continue;
        };
        match key {
            "author" => info.author = value.to_string(),
            "author-time" => info.time = value.parse().unwrap_or(0),
            "summary" => info.summary = value.to_string(),
            _ => {}
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    #[test]
    fn blames_committed_and_new_lines() {
        let root = temp_repo("blame");
        std::fs::write(root.join("README.md"), "# Test\nnew line\n").unwrap();
        let lines = blame(&root, "README.md").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].author, "Test");
        assert_eq!(lines[0].summary, "init");
        assert!(!lines[0].uncommitted());
        assert!(lines[1].uncommitted());
    }
}
