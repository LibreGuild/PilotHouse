//! Read-only diffs, shared by `.diff` files, the Git tab's review list,
//! commit details, stashes and a pull request's files.
//!
//! Drawn to be read quickly: both files' line numbers, a `+` or `−` in its
//! own column, the lines tinted by kind, and, where a line replaces another,
//! the words that changed marked more strongly (so a renamed variable shows
//! as that word, not as two whole lines to compare by eye). A long line does
//! not wrap or get cut: a file's diff scrolls sideways as one. Long hunks are
//! cut short.

use std::ops::Range;

use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, HighlightStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledText, Window, div,
};
use ph_git::{FileChange, Hunk};

/// Lines drawn per hunk before "… N more lines" (keeps huge diffs fast).
pub const MAX_HUNK_LINES: usize = 300;

/// A pair of lines sharing less than this is two different lines, not one
/// line changed: marking nearly everything in them would only add noise.
const PAIRED_AT_LEAST: f32 = 0.35;

/// Lines longer than this are not compared word by word: a minified file's
/// one line of half a megabyte would take a noticeable part of a second,
/// and marks in it would help nobody.
const LONGEST_COMPARED: usize = 400;

/// Word comparisons remembered (see [`changed_parts`]).
const REMEMBERED: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// Git's own remark (`\ No newline at end of file`).
    Remark,
}

/// One line of a hunk, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// The line without its ` `, `+` or `-`, tabs as four spaces.
    pub text: String,
    /// Its number in the old file (none for an added line).
    pub old: Option<u32>,
    /// Its number in the new file (none for a removed line).
    pub new: Option<u32>,
    /// The parts that differ from the line it replaces, as byte ranges of
    /// `text`; empty for a line that replaces none.
    pub changed: Vec<Range<usize>>,
}

/// The lines of `hunk` that are drawn (its first [`MAX_HUNK_LINES`]) with
/// their numbers (from its `@@ -a,b +c,d @@` header) and, for removed lines
/// followed by as many added ones, what changed within each pair.
pub fn lines(hunk: &Hunk) -> Vec<DiffLine> {
    let start = |sign: char| {
        hunk.header
            .split_whitespace()
            .find_map(|part| part.strip_prefix(sign))
            .and_then(|range| range.split(',').next())
            .and_then(|first| first.parse::<u32>().ok())
            .unwrap_or(1)
    };
    let shown = &hunk.lines[..hunk.lines.len().min(MAX_HUNK_LINES)];
    build(shown, Some((start('-'), start('+'))))
}

/// Lines with their ` `, `+` or `-` in front, from no known place in a file
/// (a chat's summary of an edit): drawn without line numbers.
pub fn unnumbered_lines(lines: &[String]) -> Vec<DiffLine> {
    build(lines, None)
}

fn build(raw: &[String], starts: Option<(u32, u32)>) -> Vec<DiffLine> {
    let numbered = starts.is_some();
    let (mut old, mut new) = starts.unwrap_or((1, 1));
    let mut lines: Vec<DiffLine> = raw
        .iter()
        .map(|line| {
            let mut chars = line.chars();
            let kind = match chars.next() {
                Some('+') => LineKind::Added,
                Some('-') => LineKind::Removed,
                Some('\\') => LineKind::Remark,
                _ => LineKind::Context,
            };
            let text = match kind {
                LineKind::Remark => line.trim_start_matches(['\\', ' ']).to_string(),
                _ => chars.as_str().replace('\t', "    "),
            };
            let number = |counter: &mut u32| {
                let number = *counter;
                *counter += 1;
                numbered.then_some(number)
            };
            let (old, new) = match kind {
                LineKind::Context => (number(&mut old), number(&mut new)),
                LineKind::Added => (None, number(&mut new)),
                LineKind::Removed => (number(&mut old), None),
                LineKind::Remark => (None, None),
            };
            DiffLine {
                kind,
                text,
                old,
                new,
                changed: Vec::new(),
            }
        })
        .collect();

    // Removed lines directly followed by added ones: the first replaces
    // the first, and so on.
    let mut ix = 0;
    while ix < lines.len() {
        if lines[ix].kind != LineKind::Removed {
            ix += 1;
            continue;
        }
        let removed = lines[ix..]
            .iter()
            .take_while(|line| line.kind == LineKind::Removed)
            .count();
        let added = lines[ix + removed..]
            .iter()
            .take_while(|line| line.kind == LineKind::Added)
            .count();
        for pair in 0..removed.min(added) {
            let (before, after) = (ix + pair, ix + removed + pair);
            if let Some((was, now)) = changed_parts(&lines[before].text, &lines[after].text) {
                lines[before].changed = was;
                lines[after].changed = now;
            }
        }
        ix += removed + added;
    }
    lines
}

/// The marked parts of a line, as byte ranges of its text.
type Marks = Vec<Range<usize>>;

/// What differs between a line and the one that replaces it, word by word:
/// the ranges gone from `before` and the ranges new in `after`. `None` when
/// the two share too little to be the same line edited, or are too long to
/// compare.
///
/// Diffs are built as they are drawn, and a list of them is drawn again
/// with every step of a scroll, so comparisons are remembered: the second
/// time costs a hash of the two lines.
fn changed_parts(before: &str, after: &str) -> Option<(Marks, Marks)> {
    use std::hash::{Hash as _, Hasher as _};
    type Remembered = (Box<str>, Box<str>, Option<(Marks, Marks)>);
    thread_local! {
        static PARTS: std::cell::RefCell<std::collections::HashMap<u64, Remembered>> =
            std::cell::RefCell::default();
    }
    if before.len() > LONGEST_COMPARED || after.len() > LONGEST_COMPARED {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (before, after).hash(&mut hasher);
    let key = hasher.finish();
    PARTS.with_borrow_mut(|parts| {
        // The lines are kept and checked: two pairs with one hash must not
        // get each other's ranges, which would lie outside the text.
        if let Some((was, now, found)) = parts.get(&key)
            && (&**was, &**now) == (before, after)
        {
            return found.clone();
        }
        let found = compare_words(before, after);
        if parts.len() >= REMEMBERED {
            parts.clear();
        }
        parts.insert(key, (before.into(), after.into(), found.clone()));
        found
    })
}

fn compare_words(before: &str, after: &str) -> Option<(Marks, Marks)> {
    let diff = similar::TextDiff::from_words(before, after);
    if diff.ratio() < PAIRED_AT_LEAST {
        return None;
    }
    let (mut was, mut now) = (Marks::new(), Marks::new());
    let (mut at_before, mut at_after) = (0, 0);
    // Touching ranges become one: `foo(bar)` changed to `baz(qux)` is one
    // mark, not three.
    let push = |ranges: &mut Marks, range: Range<usize>| match ranges.last_mut() {
        Some(last) if last.end == range.start => last.end = range.end,
        _ => ranges.push(range),
    };
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            similar::ChangeTag::Equal => {
                at_before += len;
                at_after += len;
            }
            similar::ChangeTag::Delete => {
                push(&mut was, at_before..at_before + len);
                at_before += len;
            }
            similar::ChangeTag::Insert => {
                push(&mut now, at_after..at_after + len);
                at_after += len;
            }
        }
    }
    Some((was, now))
}

/// How many digits the line numbers of `lines` need: 0 for lines without
/// numbers, which are then drawn without the number columns.
pub fn number_digits(lines: &[DiffLine]) -> usize {
    lines
        .iter()
        .filter_map(|line| line.old.max(line.new))
        .max()
        .map_or(0, |max| max.to_string().len())
}

/// One line as a row: the old and the new number, the sign, the text. The
/// row is as wide as its text (a diff scrolls sideways, see [`scroller`]);
/// the caller may add to it (the Git tab's note button).
pub fn line_row(line: &DiffLine, digits: usize, cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    let (tint, sign, color) = match line.kind {
        LineKind::Added => (Some(theme.success), "+", theme.foreground),
        LineKind::Removed => (Some(theme.danger), "−", theme.foreground),
        LineKind::Context => (None, "", theme.foreground.opacity(0.78)),
        LineKind::Remark => (None, "", theme.muted_foreground),
    };
    // A digit of the code font is about 0.6 of its size wide.
    let number_width = ph_ui::rems_from_px(7.4 * digits as f32 + 10.);
    let number = |number: Option<u32>| {
        div()
            .flex_none()
            .w(number_width)
            .pr_1p5()
            .text_right()
            .text_color(theme.muted_foreground)
            .when_some(tint, |this, tint| this.bg(tint.opacity(0.10)))
            .children(number.map(|number| number.to_string()))
    };
    let text: SharedString = if line.text.is_empty() {
        // An empty line keeps its height.
        " ".into()
    } else {
        line.text.clone().into()
    };
    let marks = tint.filter(|_| !line.changed.is_empty()).map(|tint| {
        let style = HighlightStyle {
            background_color: Some(tint.opacity(0.32)),
            ..Default::default()
        };
        line.changed
            .iter()
            .filter(|range| range.end <= text.len())
            .map(|range| (range.clone(), style))
            .collect::<Vec<_>>()
    });
    let code = match marks {
        Some(marks) => StyledText::new(text)
            .with_highlights(marks)
            .into_any_element(),
        None => text.into_any_element(),
    };
    h_flex()
        .flex_none()
        .items_stretch()
        .font_family(theme.mono_font_family.clone())
        .whitespace_nowrap()
        .text_color(color)
        .when_some(tint, |this, tint| this.bg(tint.opacity(0.12)))
        .when(line.kind == LineKind::Remark, |this| this.italic())
        .when(digits > 0, |this| {
            this.child(number(line.old)).child(number(line.new))
        })
        .child(
            div()
                .flex_none()
                .w_5()
                .text_center()
                .when_some(tint, |this, tint| this.text_color(tint))
                .child(sign),
        )
        .child(div().flex_none().pr_4().child(code))
}

/// A hunk's lines (without its header row).
pub fn hunk_lines(hunk: &Hunk, cx: &App) -> Vec<AnyElement> {
    let lines = lines(hunk);
    let digits = number_digits(&lines);
    let hidden = hunk.lines.len() - lines.len();
    let mut rows: Vec<AnyElement> = lines
        .iter()
        .map(|line| line_row(line, digits, cx).into_any_element())
        .collect();
    if hidden > 0 {
        rows.push(more_lines(hidden, cx));
    }
    rows
}

/// "… N more lines", under a hunk cut short.
pub fn more_lines(hidden: usize, cx: &App) -> AnyElement {
    div()
        .px_3()
        .py_1()
        .text_color(cx.theme().muted_foreground)
        .child(format!("… {hidden} more lines"))
        .into_any_element()
}

/// A hunk's `@@ … @@` line, over its rows.
pub fn hunk_header(hunk: &Hunk, cx: &App) -> gpui_kit::Div {
    div()
        .flex_none()
        .px_2()
        .py_0p5()
        .whitespace_nowrap()
        .font_family(cx.theme().mono_font_family.clone())
        .bg(cx.theme().muted.opacity(0.6))
        .text_color(cx.theme().muted_foreground)
        .child(hunk.header.clone())
}

/// What a diff's rows sit in: as wide as the longest line (so each row's
/// tint runs its whole width), scrolling sideways with a scrollbar when
/// that is wider than the room. `id` keeps each diff's own scroll position.
pub fn scroller(id: impl Into<ElementId>, rows: Vec<AnyElement>) -> impl IntoElement {
    Scroller {
        id: id.into(),
        rows,
    }
}

#[derive(IntoElement)]
struct Scroller {
    id: ElementId,
    rows: Vec<AnyElement>,
}

impl RenderOnce for Scroller {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // An area of its own, not the library's `overflow_x_scrollbar`:
        // that one takes a mouse wheel's steps (which are vertical) for the
        // sideways scroll, so a list of diffs stopped scrolling down
        // whenever the pointer came over a long line. Here the wheel is the
        // list's; sideways is a swipe, Shift with the wheel, or the bar.
        let handle = window
            .use_keyed_state(self.id.clone(), cx, |_, _| ScrollHandle::default())
            .read(cx)
            .clone();
        div()
            .id(self.id.clone())
            .relative()
            .w_full()
            .child(
                h_flex()
                    .id((self.id.clone(), "area"))
                    .w_full()
                    .items_start()
                    .overflow_x_scroll()
                    .track_scroll(&handle)
                    .restrict_scroll_to_axis()
                    .child(v_flex().flex_none().min_w_full().children(self.rows)),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::new(&handle)
                        .id((self.id, "bar"))
                        .axis(ScrollbarAxis::Horizontal)
                        .viewport_from_layout(),
                ),
            )
    }
}

/// One file's whole diff, read-only.
pub fn file_diff(file: &FileChange, cx: &App) -> AnyElement {
    let (icon, tint) = ph_ui::file_icon(&file.path);
    let rows: Vec<AnyElement> = file
        .hunks
        .iter()
        .enumerate()
        .flat_map(|(ix, hunk)| {
            let header = hunk_header(hunk, cx)
                .when(ix > 0, |this| {
                    this.border_t_1().border_color(cx.theme().border)
                })
                .into_any_element();
            std::iter::once(header).chain(hunk_lines(hunk, cx))
        })
        .collect();
    v_flex()
        // Lists of diffs scroll; a card must not shrink to fit.
        .flex_none()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .overflow_hidden()
        .text_xs()
        .child(
            h_flex()
                .px_2()
                .py_1()
                .gap_1p5()
                .bg(cx.theme().secondary)
                .child(Icon::new(icon).text_color(tint))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(file.path.clone()),
                )
                .child(change_counts(file.added(), file.removed(), cx)),
        )
        .when(file.binary, |this| {
            this.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(cx.theme().muted_foreground)
                    .child("Binary file"),
            )
        })
        .when(!rows.is_empty(), |this| {
            this.child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(scroller(
                        SharedString::from(format!("diff-{}", file.path)),
                        rows,
                    )),
            )
        })
        .into_any_element()
}

/// "+12 −3", each in its colour.
pub fn change_counts(added: usize, removed: usize, cx: &App) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap_1p5()
        .when(added > 0, |this| {
            this.child(
                div()
                    .text_color(cx.theme().success)
                    .child(format!("+{added}")),
            )
        })
        .when(removed > 0, |this| {
            this.child(
                div()
                    .text_color(cx.theme().danger)
                    .child(format!("−{removed}")),
            )
        })
}

/// About how many rows one item of a patch's list lays out: a file longer
/// than that is cut, between hunks, into several items (a hunk shows at
/// most [`MAX_HUNK_LINES`] lines, so no item is much longer).
const ROWS_PER_ITEM: usize = 400;

/// A hunk as it is drawn, read once.
#[derive(PartialEq)]
struct ReadHunk {
    /// The hunk's `@@` line alone (its lines are in `lines`).
    header: Hunk,
    lines: Vec<DiffLine>,
    digits: usize,
    /// Lines past [`MAX_HUNK_LINES`], not drawn.
    hidden: usize,
}

/// A part of one file's card ([`file_diff`]'s look): the file's name on
/// the first, then some of its hunks.
#[derive(PartialEq)]
struct PatchItem {
    /// Which file of the patch, and the first of its hunks here: together
    /// they name the item (a patch may change one path more than once).
    file: usize,
    from: usize,
    /// The file's path, lines added and removed, and whether it is binary:
    /// on the first item of its card.
    head: Option<(SharedString, usize, usize, bool)>,
    hunks: Vec<ReadHunk>,
    last: bool,
}

/// The items a patch is drawn as. Slow for a large one (every line is
/// read, replaced lines compared word by word), so called off the UI
/// thread, once for a text.
fn read_patch(text: &str) -> Vec<PatchItem> {
    let mut items = Vec::new();
    for (file, change) in ph_git::parse_diff(text).iter().enumerate() {
        let mut head = Some((
            SharedString::from(change.path.clone()),
            change.added(),
            change.removed(),
            change.binary,
        ));
        let mut hunks = Vec::new();
        let (mut from, mut rows) = (0, 0);
        for (ix, hunk) in change.hunks.iter().enumerate() {
            let lines = lines(hunk);
            let size = 1 + lines.len() + usize::from(hunk.lines.len() > lines.len());
            if rows > 0 && rows + size > ROWS_PER_ITEM {
                items.push(PatchItem {
                    file,
                    from,
                    head: head.take(),
                    hunks: std::mem::take(&mut hunks),
                    last: false,
                });
                (from, rows) = (ix, 0);
            }
            rows += size;
            hunks.push(ReadHunk {
                header: Hunk {
                    header: hunk.header.clone(),
                    lines: Vec::new(),
                    raw: String::new(),
                },
                digits: number_digits(&lines),
                hidden: hunk.lines.len() - lines.len(),
                lines,
            });
        }
        items.push(PatchItem {
            file,
            from,
            head,
            hunks,
            last: true,
        });
    }
    items
}

impl PatchItem {
    /// The item's part of its file's card; `gap` leaves room under a
    /// card's end, before the next one.
    fn render(&self, gap: bool, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let rows: Vec<AnyElement> = self
            .hunks
            .iter()
            .enumerate()
            .flat_map(|(ix, hunk)| {
                let header = hunk_header(&hunk.header, cx)
                    .when(self.from + ix > 0, |this| {
                        this.border_t_1().border_color(theme.border)
                    })
                    .into_any_element();
                let lines = hunk
                    .lines
                    .iter()
                    .map(|line| line_row(line, hunk.digits, cx).into_any_element());
                let more = (hunk.hidden > 0).then(|| more_lines(hunk.hidden, cx));
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
            .when_some(
                self.head.as_ref(),
                |this, (path, added, removed, binary)| {
                    let (icon, tint) = ph_ui::file_icon(path);
                    this.border_t_1()
                        .rounded_t(theme.radius)
                        .child(
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
                                        .child(path.clone()),
                                )
                                .child(change_counts(*added, *removed, cx)),
                        )
                        .when(*binary, |this| {
                            this.child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .text_color(theme.muted_foreground)
                                    .child("Binary file"),
                            )
                        })
                },
            )
            .when(!rows.is_empty(), |this| {
                this.child(
                    div()
                        .when(self.head.is_some(), |this| {
                            this.border_t_1().border_color(theme.border)
                        })
                        .child(scroller(
                            SharedString::from(format!("patch-{}-{}", self.file, self.from)),
                            rows,
                        )),
                )
            })
            .when(self.last, |this| this.border_b_1().rounded_b(theme.radius));
        div()
            .px_3()
            .when(self.last && gap, |this| this.pb_3())
            .child(card)
            .into_any_element()
    }
}

/// A `.diff` or `.patch` file: each file's changes, tinted. A patch can be
/// tens of thousands of lines: it is read once for a text, off the UI
/// thread, and shown as a list that builds only the rows on screen
/// (drawn whole and read again on every frame, scrolling one took seconds
/// a step).
pub struct DiffPreview {
    items: Vec<PatchItem>,
    list: gpui_kit::ListState,
    /// A text has been read (before that there is nothing to say of it).
    read: bool,
    _read: Option<gpui_kit::Task<()>>,
}

impl Default for DiffPreview {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            list: gpui_kit::ListState::new(0, gpui_kit::ListAlignment::Top, gpui_kit::px(1000.)),
            read: false,
            _read: None,
        }
    }
}

impl DiffPreview {
    pub fn set_text(&mut self, text: &str, cx: &mut gpui_kit::Context<Self>) {
        use gpui_kit::AppContext as _;
        let text: std::sync::Arc<str> = text.into();
        let read = cx.background_spawn(async move { read_patch(&text) });
        // The newest text wins: an older one still being read is dropped.
        self._read = Some(cx.spawn(async move |this, cx| {
            let items = read.await;
            _ = this.update(cx, |this, cx| {
                // Only the items that changed are measured again, so the
                // list stays where it is while the patch is edited.
                let (old, new) = (this.items.len(), items.len());
                let same_start = this
                    .items
                    .iter()
                    .zip(&items)
                    .take_while(|(a, b)| a == b)
                    .count();
                let same_end = this
                    .items
                    .iter()
                    .rev()
                    .zip(items.iter().rev())
                    .take(old.min(new) - same_start)
                    .take_while(|(a, b)| a == b)
                    .count();
                this.list
                    .splice(same_start..old - same_end, new - same_end - same_start);
                this.items = items;
                this.read = true;
                cx.notify();
            });
        }));
    }
}

impl gpui_kit::Render for DiffPreview {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl IntoElement {
        if self.items.is_empty() {
            let text = if self.read {
                "No changes in this patch."
            } else {
                "Reading the patch…"
            };
            return crate::notice(text, cx);
        }
        gpui_kit::list(
            self.list.clone(),
            cx.processor(|this, ix: usize, _, cx| match this.items.get(ix) {
                Some(item) => item.render(ix + 1 < this.items.len(), cx),
                None => div().into_any_element(),
            }),
        )
        .size_full()
        .py_3()
        .bg(cx.theme().background)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(header: &str, lines: &[&str]) -> Hunk {
        Hunk {
            header: header.into(),
            lines: lines.iter().map(|line| line.to_string()).collect(),
            raw: String::new(),
        }
    }

    #[test]
    fn a_patch_is_read_into_items_a_file_or_part_of_one_each() {
        let mut patch = String::new();
        // One short file, one long (6 hunks of 150 lines), and the first
        // path once more, as a patch of several commits has it.
        for (path, hunks, lines) in [("a.rs", 2, 10), ("b.rs", 6, 150), ("a.rs", 1, 1)] {
            patch.push_str(&format!(
                "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"
            ));
            for hunk in 0..hunks {
                let at = hunk * 1000 + 1;
                patch.push_str(&format!("@@ -{at},0 +{at},{lines} @@\n"));
                for line in 0..lines {
                    patch.push_str(&format!("+line {line}\n"));
                }
            }
        }
        let items = read_patch(&patch);
        let parts: Vec<(usize, usize, usize, bool, bool)> = items
            .iter()
            .map(|item| {
                (
                    item.file,
                    item.from,
                    item.hunks.len(),
                    item.head.is_some(),
                    item.last,
                )
            })
            .collect();
        assert_eq!(
            parts,
            [
                (0, 0, 2, true, true),
                // Cut between hunks: two of them are 302 rows, a third
                // would pass the 400 an item may be.
                (1, 0, 2, true, false),
                (1, 2, 2, false, false),
                (1, 4, 2, false, true),
                (2, 0, 1, true, true),
            ]
        );
        assert_eq!(items[0].head.as_ref().unwrap().1, 20);
        assert_eq!(items[1].hunks[0].lines.len(), 150);
        // The same text read again is the same items: an edit elsewhere
        // leaves them as they are in the list.
        assert!(read_patch(&patch) == items);
        assert!(read_patch("not a patch").is_empty());
    }

    #[test]
    fn lines_are_numbered_on_both_sides() {
        let lines = lines(&hunk(
            "@@ -10,3 +12,4 @@ fn main()",
            &[" a", "-b", "+c", "+d", " e", "\\ No newline at end of file"],
        ));
        let numbers: Vec<_> = lines.iter().map(|line| (line.old, line.new)).collect();
        assert_eq!(
            numbers,
            vec![
                (Some(10), Some(12)),
                (Some(11), None),
                (None, Some(13)),
                (None, Some(14)),
                (Some(12), Some(15)),
                (None, None),
            ]
        );
        assert_eq!(lines[1].text, "b");
        assert_eq!(lines[5].kind, LineKind::Remark);
        assert_eq!(lines[5].text, "No newline at end of file");
        assert_eq!(number_digits(&lines), 2);
    }

    #[test]
    fn a_replaced_line_marks_the_words_that_changed() {
        let lines = lines(&hunk(
            "@@ -1,2 +1,2 @@",
            &["-    let retries = 3;", "+    let attempts = 3;", " done"],
        ));
        fn marked(line: &DiffLine) -> Vec<&str> {
            line.changed
                .iter()
                .map(|range| &line.text[range.clone()])
                .collect()
        }
        assert_eq!(marked(&lines[0]), vec!["retries"]);
        assert_eq!(marked(&lines[1]), vec!["attempts"]);
        assert!(lines[2].changed.is_empty());
    }

    #[test]
    fn long_hunks_and_long_lines_are_not_compared() {
        // Only the lines that are drawn are built.
        let many: Vec<String> = (0..MAX_HUNK_LINES + 50).map(|n| format!(" {n}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(lines(&hunk("@@ -1 +1 @@", &many)).len(), MAX_HUNK_LINES);
        // A minified line is shown as changed, without marks.
        let long = "x = 1; ".repeat(LONGEST_COMPARED);
        let (was, now) = (format!("-{long}"), format!("+{long}y"));
        let lines = lines(&hunk("@@ -1 +1 @@", &[&was, &now]));
        assert!(lines[0].changed.is_empty() && lines[1].changed.is_empty());
        // Remembered comparisons give the same answer the second time.
        let pair = ["-let retries = 3;", "+let attempts = 3;"];
        let first = super::lines(&hunk("@@ -1 +1 @@", &pair));
        assert_eq!(first, super::lines(&hunk("@@ -1 +1 @@", &pair)));
        assert_eq!(first[1].changed, vec![4..12]);
    }

    #[test]
    fn unrelated_lines_are_not_marked_word_by_word() {
        let lines = lines(&hunk(
            "@@ -1 +1 @@",
            &["-use std::fmt;", "+fn render(&self) -> Element {"],
        ));
        assert!(lines[0].changed.is_empty() && lines[1].changed.is_empty());
        // A tab reads as four spaces, and ranges are of the shown text.
        let tabbed = super::lines(&hunk("@@ -1 +1 @@", &["-\tfoo(a)", "+\tfoo(b)"]));
        assert_eq!(tabbed[0].text, "    foo(a)");
    }
}
