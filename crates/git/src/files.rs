//! One file's Git: its history and diffs, comparing two files, discarding
//! its changes and ignoring it (the Files tree's menu).

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};

use crate::diff::{FileChange, parse_diff};
use crate::log::{Commit, parse_log};
use crate::run::{LITERAL, command, failure, git, ready};

/// The commits that changed `path` (following renames), newest first.
pub fn file_log(root: &Path, path: &str, limit: usize) -> Result<Vec<Commit>> {
    let limit = format!("--max-count={limit}");
    let out = git(
        root,
        &[
            LITERAL,
            "log",
            "--follow",
            "--no-color",
            "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D%x1e",
            &limit,
            "--",
            path,
        ],
    )?;
    Ok(parse_log(&out))
}

/// What commit `id` changed in `path`.
pub fn commit_diff(root: &Path, id: &str, path: &str) -> Result<Vec<FileChange>> {
    crate::check_ref(id)?;
    let out = git(
        root,
        &[
            LITERAL,
            "show",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--format=",
            "--follow",
            id,
            "--",
            path,
        ],
    )?;
    Ok(parse_diff(&out))
}

/// `path`'s changes since the last commit (staged or not).
pub fn diff_with_head(root: &Path, path: &str) -> Result<Vec<FileChange>> {
    let out = git(
        root,
        &[
            LITERAL,
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
            path,
        ],
    )?;
    Ok(parse_diff(&out))
}

/// The differences between two files anywhere on disk.
pub fn diff_files(a: &Path, b: &Path) -> Result<Vec<FileChange>> {
    let dir = a.parent().unwrap_or(Path::new("."));
    let mut cmd = command(dir);
    cmd.args([
        "diff",
        "--no-index",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--",
    ])
    .arg(a)
    .arg(b);
    let output = ready(cmd)
        .stdin(Stdio::null())
        .output()
        .context("running git (is it installed?)")?;
    // `--no-index` exits 1 when the files differ.
    if !matches!(output.status.code(), Some(0 | 1)) {
        bail!("{}", failure(dir, "diff", &output));
    }
    Ok(parse_diff(&String::from_utf8_lossy(&output.stdout)))
}

/// Puts `path` back as the last commit has it, in the index and on disk.
pub fn discard_path(root: &Path, path: &str) -> Result<()> {
    git(
        root,
        &[
            LITERAL,
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            path,
        ],
    )
    .map(|_| ())
}

/// Adds `path` (`/`-separated, relative to `root`) to the repository's
/// `.gitignore`, anchored to the top so only that path is ignored.
pub fn ignore_path(root: &Path, path: &str, is_dir: bool) -> Result<()> {
    let file = root.join(".gitignore");
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    let line = format!(
        "/{}{}",
        ignore_pattern(path.trim_matches('/')),
        if is_dir { "/" } else { "" }
    );
    if text.lines().any(|existing| existing.trim() == line) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))
}

/// `path` as a `.gitignore` pattern matching only that name: wildcard
/// characters and trailing spaces are escaped.
fn ignore_pattern(path: &str) -> String {
    let mut pattern = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '*' | '?' | '[') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    let kept = pattern.trim_end_matches(' ').len();
    let spaces = pattern.len() - kept;
    pattern.truncate(kept);
    pattern.push_str(&"\\ ".repeat(spaces));
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    fn commit(root: &Path, message: &str) {
        git(root, &["add", "-A"]).unwrap();
        git(root, &["commit", "-q", "-m", message]).unwrap();
    }

    #[test]
    fn history_diffs_and_discard() {
        let root = temp_repo("file-history");
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        commit(&root, "first");
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        commit(&root, "second");
        let log = file_log(&root, "a.txt", 10).unwrap();
        assert_eq!(
            log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(),
            ["second", "first"]
        );
        let shown = commit_diff(&root, &log[0].id, "a.txt").unwrap();
        assert_eq!(shown[0].added(), 1);

        std::fs::write(root.join("a.txt"), "changed\n").unwrap();
        assert_eq!(diff_with_head(&root, "a.txt").unwrap().len(), 1);
        discard_path(&root, "a.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
    }

    #[test]
    fn compares_files_outside_a_repository() {
        let dir = std::env::temp_dir().join(format!("ph-diff-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a, "same\nold\n").unwrap();
        std::fs::write(&b, "same\nnew\n").unwrap();
        let changes = diff_files(&a, &b).unwrap();
        assert_eq!((changes[0].added(), changes[0].removed()), (1, 1));
        assert!(diff_files(&a, &a).unwrap().is_empty());
    }

    #[test]
    fn ignores_a_path_once() {
        let root = temp_repo("file-ignore");
        ignore_path(&root, "build", true).unwrap();
        ignore_path(&root, "build", true).unwrap();
        ignore_path(&root, "notes.txt", false).unwrap();
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(text, "/build/\n/notes.txt\n");
        assert_eq!(ignore_pattern("logs/[a]*?.txt "), r"logs/\[a]\*\?.txt\ ");
    }

    #[test]
    fn paths_are_names_not_patterns() {
        let root = temp_repo("file-literal");
        std::fs::write(root.join("a.rs"), "one\n").unwrap();
        std::fs::write(root.join("*.rs"), "star\n").unwrap();
        commit(&root, "first");
        std::fs::write(root.join("a.rs"), "changed\n").unwrap();
        std::fs::write(root.join("*.rs"), "changed\n").unwrap();
        discard_path(&root, "*.rs").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("*.rs")).unwrap(),
            "star\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.rs")).unwrap(),
            "changed\n"
        );
    }
}
