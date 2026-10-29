//! Unified diffs split into files and hunks, so changes can be reviewed and
//! applied one hunk at a time.

/// One file with unstaged changes.
#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    pub path: String,
    /// The `diff --git` … `+++` lines; a hunk patch is this header plus the hunk.
    header: String,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@ context` line.
    pub header: String,
    /// Lines with their ` `/`+`/`-` prefix.
    pub lines: Vec<String>,
}

impl Hunk {
    pub fn added(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.starts_with('+'))
            .count()
    }

    pub fn removed(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.starts_with('-'))
            .count()
    }
}

impl FileChange {
    /// A patch containing only hunk `ix`, for `git apply`.
    pub fn hunk_patch(&self, ix: usize) -> Option<String> {
        let hunk = self.hunks.get(ix)?;
        let mut patch = self.header.clone();
        patch.push_str(&hunk.header);
        patch.push('\n');
        for line in &hunk.lines {
            patch.push_str(line);
            patch.push('\n');
        }
        Some(patch)
    }

    pub fn added(&self) -> usize {
        self.hunks.iter().map(Hunk::added).sum()
    }

    pub fn removed(&self) -> usize {
        self.hunks.iter().map(Hunk::removed).sum()
    }

    /// The whole file's patch, as `git diff` printed it.
    pub fn patch(&self) -> String {
        let mut patch = self.header.clone();
        for hunk in &self.hunks {
            patch.push_str(&hunk.header);
            patch.push('\n');
            for line in &hunk.lines {
                patch.push_str(line);
                patch.push('\n');
            }
        }
        patch
    }

    /// What happened to the file: `New`, `Deleted`, `Renamed` or `Changed`.
    pub fn kind(&self) -> ChangeKind {
        let has = |prefix: &str| self.header.lines().any(|line| line.starts_with(prefix));
        if has("new file mode") {
            ChangeKind::New
        } else if has("deleted file mode") {
            ChangeKind::Deleted
        } else if has("rename from ") {
            ChangeKind::Renamed
        } else {
            ChangeKind::Changed
        }
    }
}

/// What a change did to its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    New,
    Deleted,
    Renamed,
    Changed,
}

/// Splits `git diff` output into files and hunks.
pub fn parse_diff(diff: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            files.push(FileChange {
                path: path_from_header(line),
                header: format!("{line}\n"),
                hunks: Vec::new(),
                binary: false,
            });
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            file.hunks.push(Hunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
            in_hunk = true;
        } else if in_hunk {
            if let Some(hunk) = file.hunks.last_mut() {
                hunk.lines.push(line.to_string());
            }
        } else {
            if line.starts_with("Binary files") {
                file.binary = true;
            }
            if let Some(path) = line.strip_prefix("+++ b/") {
                file.path = path.to_string();
            }
            file.header.push_str(line);
            file.header.push('\n');
        }
    }
    files
}

/// `diff --git a/x b/x` → `x` (the `+++` line refines it later).
fn path_from_header(line: &str) -> String {
    line.rsplit_once(" b/")
        .map(|(_, path)| path.to_string())
        .unwrap_or_else(|| line.trim_start_matches("diff --git ").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_patch_and_kind_come_back() {
        let diff = "diff --git a/new.rs b/new.rs\nnew file mode 100644\nindex 0000000..e69de29\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1,2 @@\n+fn a() {}\n+fn b() {}\ndiff --git a/old.rs b/old.rs\nindex 1..2 100644\n--- a/old.rs\n+++ b/old.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let files = parse_diff(diff);
        assert_eq!(files[0].kind(), ChangeKind::New);
        assert_eq!(files[1].kind(), ChangeKind::Changed);
        let joined: String = files.iter().map(FileChange::patch).collect();
        assert_eq!(joined, diff);
    }

    const DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,3 @@
 fn a() {}
-fn b() {}
+fn b() { todo!() }
 fn c() {}
@@ -10,2 +10,3 @@ fn d() {
 let x = 1;
+let y = 2;
 let z = 3;
";

    #[test]
    fn parses_files_hunks_and_counts() {
        let files = parse_diff(DIFF);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/lib.rs");
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!((files[0].added(), files[0].removed()), (2, 1));
        let patch = files[0].hunk_patch(1).unwrap();
        assert!(patch.starts_with("diff --git a/src/lib.rs b/src/lib.rs\n"));
        assert!(patch.contains("+++ b/src/lib.rs\n@@ -10,2 +10,3 @@"));
        assert!(!patch.contains("todo!"));
    }
}

/// What the branch checked out at `root` changed against its base: the
/// default branch (`origin/HEAD`, else `main` or `master`), from where they
/// part. `None` when there is no base to compare with, or HEAD is it.
pub fn branch_changes(root: &std::path::Path) -> anyhow::Result<Option<BranchChanges>> {
    let git = |args: &[&str]| crate::git(root, args).map(|out| out.trim().to_string());
    let Some(base) = default_base(root) else {
        return Ok(None);
    };
    // On the base branch itself there is nothing to compare. (A local
    // `main` against `origin/main` shows what is not pushed yet.)
    let head = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if head == base {
        return Ok(None);
    }
    let fork = git(&["merge-base", &base, "HEAD"])?;
    let diff = crate::git(
        root,
        // As the Changes view reads diffs: no converters, three lines of
        // context whatever the user's config says.
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=3",
            &fork,
            "HEAD",
        ],
    )?;
    Ok(Some(BranchChanges {
        base,
        fork: fork.chars().take(7).collect(),
        files: parse_diff(&diff),
    }))
}

/// The repository's default branch, as a ref to compare with:
/// `origin/HEAD`'s branch (`origin/main`), else a local `main` or `master`.
pub fn default_base(root: &std::path::Path) -> Option<String> {
    let git = |args: &[&str]| crate::git(root, args).map(|out| out.trim().to_string());
    git(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ])
    .ok()
    .filter(|base| !base.is_empty())
    .or_else(|| {
        ["main", "master"]
            .into_iter()
            .find(|name| {
                git(&[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{name}"),
                ])
                .is_ok()
            })
            .map(str::to_string)
    })
}

/// See [`branch_changes`].
#[derive(Debug, Clone, PartialEq)]
pub struct BranchChanges {
    pub base: String,
    /// Where the branch left the base (short commit id).
    pub fork: String,
    pub files: Vec<FileChange>,
}
