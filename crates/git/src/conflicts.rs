//! Merge conflicts: the blocks between `<<<<<<<` and `>>>>>>>`, and writing
//! a file back with each block resolved.

use std::path::Path;

use anyhow::Result;

use crate::run::git;

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    /// Text both sides agree on, kept as is (line endings included).
    Text(String),
    Conflict(Conflict),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// The labels after the markers, e.g. `HEAD` and `feature`.
    pub ours_label: String,
    pub theirs_label: String,
    pub ours: String,
    pub theirs: String,
    /// The common ancestor (diff3 style), when git wrote it.
    pub base: Option<String>,
    pub base_label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    Ours,
    Theirs,
    /// Ours, then theirs.
    Both,
    Custom(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Ours,
    Base,
    Theirs,
}

/// Splits a file into agreed text and conflict blocks.
pub fn parse(text: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut plain = String::new();
    let mut conflict: Option<(Conflict, Part)> = None;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        match &mut conflict {
            None => {
                if let Some(label) = marker(bare, '<') {
                    if !plain.is_empty() {
                        segments.push(Segment::Text(std::mem::take(&mut plain)));
                    }
                    let block = Conflict {
                        ours_label: label.to_string(),
                        theirs_label: String::new(),
                        ours: String::new(),
                        theirs: String::new(),
                        base: None,
                        base_label: String::new(),
                    };
                    conflict = Some((block, Part::Ours));
                } else {
                    plain.push_str(line);
                }
            }
            Some((block, part)) => {
                if let (Some(label), Part::Ours) = (marker(bare, '|'), *part) {
                    block.base = Some(String::new());
                    block.base_label = label.to_string();
                    *part = Part::Base;
                } else if bare == "=======" && *part != Part::Theirs {
                    *part = Part::Theirs;
                } else if let (Some(label), Part::Theirs) = (marker(bare, '>'), *part) {
                    block.theirs_label = label.to_string();
                    if let Some((block, _)) = conflict.take() {
                        segments.push(Segment::Conflict(block));
                    }
                } else {
                    match part {
                        Part::Ours => block.ours.push_str(line),
                        Part::Base => block.base.get_or_insert_default().push_str(line),
                        Part::Theirs => block.theirs.push_str(line),
                    }
                }
            }
        }
    }
    // An unterminated block is not a conflict: keep its text as it was.
    if let Some((block, _)) = conflict {
        plain.push_str(&format!("<<<<<<< {}\n{}", block.ours_label, block.ours));
        if let Some(base) = block.base {
            plain.push_str(&format!("||||||| {}\n{base}", block.base_label));
        }
        plain.push_str(&format!("=======\n{}", block.theirs));
    }
    if !plain.is_empty() {
        segments.push(Segment::Text(plain));
    }
    segments
}

/// `<<<<<<< label` → `label` (seven marker characters, then a space or end).
fn marker(line: &str, kind: char) -> Option<&str> {
    let rest = line.strip_prefix(&kind.to_string().repeat(7))?;
    if rest.is_empty() {
        return Some("");
    }
    rest.strip_prefix(' ')
}

/// The conflicts in `segments`, in order.
pub fn conflicts(segments: &[Segment]) -> Vec<&Conflict> {
    segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Conflict(conflict) => Some(conflict),
            Segment::Text(_) => None,
        })
        .collect()
}

/// Writes the file text back: resolved blocks replaced, unresolved ones
/// kept with their markers. `resolutions[i]` is for the i-th conflict.
pub fn render(segments: &[Segment], resolutions: &[Option<Resolution>]) -> String {
    let mut out = String::new();
    let mut ix = 0;
    for segment in segments {
        match segment {
            Segment::Text(text) => out.push_str(text),
            Segment::Conflict(conflict) => {
                match resolutions.get(ix).cloned().flatten() {
                    Some(Resolution::Ours) => out.push_str(&conflict.ours),
                    Some(Resolution::Theirs) => out.push_str(&conflict.theirs),
                    Some(Resolution::Both) => {
                        out.push_str(&conflict.ours);
                        out.push_str(&conflict.theirs);
                    }
                    Some(Resolution::Custom(text)) => out.push_str(&text),
                    None => {
                        out.push_str(&format!(
                            "<<<<<<< {}\n{}",
                            conflict.ours_label, conflict.ours
                        ));
                        if let Some(base) = &conflict.base {
                            out.push_str(&format!("||||||| {}\n{base}", conflict.base_label));
                        }
                        out.push_str(&format!(
                            "=======\n{}>>>>>>> {}\n",
                            conflict.theirs, conflict.theirs_label
                        ));
                    }
                }
                ix += 1;
            }
        }
    }
    out
}

/// Marks a resolved file as such (stages it).
pub fn mark_resolved(root: &Path, path: &str) -> Result<()> {
    crate::check_name("path", path)?;
    git(root, &["add", "--", path]).map(|_| ())
}

/// What is in progress: a merge, rebase or cherry-pick, if any.
pub fn operation(root: &Path) -> Option<&'static str> {
    for (file, name) in [
        ("MERGE_HEAD", "merge"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
    ] {
        let path = git(root, &["rev-parse", "--git-path", file]).ok()?;
        if root.join(path.trim()).exists() {
            return Some(name);
        }
    }
    None
}

/// Gives up on the operation in progress.
pub fn abort(root: &Path) -> Result<()> {
    let command = operation(root).unwrap_or("merge");
    git(root, &[command, "--abort"]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    const FILE: &str =
        "keep\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs\n>>>>>>> feature\nend\n";

    #[test]
    fn parses_and_resolves_blocks() {
        let segments = parse(FILE);
        assert_eq!(segments.len(), 3);
        let found = conflicts(&segments);
        assert_eq!(found[0].ours, "ours 1\nours 2\n");
        assert_eq!(found[0].theirs, "theirs\n");
        assert_eq!(
            (found[0].ours_label.as_str(), found[0].theirs_label.as_str()),
            ("HEAD", "feature")
        );
        assert_eq!(
            render(&segments, &[Some(Resolution::Theirs)]),
            "keep\ntheirs\nend\n"
        );
        assert_eq!(
            render(&segments, &[Some(Resolution::Both)]),
            "keep\nours 1\nours 2\ntheirs\nend\n"
        );
        // Unresolved blocks come back exactly as they were.
        assert_eq!(render(&segments, &[None]), FILE);
    }

    #[test]
    fn reads_diff3_bases_and_keeps_broken_blocks() {
        let diff3 = "<<<<<<< ours\na\n||||||| base\no\n=======\nb\n>>>>>>> theirs\n";
        let segments = parse(diff3);
        assert_eq!(conflicts(&segments)[0].base.as_deref(), Some("o\n"));
        assert_eq!(render(&segments, &[None]), diff3);
        let broken = "x\n<<<<<<< HEAD\ny\n";
        assert!(conflicts(&parse(broken)).is_empty());
    }

    #[test]
    fn detects_a_real_merge_conflict() {
        let root = temp_repo("conflict");
        let run = |args: &[&str]| git(&root, args);
        run(&["switch", "-q", "-c", "feature"]).unwrap();
        std::fs::write(root.join("README.md"), "# Feature\n").unwrap();
        run(&["commit", "-q", "-am", "feature"]).unwrap();
        run(&["switch", "-q", "main"]).unwrap();
        std::fs::write(root.join("README.md"), "# Main\n").unwrap();
        run(&["commit", "-q", "-am", "main"]).unwrap();
        assert!(run(&["merge", "feature"]).is_err());
        assert_eq!(operation(&root), Some("merge"));
        assert_eq!(crate::status(&root).unwrap().conflicted, ["README.md"]);
        let text = std::fs::read_to_string(root.join("README.md")).unwrap();
        let segments = parse(&text);
        std::fs::write(
            root.join("README.md"),
            render(&segments, &[Some(Resolution::Ours)]),
        )
        .unwrap();
        mark_resolved(&root, "README.md").unwrap();
        assert!(crate::status(&root).unwrap().conflicted.is_empty());
        abort(&root).unwrap();
        assert_eq!(operation(&root), None);
    }
}
