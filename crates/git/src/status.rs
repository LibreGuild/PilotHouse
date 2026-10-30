//! The working tree: what changed, what is staged, and staging or
//! discarding changes hunk by hunk.

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};

use crate::diff::{FileChange, parse_diff};
use crate::run::{command, git};

/// Everything not yet staged: modified files (as hunks) and new files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkingChanges {
    pub files: Vec<FileChange>,
    pub untracked: Vec<String>,
    /// Number of files with changes already staged (kept).
    pub staged_files: usize,
}

/// Whether `root` is in a Git working tree: it or a folder above it holds
/// a `.git` (a folder, or a file in a linked worktree or submodule). The
/// file system is looked at rather than asking `git`, which takes long to
/// start on Windows: this is asked while the UI draws. On a mapped network
/// drive the search stops at the drive, so a server that cannot be reached
/// reads as that (Git fails with why), not as "no repository".
pub fn is_repository(root: &Path) -> bool {
    let local = ph_remote::remote_of(root).is_none();
    root.ancestors()
        .take_while(|dir| local || ph_remote::remote_of(dir).is_some())
        .any(|dir| dir.join(".git").exists())
}

pub fn current_branch(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|out| out.trim().to_string())
        .filter(|branch| !branch.is_empty())
}

pub fn working_changes(root: &Path) -> Result<WorkingChanges> {
    let diff = git(
        root,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=3",
        ],
    )?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    let staged_files = git(root, &["diff", "--cached", "--name-only", "-z"])?
        .split('\0')
        .filter(|path| !path.is_empty())
        .count();
    Ok(WorkingChanges {
        files: parse_diff(&diff),
        untracked,
        staged_files,
    })
}

/// Stages `patch` (keep) or reverts it in the working tree (discard).
pub fn apply(root: &Path, patch: &str, mode: ApplyMode) -> Result<()> {
    let args: &[&str] = match mode {
        ApplyMode::Stage => &["apply", "--cached", "--whitespace=nowarn", "-"],
        ApplyMode::Discard => &["apply", "--reverse", "--whitespace=nowarn", "-"],
        ApplyMode::Restore => &["apply", "--whitespace=nowarn", "-"],
    };
    let mut command = command(root);
    command.args(args);
    let mut child = crate::run::ready(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("running git")?;
    child
        .stdin
        .take()
        .context("git stdin")?
        .write_all(patch.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("{}", crate::run::failure(root, "apply", &output));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// Add the change to the index.
    Stage,
    /// Remove the change from the working tree.
    Discard,
    /// Put a discarded change back (undo).
    Restore,
}

/// Adds `path` (relative to `root`; a folder when `folder`) to the
/// repository's top `.gitignore`, as VS Code's Add to .gitignore does.
/// Returns `false` when it is listed there already.
pub fn add_to_gitignore(root: &Path, path: &str, folder: bool) -> Result<bool> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    let top = Path::new(top.trim());
    let prefix = git(root, &["rev-parse", "--show-prefix"])?;
    let entry = gitignore_entry(prefix.trim(), path, folder);
    let file = top.join(".gitignore");
    let current = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).context("reading .gitignore"),
    };
    if current.lines().any(|line| line.trim() == entry) {
        return Ok(false);
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&entry);
    text.push('\n');
    std::fs::write(&file, text).context("writing .gitignore")?;
    Ok(true)
}

/// The `.gitignore` line for `path` under the repository's `prefix`
/// (`sub/` when the folder is inside it): from the top, with `/` between
/// folders, and a trailing `/` for a folder.
fn gitignore_entry(prefix: &str, path: &str, folder: bool) -> String {
    let path = path.trim_matches('/').replace('\\', "/");
    let mut entry = format!("{prefix}{path}");
    // Names that start like a comment or a negation are escaped.
    if entry.starts_with('#') || entry.starts_with('!') {
        entry.insert(0, '\\');
    }
    if folder {
        entry.push('/');
    }
    entry
}

/// Stages a whole path, new files included.
pub fn stage_path(root: &Path, path: &str) -> Result<()> {
    git(root, &[crate::run::LITERAL, "add", "--", path]).map(|_| ())
}

/// Branch, upstream and the index, from `git status --porcelain=v2`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepoStatus {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Short commit id of HEAD (`None` before the first commit).
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Paths with staged changes.
    pub staged: Vec<String>,
    /// Paths with merge conflicts.
    pub conflicted: Vec<String>,
    /// Every changed path (staged or not, and new files) with what
    /// happened to it, for marking files in a tree.
    pub changes: Vec<(String, Change)>,
}

/// What happened to a path since the last commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
    /// New and not added yet.
    Untracked,
    Conflicted,
}

impl Change {
    /// The letter Git tools show for it.
    pub fn letter(self) -> char {
        match self {
            Change::Modified => 'M',
            Change::Added => 'A',
            Change::Deleted => 'D',
            Change::Renamed => 'R',
            Change::Untracked => 'U',
            Change::Conflicted => '!',
        }
    }
}

pub fn status(root: &Path) -> Result<RepoStatus> {
    let out = git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
        ],
    )?;
    Ok(parse_status(&out))
}

fn parse_status(out: &str) -> RepoStatus {
    let mut status = RepoStatus::default();
    let mut records = out.split('\0').peekable();
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => {
                    status.head = Some(value.chars().take(8).collect())
                }
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
                "branch.upstream" => status.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let mut fields = record.splitn(9, ' ');
        match fields.next() {
            // `1 XY sub mH mI mW hH hI path` (ordinary change).
            Some("1") => {
                let xy = fields.next().unwrap_or("..");
                if let Some(path) = record.splitn(9, ' ').nth(8) {
                    if !xy.starts_with('.') {
                        status.staged.push(path.to_string());
                    }
                    let change = if xy.starts_with('A') {
                        Change::Added
                    } else if xy.contains('D') {
                        Change::Deleted
                    } else {
                        Change::Modified
                    };
                    status.changes.push((path.to_string(), change));
                }
            }
            // `2 XY … path` then the original path as the next record.
            Some("2") => {
                let xy = fields.next().unwrap_or("..");
                let path = record.splitn(10, ' ').nth(9).unwrap_or_default();
                if !xy.starts_with('.') {
                    status.staged.push(path.to_string());
                }
                status.changes.push((path.to_string(), Change::Renamed));
                records.next();
            }
            // `u XY sub m1 m2 m3 mW h1 h2 h3 path` (unmerged).
            Some("u") => {
                if let Some(path) = record.splitn(11, ' ').nth(10) {
                    status.conflicted.push(path.to_string());
                    status.changes.push((path.to_string(), Change::Conflicted));
                }
            }
            // `? path` (untracked; a new folder is one path ending in `/`).
            Some("?") => {
                if let Some(path) = record.strip_prefix("? ") {
                    let path = path.trim_end_matches('/');
                    status.changes.push((path.to_string(), Change::Untracked));
                }
            }
            _ => {}
        }
    }
    status
}

/// Takes `path` out of the index (keeps the working tree as it is).
pub fn unstage_path(root: &Path, path: &str) -> Result<()> {
    git(
        root,
        &[crate::run::LITERAL, "restore", "--staged", "--", path],
    )
    .map(|_| ())
}

/// What a commit would contain, for writing its message.
/// Every change against the last commit, staged or not, and the names of
/// new files: what Commit All would commit, for writing its message.
pub fn all_changes_diff(root: &Path) -> Result<String> {
    let args = ["diff", "--no-color", "--no-ext-diff", "--no-textconv"];
    let mut diff = match git(root, &[&args[..], &["HEAD"]].concat()) {
        Ok(diff) => diff,
        // No commit yet: what is staged, then the rest.
        Err(_) => format!(
            "{}{}",
            git(root, &[&args[..], &["--cached"]].concat())?,
            git(root, &args)?
        ),
    };
    let new_files = git(root, &["ls-files", "--others", "--exclude-standard"])?;
    if !new_files.trim().is_empty() {
        diff.push_str("\nNew files:\n");
        diff.push_str(&new_files);
    }
    Ok(diff)
}

/// Stages every change, new files included (`git add -A`), for Commit All.
pub fn stage_all(root: &Path) -> Result<()> {
    git(root, &["add", "-A"]).map(|_| ())
}

pub fn staged_diff(root: &Path) -> Result<String> {
    git(
        root,
        &[
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    #[test]
    fn a_repository_is_found_from_its_folders_and_worktrees() {
        let root = temp_repo("is-repo");
        let nested = root.join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(is_repository(&root));
        assert!(is_repository(&nested));
        // A linked worktree has a `.git` file, not a folder.
        let linked = root.with_file_name(format!("ph-git-is-repo-linked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&linked);
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                linked.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert!(linked.join(".git").is_file());
        assert!(is_repository(&linked));
        let plain = root.with_file_name(format!("ph-git-not-repo-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        assert!(!is_repository(&plain));
        let _ = git(
            &root,
            &["worktree", "remove", "--force", linked.to_str().unwrap()],
        );
        for dir in [&root, &linked, &plain] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn parses_porcelain_status() {
        let out = [
            "# branch.oid 0123456789abcdef",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "1 M. N... 100644 100644 100644 aaa bbb src/a.rs",
            "1 .M N... 100644 100644 100644 aaa bbb b.rs",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new name.rs",
            "old.rs",
            "u UU N... 100644 100644 100644 100644 a b c conflict.rs",
            "? notes/",
        ]
        .join("\0");
        let status = parse_status(&out);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.head.as_deref(), Some("01234567"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.staged, ["src/a.rs", "new name.rs"]);
        assert_eq!(status.conflicted, ["conflict.rs"]);
        let changes: Vec<(&str, char)> = status
            .changes
            .iter()
            .map(|(path, change)| (path.as_str(), change.letter()))
            .collect();
        assert_eq!(
            changes,
            [
                ("src/a.rs", 'M'),
                ("b.rs", 'M'),
                ("new name.rs", 'R'),
                ("conflict.rs", '!'),
                ("notes", 'U')
            ]
        );
    }

    /// Keep one hunk, discard the other, then undo the discard, in a real repo.
    #[test]
    fn stage_discard_and_restore_hunks() {
        let root = temp_repo("hunks");
        let run = |args: &[&str]| git(&root, args).unwrap();
        let original: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        std::fs::write(root.join("f.txt"), &original).unwrap();
        run(&["add", "f.txt"]);
        run(&["commit", "-q", "-m", "init"]);

        let edited = original
            .replace("line 2\n", "line two\n")
            .replace("line 28\n", "line twenty-eight\n");
        std::fs::write(root.join("f.txt"), &edited).unwrap();
        let changes = working_changes(&root).unwrap();
        let file = &changes.files[0];
        assert_eq!(file.hunks.len(), 2);

        apply(&root, &file.hunk_patch(0).unwrap(), ApplyMode::Stage).unwrap();
        let discard = file.hunk_patch(1).unwrap();
        apply(&root, &discard, ApplyMode::Discard).unwrap();
        let now = std::fs::read_to_string(root.join("f.txt")).unwrap();
        assert!(now.contains("line two") && now.contains("line 28\n"));
        assert_eq!(working_changes(&root).unwrap().staged_files, 1);

        apply(&root, &discard, ApplyMode::Restore).unwrap();
        assert!(
            std::fs::read_to_string(root.join("f.txt"))
                .unwrap()
                .contains("line twenty-eight")
        );
    }
}

#[cfg(test)]
mod gitignore_tests {
    use super::gitignore_entry;

    #[test]
    fn adds_once_to_the_top_gitignore() {
        let root = std::env::temp_dir().join(format!("ph-gitignore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("app")).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "target/").unwrap();
        // From a folder inside the repository.
        let app = root.join("app");
        assert!(super::add_to_gitignore(&app, "dist", true).unwrap());
        assert!(super::add_to_gitignore(&root, "notes.txt", false).unwrap());
        assert!(!super::add_to_gitignore(&root, "notes.txt", false).unwrap());
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(text, "target/\napp/dist/\nnotes.txt\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn every_change_for_commit_all() {
        let root = std::env::temp_dir().join(format!("ph-commit-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        // Before the first commit too.
        assert!(super::all_changes_diff(&root).unwrap().contains("a.txt"));
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("new.txt"), "x\n").unwrap();
        let diff = super::all_changes_diff(&root).unwrap();
        assert!(diff.contains("+two") && diff.contains("new.txt"), "{diff}");
        assert!(super::staged_diff(&root).unwrap().is_empty());
        super::stage_all(&root).unwrap();
        let staged = super::staged_diff(&root).unwrap();
        assert!(staged.contains("+two") && staged.contains("new.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn entries_from_the_top() {
        assert_eq!(gitignore_entry("", "build/out.log", false), "build/out.log");
        assert_eq!(gitignore_entry("app/", "dist", true), "app/dist/");
        assert_eq!(gitignore_entry("", "#notes", false), "\\#notes");
    }
}
