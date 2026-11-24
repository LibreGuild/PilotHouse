//! The Files tab: each changed file's diff, with what is said about its
//! lines where they are — threads from GitHub, the agent's open findings,
//! and the reviewer's own comments, which wait here until the review is
//! finished ([`crate::finish_view`]).
//!
//! Lines are picked by pressing on one and dragging over more (or with
//! Shift): a box opens under them to comment, to suggest a change, or to
//! ask an agent. A right click on a line (or on the lines picked) has the
//! agent explain them, or copies them; a file's menu has it explain the
//! whole file's changes. The diff is drawn in one column or side by side.
//! A file is ticked off once read, and folds; its tick is kept until the
//! file changes. After a review, the tab can show only what changed since.

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::Textarea,
    menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    text::TextView,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window, div, list,
};
use ph_github::models::{PullFile, ReviewComment};
use ph_github::pulls;
use ph_preview::diff::{DiffLine, LineKind};
use ph_ui::{Icons, MenuButton as _};

use super::{FOLD_LINES, Loaded, PullView, PullViewEvent, Tab};
use crate::findings_view::{severity_color, tag_owned};
use crate::finish_view::{Draft, save_drafts};

/// The lines a comment is being written on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Composing {
    pub path: String,
    /// In the new file, first and last.
    pub start: u64,
    pub end: u64,
    /// The line the pick began on: a drag reaches from it.
    pub anchor: u64,
    /// The comment not sent yet that is being changed, by its place in the
    /// list.
    pub editing: Option<usize>,
}

/// The files shown instead of the whole change: what moved since a review.
pub(super) struct Since {
    /// The commit the review read.
    pub base: String,
    pub files: Vec<PullFile>,
    pub changes: Vec<Option<ph_git::FileChange>>,
}

/// What `Since` is, while it is read.
pub(super) enum SinceState {
    Off,
    Loading,
    On(Rc<Since>),
    Failed(SharedString),
}

fn viewed_key(pull: &ph_store::reviews::PullRef) -> String {
    format!(
        "review.viewed.{}/{}/{}#{}",
        pull.host, pull.owner, pull.repo, pull.number
    )
}

/// A file as it is ticked off: its path at its content, so a file that
/// changes is to be read again.
fn viewed_mark(file: &PullFile) -> String {
    format!("{}@{}", file.filename, file.sha)
}

impl PullView {
    pub(super) fn read_viewed(&mut self) {
        let marks: Vec<String> = self.store.setting(&viewed_key(&self.pull_ref()));
        self.viewed = marks.into_iter().collect();
    }

    fn set_viewed(&mut self, file: &PullFile, viewed: bool, cx: &mut Context<Self>) {
        let mark = viewed_mark(file);
        if viewed {
            self.viewed.insert(mark);
        } else {
            self.viewed.remove(&mark);
        }
        // Only marks of files as they are now are kept.
        let current: HashSet<String> = self
            .data
            .iter()
            .flat_map(|data| data.files.iter().map(viewed_mark))
            .collect();
        let kept: Vec<&String> = self
            .viewed
            .iter()
            .filter(|mark| current.contains(*mark))
            .collect();
        let _ = self.store.set_setting(&viewed_key(&self.pull_ref()), &kept);
        self.files_list.remeasure();
        cx.notify();
    }

    /// Brings the next file (or the one before) to the top.
    pub(super) fn step_file(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.files_list.item_count();
        if count == 0 {
            return;
        }
        self.file_cursor = if forward {
            (self.file_cursor + 1).min(count - 1)
        } else {
            self.file_cursor.saturating_sub(1)
        };
        self.files_list.scroll_to(gpui_kit::ListOffset {
            item_ix: self.file_cursor,
            offset_in_item: gpui_kit::px(0.),
        });
        cx.notify();
    }

    /// Ticks the file the keyboard is on as read, and goes to the next.
    pub(super) fn view_current(&mut self, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let (files, _) = self.shown_files(&data);
        let Some(file) = files.get(self.file_cursor).cloned() else {
            return;
        };
        let viewed = self.viewed.contains(&viewed_mark(&file));
        self.set_viewed(&file, !viewed, cx);
        if !viewed {
            self.step_file(true, cx);
        }
    }

    /// The files the tab shows, with their parsed patches.
    fn shown_files<'a>(
        &'a self,
        data: &'a Loaded,
    ) -> (&'a [PullFile], &'a [Option<ph_git::FileChange>]) {
        match &self.since {
            SinceState::On(since) => (&since.files, &since.changes),
            _ => (&data.files, &data.changes),
        }
    }

    /// The commit the reviewer last read the change at: an agent's round
    /// here, else their own newest review on GitHub. `None` when that is
    /// the head (nothing changed since), or there is no review.
    fn last_reviewed(&self, data: &Loaded, cx: &Context<Self>) -> Option<String> {
        let me = &self.hosting.account.login;
        let on_github = data
            .reviews
            .iter()
            .rev()
            .filter(|review| review.user.login.eq_ignore_ascii_case(me))
            .find_map(|review| review.commit_id.clone());
        self.findings
            .read(cx)
            .reviewed_at()
            .or(on_github)
            .filter(|sha| !sha.is_empty() && *sha != data.pull.head.sha)
    }

    /// Shows what changed since commit `base`, or the whole change again.
    fn toggle_since(&mut self, base: Option<String>, cx: &mut Context<Self>) {
        let (Some(base), Some(data)) = (base, self.data.clone()) else {
            return;
        };
        if !matches!(self.since, SinceState::Off | SinceState::Failed(_)) {
            self.since = SinceState::Off;
            self.files_list.reset(data.files.len());
            cx.notify();
            return;
        }
        self.since = SinceState::Loading;
        cx.notify();
        let (client, repo) = (self.hosting.client.clone(), self.hosting.repo.clone());
        let head = data.pull.head.sha.clone();
        let from = base.clone();
        let load = cx.background_spawn(async move {
            let files = pulls::compare(&client, &repo, &from, &head)?;
            let changes = files.iter().map(super::file_change).collect();
            anyhow::Ok((files, changes))
        });
        self._since = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok((files, changes)) => {
                        this.files_list.reset(files.len());
                        this.since = SinceState::On(Rc::new(Since {
                            base,
                            files,
                            changes,
                        }));
                    }
                    Err(err) => this.since = SinceState::Failed(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    /// A line was pressed: a comment starts on it, or (with Shift) the one
    /// being written covers up to it.
    fn pick_line(
        &mut self,
        path: &str,
        line: u64,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &mut self.composing {
            Some(composing) if extend && composing.path == path => {
                composing.start = composing.anchor.min(line);
                composing.end = composing.anchor.max(line);
            }
            _ => {
                self.pick_range(path, line, line, window, cx);
            }
        }
        // The box waits for the button to be let go: opening under the
        // first line at once, it would push the lines below it from under
        // the pointer on its way down.
        self.picking = true;
        self.files_list.remeasure();
        cx.notify();
    }

    /// The button was let go: the lines picked get their box.
    pub(super) fn end_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.picking) {
            return;
        }
        if self.composing.is_some() {
            self.comment_box
                .update(cx, |input, cx| input.focus(window, cx));
        }
        self.files_list.remeasure();
        cx.notify();
    }

    /// The pointer went over `line` with the button held: the pick reaches
    /// from where it began to there.
    fn drag_to(&mut self, path: &str, line: u64, cx: &mut Context<Self>) {
        let Some(composing) = self
            .composing
            .as_mut()
            .filter(|composing| composing.path == path && composing.editing.is_none())
        else {
            return;
        };
        let (start, end) = (composing.anchor.min(line), composing.anchor.max(line));
        if (composing.start, composing.end) != (start, end) {
            composing.start = start;
            composing.end = end;
            self.files_list.remeasure();
            cx.notify();
        }
    }

    /// Opens the box on lines `start..=end` of `path`, empty.
    fn pick_range(
        &mut self,
        path: &str,
        start: u64,
        end: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composing = Some(Composing {
            path: path.to_string(),
            start,
            end,
            anchor: start,
            editing: None,
        });
        self.comment_box.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.files_list.remeasure();
        cx.notify();
    }

    /// The lines `start..=end` of the new version of `path`, as the diff
    /// shows them.
    pub(super) fn lines_text(&self, path: &str, start: u64, end: u64) -> Vec<String> {
        let Some(data) = self.data.clone() else {
            return Vec::new();
        };
        let (files, changes) = self.shown_files(&data);
        let Some(change) = files
            .iter()
            .position(|file| file.filename == path)
            .and_then(|ix| changes.get(ix)?.as_ref())
        else {
            return Vec::new();
        };
        change
            .hunks
            .iter()
            .flat_map(ph_preview::diff::lines)
            .filter(|line| {
                line.new
                    .is_some_and(|n| (start..=end).contains(&u64::from(n)))
            })
            .map(|line| line.text)
            .collect()
    }

    /// Asks the chat's agent to explain lines `start..=end` of `path`.
    fn explain_lines(&mut self, path: &str, start: u64, end: u64, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else {
            return;
        };
        cx.emit(PullViewEvent::Agent(crate::AgentRequest::AskLines {
            pull: Box::new(data.pull.clone()),
            path: path.to_string(),
            start,
            end,
            question: "Explain these lines to someone reviewing this change: what they do, why \
                       they are written this way, how they fit the code around them, and \
                       anything in them worth a second look."
                .into(),
        }));
    }

    /// Asks the chat's agent to explain what changes in file `ix`.
    fn explain_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let (files, changes) = self.shown_files(&data);
        let (Some(file), Some(Some(change))) = (files.get(ix), changes.get(ix)) else {
            return;
        };
        // The lines the diff shows of the new file, first to last.
        let shown: Vec<u64> = change
            .hunks
            .iter()
            .flat_map(ph_preview::diff::lines)
            .filter_map(|line| line.new.map(u64::from))
            .collect();
        let (Some(start), Some(end)) = (shown.first().copied(), shown.last().copied()) else {
            return;
        };
        cx.emit(PullViewEvent::Agent(crate::AgentRequest::AskLines {
            pull: Box::new(data.pull.clone()),
            path: file.filename.clone(),
            start,
            end,
            question: "Explain what this pull request changes in this file, for someone \
                       reviewing it: what the file is for, what is different now and why, and \
                       what in it deserves a careful look."
                .into(),
        }));
    }

    fn copy_lines(&mut self, path: &str, start: u64, end: u64, cx: &mut Context<Self>) {
        let lines = self.lines_text(path, start, end);
        if !lines.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
        }
    }

    /// One column, or the old and the new side by side; kept.
    pub(super) fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        self.split = split;
        let _ = self.store.set_setting(SPLIT_KEY, &Some(split));
        self.files_list.remeasure();
        cx.notify();
    }

    fn stop_composing(&mut self, cx: &mut Context<Self>) {
        self.composing = None;
        self.files_list.remeasure();
        cx.notify();
    }

    /// Keeps the comment being written, to be sent when the review is
    /// finished.
    pub(super) fn add_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composing) = self.composing.clone() else {
            return;
        };
        let body = self.comment_box.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        let draft = Draft {
            path: composing.path,
            line: composing.end,
            start: (composing.start < composing.end).then_some(composing.start),
            body,
        };
        match composing.editing.filter(|ix| *ix < self.drafts.len()) {
            Some(ix) => self.drafts[ix] = draft,
            None => self.drafts.push(draft),
        }
        save_drafts(&self.store, &self.pull_ref(), &self.drafts);
        self.comment_box
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.stop_composing(cx);
    }

    fn edit_draft(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.drafts.get(ix).cloned() else {
            return;
        };
        let start = draft.start.unwrap_or(draft.line);
        self.composing = Some(Composing {
            path: draft.path,
            start,
            end: draft.line,
            anchor: start,
            editing: Some(ix),
        });
        self.comment_box.update(cx, |input, cx| {
            input.set_value(draft.body, window, cx);
            input.focus(window, cx);
        });
        self.files_list.remeasure();
        cx.notify();
    }

    fn drop_draft(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.drafts.len() {
            self.drafts.remove(ix);
            save_drafts(&self.store, &self.pull_ref(), &self.drafts);
            self.files_list.remeasure();
            cx.notify();
        }
    }

    /// Asks an agent what was typed, about the lines being commented.
    fn ask_composed(&mut self, cx: &mut Context<Self>) {
        let (Some(composing), Some(data)) = (self.composing.clone(), self.data.clone()) else {
            return;
        };
        let question = self.comment_box.read(cx).value().trim().to_string();
        if question.is_empty() {
            return;
        }
        cx.emit(PullViewEvent::Agent(crate::AgentRequest::AskLines {
            pull: Box::new(data.pull.clone()),
            path: composing.path,
            start: composing.start,
            end: composing.end,
            question,
        }));
        self.stop_composing(cx);
    }

    /// Puts a suggestion block with the lines being commented, as they
    /// are, into the box: edited there, it becomes a change the author
    /// takes with one click.
    fn suggest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composing) = self.composing.clone() else {
            return;
        };
        let lines = self.lines_text(&composing.path, composing.start, composing.end);
        if lines.is_empty() {
            return;
        }
        let block = format!("```suggestion\n{}\n```\n", lines.join("\n"));
        self.comment_box.update(cx, |input, cx| {
            let text = input.value().to_string();
            let text = if text.trim().is_empty() {
                block
            } else {
                format!("{}\n\n{block}", text.trim_end())
            };
            input.set_value(text, window, cx);
            input.focus(window, cx);
        });
        self.files_list.remeasure();
        cx.notify();
    }

    /// The strip over the files: how many are read, what is shown, and a
    /// list to jump by.
    fn render_files_bar(&self, data: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        let (files, _) = self.shown_files(data);
        let viewed = files
            .iter()
            .filter(|file| self.viewed.contains(&viewed_mark(file)))
            .count();
        let base = self.last_reviewed(data, cx);
        let since_on = matches!(self.since, SinceState::On(_) | SinceState::Loading);
        let view = cx.weak_entity();
        let jump: Vec<(usize, SharedString, bool)> = files
            .iter()
            .enumerate()
            .map(|(ix, file)| {
                (
                    ix,
                    SharedString::from(format!(
                        "{}  +{} −{}",
                        file.filename, file.additions, file.deletions
                    )),
                    self.viewed.contains(&viewed_mark(file)),
                )
            })
            .collect();
        let waiting = self.drafts.len();
        h_flex()
            .flex_none()
            .px_3()
            .py_1p5()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                Button::new("files-jump")
                    .xsmall()
                    .ghost()
                    .icon(Icons::Files)
                    .label(format!("{viewed} of {} read", files.len()))
                    .tooltip("Go to a file")
                    .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                        let mut menu = menu.max_h(gpui_kit::px(420.));
                        for (ix, name, viewed) in jump.iter().cloned() {
                            let view = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(name)
                                    .when(viewed, |item| item.icon(IconName::Check))
                                    .on_click(move |_, _, cx| {
                                        _ = view.update(cx, |this, cx| {
                                            this.file_cursor = ix;
                                            this.files_list.scroll_to_reveal_item(ix);
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .when(waiting > 0, |this| {
                this.child(div().child(match waiting {
                    1 => "1 comment of yours waits to be sent".to_string(),
                    n => format!("{n} comments of yours wait to be sent"),
                }))
            })
            .child(div().flex_1())
            .when_some(
                match &self.since {
                    SinceState::Failed(why) => Some(why.clone()),
                    _ => None,
                },
                |this, why| this.child(div().text_color(cx.theme().danger).child(why)),
            )
            .when(matches!(self.since, SinceState::Loading), |this| {
                this.child(ph_ui::Loader::new().xsmall())
            })
            .child(
                Button::new("files-split")
                    .xsmall()
                    .ghost()
                    .icon(Icons::Columns2)
                    .selected(self.split)
                    .accessibility_label("Side by side")
                    .tooltip(if self.split {
                        "Side by side. Click for one column."
                    } else {
                        "One column. Click for the old and the new side by side."
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let split = !this.split;
                        this.set_split(split, cx)
                    })),
            )
            // Only once a review read an older commit: there is a "since".
            .when(base.is_some() || since_on, |this| {
                this.child(
                    Button::new("files-since")
                        .xsmall()
                        .ghost()
                        .selected(since_on)
                        .label("Since your review")
                        .tooltip(ph_ui::tip(
                            "Only what the author changed after the commit your last review read",
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let base = match &this.since {
                                SinceState::On(since) => Some(since.base.clone()),
                                _ => base.clone(),
                            };
                            this.toggle_since(base, cx)
                        })),
                )
            })
            .into_any_element()
    }

    /// The Files tab: the strip, then each file, drawn only while in sight
    /// (a pull request can change thousands of lines).
    pub(super) fn render_files(&self, data: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .child(self.render_files_bar(data, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div().absolute().inset_0().child(
                            list(
                                self.files_list.clone(),
                                cx.processor(|this, ix: usize, _, cx| this.render_file(ix, cx)),
                            )
                            .size_full(),
                        ),
                    )
                    .vertical_scrollbar(&self.files_list),
            )
            .into_any_element()
    }

    /// A thread from GitHub at its line: the comments, and a reply.
    fn render_thread(
        &self,
        root: &ReviewComment,
        replies: &[&ReviewComment],
        outdated: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let thread = root.id;
        let said = |comment: &ReviewComment| {
            v_flex()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(comment.user.login.clone()),
                        )
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(ph_ui::time::ago_of(&comment.created_at)),
                        ),
                )
                .children(self.bodies.get(&comment.id).map(|view| {
                    TextView::new(view)
                        .text_sm()
                        .image_source(ph_ui::images_safe::web_only)
                }))
        };
        v_flex()
            .gap_2()
            .mx_2()
            .my_1()
            .p_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .when(outdated, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(match root.original_line {
                            Some(line) => format!("On line {line} of an earlier version"),
                            None => "On an earlier version".to_string(),
                        }),
                )
            })
            .child(said(root))
            .children(replies.iter().map(|reply| said(reply)))
            .map(|this| {
                if self.replying == Some(thread) {
                    this.child(
                        v_flex()
                            .gap_1()
                            .child(Textarea::new(&self.reply_box).small())
                            .child(
                                h_flex()
                                    .gap_1()
                                    .justify_end()
                                    .child(
                                        Button::new(("cancel-reply", thread))
                                            .xsmall()
                                            .ghost()
                                            .label("Cancel")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.replying = None;
                                                this.files_list.remeasure();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new(("send-reply", thread))
                                            .xsmall()
                                            .primary()
                                            .label("Reply")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.reply(window, cx)
                                            })),
                                    ),
                            ),
                    )
                } else {
                    this.child(
                        h_flex().child(
                            Button::new(("start-reply", thread))
                                .xsmall()
                                .ghost()
                                .label("Reply")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.replying = Some(thread);
                                    this.reply_box
                                        .update(cx, |input, cx| input.focus(window, cx));
                                    this.files_list.remeasure();
                                    cx.notify();
                                })),
                        ),
                    )
                }
            })
            .into_any_element()
    }

    /// The box a comment is written in, under the last of its lines.
    fn render_composer(&self, composing: &Composing, cx: &mut Context<Self>) -> AnyElement {
        let lines = if composing.start == composing.end {
            format!("Line {}", composing.start)
        } else {
            format!("Lines {}-{}", composing.start, composing.end)
        };
        let empty = self.comment_box.read(cx).value().trim().is_empty();
        v_flex()
            .id("line-composer")
            .gap_1p5()
            .mx_2()
            .my_1()
            .p_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().primary.opacity(0.5))
            .bg(cx.theme().background)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{lines} · drag or Shift-click to cover more · ⌘↵ adds it"
                    )),
            )
            .child(Textarea::new(&self.comment_box).small())
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(
                        Button::new("line-suggest")
                            .xsmall()
                            .ghost()
                            .label("Suggest a change")
                            .tooltip(ph_ui::tip("Puts these lines in the box as a suggestion: edit them, and the author can take the change with one click"))
                            .on_click(cx.listener(|this, _, window, cx| this.suggest(window, cx))),
                    )
                    .child(
                        Button::new("line-ask")
                            .xsmall()
                            .ghost()
                            .icon(Icons::Sparkles)
                            .label("Ask an agent")
                            .tooltip("Sends what you typed to the chat as a question about these lines")
                            .when(empty, |this| this.disabled(true))
                            .on_click(cx.listener(|this, _, _, cx| this.ask_composed(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("line-cancel")
                            .xsmall()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.stop_composing(cx))),
                    )
                    .child(
                        Button::new("line-add")
                            .xsmall()
                            .primary()
                            .label(if composing.editing.is_some() {
                                "Save"
                            } else {
                                "Add comment"
                            })
                            .tooltip("Kept here until you finish the review")
                            .when(empty, |this| this.disabled(true))
                            .on_click(cx.listener(|this, _, window, cx| this.add_draft(window, cx))),
                    ),
            )
            .test_support()
            .into_any_element()
    }

    fn render_file(&self, list_ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.data.clone() else {
            return div().into_any_element();
        };
        let (files, changes) = self.shown_files(&data);
        let ix = list_ix;
        let last = ix + 1 == files.len();
        let (Some(file), Some(change)) = (files.get(ix), changes.get(ix)) else {
            return div().into_any_element();
        };
        let path = file.filename.clone();
        let viewed = self.viewed.contains(&viewed_mark(file));
        // Line comment threads of this file, each root with its replies.
        let threads: Vec<(&ReviewComment, Vec<&ReviewComment>)> = data
            .line_comments
            .iter()
            .filter(|c| c.in_reply_to_id.is_none() && c.path == path)
            .map(|root| {
                let replies = data
                    .line_comments
                    .iter()
                    .filter(|c| c.in_reply_to_id == Some(root.id))
                    .collect();
                (root, replies)
            })
            .collect();
        let findings = self.findings.read(cx).on_file(&path);
        let drafts: Vec<(usize, &Draft)> = self
            .drafts
            .iter()
            .enumerate()
            .filter(|(_, draft)| draft.path == path)
            .collect();
        let composing = self
            .composing
            .as_ref()
            .filter(|composing| composing.path == path);
        let count: usize = change.as_ref().map_or(0, |change| {
            change.hunks.iter().map(|hunk| hunk.lines.len()).sum()
        });
        let folded = count > FOLD_LINES && !self.unfolded.contains(&ix);
        let (icon, tint) = ph_ui::file_icon(&path);
        let said = threads.len() + findings.len() + drafts.len();
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_1p5()
            .bg(cx.theme().secondary)
            .child(
                Checkbox::new(SharedString::from(format!("file-viewed-{path}")))
                    .checked(viewed)
                    .tooltip("Read: fold it, until it changes")
                    .on_click(cx.listener({
                        let file = file.clone();
                        move |this, checked: &bool, _, cx| this.set_viewed(&file, *checked, cx)
                    })),
            )
            .child(Icon::new(icon).text_color(tint))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .when(viewed, |this| this.text_color(cx.theme().muted_foreground))
                    .child(match &file.previous_filename {
                        Some(before) if *before != path => format!("{before} → {path}"),
                        _ => path.clone(),
                    }),
            )
            .when(viewed && said > 0, |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(match said {
                            1 => "1 comment".to_string(),
                            n => format!("{n} comments"),
                        }),
                )
            })
            .child(ph_preview::diff::change_counts(
                file.additions as usize,
                file.deletions as usize,
                cx,
            ))
            .child({
                let view = cx.weak_entity();
                let name = path.clone();
                Button::new(SharedString::from(format!("file-more-{path}")))
                    .xsmall()
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .accessibility_label("More for this file")
                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                        let (explain, name) = (view.clone(), name.clone());
                        menu.item(
                            PopupMenuItem::new("Explain what changes in this file")
                                .icon(Icons::Sparkles)
                                .on_click(move |_, _, cx| {
                                    _ = explain.update(cx, |this, cx| this.explain_file(ix, cx));
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Copy its path")
                                .icon(IconName::Copy)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(name.clone()));
                                }),
                        )
                    })
            });
        let card = v_flex()
            .flex_none()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .text_xs()
            .child(header);
        let card = match change {
            _ if viewed => card,
            None => card.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} (no diff shown)", file.status)),
            ),
            Some(_) if folded => card.child(
                h_flex().px_2().py_1().child(
                    Button::new(("show-diff", ix))
                        .xsmall()
                        .ghost()
                        .label(format!("Show diff ({count} lines)"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.unfolded.insert(ix);
                            this.files_list.remeasure_items(list_ix..list_ix + 1);
                            cx.notify();
                        })),
                ),
            ),
            Some(change) => {
                let said = Said {
                    path: &path,
                    threads: &threads,
                    findings: &findings,
                    drafts: &drafts,
                    composing,
                };
                let mut placed: HashSet<u64> = HashSet::new();
                let mut card = card;
                for (hunk_ix, hunk) in change.hunks.iter().enumerate() {
                    card = card.child(
                        ph_preview::diff::hunk_header(hunk, cx)
                            .border_t_1()
                            .border_color(cx.theme().border),
                    );
                    card = if self.split {
                        self.split_hunk(card, &said, (ix, hunk_ix), hunk, &mut placed, cx)
                    } else {
                        self.unified_hunk(card, &said, (ix, hunk_ix), hunk, &mut placed, cx)
                    };
                }
                // Threads whose line the diff no longer shows.
                let outdated: Vec<AnyElement> = threads
                    .iter()
                    .filter(|(root, _)| !placed.contains(&root.id))
                    .map(|(root, replies)| self.render_thread(root, replies, true, cx))
                    .collect();
                card.when(!outdated.is_empty(), |this| {
                    this.child(
                        v_flex()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .py_1()
                            .children(outdated),
                    )
                })
            }
        };
        // A list item sizes to its content: fill the column.
        v_flex()
            .w_full()
            .px_3()
            .pt_2()
            .when(last, |this| this.pb_3())
            .child(div().w_full().child(card))
            .into_any_element()
    }

    /// What is said about `line`, to draw under it: threads from GitHub on
    /// it (each placed once), and on a line of the new file the agent's
    /// findings, the reviewer's comments not sent yet, and the box a
    /// comment is being written in.
    fn said_under(
        &self,
        said: &Said,
        line: &DiffLine,
        placed: &mut HashSet<u64>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut under: Vec<AnyElement> = Vec::new();
        for (root, replies) in said.threads {
            let here = match root.side.as_deref() {
                Some("LEFT") => {
                    line.kind == LineKind::Removed
                        && root.line.is_some()
                        && line.old.map(u64::from) == root.line
                }
                _ => root.line.is_some() && line.new.map(u64::from) == root.line,
            };
            if here && placed.insert(root.id) {
                under.push(self.render_thread(root, replies, false, cx));
            }
        }
        let Some(new) = line.new.map(u64::from) else {
            return under;
        };
        for finding in said
            .findings
            .iter()
            .filter(|f| f.end_line.or(f.line) == Some(new))
        {
            under.push(self.render_finding_note(finding, cx));
        }
        for (draft_ix, draft) in said.drafts.iter().filter(|(_, d)| d.line == new) {
            // The one being changed shows in the box.
            if said.composing.and_then(|c| c.editing) != Some(*draft_ix) {
                under.push(self.render_draft(*draft_ix, draft, cx));
            }
        }
        if let Some(composing) = said.composing.filter(|c| c.end == new && !self.picking) {
            under.push(self.render_composer(composing, cx));
        }
        under
    }

    /// A hunk in one column. Rows up to a line something is said about
    /// scroll sideways together; what is said sits under them at the
    /// card's width.
    fn unified_hunk(
        &self,
        mut card: gpui_kit::Div,
        said: &Said,
        (file_ix, hunk_ix): (usize, usize),
        hunk: &ph_git::Hunk,
        placed: &mut HashSet<u64>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let lines = ph_preview::diff::lines(hunk);
        let digits = ph_preview::diff::number_digits(&lines);
        let hidden = hunk.lines.len() - lines.len();
        let path = said.path;
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut part = 0;
        for (line_ix, line) in lines.iter().enumerate() {
            let row = ph_preview::diff::line_row(line, digits, cx);
            rows.push(match line.new {
                Some(new) => self.pickable(
                    row,
                    v_flex().flex_none(),
                    format!("line-{file_ix}-{hunk_ix}-{line_ix}"),
                    path,
                    u64::from(new),
                    said.composing,
                    cx,
                ),
                None => row.into_any_element(),
            });
            let under = self.said_under(said, line, placed, cx);
            if !under.is_empty() {
                card = card.child(ph_preview::diff::scroller(
                    SharedString::from(format!("diff-{path}-{hunk_ix}-{part}")),
                    std::mem::take(&mut rows),
                ));
                part += 1;
                card = card.children(under);
            }
        }
        if hidden > 0 {
            rows.push(ph_preview::diff::more_lines(hidden, cx));
        }
        if !rows.is_empty() {
            card = card.child(ph_preview::diff::scroller(
                SharedString::from(format!("diff-{path}-{hunk_ix}-{part}")),
                rows,
            ));
        }
        card
    }

    /// A hunk with the old file on the left and the new on the right: a
    /// removed line beside the line that replaces it. Long lines wrap in
    /// their half, so nothing scrolls sideways.
    fn split_hunk(
        &self,
        mut card: gpui_kit::Div,
        said: &Said,
        (file_ix, hunk_ix): (usize, usize),
        hunk: &ph_git::Hunk,
        placed: &mut HashSet<u64>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let lines = ph_preview::diff::lines(hunk);
        let digits = ph_preview::diff::number_digits(&lines);
        let hidden = hunk.lines.len() - lines.len();
        for (row_ix, (left, right)) in side_by_side(&lines).into_iter().enumerate() {
            let old = left.map(|at| &lines[at]);
            let new = right.map(|at| &lines[at]);
            let left_cell = split_cell(old, true, digits, cx).w_1_2().into_any_element();
            let right_cell = split_cell(new, false, digits, cx);
            let right_cell = match new.and_then(|line| line.new) {
                Some(number) => self.pickable(
                    right_cell,
                    v_flex().w_1_2().min_w_0(),
                    // Named as the row is in one column, by its line there.
                    format!("line-{file_ix}-{hunk_ix}-{}", right.unwrap_or(row_ix)),
                    said.path,
                    u64::from(number),
                    said.composing,
                    cx,
                ),
                None => right_cell.w_1_2().into_any_element(),
            };
            card = card.child(
                h_flex()
                    .w_full()
                    .items_stretch()
                    .child(left_cell)
                    .child(div().flex_none().w(gpui_kit::px(1.)).bg(cx.theme().border))
                    .child(right_cell),
            );
            // A context line is one line on both sides: said once.
            let mut under = Vec::new();
            if let Some(old) = old.filter(|_| left != right) {
                under.extend(self.said_under(said, old, placed, cx));
            }
            if let Some(new) = new {
                under.extend(self.said_under(said, new, placed, cx));
            }
            card = card.children(under);
        }
        if hidden > 0 {
            card = card.child(ph_preview::diff::more_lines(hidden, cx));
        }
        card
    }

    /// A line of the new file made pickable: a press starts a comment on
    /// it, a drag (or Shift) covers more lines, a right click has what can
    /// be done with it or with the lines picked. `row` is the line as it
    /// is drawn; `frame` is what holds it, sized by the caller. (The frame
    /// takes the press and carries the menu, so the menu is drawn in the
    /// interface's font, not the code's; the pick is a wash over the row,
    /// since an added line's own tint would hide one behind it.)
    #[allow(clippy::too_many_arguments)]
    fn pickable(
        &self,
        row: gpui_kit::Div,
        frame: gpui_kit::Div,
        id: String,
        path: &str,
        line: u64,
        composing: Option<&Composing>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let picked = composing.is_some_and(|c| (c.start..=c.end).contains(&line));
        // The lines a right click is about: those picked when it is on one
        // of them, else the line itself.
        let (start, end) = composing
            .filter(|_| picked)
            .map_or((line, line), |c| (c.start, c.end));
        let view = cx.weak_entity();
        let (pressed, dragged, menu_path) = (path.to_string(), path.to_string(), path.to_string());
        let wash = cx.theme().primary;
        frame
            .id(SharedString::from(id))
            .relative()
            .group("diff-line")
            .cursor_pointer()
            .child(row)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .when(picked, |this| this.bg(wash.opacity(0.16)))
                    .group_hover("diff-line", |style| style.bg(wash.opacity(0.10))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    this.pick_line(&pressed, line, event.modifiers.shift, window, cx)
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_to(&dragged, line, cx)
                    }
                }),
            )
            .test_support()
            .context_menu(move |menu, _, _| {
                lines_menu(menu, view.clone(), menu_path.clone(), start, end)
            })
            .into_any_element()
    }

    /// An agent's open finding at its line: what it says, and the way to
    /// read it in full.
    fn render_finding_note(
        &self,
        finding: &ph_store::reviews::Finding,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = finding.label.clone();
        let color = severity_color(&finding.severity, cx);
        h_flex()
            .id(SharedString::from(format!("line-finding-{label}")))
            .gap_2()
            .items_start()
            .mx_2()
            .my_1()
            .p_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(color.opacity(0.4))
            .bg(cx.theme().background)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tab = Tab::Findings;
                this.findings
                    .update(cx, |findings, cx| findings.show(&label, cx));
                cx.notify();
            }))
            .child(tag_owned(
                format!("{} {}", finding.label, finding.severity),
                color,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(finding.title.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().link)
                    .child("Read"),
            )
            .test_support()
            .into_any_element()
    }

    /// A comment of the reviewer's own, not sent yet.
    fn render_draft(&self, ix: usize, draft: &Draft, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1()
            .mx_2()
            .my_1()
            .p_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().primary.opacity(0.4))
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Your comment on line{} {} · not sent yet",
                        if draft.start.is_some_and(|start| start < draft.line) {
                            "s"
                        } else {
                            ""
                        },
                        draft.lines()
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new(("draft-edit", ix))
                            .xsmall()
                            .ghost()
                            .label("Edit")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_draft(ix, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("draft-drop", ix))
                            .xsmall()
                            .ghost()
                            .label("Delete")
                            .on_click(cx.listener(move |this, _, _, cx| this.drop_draft(ix, cx))),
                    ),
            )
            .child(div().text_sm().child(draft.body.clone()))
            .into_any_element()
    }
}

/// What is said about a file's lines, for drawing it under them.
struct Said<'a> {
    path: &'a str,
    /// Threads from GitHub, each root with its replies.
    threads: &'a [(&'a ReviewComment, Vec<&'a ReviewComment>)],
    findings: &'a [ph_store::reviews::Finding],
    /// The reviewer's comments not sent yet, with their place in the list.
    drafts: &'a [(usize, &'a Draft)],
    composing: Option<&'a Composing>,
}

/// Settings key: the diff is drawn side by side.
pub(super) const SPLIT_KEY: &str = "review.files.split";

/// The right-click menu of lines `start..=end` of `path`.
fn lines_menu(
    menu: PopupMenu,
    view: WeakEntity<PullView>,
    path: String,
    start: u64,
    end: u64,
) -> PopupMenu {
    let what = if start == end {
        format!("line {start}")
    } else {
        format!("lines {start}-{end}")
    };
    let act = |run: fn(&mut PullView, &str, u64, u64, &mut Window, &mut Context<PullView>)| {
        let (view, path) = (view.clone(), path.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            _ = view.update(cx, |this, cx| run(this, &path, start, end, window, cx));
        }
    };
    menu.item(
        PopupMenuItem::new(format!("Explain {what}"))
            .icon(Icons::Sparkles)
            .on_click(act(|this, path, start, end, _, cx| {
                this.explain_lines(path, start, end, cx)
            })),
    )
    .item(
        PopupMenuItem::new(format!("Ask or comment on {what}…"))
            .icon(Icons::MessageSquare)
            .on_click(act(|this, path, start, end, window, cx| {
                this.pick_range(path, start, end, window, cx)
            })),
    )
    .separator()
    .item(
        PopupMenuItem::new(format!("Copy {what}"))
            .icon(IconName::Copy)
            .on_click(act(|this, path, start, end, _, cx| {
                this.copy_lines(path, start, end, cx)
            })),
    )
}

/// The rows of a hunk drawn side by side: for each, the line on the left
/// (the old file) and the one on the right (the new), by their place in
/// `lines`. A line that did not change is on both sides; removed lines
/// stand beside the added ones that follow them, one to one, and what is
/// left over stands alone.
fn side_by_side(lines: &[DiffLine]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut rows = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        match lines[at].kind {
            LineKind::Removed => {
                let removed_end = (at..lines.len())
                    .find(|ix| lines[*ix].kind != LineKind::Removed)
                    .unwrap_or(lines.len());
                let added_end = (removed_end..lines.len())
                    .find(|ix| lines[*ix].kind != LineKind::Added)
                    .unwrap_or(lines.len());
                let (removed, added) = (removed_end - at, added_end - removed_end);
                for ix in 0..removed.max(added) {
                    rows.push((
                        (ix < removed).then_some(at + ix),
                        (ix < added).then_some(removed_end + ix),
                    ));
                }
                at = added_end;
            }
            LineKind::Added => {
                rows.push((None, Some(at)));
                at += 1;
            }
            // Git's remark is about the line before it, on that line's side.
            LineKind::Context | LineKind::Remark => {
                rows.push((Some(at), Some(at)));
                at += 1;
            }
        }
    }
    rows
}

/// One half of a side-by-side row: the line's number on that side, its
/// sign and its text (wrapped in the half's width), or nothing where the
/// other side has a line this one lacks.
fn split_cell(line: Option<&DiffLine>, old_side: bool, digits: usize, cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    let cell = h_flex()
        .min_w_0()
        .items_start()
        .font_family(theme.mono_font_family.clone());
    let Some(line) = line else {
        return cell.bg(theme.muted.opacity(0.35));
    };
    let (tint, sign, color) = match line.kind {
        LineKind::Added => (Some(theme.success), "+", theme.foreground),
        LineKind::Removed => (Some(theme.danger), "−", theme.foreground),
        LineKind::Context => (None, "", theme.foreground.opacity(0.78)),
        LineKind::Remark => (None, "", theme.muted_foreground),
    };
    let number = if old_side { line.old } else { line.new };
    let text: SharedString = if line.text.is_empty() {
        // An empty line keeps its height.
        " ".into()
    } else {
        line.text.clone().into()
    };
    let marks: Vec<_> = tint
        .map(|tint| {
            let style = gpui_kit::HighlightStyle {
                background_color: Some(tint.opacity(0.32)),
                ..Default::default()
            };
            line.changed
                .iter()
                .filter(|range| range.end <= text.len())
                .map(|range| (range.clone(), style))
                .collect()
        })
        .unwrap_or_default();
    cell.text_color(color)
        .when_some(tint, |this, tint| this.bg(tint.opacity(0.12)))
        .when(line.kind == LineKind::Remark, |this| this.italic())
        .child(
            div()
                .flex_none()
                // A digit of the code font is about 0.6 of its size wide.
                .w(ph_ui::rems_from_px(7.4 * digits as f32 + 10.))
                .pr_1p5()
                .text_right()
                .text_color(theme.muted_foreground)
                .children(number.map(|number| number.to_string())),
        )
        .child(
            div()
                .flex_none()
                .w_4()
                .text_center()
                .when_some(tint, |this, tint| this.text_color(tint))
                .child(sign),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pr_2()
                .child(gpui_kit::StyledText::new(text).with_highlights(marks)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind) -> DiffLine {
        DiffLine {
            kind,
            text: String::new(),
            old: None,
            new: None,
            changed: Vec::new(),
        }
    }

    #[test]
    fn removed_lines_stand_beside_what_replaces_them() {
        use LineKind::{Added, Context, Removed};
        let lines: Vec<DiffLine> = [Context, Removed, Removed, Added, Context, Added, Removed]
            .into_iter()
            .map(line)
            .collect();
        assert_eq!(
            side_by_side(&lines),
            [
                (Some(0), Some(0)),
                // Two removed, one added: the second removed stands alone.
                (Some(1), Some(3)),
                (Some(2), None),
                (Some(4), Some(4)),
                (None, Some(5)),
                (Some(6), None),
            ]
        );
    }
}
