//! What content the working tree, or a commit, holds for a file: its Git
//! blob id. The same text has the same id wherever it is, so content written
//! down for one state of a file is recognised in whichever commit carries it
//! later, also after a rebase, a cherry-pick or a squash.

use std::path::Path;

use anyhow::Result;

use crate::run::{git, git_with_input};

/// The id Git gives a file that is not there (a deleted one).
pub const GONE: &str = "0000000000000000000000000000000000000000";

/// A file and the blob id of its content.
pub type Content = (String, String);

/// A file with the lines a change added to it, and the lines it removed.
pub type LinesChanged = (String, Vec<String>, Vec<String>);

/// The top folder of the repository `folder` is in. Content is named by
/// paths from there, whichever of its folders a workspace has open.
pub fn top(folder: &Path) -> Result<std::path::PathBuf> {
    let top = git(folder, &["rev-parse", "--show-toplevel"])?;
    Ok(crate::local_path(folder, top.trim()))
}

/// The files not committed yet in `root` (changed, new, deleted; staged or
/// not), each with the id of its content now ([`GONE`] for a deleted file).
pub fn pending(root: &Path) -> Result<Vec<Content>> {
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut present = Vec::new();
    let mut gone = Vec::new();
    let mut entries = status.split('\0');
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (state, path) = (&entry[..2], &entry[3..]);
        // A rename or a copy is followed by the path it came from.
        if state.contains('R') || state.contains('C') {
            let from = entries.next().unwrap_or_default();
            if state.contains('R') && !from.is_empty() {
                gone.push(from.to_string());
            }
        }
        // `hash-object` reads one path a line.
        if path.contains('\n') {
            continue;
        }
        let file = root.join(path);
        if file.is_file() {
            present.push(path.to_string());
        } else if !file.is_dir() {
            // A folder here is a submodule: its content is not this
            // repository's.
            gone.push(path.to_string());
        }
    }
    let mut content = Vec::with_capacity(present.len() + gone.len());
    if !present.is_empty() {
        let ids = git_with_input(
            root,
            &["hash-object", "--stdin-paths"],
            &format!("{}\n", present.join("\n")),
        )?;
        content.extend(
            present
                .into_iter()
                .zip(ids.lines())
                .map(|(path, id)| (path, id.trim().to_string())),
        );
    }
    content.extend(gone.into_iter().map(|path| (path, GONE.to_string())));
    content.sort();
    content.dedup_by(|a, b| a.0 == b.0);
    Ok(content)
}

/// What commit `commit` put in the repository: each file it changed against
/// its first parent, with the id of the file's content after it ([`GONE`]
/// for one it deleted). A merge is compared with its first parent too, so
/// what it brought in counts as its own.
pub fn of_commit(root: &Path, commit: &str) -> Result<Vec<Content>> {
    crate::check_ref(commit)?;
    let out = git(
        root,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--root",
            "--no-commit-id",
            "--no-abbrev",
            "--no-renames",
            "-m",
            "--first-parent",
            commit,
        ],
    )?;
    // `:100644 100644 <old> <new> M\0path\0`, entry after entry.
    let mut content = Vec::new();
    let mut parts = out.split('\0');
    while let Some(meta) = parts.next() {
        let Some(meta) = meta.strip_prefix(':') else {
            continue;
        };
        let Some(path) = parts.next() else {
            break;
        };
        if let Some(id) = meta.split(' ').nth(3) {
            content.push((path.to_string(), id.to_string()));
        }
    }
    Ok(content)
}

/// How many lines the work not committed yet in `root` adds and removes,
/// together (a new file counts with all its lines; a binary one with none).
pub fn lines_pending(root: &Path) -> usize {
    let tracked = git(root, &["diff", "HEAD", "--numstat", "-z", "--no-renames"])
        .map(|out| numstat_lines(&out))
        .unwrap_or(0);
    let new = git(root, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    let new: usize = new
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::fs::read(root.join(path)).ok())
        // Not text: nothing a reader goes through line by line.
        .filter(|bytes| bytes.len() < 2_000_000 && !bytes.iter().take(8_000).any(|b| *b == 0))
        .map(|bytes| bytes.iter().filter(|b| **b == b'\n').count())
        .sum();
    tracked + new
}

/// How many lines commit `commit` adds and removes, together, against its
/// first parent.
pub fn lines_of_commit(root: &Path, commit: &str) -> usize {
    if crate::check_ref(commit).is_err() {
        return 0;
    }
    git(
        root,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--root",
            "--no-commit-id",
            "--numstat",
            "--no-renames",
            "-m",
            "--first-parent",
            commit,
        ],
    )
    .map(|out| numstat_lines(&out))
    .unwrap_or(0)
}

/// The lines `--numstat -z` output counts: `added\tremoved\tpath\0`, entry
/// after entry (`-` for a binary file).
fn numstat_lines(out: &str) -> usize {
    out.split('\0')
        .filter_map(|entry| {
            let mut parts = entry.split('\t');
            let added = parts.next()?.trim().parse::<usize>().ok()?;
            let removed = parts.next()?.trim().parse::<usize>().ok()?;
            Some(added + removed)
        })
        .sum()
}

/// The lines a change adds and removes, file by file, without their `+`
/// and `-`: of commit `commit` against its first parent, or of the work not
/// committed yet (a new file with all its lines). By its lines a change is
/// recognised in what an agent's edits wrote, whenever that was.
pub fn changed_lines(root: &Path, commit: Option<&str>) -> Result<Vec<(String, Vec<String>)>> {
    /// A file changed beyond this is told by its first lines.
    const LINES: usize = 4_000;
    let patch = match commit {
        Some(commit) => {
            crate::check_ref(commit)?;
            git(
                root,
                &[
                    "diff-tree",
                    "-p",
                    "-U0",
                    "-r",
                    "--root",
                    "--no-commit-id",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "-m",
                    "--first-parent",
                    commit,
                ],
            )?
        }
        None => git(
            root,
            &[
                "diff",
                "HEAD",
                "-U0",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
            ],
        )?,
    };
    let mut changed: Vec<(String, Vec<String>)> = crate::parse_diff(&patch)
        .into_iter()
        .map(|file| {
            let lines = file
                .hunks
                .iter()
                .flat_map(|hunk| hunk.lines.iter())
                .filter_map(|line| line.strip_prefix(['+', '-']))
                .take(LINES)
                .map(str::to_string)
                .collect();
            (file.path, lines)
        })
        .collect();
    if commit.is_none() {
        let new = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for path in new.split('\0').filter(|path| !path.is_empty()) {
            let Ok(text) = std::fs::read_to_string(root.join(path)) else {
                continue;
            };
            changed.push((
                path.to_string(),
                text.lines().take(LINES).map(str::to_string).collect(),
            ));
        }
    }
    Ok(changed)
}

/// The lines that differ in `paths` between commit `from` and commit `to`
/// (the working tree when `None`, with the files Git does not track yet):
/// file by file, the lines added and the lines removed, without their `+`
/// and `-`. This is what was done between two checkpoints, however it was
/// done.
pub fn lines_between(
    root: &Path,
    from: &str,
    to: Option<&str>,
    paths: &[String],
) -> Result<Vec<LinesChanged>> {
    /// A file changed beyond this is told by its first lines.
    const LINES: usize = 4_000;
    crate::check_ref(from)?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec![
        crate::run::LITERAL,
        "diff",
        "-U0",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        from,
    ];
    if let Some(to) = to {
        crate::check_ref(to)?;
        args.push(to);
    }
    args.push("--");
    args.extend(paths.iter().map(String::as_str));
    let patch = git(root, &args)?;
    let mut changed: Vec<LinesChanged> = crate::parse_diff(&patch)
        .into_iter()
        .map(|file| {
            let with = |mark: char| {
                file.hunks
                    .iter()
                    .flat_map(|hunk| hunk.lines.iter())
                    .filter_map(|line| line.strip_prefix(mark))
                    .take(LINES)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            };
            let (added, removed) = (with('+'), with('-'));
            (file.path, added, removed)
        })
        .collect();
    if to.is_none() {
        // A file Git does not track counts as gone in a diff with the
        // working tree, though it is there (a checkpoint holds such files
        // too). Those are compared here, line by line as sets: which lines
        // are new, which are gone.
        let mut args = vec![
            crate::run::LITERAL,
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ];
        args.extend(paths.iter().map(String::as_str));
        let untracked = git(root, &args).unwrap_or_default();
        for path in untracked.split('\0').filter(|path| !path.is_empty()) {
            changed.retain(|(known, ..)| known != path);
            let Ok(now) = std::fs::read_to_string(root.join(path)) else {
                continue;
            };
            let was = git(root, &["show", &format!("{from}:{path}")]).unwrap_or_default();
            let (old, new): (
                std::collections::HashSet<&str>,
                std::collections::HashSet<&str>,
            ) = (was.lines().collect(), now.lines().collect());
            let added: Vec<String> = now
                .lines()
                .filter(|line| !old.contains(line))
                .take(LINES)
                .map(str::to_string)
                .collect();
            let removed: Vec<String> = was
                .lines()
                .filter(|line| !new.contains(line))
                .take(LINES)
                .map(str::to_string)
                .collect();
            if !added.is_empty() || !removed.is_empty() {
                changed.push((path.to_string(), added, removed));
            }
        }
    }
    Ok(changed)
}

/// The id of `path`'s content at `HEAD`; `None` when it is not there.
pub fn at_head(root: &Path, path: &str) -> Option<String> {
    crate::check_name("path", path).ok()?;
    git(
        root,
        &["rev-parse", "--verify", "-q", &format!("HEAD:{path}")],
    )
    .ok()
    .map(|id| id.trim().to_string())
    .filter(|id| !id.is_empty())
}

/// Who last wrote a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineOrigin {
    /// The commit, in full; all zeros for a line not committed yet.
    pub commit: String,
    pub author: String,
    /// Unix time.
    pub time: i64,
    /// The commit's first line.
    pub summary: String,
    /// The file's path in that commit (another one before a rename).
    pub path: String,
    /// The line's text.
    pub text: String,
}

impl LineOrigin {
    pub fn uncommitted(&self) -> bool {
        self.commit.chars().all(|c| c == '0')
    }
}

/// The commit that last wrote line `line` (1-based) of `path`, as the file
/// is in the working tree. Changes of whitespace only and lines moved or
/// copied from elsewhere are seen through (`-w -M -C`), so a commit that
/// only reformats or moves code does not take the line from the one that
/// wrote it.
pub fn line_origin(root: &Path, path: &str, line: u32) -> Result<Option<LineOrigin>> {
    crate::check_name("path", path)?;
    let line = line.max(1);
    let out = git(
        root,
        &[
            crate::run::LITERAL,
            "blame",
            "--porcelain",
            "--no-textconv",
            "-w",
            "-M",
            "-C",
            "-L",
            &format!("{line},{line}"),
            "--",
            path,
        ],
    )?;
    let mut origin: Option<LineOrigin> = None;
    for row in out.lines() {
        if let Some(text) = row.strip_prefix('\t') {
            if let Some(origin) = origin.as_mut() {
                origin.text = text.to_string();
            }
            break;
        }
        let (key, value) = row.split_once(' ').unwrap_or((row, ""));
        match origin.as_mut() {
            None if key.len() == 40 && key.chars().all(|c| c.is_ascii_hexdigit()) => {
                origin = Some(LineOrigin {
                    commit: key.to_string(),
                    author: String::new(),
                    time: 0,
                    summary: String::new(),
                    path: path.to_string(),
                    text: String::new(),
                });
            }
            Some(origin) => match key {
                "author" => origin.author = value.to_string(),
                "author-time" => origin.time = value.parse().unwrap_or(0),
                "summary" => origin.summary = value.to_string(),
                "filename" => origin.path = value.to_string(),
                _ => {}
            },
            None => {}
        }
    }
    Ok(origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    #[test]
    fn content_written_down_is_found_in_the_commit_that_carries_it() {
        let root = temp_repo("content");
        std::fs::write(root.join("README.md"), "# Test\nmore\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/new file.rs"), "fn main() {}\n").unwrap();
        let before = pending(&root).unwrap();
        assert_eq!(
            before
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            ["README.md", "src/new file.rs"]
        );
        assert!(before.iter().all(|(_, id)| id.len() == 40 && id != GONE));

        // One line added to a file there was, and a new file of one line.
        assert_eq!(lines_pending(&root), 2);
        let lines = changed_lines(&root, None).unwrap();
        assert_eq!(
            lines,
            [
                ("README.md".to_string(), vec!["more".to_string()]),
                (
                    "src/new file.rs".to_string(),
                    vec!["fn main() {}".to_string()]
                ),
            ]
        );
        git(&root, &["add", "-A"]).unwrap();
        // Staged or not, it is still not committed.
        assert_eq!(pending(&root).unwrap(), before);
        git(&root, &["commit", "-q", "-m", "more"]).unwrap();
        assert!(pending(&root).unwrap().is_empty());
        assert_eq!(lines_pending(&root), 0);
        assert_eq!(lines_of_commit(&root, "HEAD"), 2);
        // The commit's lines are the ones the work had.
        assert_eq!(changed_lines(&root, Some("HEAD")).unwrap(), lines);
        // The commit holds exactly what was written down.
        assert_eq!(of_commit(&root, "HEAD").unwrap(), before);
        assert_eq!(
            at_head(&root, "README.md").as_deref(),
            Some(before[0].1.as_str())
        );
        assert_eq!(at_head(&root, "nowhere.md"), None);

        // A deleted file is gone, in the tree and in its commit.
        std::fs::remove_file(root.join("README.md")).unwrap();
        assert_eq!(
            pending(&root).unwrap(),
            [("README.md".to_string(), GONE.to_string())]
        );
        git(&root, &["commit", "-q", "-am", "gone"]).unwrap();
        assert_eq!(
            of_commit(&root, "HEAD").unwrap(),
            [("README.md".to_string(), GONE.to_string())]
        );
        // The first commit has no parent: everything in it is its own.
        let first = git(&root, &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
        assert_eq!(of_commit(&root, first.trim()).unwrap().len(), 1);
    }

    #[test]
    fn what_was_done_between_two_checkpoints_is_told_by_its_lines() {
        let root = temp_repo("between");
        let before = crate::checkpoint::create(&root, "before").unwrap().unwrap();
        std::fs::write(root.join("README.md"), "# Notes\nmore\n").unwrap();
        std::fs::write(root.join("new.txt"), "a new file\n").unwrap();
        let paths = vec!["README.md".to_string(), "new.txt".to_string()];
        // Against the working tree: the file Git does not track counts.
        let now = lines_between(&root, &before, None, &paths).unwrap();
        assert_eq!(
            now,
            [
                (
                    "README.md".to_string(),
                    vec!["# Notes".to_string(), "more".to_string()],
                    vec!["# Test".to_string()]
                ),
                (
                    "new.txt".to_string(),
                    vec!["a new file".to_string()],
                    vec![]
                ),
            ]
        );
        // Between two checkpoints: the same, and nothing of what came after.
        let after = crate::checkpoint::create(&root, "after").unwrap().unwrap();
        std::fs::write(root.join("README.md"), "# Notes\nmore\nlater\n").unwrap();
        assert_eq!(
            lines_between(&root, &before, Some(&after), &paths).unwrap(),
            now
        );
        let later = lines_between(&root, &after, None, &paths).unwrap();
        assert_eq!(
            later,
            [("README.md".to_string(), vec!["later".to_string()], vec![])]
        );
        // Only the files asked about.
        assert!(
            lines_between(&root, &before, None, &["other.txt".to_string()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_line_knows_the_commit_that_wrote_it() {
        let root = temp_repo("line-origin");
        std::fs::write(root.join("README.md"), "# Test\nsecond\n").unwrap();
        git(&root, &["commit", "-q", "-am", "second line"]).unwrap();
        // Indenting a line does not make it this commit's.
        std::fs::write(root.join("README.md"), "# Test\n    second\nthird\n").unwrap();
        git(&root, &["commit", "-q", "-am", "indent"]).unwrap();
        std::fs::write(root.join("README.md"), "# Test\n    second\nthird\nnew\n").unwrap();

        let second = line_origin(&root, "README.md", 2).unwrap().unwrap();
        assert_eq!(second.summary, "second line");
        assert_eq!(second.author, "Test");
        assert_eq!(second.text, "    second");
        assert!(!second.uncommitted());
        let third = line_origin(&root, "README.md", 3).unwrap().unwrap();
        assert_eq!(third.summary, "indent");
        assert!(
            line_origin(&root, "README.md", 4)
                .unwrap()
                .unwrap()
                .uncommitted()
        );
        // No such line.
        assert!(line_origin(&root, "README.md", 99).is_err());
    }
}
