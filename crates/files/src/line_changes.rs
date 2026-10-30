//! Which lines of an open file differ from what Git has staged, for the
//! bars beside the line numbers (VS Code's quick diff): green for added
//! lines, blue for changed ones, a red mark where lines were deleted.

use std::time::Duration;

use similar::{Algorithm, DiffOp, TextDiff};

/// How long a diff may take before it settles for a rougher answer (a big
/// file rewritten throughout).
const DIFF_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineChange {
    Added,
    Modified,
    /// Lines were deleted just above this one (or at the end, when it is
    /// one past the last line).
    Deleted,
}

/// The changed lines of `text` against `base`, by 0-based line, in order.
pub fn line_changes(base: &str, text: &str) -> Vec<(usize, LineChange)> {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DIFF_TIMEOUT)
        .diff_lines(base, text);
    let mut changes = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => changes
                .extend((new_index..new_index + new_len).map(|line| (line, LineChange::Added))),
            DiffOp::Delete { new_index, .. } => changes.push((new_index, LineChange::Deleted)),
            DiffOp::Replace {
                old_len,
                new_index,
                new_len,
                ..
            } => {
                let changed = old_len.min(new_len);
                changes.extend(
                    (new_index..new_index + changed).map(|line| (line, LineChange::Modified)),
                );
                changes.extend(
                    (new_index + changed..new_index + new_len)
                        .map(|line| (line, LineChange::Added)),
                );
                if old_len > new_len {
                    changes.push((new_index + new_len, LineChange::Deleted));
                }
            }
        }
    }
    changes.sort_by_key(|(line, _)| *line);
    // A deletion right under a changed line is shown by that line.
    changes.dedup_by(|later, earlier| later.0 == earlier.0 && later.1 == LineChange::Deleted);
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use LineChange::*;

    #[test]
    fn added_changed_and_deleted_lines() {
        let base = "a\nb\nc\nd\ne\n";
        assert!(line_changes(base, base).is_empty());
        assert_eq!(line_changes(base, "a\nb\nnew\nc\nd\ne\n"), [(2, Added)]);
        assert_eq!(line_changes(base, "a\nB\nc\nd\ne\n"), [(1, Modified)]);
        assert_eq!(line_changes(base, "a\nc\nd\ne\n"), [(1, Deleted)]);
        assert_eq!(line_changes(base, "a\nb\nc\nd\n"), [(4, Deleted)]);
        // Two lines become three: two changed, one added.
        assert_eq!(
            line_changes(base, "a\nB\nC\nX\nd\ne\n"),
            [(1, Modified), (2, Modified), (3, Added)]
        );
        // Three become one: changed, with a deletion under it.
        assert_eq!(
            line_changes(base, "a\nX\ne\n"),
            [(1, Modified), (2, Deleted)]
        );
        // A new file: every line added.
        assert_eq!(line_changes("", "x\ny"), [(0, Added), (1, Added)]);
    }
}
