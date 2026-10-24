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
