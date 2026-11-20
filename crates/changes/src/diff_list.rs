//! Files' diffs for a list that draws only what shows (the Branch view, a
//! commit in History, an open stash).
//!
//! A branch can differ from its base in hundreds of files and tens of
//! thousands of lines: drawn whole, every frame took seconds. Here the lines
//! are read once, off the UI thread (word marks included), and each file is
//! one or more items of a `gpui_kit::list`, so a frame builds the rows on
//! screen only. The items look like [`ph_preview::diff::file_diff`]'s card:
//! a file too long for one item is cut at its hunks into several, which
//! together draw that card.

use std::ops::Range;

use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use ph_git::{FileChange, Hunk};
use ph_preview::diff::{self as diff_view, DiffLine};

/// About how many rows one item may lay out: a file longer than that is
/// cut, between hunks, into several items (a hunk shows at most
/// [`diff_view::MAX_HUNK_LINES`] lines, so no item is much longer).
const ROWS_PER_ITEM: usize = 400;

/// What a list of diffs draws, built once when the files are read.
#[derive(Default)]
pub(crate) struct Diffs {
    files: Vec<File>,
    items: Vec<Item>,
}

struct File {
    path: SharedString,
    added: usize,
    removed: usize,
    binary: bool,
    hunks: Vec<BuiltHunk>,
}

struct BuiltHunk {
    /// The hunk's `@@` line alone (its lines are in `lines`).
    header: Hunk,
    lines: Vec<DiffLine>,
    digits: usize,
    /// Lines past [`diff_view::MAX_HUNK_LINES`], not drawn.
    hidden: usize,
}

/// A part of one file's card: its name on the first, then some of its hunks.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    file: usize,
    hunks: Range<usize>,
    first: bool,
    last: bool,
}

impl Diffs {
    /// Reads `files` into rows; slow for a large diff, so called off the UI
    /// thread.
    pub(crate) fn build(files: &[FileChange]) -> Self {
        let mut diffs = Diffs::default();
        for change in files {
            let hunks: Vec<BuiltHunk> = change
                .hunks
                .iter()
                .map(|hunk| {
                    let lines = diff_view::lines(hunk);
                    BuiltHunk {
                        header: Hunk {
                            header: hunk.header.clone(),
                            lines: Vec::new(),
                            raw: String::new(),
                        },
                        digits: diff_view::number_digits(&lines),
                        hidden: hunk.lines.len() - lines.len(),
                        lines,
                    }
                })
                .collect();
            let file = diffs.files.len();
            let start = diffs.items.len();
            let mut from = 0;
            let mut rows = 0;
            for (ix, hunk) in hunks.iter().enumerate() {
                let size = 1 + hunk.lines.len() + usize::from(hunk.hidden > 0);
                if rows > 0 && rows + size > ROWS_PER_ITEM {
                    diffs.items.push(Item {
                        file,
                        hunks: from..ix,
                        first: from == 0,
                        last: false,
                    });
                    from = ix;
                    rows = 0;
                }
                rows += size;
            }
            diffs.items.push(Item {
                file,
                hunks: from..hunks.len(),
                first: diffs.items.len() == start,
                last: true,
            });
            diffs.files.push(File {
                path: change.path.clone().into(),
                added: change.added(),
                removed: change.removed(),
                binary: change.binary,
                hunks,
            });
        }
        diffs
    }

    /// How many list items the diffs are.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Item `ix`. `id` tells this list's sideways scroll positions from
    /// another's. A gap follows each file's card but the last one's.
    pub(crate) fn render_item(&self, id: &str, ix: usize, cx: &App) -> AnyElement {
        let Some(item) = self.items.get(ix) else {
            return div().into_any_element();
        };
        let file = &self.files[item.file];
        let theme = cx.theme();
        let rows: Vec<AnyElement> = file.hunks[item.hunks.clone()]
            .iter()
            .zip(item.hunks.clone())
            .flat_map(|(hunk, at)| {
                let header = diff_view::hunk_header(&hunk.header, cx)
                    .when(at > 0, |this| this.border_t_1().border_color(theme.border))
                    .into_any_element();
                let lines = hunk
                    .lines
                    .iter()
                    .map(|line| diff_view::line_row(line, hunk.digits, cx).into_any_element());
                let more = (hunk.hidden > 0).then(|| diff_view::more_lines(hunk.hidden, cx));
                std::iter::once(header).chain(lines).chain(more)
            })
            .collect();
        let card = v_flex()
            .flex_none()
            .border_l_1()
            .border_r_1()
            .border_color(theme.border)
            .overflow_hidden()
            .text_xs()
            .when(item.first, |this| {
                let (icon, tint) = ph_ui::file_icon(&file.path);
                this.border_t_1().rounded_t(theme.radius).child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .gap_1p5()
                        .bg(theme.secondary)
                        .child(Icon::new(icon).text_color(tint))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .child(file.path.clone()),
                        )
                        .child(diff_view::change_counts(file.added, file.removed, cx)),
                )
            })
            .when(item.first && file.binary, |this| {
                this.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_color(theme.muted_foreground)
                        .child("Binary file"),
                )
            })
            .when(!rows.is_empty(), |this| {
                this.child(
                    div()
                        .when(item.first, |this| {
                            this.border_t_1().border_color(theme.border)
                        })
                        .child(diff_view::scroller(
                            SharedString::from(format!(
                                "{id}-diff-{}-{}",
                                file.path, item.hunks.start
                            )),
                            rows,
                        )),
                )
            })
            .when(item.last, |this| this.border_b_1().rounded_b(theme.radius));
        div()
            .when(item.last && ix + 1 < self.items.len(), |this| this.pb_2())
            .child(card)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, hunks: usize, lines: usize) -> FileChange {
        let mut patch = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
        for hunk in 0..hunks {
            let at = hunk * 1000 + 1;
            patch.push_str(&format!("@@ -{at},0 +{at},{lines} @@\n"));
            for line in 0..lines {
                patch.push_str(&format!("+line {line}\n"));
            }
        }
        ph_git::parse_diff(&patch).remove(0)
    }

    #[test]
    fn a_long_file_is_cut_between_hunks_and_a_short_one_is_one_item() {
        let diffs = Diffs::build(&[file("a.rs", 2, 10), file("b.rs", 6, 150)]);
        let parts: Vec<(usize, Range<usize>, bool, bool)> = diffs
            .items
            .iter()
            .map(|item| (item.file, item.hunks.clone(), item.first, item.last))
            .collect();
        assert_eq!(
            parts,
            vec![
                (0, 0..2, true, true),
                (1, 0..2, true, false),
                (1, 2..4, false, false),
                (1, 4..6, false, true),
            ]
        );
    }

    #[test]
    fn a_hunk_longer_than_an_item_is_one_item_and_no_file_is_lost() {
        let diffs = Diffs::build(&[file("big.rs", 1, 2000)]);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs.items[0].hunks, 0..1);
        let hunk = &diffs.files[0].hunks[0];
        assert_eq!(hunk.lines.len(), diff_view::MAX_HUNK_LINES);
        assert_eq!(hunk.hidden, 2000 - diff_view::MAX_HUNK_LINES);
    }
}
