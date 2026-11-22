//! The review of the working tree's changes, beside their list: every
//! changed file in one diff (gpui-component's viewer: in one column or side
//! by side, with syntax colours), where a file is ticked as looked at, kept
//! or undone, a hunk kept or discarded, and a line given a note for the
//! agent.
//!
//! The list's rows unfolded to their hunks before, and a click on a row
//! opened the same changes once more in the editor: two places to read one
//! diff, with what can be done about it in only one of them. Now the list
//! picks and this pane shows.
//!
//! While an agent writes, the list follows it; the diff does not, or the
//! lines being read would jump away. It says the files moved on and waits to
//! be asked (or for the next file to be picked).

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::diff::{
    Diff, DiffAnnotation, DiffFile, DiffLinePosition, DiffLineRange, DiffMode, DiffSide, DiffState,
};
use gpui_kit::component::resizable::ResizableState;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::{App, ElementId, WeakEntity};

use super::*;

/// The fingerprint a new file's tick is kept with: Git has no changes of
/// its to tell one state from another.
const NEW_FILE: u64 = 0;

/// Where the diff goes once its files are drawn anew.
#[derive(Clone)]
pub(super) enum Goto {
    File(String),
    Line(DiffLinePosition),
}

/// What stands under a line of the diff.
#[derive(Clone, Copy)]
enum Mark {
    /// Review note `ix`.
    Note(usize),
    /// What to do with hunk `hunk` of changed file `file`.
    Hunk { file: usize, hunk: usize },
}

pub(super) struct Review {
    pub state: Entity<DiffState>,
    /// The patch the diff shows.
    shown: String,
    /// The working tree changed since the diff was drawn.
    pub stale: bool,
    /// The next read of the working tree is drawn at once: the user caused
    /// it (kept, discarded, asked).
    pub follow: bool,
    /// The file picked in the list.
    pub selected: Option<String>,
    /// A new file picked in the list, shown with the changes, all added.
    pub new_file: Option<String>,
    /// That file's lines as the diff has them, for a note's context.
    new_change: Option<FileChange>,
    /// Files ticked as looked at, each with what its changes were then: a
    /// file changed since shows unticked again.
    pub viewed: HashMap<String, u64>,
    goto: Option<Goto>,
    _load: Option<Task<()>>,
    /// The line between the list and the diff: beside each other, or the
    /// list over the diff in a narrow pane.
    pub split: Entity<ResizableState>,
    pub stack: Entity<ResizableState>,
}

impl Review {
    pub fn new(cx: &mut Context<ChangesPanel>) -> Self {
        Self {
            state: cx.new(|cx| DiffState::new(Vec::new(), cx)),
            shown: String::new(),
            stale: false,
            follow: false,
            selected: None,
            new_file: None,
            new_change: None,
            viewed: HashMap::new(),
            goto: None,
            _load: None,
            split: cx.new(|_| ResizableState::default()),
            stack: cx.new(|_| ResizableState::default()),
        }
    }

    /// There is a diff to show.
    pub fn has_files(&self, cx: &App) -> bool {
        !self.state.read(cx).files().is_empty()
    }
}

/// What a file's changes are, in a number: the same changes give the same
/// one.
fn fingerprint(file: &FileChange) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    file.binary.hash(&mut hasher);
    for hunk in &file.hunks {
        hunk.header.hash(&mut hasher);
        hunk.lines.hash(&mut hasher);
    }
    // Never a new file's.
    hasher.finish().max(NEW_FILE + 1)
}

/// Each line of `hunk` with its side and number there (a line that stays
/// counts on the new side), from its `@@ -a,b +c,d @@` header.
fn numbered(hunk: &git::Hunk) -> impl Iterator<Item = (DiffSide, usize, &str)> {
    let start = |mark: char| {
        hunk.header
            .split_whitespace()
            .find_map(|part| part.strip_prefix(mark))
            .and_then(|range| range.split(',').next())
            .and_then(|first| first.parse::<usize>().ok())
            .unwrap_or(1)
    };
    let (mut old, mut new) = (start('-'), start('+'));
    hunk.lines.iter().filter_map(move |line| {
        let at = match line.chars().next() {
            Some('-') => {
                old += 1;
                (DiffSide::Original, old - 1)
            }
            // "\ No newline at end of file" is no line of the file.
            Some('\\') => return None,
            Some('+') => {
                new += 1;
                (DiffSide::Modified, new - 1)
            }
            _ => {
                old += 1;
                new += 1;
                (DiffSide::Modified, new - 1)
            }
        };
        Some((at.0, at.1, line.as_str()))
    })
}

impl ChangesPanel {
    /// The working tree was read and is not as it was: ticks of files that
    /// changed since go, and the diff is drawn anew or says it is behind.
    pub(super) fn changes_read(&mut self, cx: &mut Context<Self>) {
        let State::Loaded(changes) = &self.state else {
            self.review.viewed.clear();
            self.review.selected = None;
            self.review.new_file = None;
            self.review.new_change = None;
            self.review.shown.clear();
            self.review.stale = false;
            self.review
                .state
                .update(cx, |state, cx| state.set_files(Vec::new(), cx));
            return;
        };
        let prints: HashMap<&str, u64> = changes
            .files
            .iter()
            .map(|file| (file.path.as_str(), fingerprint(file)))
            .collect();
        let is_new = |path: &str| changes.untracked.iter().any(|new| new == path);
        self.review.viewed.retain(|path, print| {
            prints.get(path.as_str()) == Some(print) || (*print == NEW_FILE && is_new(path))
        });
        if self
            .review
            .selected
            .as_deref()
            .is_some_and(|path| !prints.contains_key(path) && !is_new(path))
        {
            self.review.selected = None;
        }
        if self
            .review
            .new_file
            .as_deref()
            .is_some_and(|path| !is_new(path))
        {
            self.review.new_file = None;
            self.review.new_change = None;
        }
        let drawn = changes.patch == self.review.shown;
        // Nothing drawn yet, or the user caused the change: at once.
        if self.review.follow || self.review.shown.is_empty() || !self.review.has_files(cx) {
            self.load_review(cx);
        } else if !drawn {
            self.review.stale = true;
        }
    }

    /// Draws the diff from the working tree as it was last read, off the
    /// UI thread (a long patch takes a moment to take apart).
    pub(super) fn load_review(&mut self, cx: &mut Context<Self>) {
        let State::Loaded(changes) = &self.state else {
            return;
        };
        let patch = changes.patch.clone();
        let new_file = self.review.new_file.clone();
        let root = self.root.clone();
        self.review.follow = false;
        self.review.stale = false;
        self.review._load = Some(cx.spawn(async move |this, cx| {
            let shown = patch.clone();
            let (files, new_change) = cx
                .background_spawn(async move {
                    let mut patch = patch;
                    let added = new_file.and_then(|file| git::files::new_file_patch(&root, &file));
                    let new_change = added
                        .as_deref()
                        .and_then(|added| git::parse_diff(added).into_iter().next());
                    if let Some(added) = &added {
                        patch.push_str(added);
                    }
                    let files = DiffFile::parse(&patch).unwrap_or_else(|err| {
                        tracing::warn!("the changes could not be drawn: {err:?}");
                        Vec::new()
                    });
                    (files, new_change)
                })
                .await;
            _ = this.update(cx, |this, cx| {
                this.review.shown = shown;
                this.review.new_change = new_change;
                let viewed: Vec<String> = this.review.viewed.keys().cloned().collect();
                let goto = this
                    .review
                    .goto
                    .take()
                    .or_else(|| this.review.selected.clone().map(Goto::File));
                this.review.state.update(cx, |state, cx| {
                    state.set_files(files, cx);
                    for path in &viewed {
                        state.set_file_collapsed(path, true, cx);
                    }
                    match goto {
                        Some(Goto::File(path)) => state.scroll_to_file(&path, cx),
                        Some(Goto::Line(line)) => state.scroll_to_line(line, cx),
                        None => {}
                    }
                });
                cx.notify();
            });
        }));
    }

    /// Shows `path`'s changes: the diff goes to it (and takes up a new
    /// file, which Git's own diff leaves out).
    pub(super) fn select_file(&mut self, path: String, cx: &mut Context<Self>) {
        let is_new = matches!(
            &self.state,
            State::Loaded(changes) if changes.untracked.contains(&path)
        );
        self.review.selected = Some(path.clone());
        if is_new {
            self.review.new_file = Some(path.clone());
        }
        if is_new || self.review.stale {
            self.review.goto = Some(Goto::File(path));
            self.load_review(cx);
        } else {
            self.review.state.update(cx, |state, cx| {
                state.set_file_collapsed(&path, false, cx);
                state.scroll_to_file(&path, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn is_viewed(&self, path: &str) -> bool {
        self.review.viewed.contains_key(path)
    }

    /// Ticks `path` as looked at (its changes fold away in the diff, as on
    /// a pull request's page), or takes the tick off.
    pub(super) fn set_viewed(&mut self, path: &str, viewed: bool, cx: &mut Context<Self>) {
        if viewed {
            let print = match &self.state {
                State::Loaded(changes) => changes
                    .files
                    .iter()
                    .find(|file| file.path == path)
                    .map_or(NEW_FILE, fingerprint),
                _ => NEW_FILE,
            };
            self.review.viewed.insert(path.to_string(), print);
        } else {
            self.review.viewed.remove(path);
        }
        self.review
            .state
            .update(cx, |state, cx| state.set_file_collapsed(path, viewed, cx));
        // The summary over the list counts them.
        self.list.remeasure_items(0..1);
        cx.notify();
    }

    /// How many of the changed files are ticked as looked at, and how many
    /// there are.
    pub fn viewed_count(&self) -> (usize, usize) {
        (self.review.viewed.len(), self.change_count())
    }

    /// Changed file `path`'s place in the list of changes.
    pub(super) fn file_ix(&self, path: &str) -> Option<usize> {
        match &self.state {
            State::Loaded(changes) => changes.files.iter().position(|file| file.path == path),
            _ => None,
        }
    }

    /// The text of line `line` on `side` of `path`, as the diff has it
    /// (with its mark).
    fn line_code(&self, path: &str, side: DiffSide, line: usize) -> Option<String> {
        let file = match &self.state {
            State::Loaded(changes) => changes.files.iter().find(|file| file.path == path),
            _ => None,
        }
        .or_else(|| {
            self.review
                .new_change
                .as_ref()
                .filter(|file| file.path == path)
        })?;
        file.hunks.iter().find_map(|hunk| {
            numbered(hunk)
                .find(|(at, number, _)| *at == side && *number == line)
                .map(|(_, _, text)| text.to_string())
        })
    }

    /// Asks for a note on the lines picked in the diff: on the last of
    /// them as the file is now, or on a removed line when only those were
    /// picked.
    fn note_on(&mut self, range: &DiffLineRange, window: &mut Window, cx: &mut Context<Self>) {
        let path = range.path().to_string();
        let (side, line) = if range.end_side() == DiffSide::Modified {
            (DiffSide::Modified, range.end())
        } else if range.side() == DiffSide::Modified {
            (DiffSide::Modified, range.start())
        } else {
            (DiffSide::Original, range.start().max(range.end()))
        };
        let code = self.line_code(&path, side, line).unwrap_or_default();
        ask_note(
            path,
            line as u32,
            side == DiffSide::Original,
            code,
            window,
            cx,
        );
    }

    fn remove_note(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.notes.len() {
            self.notes.remove(ix);
        }
        self.list.remeasure_items(0..1);
        cx.notify();
    }

    /// Keeps or discards a hunk from the diff, which then stays at it.
    fn decide_hunk(
        &mut self,
        file: usize,
        hunk: usize,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The diff comes back to where this hunk was.
        self.review.goto = self.file(file).and_then(|change| {
            let (side, line, _) = numbered(change.hunks.get(hunk)?).next()?;
            Some(Goto::Line(DiffLinePosition::new(
                change.path.clone(),
                side,
                line,
            )))
        });
        if keep {
            self.keep_hunk(file, hunk, window, cx);
        } else {
            self.discard_hunk(file, hunk, window, cx);
        }
    }

    /// What stands under the diff's lines: the notes, and under each hunk
    /// of a file with several what to do with that one (a file's only hunk
    /// is the file: its header has that).
    fn marks(&self) -> Vec<(DiffAnnotation, Mark)> {
        let mut marks = Vec::new();
        for (ix, note) in self.notes.iter().enumerate() {
            let side = if note.removed {
                DiffSide::Original
            } else {
                DiffSide::Modified
            };
            marks.push((
                DiffAnnotation::line(
                    ("review-note", ix),
                    DiffLinePosition::new(note.path.clone(), side, note.line as usize),
                ),
                Mark::Note(ix),
            ));
        }
        // A diff behind the working tree shows other hunks than Git has
        // now: nothing is offered on them.
        if self.review.stale {
            return marks;
        }
        if let State::Loaded(changes) = &self.state {
            for (file_ix, file) in changes.files.iter().enumerate() {
                if file.hunks.len() < 2 {
                    continue;
                }
                for (hunk_ix, hunk) in file.hunks.iter().enumerate() {
                    if let Some((side, line, _)) = numbered(hunk).last() {
                        marks.push((
                            DiffAnnotation::line(
                                ("review-hunk", file_ix * 10_000 + hunk_ix),
                                DiffLinePosition::new(file.path.clone(), side, line),
                            ),
                            Mark::Hunk {
                                file: file_ix,
                                hunk: hunk_ix,
                            },
                        ));
                    }
                }
            }
        }
        marks
    }

    /// The diff of every changed file, with what can be done about each.
    pub(super) fn render_review(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = cx.weak_entity();
        let state = self.review.state.clone();
        let split = state.read(cx).mode() == DiffMode::Split;
        let (annotations, marks): (Vec<DiffAnnotation>, HashMap<ElementId, Mark>) = {
            let marks = self.marks();
            (
                marks
                    .iter()
                    .map(|(annotation, _)| annotation.clone())
                    .collect(),
                marks
                    .into_iter()
                    .map(|(annotation, mark)| (annotation.id().clone(), mark))
                    .collect(),
            )
        };
        let notes: Rc<Vec<SharedString>> = Rc::new(
            self.notes
                .iter()
                .map(|note| SharedString::from(note.text.clone()))
                .collect(),
        );
        let viewed: Rc<HashSet<String>> = Rc::new(self.review.viewed.keys().cloned().collect());
        let new_file = self.review.new_file.clone();
        let stale = self.review.stale;
        let diff = Diff::new(&state)
            // Side by side, long lines wrap so both sides stay in view.
            .soft_wrap(split)
            .annotations(annotations)
            .render_annotation({
                let panel = panel.clone();
                move |annotation, _, cx| match marks.get(annotation.id()) {
                    Some(Mark::Note(ix)) => note_row(
                        *ix,
                        notes.get(*ix).cloned().unwrap_or_default(),
                        panel.clone(),
                        cx,
                    ),
                    Some(Mark::Hunk { file, hunk }) => hunk_bar(*file, *hunk, panel.clone(), cx),
                    None => div().into_any_element(),
                }
            })
            // Before and right after the path, not at the header's far
            // end: that is off the pane's edge when a long line makes the
            // diff wider than the pane.
            .render_header_prefix({
                let (panel, viewed) = (panel.clone(), viewed.clone());
                move |file, _, _| {
                    let path = file.path().to_string();
                    let seen = viewed.contains(&path);
                    file_tick(path, seen, panel.clone())
                }
            })
            .render_header_title_suffix({
                let panel = panel.clone();
                move |file, _, _| {
                    let path = file.path().to_string();
                    let is_new = new_file.as_deref() == Some(path.as_str());
                    // A diff behind the working tree is only read.
                    file_actions(path, !stale, is_new, panel.clone())
                }
            })
            .on_add_annotation({
                let panel = panel.clone();
                move |range, window, cx| {
                    _ = panel.update(cx, |panel, cx| panel.note_on(range, window, cx));
                }
            });
        let (seen, files) = self.viewed_count();
        // The changes as they are, or told in plain words: one pane, two
        // ways to read it (where the build has explanations).
        let explaining = self.explaining && self.explanation.is_some();
        let which = self.explanation.is_some().then(|| {
            TabBar::new("review-reading")
                .segmented()
                .xsmall()
                .selected_index(usize::from(explaining))
                .on_click(cx.listener(|this, ix: &usize, _, cx| {
                    this.explaining = *ix == 1;
                    cx.notify();
                }))
                .child(Tab::new().label("Changes"))
                .child(Tab::new().label("Explanation"))
        });
        if explaining {
            let bar = h_flex()
                .flex_none()
                .h_9()
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .children(which);
            return v_flex()
                .id("git-explanation")
                .size_full()
                .bg(cx.theme().background)
                .child(bar)
                .child(
                    div()
                        .id("git-explanation-text")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_3()
                        .children(self.explanation.clone()),
                )
                .test_support()
                .into_any_element();
        }
        let toolbar = h_flex()
            .flex_none()
            .h_9()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .children(which)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(format!("{seen} of {files} viewed")),
            )
            .child(
                Button::new("review-previous")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronUp)
                    .tooltip("Previous change")
                    .accessibility_label("Previous change")
                    .on_click({
                        let state = state.clone();
                        move |_, _, cx| state.update(cx, |state, cx| state.previous_change(cx))
                    }),
            )
            .child(
                Button::new("review-next")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .tooltip("Next change")
                    .accessibility_label("Next change")
                    .on_click({
                        let state = state.clone();
                        move |_, _, cx| state.update(cx, |state, cx| state.next_change(cx))
                    }),
            )
            .child(
                TabBar::new("review-mode")
                    .segmented()
                    .xsmall()
                    .selected_index(usize::from(split))
                    .on_click({
                        let state = state.clone();
                        move |ix: &usize, _, cx| {
                            let mode = if *ix == 1 {
                                DiffMode::Split
                            } else {
                                DiffMode::Unified
                            };
                            state.update(cx, |state, cx| state.set_mode(mode, cx));
                        }
                    })
                    .child(Tab::new().label("Unified"))
                    .child(Tab::new().label("Split")),
            );
        let behind = stale.then(|| {
            h_flex()
                .id("review-behind")
                .flex_none()
                .px_3()
                .py_1()
                .gap_2()
                .bg(cx.theme().muted)
                .text_xs()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child("The files changed since this was drawn."),
                )
                .child(
                    Button::new("review-catch-up")
                        .small()
                        .outline()
                        .label("Show them")
                        .on_click(cx.listener(|this, _, _, cx| this.load_review(cx))),
                )
        });
        v_flex()
            .id("git-review")
            .size_full()
            .bg(cx.theme().background)
            .child(toolbar)
            .children(behind)
            .child(div().flex_1().min_h_0().child(diff))
            .test_support()
            .into_any_element()
    }
}

/// A review note under its line.
fn note_row(
    ix: usize,
    text: SharedString,
    panel: WeakEntity<ChangesPanel>,
    cx: &App,
) -> AnyElement {
    h_flex()
        .items_start()
        .gap_2()
        .mx_2()
        .my_1()
        .px_2()
        .py_1p5()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .text_sm()
        .child(
            div().pt_0p5().child(
                Icon::new(Icons::MessageSquare)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            ),
        )
        .child(div().flex_1().min_w_0().child(text))
        .child(
            Button::new(("remove-note", ix))
                .ghost()
                .xsmall()
                .icon(IconName::Close)
                .tooltip("Remove the note")
                .accessibility_label("Remove the note")
                .on_click(move |_, _, cx| {
                    _ = panel.update(cx, |panel, cx| panel.remove_note(ix, cx));
                }),
        )
        .into_any_element()
}

/// Under a hunk of a file with several: keep that one (it is staged and
/// leaves the changes), or discard it.
fn hunk_bar(file: usize, hunk: usize, panel: WeakEntity<ChangesPanel>, cx: &App) -> AnyElement {
    let id = file * 10_000 + hunk;
    let decide = move |keep: bool| {
        let panel = panel.clone();
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            _ = panel.update(cx, |panel, cx| {
                panel.decide_hunk(file, hunk, keep, window, cx)
            });
        }
    };
    // At the left: the right end is off the pane's edge when a long line
    // makes the diff wider than the pane.
    h_flex()
        .gap_1()
        .px_2()
        .py_0p5()
        .bg(cx.theme().muted.opacity(0.5))
        .child(
            Button::new(("keep-hunk", id))
                .ghost()
                .xsmall()
                .icon(IconName::Check)
                .label("Keep these lines")
                .tooltip(ph_ui::tip(
                    "Stage this part: it is ready to commit and leaves the changes",
                ))
                .on_click(decide(true)),
        )
        .child(
            Button::new(("discard-hunk", id))
                .ghost()
                .xsmall()
                .icon(IconName::Undo2)
                .label("Discard these lines")
                .tooltip(ph_ui::tip(
                    "Take this part back out of the file (Undo discard brings it back)",
                ))
                .on_click(decide(false)),
        )
        .into_any_element()
}

/// Before a file's path in the diff: the tick for "looked at" (its changes
/// fold away, as on a pull request's page).
fn file_tick(path: String, viewed: bool, panel: WeakEntity<ChangesPanel>) -> AnyElement {
    div()
        .flex_none()
        // A press here is not one on the header (which folds the file).
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
            cx.stop_propagation()
        })
        .child(
            Checkbox::new(SharedString::from(format!("review-viewed-{path}")))
                .checked(viewed)
                .tooltip(if viewed {
                    "Viewed. Changes made after this take the tick off."
                } else {
                    "Mark as viewed"
                })
                .on_click(move |checked: &bool, _, cx| {
                    cx.stop_propagation();
                    _ = panel.update(cx, |panel, cx| panel.set_viewed(&path, *checked, cx));
                }),
        )
        .into_any_element()
}

/// After a file's path in the diff, while the diff is the working tree's
/// own: undo its changes, or keep them.
fn file_actions(
    path: String,
    current: bool,
    is_new: bool,
    panel: WeakEntity<ChangesPanel>,
) -> AnyElement {
    let undo = {
        let (panel, path) = (panel.clone(), path.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            _ = panel.update(cx, |panel, cx| {
                panel.discard_files(std::slice::from_ref(&path), window, cx)
            });
        }
    };
    let keep = {
        let (panel, path) = (panel.clone(), path.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            _ = panel.update(cx, |panel, cx| panel.keep_path(path.clone(), window, cx));
        }
    };
    h_flex()
        .flex_none()
        .gap_0p5()
        // A press here is not one on the header (which folds the file).
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
            cx.stop_propagation()
        })
        // A new file has nothing to go back to: it is deleted from its
        // row's menu, which asks first.
        .when(current && !is_new, |this| {
            this.child(
                Button::new(SharedString::from(format!("review-undo-{path}")))
                    .ghost()
                    .xsmall()
                    .label("Undo")
                    .tooltip(ph_ui::tip(
                        "Discard this file's changes (Undo discard brings them back)",
                    ))
                    .on_click(undo),
            )
        })
        .when(current, |this| {
            this.child(
                Button::new(SharedString::from(format!("review-keep-{path}")))
                    .ghost()
                    .xsmall()
                    .label("Keep")
                    .tooltip(ph_ui::tip(
                        "Stage this file: it is ready to commit and leaves the changes",
                    ))
                    .on_click(keep),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(header: &str, lines: &[&str]) -> git::Hunk {
        git::Hunk {
            header: header.to_string(),
            lines: lines.iter().map(|line| line.to_string()).collect(),
            raw: String::new(),
        }
    }

    #[test]
    fn lines_are_numbered_on_their_side() {
        let hunk = hunk(
            "@@ -10,3 +12,4 @@ fn main()",
            &[" a", "-b", "+c", "+d", " e", "\\ No newline at end of file"],
        );
        let lines: Vec<(DiffSide, usize)> = numbered(&hunk)
            .map(|(side, line, _)| (side, line))
            .collect();
        assert_eq!(
            lines,
            vec![
                (DiffSide::Modified, 12),
                (DiffSide::Original, 11),
                (DiffSide::Modified, 13),
                (DiffSide::Modified, 14),
                (DiffSide::Modified, 15),
            ]
        );
    }

    #[test]
    fn a_tick_lasts_while_the_changes_are_the_same() {
        let file = |lines: &[&str]| {
            git::parse_diff(&format!(
                "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,1 @@\n{}\n",
                lines.join("\n")
            ))
            .remove(0)
        };
        let before = fingerprint(&file(&["-old", "+new"]));
        assert_eq!(before, fingerprint(&file(&["-old", "+new"])));
        assert_ne!(before, fingerprint(&file(&["-old", "+newer"])));
        assert_ne!(before, NEW_FILE);
    }
}
