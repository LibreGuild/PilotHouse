//! How lines moved between two commits, from `git diff -U0 -M`: where a
//! finding's lines are now, and whether code changed there. Decided from
//! git, not by the agent: a new finding in lines that did not change since
//! the last round was missed earlier ("found late").

/// One changed run of lines: `count` lines at `start` before, and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    old_start: u64,
    old_count: u64,
    new_start: u64,
    new_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FileChange {
    /// `None`: added.
    old: Option<String>,
    /// `None`: deleted.
    new: Option<String>,
    hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineMap {
    files: Vec<FileChange>,
}

/// Where lines are after the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moved {
    /// In `path` at `start..=end`; `edited` when some of them changed.
    At {
        path: String,
        start: u64,
        end: u64,
        edited: bool,
    },
    /// Their file was deleted.
    Deleted,
}

impl LineMap {
    /// Reads `git diff -U0 -M <old> <new>`.
    pub fn parse(diff: &str) -> Self {
        let mut files: Vec<FileChange> = Vec::new();
        for line in diff.lines() {
            if line.starts_with("diff --git ") {
                files.push(FileChange::default());
                continue;
            }
            let Some(file) = files.last_mut() else {
                continue;
            };
            if let Some(path) = line.strip_prefix("--- ") {
                file.old = side(path, "a/");
            } else if let Some(path) = line.strip_prefix("+++ ") {
                file.new = side(path, "b/");
            } else if let Some(path) = line.strip_prefix("rename from ") {
                file.old = Some(path.to_string());
            } else if let Some(path) = line.strip_prefix("rename to ") {
                file.new = Some(path.to_string());
            } else if line.starts_with("@@")
                && let Some(hunk) = parse_hunk(line)
            {
                file.hunks.push(hunk);
            }
        }
        Self { files }
    }

    /// Where lines `start..=end` of `path` (before) are now.
    pub fn relocate(&self, path: &str, start: u64, end: u64) -> Moved {
        let end = end.max(start);
        let Some(file) = self
            .files
            .iter()
            .find(|file| file.old.as_deref() == Some(path))
        else {
            // Not in the diff: unchanged and in place.
            return Moved::At {
                path: path.to_string(),
                start,
                end,
                edited: false,
            };
        };
        let Some(new_path) = file.new.clone() else {
            return Moved::Deleted;
        };
        let mut shift: i64 = 0;
        let mut edited = false;
        for hunk in &file.hunks {
            let old_end = hunk.old_start + hunk.old_count.max(1) - 1;
            if hunk.old_count > 0 && hunk.old_start <= end && old_end >= start {
                edited = true;
            }
            // Hunks wholly before the lines move them.
            let before = if hunk.old_count == 0 {
                hunk.old_start < start
            } else {
                old_end < start
            };
            if before {
                shift += hunk.new_count as i64 - hunk.old_count as i64;
            }
        }
        let place = |line: u64| (line as i64 + shift).max(1) as u64;
        Moved::At {
            path: new_path,
            start: place(start),
            end: place(end),
            edited,
        }
    }

    /// Whether any of lines `start..=end` of `path` (after) changed.
    pub fn touched(&self, path: &str, start: u64, end: u64) -> bool {
        let end = end.max(start);
        self.files
            .iter()
            .filter(|file| file.new.as_deref() == Some(path))
            .any(|file| {
                file.old.is_none()
                    || file.hunks.iter().any(|hunk| {
                        hunk.new_count > 0
                            && hunk.new_start <= end
                            && hunk.new_start + hunk.new_count > start
                    })
            })
    }
}

/// `a/src/x.rs` → `src/x.rs`; `/dev/null` → none.
fn side(path: &str, prefix: &str) -> Option<String> {
    let path = path.trim();
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix(prefix).unwrap_or(path).to_string())
}

/// `@@ -12,3 +14,0 @@`.
fn parse_hunk(line: &str) -> Option<Hunk> {
    let mut parts = line.split_whitespace().skip(1);
    let range = |text: &str| -> Option<(u64, u64)> {
        let (start, count) = match text.split_once(',') {
            Some((start, count)) => (start.parse().ok()?, count.parse().ok()?),
            None => (text.parse().ok()?, 1),
        };
        Some((start, count))
    };
    let (old_start, old_count) = range(parts.next()?.strip_prefix('-')?)?;
    let (new_start, new_count) = range(parts.next()?.strip_prefix('+')?)?;
    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -2,0 +3,2 @@ fn x\n+one\n+two\n@@ -10,2 +12,1 @@\n-old\n-old\n+new\ndiff --git a/old.rs b/new.rs\nsimilarity index 90%\nrename from old.rs\nrename to new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\n--- a/gone.rs\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-a\n-b\n-c\n";

    #[test]
    fn moves_lines_past_changes_above_them() {
        let map = LineMap::parse(DIFF);
        assert_eq!(
            map.relocate("src/a.rs", 5, 6),
            Moved::At {
                path: "src/a.rs".into(),
                start: 7,
                end: 8,
                edited: false
            }
        );
        assert!(matches!(
            map.relocate("src/a.rs", 10, 10),
            Moved::At { edited: true, .. }
        ));
        assert!(matches!(
            map.relocate("old.rs", 3, 3),
            Moved::At { ref path, start: 3, .. } if path == "new.rs"
        ));
        assert_eq!(map.relocate("gone.rs", 1, 1), Moved::Deleted);
        assert!(matches!(
            map.relocate("same.rs", 4, 4),
            Moved::At {
                start: 4,
                edited: false,
                ..
            }
        ));
    }

    #[test]
    fn knows_which_lines_changed() {
        let map = LineMap::parse(DIFF);
        assert!(map.touched("src/a.rs", 3, 4));
        assert!(map.touched("src/a.rs", 12, 12));
        assert!(!map.touched("src/a.rs", 6, 9));
        assert!(!map.touched("other.rs", 1, 5));
    }
}
