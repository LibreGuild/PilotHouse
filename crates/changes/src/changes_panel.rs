//! The Git tab. **Changes**: the commit box, merge conflicts, then unstaged
//! edits hunk by hunk. **History**: commits with a graph. **Stashes**.
//!
//! **Keep** stages a hunk (it leaves this list and is ready to commit).
//! **Discard** reverts it in the working tree; the last discards can be undone
//! from the header, since the reverted text exists nowhere else.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div,
};
use ph_files::{FilesChanged, ProjectWatcher};
use ph_ui::{EmptyState, Icons};

use ph_git::{self as git, ApplyMode, FileChange, WorkingChanges};

use crate::commit_box::{CommitBox, Writer};
use crate::conflict_view::{ConflictEvent, ConflictView};
use crate::diff_view;
use crate::history_view::HistoryView;
use crate::repo::Repo;
use crate::stash_view::StashView;
/// How many discards can be undone.
const UNDO_DEPTH: usize = 20;

#[derive(Debug, Clone)]
pub enum ChangesPanelEvent {
    /// Open this project-relative file in the editor at a 0-based line.
    OpenFile { path: String, line: u32 },
    /// Give the review notes (as one message) to the chat in front.
    SendNotes(String),
    /// Have an agent review the changes before they are pushed; its
    /// findings come back as notes on the lines.
    ReviewChanges,
}

enum State {
    Loading,
    NotRepository,
    /// Shared: each row drawn reads it without copying every diff.
    Loaded(std::rc::Rc<WorkingChanges>),
    Failed(SharedString),
}

/// A row of the changes list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// The commit box, conflicts, the summary and the notes bar.
    Top,
    File(usize),
    /// A binary file's note, in place of its hunks.
    Binary(usize),
    Hunk(usize, usize),
    /// The new files.
    Untracked,
}

/// The views of the Git tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitView {
    Changes,
    /// What the branch changed against its base.
    Branch,
    History,
    Stashes,
}

impl GitView {
    pub fn label(self) -> &'static str {
        match self {
            GitView::Changes => "Changes",
            GitView::Branch => "Branch",
            GitView::History => "History",
            GitView::Stashes => "Stashes",
        }
    }
}

pub struct ChangesPanel {
    root: PathBuf,
    state: State,
    repo: Entity<Repo>,
    view: GitView,
    commit_box: Entity<CommitBox>,
    history: Option<Entity<HistoryView>>,
    stashes: Option<Entity<StashView>>,
    /// The file whose conflicts are being resolved.
    conflict: Option<Entity<ConflictView>>,
    writer: Option<Writer>,
    collapsed: HashSet<String>,
    /// Patches that were discarded, newest last.
    undo: Vec<String>,
    /// A refresh was asked for while one was running.
    refresh_again: bool,
    /// Review notes on changed lines, waiting to go to a chat.
    notes: Vec<crate::notes::Note>,
    /// The changes as a list drawn only where it shows (see [`Row`]). Many
    /// changed files with long diffs would otherwise be drawn whole on
    /// every frame.
    list: gpui_kit::ListState,
    list_rows: Vec<Row>,
    /// The branch against its base, once looked at (`None` inside: there
    /// is no base, or this is it).
    branch: Option<Result<Option<git::diff::BranchChanges>, SharedString>>,
    _branch_task: Option<Task<()>>,
    _refresh: Option<Task<()>>,
    _subscription: Subscription,
}

impl EventEmitter<ChangesPanelEvent> for ChangesPanel {}

impl ChangesPanel {
    pub fn new(
        watcher: &Entity<ProjectWatcher>,
        repo: Entity<Repo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let root = watcher.read(cx).root().to_path_buf();
        let subscription = cx.subscribe(watcher, |this, _, _: &FilesChanged, cx| this.refresh(cx));
        // The repository's state (conflicts, staged files) is in the list's
        // first row; it is measured again when that changes.
        cx.observe(&repo, |this, _, cx| {
            this.list.remeasure_items(0..1);
            cx.notify()
        })
        .detach();
        let commit_box = cx.new(|cx| CommitBox::new(repo.clone(), window, cx));
        // The commit box grows with its message and the staged files.
        cx.observe(&commit_box, |this, _, cx| {
            this.list.remeasure_items(0..1);
            cx.notify()
        })
        .detach();
        let mut panel = Self {
            root,
            state: State::Loading,
            repo,
            view: GitView::Changes,
            commit_box,
            history: None,
            stashes: None,
            conflict: None,
            writer: None,
            collapsed: HashSet::new(),
            undo: Vec::new(),
            refresh_again: false,
            notes: Vec::new(),
            list: gpui_kit::ListState::new(0, gpui_kit::ListAlignment::Top, gpui_kit::px(800.)),
            list_rows: Vec::new(),
            branch: None,
            _branch_task: None,
            _refresh: None,
            _subscription: subscription,
        };
        panel.refresh(cx);
        panel
    }

    /// Number of changed files, for the tab badge.
    pub fn change_count(&self) -> usize {
        match &self.state {
            State::Loaded(changes) => changes.files.len() + changes.untracked.len(),
            _ => 0,
        }
    }

    /// Re-reads `git diff`; a refresh already running is replaced.
    /// Re-reads the working tree. While a read is running, further requests
    /// are folded into one more read after it, so a burst of file events
    /// never cancels work that is almost done (and never starves).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self._refresh.is_some() {
            self.refresh_again = true;
            return;
        }
        let root = self.root.clone();
        let load = cx.background_spawn(async move {
            if !git::is_repository(&root) {
                return Ok(None);
            }
            git::working_changes(&root).map(Some)
        });
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            _ = this.update(cx, |this, cx| {
                this._refresh = None;
                if std::mem::take(&mut this.refresh_again) {
                    this.refresh(cx);
                }
                this.state = match result {
                    Ok(Some(changes)) => State::Loaded(std::rc::Rc::new(changes)),
                    Ok(None) => State::NotRepository,
                    Err(err) => State::Failed(format!("{err:#}").into()),
                };
                this.sync_rows();
                cx.notify();
            });
        }));
    }

    fn file(&self, ix: usize) -> Option<&FileChange> {
        match &self.state {
            State::Loaded(changes) => changes.files.get(ix),
            _ => None,
        }
    }

    /// Runs a git change in the background, then refreshes.
    fn run(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce(&std::path::Path) -> anyhow::Result<()> + Send + 'static,
    ) {
        let root = self.root.clone();
        let task = cx.background_spawn(async move { work(&root) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            _ = this.update_in(cx, |this, window, cx| {
                if let Err(err) = result {
                    window.push_notification(Notification::error(format!("{err:#}")), cx);
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    fn keep_hunk(
        &mut self,
        file_ix: usize,
        hunk_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(patch) = self.file(file_ix).and_then(|file| file.hunk_patch(hunk_ix)) else {
            return;
        };
        self.run(window, cx, move |root| {
            git::apply(root, &patch, ApplyMode::Stage)
        });
    }

    fn discard_hunk(
        &mut self,
        file_ix: usize,
        hunk_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(patch) = self.file(file_ix).and_then(|file| file.hunk_patch(hunk_ix)) else {
            return;
        };
        self.remember_discard(patch.clone());
        self.run(window, cx, move |root| {
            git::apply(root, &patch, ApplyMode::Discard)
        });
    }

    fn keep_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, move |root| git::stage_path(root, &path));
    }

    fn discard_file(&mut self, file_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.file(file_ix) else {
            return;
        };
        // Discard from the last hunk up, so earlier hunks' line numbers stay valid.
        let patches: Vec<String> = (0..file.hunks.len())
            .rev()
            .filter_map(|ix| file.hunk_patch(ix))
            .collect();
        for patch in patches.iter().rev() {
            self.remember_discard(patch.clone());
        }
        self.run(window, cx, move |root| {
            for patch in &patches {
                git::apply(root, patch, ApplyMode::Discard)?;
            }
            Ok(())
        });
    }

    fn undo_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(patch) = self.undo.pop() else { return };
        self.run(window, cx, move |root| {
            git::apply(root, &patch, ApplyMode::Restore)
        });
    }

    fn remember_discard(&mut self, patch: String) {
        self.undo.push(patch);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    // ---- Rendering -----------------------------------------------------------

    /// Lets an agent write commit messages and merge conflicts.
    pub fn set_writer(&mut self, writer: Writer, cx: &mut Context<Self>) {
        self.commit_box
            .update(cx, |commit_box, _| commit_box.set_writer(writer.clone()));
        self.writer = Some(writer);
    }

    pub fn show_view(&mut self, view: GitView, window: &mut Window, cx: &mut Context<Self>) {
        self.view = view;
        match view {
            GitView::History if self.history.is_none() => {
                let (repo, writer) = (self.repo.clone(), self.writer.clone());
                self.history = Some(cx.new(|cx| {
                    let mut history = HistoryView::new(repo, cx);
                    history.set_writer(writer);
                    history
                }));
            }
            GitView::Stashes if self.stashes.is_none() => {
                let repo = self.repo.clone();
                self.stashes = Some(cx.new(|cx| StashView::new(repo, window, cx)));
            }
            GitView::Branch => self.load_branch(cx),
            _ => {}
        }
        cx.notify();
    }

    /// Reads the branch's changes against its base, off the UI thread.
    fn load_branch(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let read = cx.background_spawn(async move { git::diff::branch_changes(&root) });
        self._branch_task = Some(cx.spawn(async move |this, cx| {
            let result = read
                .await
                .map_err(|err| SharedString::from(format!("{err:#}")));
            _ = this.update(cx, |this, cx| {
                this.branch = Some(result);
                cx.notify();
            });
        }));
    }

    fn render_branch(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.branch {
            None => div().into_any_element(),
            Some(Err(err)) => EmptyState::new("Could not compare the branch")
                .icon(IconName::CircleAlert)
                .description(err.clone())
                .into_any_element(),
            Some(Ok(None)) => EmptyState::new("Nothing to compare")
                .icon(Icons::GitBranch)
                .description(
                    "This is the default branch, or the repository has none to compare with.",
                )
                .into_any_element(),
            Some(Ok(Some(branch))) => {
                let added: usize = branch.files.iter().map(FileChange::added).sum();
                let removed: usize = branch.files.iter().map(FileChange::removed).sum();
                v_flex()
                    .id("branch-changes")
                    .size_full()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_3()
                    .gap_2()
                    .child(
                        h_flex()
                            .px_1()
                            .pt_2()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(div().flex_1().child(format!(
                                "{} files against {} (since {})",
                                branch.files.len(),
                                branch.base,
                                branch.fork
                            )))
                            .child(change_counts(added, removed, cx))
                            .child(
                                Button::new("refresh-branch")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::RefreshCw)
                                    .tooltip("Compare again")
                                    .accessibility_label("Compare again")
                                    .on_click(cx.listener(|this, _, _, cx| this.load_branch(cx))),
                            ),
                    )
                    .when(branch.files.is_empty(), |this| {
                        this.child(
                            div()
                                .px_1()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No commits beyond the base yet."),
                        )
                    })
                    .children(
                        branch
                            .files
                            .iter()
                            .map(|file| diff_view::file_diff(file, cx)),
                    )
                    .into_any_element()
            }
        }
    }

    fn open_conflict(&mut self, path: String, cx: &mut Context<Self>) {
        let (repo, writer) = (self.repo.clone(), self.writer.clone());
        let view = cx.new(|cx| ConflictView::new(repo, path, writer, cx));
        cx.subscribe(&view, |this, _, event: &ConflictEvent, cx| match event {
            ConflictEvent::Close => {
                this.conflict = None;
                cx.notify();
            }
        })
        .detach();
        self.conflict = Some(view);
        cx.notify();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.change_count();
        let tab = |view: GitView, label: String, cx: &mut Context<Self>| {
            let selected = self.view == view;
            div()
                .id(SharedString::from(format!("git-view-{label}")))
                .px_2p5()
                .py_1()
                .rounded(cx.theme().radius)
                .text_sm()
                .cursor_pointer()
                .text_color(if selected {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .when(selected, |this| {
                    this.bg(cx.theme().secondary)
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                })
                .when(!selected, |this| {
                    this.hover(|style| style.text_color(cx.theme().foreground))
                })
                .on_click(cx.listener(move |this, _, window, cx| this.show_view(view, window, cx)))
                .role(gpui_kit::accesskit::Role::Tab)
                .aria_label(view.label())
                .child(label)
                .test_support()
        };
        h_flex()
            .flex_none()
            .px_2()
            .pt_2()
            .gap_1()
            .child(tab(
                GitView::Changes,
                if count > 0 {
                    format!("Changes {count}")
                } else {
                    "Changes".into()
                },
                cx,
            ))
            .child(tab(GitView::Branch, "Branch".into(), cx))
            .child(tab(GitView::History, "History".into(), cx))
            .child(tab(GitView::Stashes, "Stashes".into(), cx))
    }

    /// Conflicted files and the operation in progress (merge, rebase…).
    fn render_conflicts(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let repo = self.repo.read(cx);
        let operation = repo.operation();
        let conflicted = repo
            .status()
            .map(|status| status.conflicted.clone())
            .unwrap_or_default();
        if operation.is_none() && conflicted.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .mx_3()
                .mt_3()
                .p_2()
                .gap_1()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().warning.opacity(0.6))
                .bg(cx.theme().warning.opacity(0.08))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .small()
                                .text_color(cx.theme().warning),
                        )
                        .child(div().flex_1().text_sm().child(
                            match (operation, conflicted.len()) {
                                (Some(op), 0) => {
                                    format!("A {op} is in progress. Commit to finish it.")
                                }
                                (Some(op), n) => format!(
                                    "{n} conflicted file{} in this {op}",
                                    if n == 1 { "" } else { "s" }
                                ),
                                (None, n) => {
                                    format!("{n} conflicted file{}", if n == 1 { "" } else { "s" })
                                }
                            },
                        ))
                        .when(operation.is_some(), |this| {
                            this.child(
                                Button::new("abort-operation")
                                    .xsmall()
                                    .ghost()
                                    .label("Abort")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.repo.update(cx, |repo, cx| {
                                            repo.run(
                                                "Aborting",
                                                |root| {
                                                    git::conflicts::abort(root)
                                                        .map(|_| "Aborted".into())
                                                },
                                                cx,
                                            )
                                        })
                                    })),
                            )
                        }),
                )
                .children(conflicted.into_iter().enumerate().map(|(ix, path)| {
                    let open = path.clone();
                    h_flex()
                        .id(("conflict-file", ix))
                        .gap_1p5()
                        .px_1()
                        .py_0p5()
                        .rounded(cx.theme().radius)
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .text_sm()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_conflict(open.clone(), cx)),
                        )
                        .child(ph_ui::file_icon_element(&path, gpui_kit::px(14.)))
                        .child(div().flex_1().truncate().child(path))
                        .child(div().text_xs().text_color(cx.theme().info).child("Resolve"))
                }))
                .into_any_element(),
        )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = match &self.state {
            State::Loaded(changes) => {
                let added: usize = changes.files.iter().map(FileChange::added).sum();
                let removed: usize = changes.files.iter().map(FileChange::removed).sum();
                let files = changes.files.len() + changes.untracked.len();
                let mut text = format!("{files} changed · +{added} −{removed}");
                if changes.staged_files > 0 {
                    text.push_str(&format!(" · {} kept", changes.staged_files));
                }
                text
            }
            _ => String::new(),
        };
        h_flex()
            .flex_none()
            .px_3()
            .py_2()
            .gap_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(div().flex_1().truncate().child(summary))
            .when(matches!(&self.state, State::Loaded(changes) if !changes.files.is_empty() || !changes.untracked.is_empty()), |this| {
                this.child(
                    Button::new("review-changes")
                        .ghost()
                        .xsmall()
                        .icon(ph_ui::Icons::Sparkles)
                        .label("Review")
                        .tooltip("An agent reviews these changes before you push; its findings land on the lines as notes")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ChangesPanelEvent::ReviewChanges))),
                )
            })
            .when(!self.undo.is_empty(), |this| {
                this.child(
                    Button::new("undo-discard")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Undo2)
                        .label("Undo discard")
                        .on_click(cx.listener(|this, _, window, cx| this.undo_discard(window, cx))),
                )
            })
            .child(
                Button::new("refresh-changes")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh")
                    .accessibility_label("Refresh changes")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
    }

    fn render_file(&self, ix: usize, file: &FileChange, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.collapsed.contains(&file.path);
        let path = file.path.clone();
        let open_path = file.path.clone();
        let (dir, name) = match file.path.rsplit_once('/') {
            Some((dir, name)) => (format!("{dir}/"), name.to_string()),
            None => (String::new(), file.path.clone()),
        };
        let group = SharedString::from(format!("change-file-{ix}"));

        v_flex()
            .flex_none()
            .child(
                h_flex()
                    .id(("change-file", ix))
                    .group(group.clone())
                    .h_7()
                    .px_1p5()
                    .gap_1p5()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&path) {
                            this.collapsed.insert(path.clone());
                        }
                        this.toggled_file(ix);
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child({
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1p5()
                            .text_sm()
                            .child(ph_ui::file_icon_element(&name, gpui_kit::px(14.)))
                            .child(div().flex_none().child(name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(dir),
                            )
                    })
                    .child(change_counts(file.added(), file.removed(), cx))
                    // Acting on a file is for the row under the pointer.
                    .child(
                        h_flex()
                            .gap_0p5()
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .child(
                                Button::new(("open-changed-file", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::File)
                                    .tooltip("Open file")
                                    .accessibility_label("Open file")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ChangesPanelEvent::OpenFile {
                                            path: open_path.clone(),
                                            line: 0,
                                        });
                                    })),
                            )
                            .child(
                                Button::new(("discard-file", ix))
                                    .ghost()
                                    .xsmall()
                                    .label("Discard")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.discard_file(ix, window, cx)
                                    })),
                            )
                            .child({
                                let path = file.path.clone();
                                Button::new(("keep-file", ix))
                                    .outline()
                                    .xsmall()
                                    .label("Keep")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.keep_path(path.clone(), window, cx)
                                    }))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_hunk(
        &self,
        file_ix: usize,
        hunk_ix: usize,
        hunk: &git::Hunk,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = (file_ix * 10_000 + hunk_ix) as u64;
        let group = SharedString::from(format!("change-hunk-{id}"));
        let mono = cx.theme().mono_font_family.clone();
        v_flex()
            .when(hunk_ix > 0, |this| {
                this.border_t_1().border_color(cx.theme().border)
            })
            .text_xs()
            .child(
                h_flex()
                    .id(("change-hunk", id))
                    .group(group.clone())
                    .h_6()
                    .px_2()
                    .gap_2()
                    .bg(cx.theme().muted.opacity(0.6))
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_family(mono.clone())
                            .child(hunk.header.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_0p5()
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .child(
                                Button::new(("discard-hunk", id))
                                    .ghost()
                                    .xsmall()
                                    .label("Discard")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.discard_hunk(file_ix, hunk_ix, window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("keep-hunk", id))
                                    .ghost()
                                    .xsmall()
                                    .label("Keep")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.keep_hunk(file_ix, hunk_ix, window, cx)
                                    })),
                            ),
                    ),
            )
            .children(self.hunk_rows(file_ix, hunk_ix, hunk, cx))
            .into_any_element()
    }

    /// A hunk's lines; on hover a line offers a review note (the new side's
    /// lines), and its notes show under it.
    fn hunk_rows(
        &self,
        file_ix: usize,
        hunk_ix: usize,
        hunk: &git::Hunk,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(path) = self.file(file_ix).map(|file| file.path.clone()) else {
            return diff_view::hunk_lines(hunk, cx);
        };
        let numbers = crate::notes::new_line_numbers(&hunk.header, &hunk.lines);
        let added_bg = cx.theme().success.opacity(0.12);
        let removed_bg = cx.theme().danger.opacity(0.12);
        let mono = cx.theme().mono_font_family.clone();
        let mut rows = Vec::new();
        for (line_ix, (line, number)) in hunk
            .lines
            .iter()
            .zip(numbers)
            .take(diff_view::MAX_HUNK_LINES)
            .enumerate()
        {
            let id = ((file_ix * 10_000 + hunk_ix) * 1_000 + line_ix) as u64;
            let group = SharedString::from(format!("diff-line-{id}"));
            let row = h_flex()
                .id(("diff-line", id))
                .group(group.clone())
                .relative()
                .pl_3()
                .font_family(mono.clone())
                .whitespace_nowrap()
                .overflow_hidden()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(line.clone()),
                )
                .when_some(number, |this, number| {
                    let (path, code) = (path.clone(), line.clone());
                    this.child(
                        div()
                            .absolute()
                            .right_1()
                            .top_0()
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .child(
                                Button::new(("note-line", id))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .tooltip("Add a review note")
                                    .accessibility_label("Add a review note")
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        ask_note(path.clone(), number, code.clone(), window, cx)
                                    })),
                            ),
                    )
                });
            let row = match line.chars().next() {
                Some('+') => row.bg(added_bg),
                Some('-') => row.bg(removed_bg),
                _ => row.text_color(cx.theme().muted_foreground),
            };
            rows.push(row.into_any_element());
            if let Some(number) = number {
                for (note_ix, note) in self
                    .notes
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| note.path == path && note.line == number)
                {
                    rows.push(self.note_row(note_ix, note, cx));
                }
            }
        }
        if hunk.lines.len() > diff_view::MAX_HUNK_LINES {
            rows.push(
                div()
                    .px_3()
                    .py_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "… {} more lines",
                        hunk.lines.len() - diff_view::MAX_HUNK_LINES
                    ))
                    .into_any_element(),
            );
        }
        rows
    }

    /// A review note under its line.
    fn note_row(&self, ix: usize, note: &crate::notes::Note, cx: &mut Context<Self>) -> AnyElement {
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
            .child(div().flex_1().min_w_0().child(note.text.clone()))
            .child(
                Button::new(("remove-note", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Remove the note")
                    .accessibility_label("Remove the note")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if ix < this.notes.len() {
                            this.notes.remove(ix);
                        }
                        this.list.remeasure();
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// Puts an agent's review findings on the lines as notes, in place of
    /// those of its review before.
    pub fn set_review_notes(&mut self, notes: Vec<crate::notes::Note>, cx: &mut Context<Self>) {
        self.notes.retain(|note| !note.from_review);
        self.notes.extend(notes);
        self.list.remeasure();
        cx.notify();
    }

    /// Adds a review note.
    pub fn add_note(&mut self, note: crate::notes::Note, cx: &mut Context<Self>) {
        if !note.text.trim().is_empty() {
            self.notes.push(note);
            self.list.remeasure();
            cx.notify();
        }
    }

    /// The changes list's rows: the top (commit box, header), each file's
    /// row and its hunks (unless folded), and the new files. A hunk is a
    /// row of its own so a long diff is drawn only where it shows.
    fn rows(&self) -> Vec<Row> {
        let State::Loaded(changes) = &self.state else {
            return Vec::new();
        };
        let mut rows = vec![Row::Top];
        for (ix, file) in changes.files.iter().enumerate() {
            rows.push(Row::File(ix));
            if self.collapsed.contains(&file.path) {
                continue;
            }
            if file.binary {
                rows.push(Row::Binary(ix));
            }
            rows.extend((0..file.hunks.len()).map(|hunk| Row::Hunk(ix, hunk)));
        }
        if !changes.untracked.is_empty() {
            rows.push(Row::Untracked);
        }
        rows
    }

    /// Takes the rows as they are now; the list keeps its place when they
    /// are the same ones.
    fn sync_rows(&mut self) {
        let rows = self.rows();
        if rows != self.list_rows {
            self.list.reset(rows.len());
            self.list_rows = rows;
        } else {
            // The same rows, but a hunk can have other lines now.
            self.list.remeasure();
        }
    }

    /// File `ix` was folded or unfolded: its hunk rows go or come back,
    /// the rest of the list stays where it is.
    fn toggled_file(&mut self, ix: usize) {
        let rows = self.rows();
        let Some(at) = self.list_rows.iter().position(|row| *row == Row::File(ix)) else {
            self.sync_rows();
            return;
        };
        let was = self.list_rows[at + 1..]
            .iter()
            .take_while(|row| matches!(row, Row::Hunk(file, _) | Row::Binary(file) if *file == ix))
            .count();
        let now = rows[at + 1..]
            .iter()
            .take_while(|row| matches!(row, Row::Hunk(file, _) | Row::Binary(file) if *file == ix))
            .count();
        self.list.splice(at + 1..at + 1 + was, now);
        self.list.remeasure_items(at..at + 1);
        self.list_rows = rows;
    }

    fn render_list_item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let (State::Loaded(changes), Some(row)) = (&self.state, self.list_rows.get(ix).copied())
        else {
            return div().into_any_element();
        };
        let changes = changes.clone();
        match row {
            Row::Top => v_flex()
                .w_full()
                .child(div().flex_none().child(self.commit_box.clone()))
                .children(self.render_conflicts(cx))
                .child(div().flex_none().child(self.render_header(cx)))
                .children(self.render_notes_bar(cx))
                .into_any_element(),
            Row::File(file) => div()
                .px_2()
                .child(self.render_file(file, &changes.files[file], cx))
                .into_any_element(),
            Row::Binary(file) => self.hunk_frame(
                file,
                true,
                changes.files[file].hunks.is_empty(),
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Binary file changed.")
                    .into_any_element(),
                cx,
            ),
            Row::Hunk(file, hunk) => {
                let hunks = &changes.files[file].hunks;
                let first = hunk == 0 && !changes.files[file].binary;
                let last = hunk + 1 == hunks.len();
                let body = self.render_hunk(file, hunk, &hunks[hunk], cx);
                self.hunk_frame(file, first, last, body, cx)
            }
            Row::Untracked => div()
                .px_2()
                .pb_3()
                .child(self.render_untracked(&changes.untracked, cx))
                .into_any_element(),
        }
    }

    /// A piece of a file's diff in the bordered block under its row: the
    /// block's top edge on the `first` piece, its bottom on the `last`.
    fn hunk_frame(
        &self,
        _file: usize,
        first: bool,
        last: bool,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let radius = cx.theme().radius;
        div()
            .px_2()
            .child(
                div()
                    .ml_5()
                    .border_x_1()
                    .border_color(cx.theme().border)
                    .overflow_hidden()
                    .when(first, |this| this.mt_0p5().border_t_1().rounded_t(radius))
                    .when(last, |this| this.mb_2().border_b_1().rounded_b(radius))
                    .child(body),
            )
            .into_any_element()
    }

    /// The bar over the changes while there are notes: how many, and
    /// sending them to the chat.
    fn render_notes_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.notes.is_empty() {
            return None;
        }
        let count = self.notes.len();
        Some(
            h_flex()
                .flex_none()
                .mx_2()
                .mb_1()
                .px_2()
                .py_1()
                .gap_2()
                .rounded(cx.theme().radius)
                .bg(cx.theme().muted)
                .text_xs()
                .child(div().flex_1().child(format!(
                    "{count} review note{}",
                    if count == 1 { "" } else { "s" }
                )))
                .child(
                    Button::new("clear-notes")
                        .ghost()
                        .xsmall()
                        .label("Clear")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notes.clear();
                            this.list.remeasure();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("send-notes")
                        .small()
                        .primary()
                        .label(if self.notes.iter().any(|note| note.from_review) {
                            "Fix with agent"
                        } else {
                            "Send to chat"
                        })
                        .tooltip("Give every note to the chat in front, as one message")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let text = crate::notes::prompt(&this.notes, &this.root);
                            this.notes.clear();
                            this.list.remeasure();
                            cx.emit(ChangesPanelEvent::SendNotes(text));
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_untracked(&self, untracked: &[String], cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .child(
                div()
                    .px_1p5()
                    .pt_3()
                    .pb_1()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child("New files"),
            )
            .children(untracked.iter().enumerate().map(|(ix, path)| {
                let keep = path.clone();
                let open = path.clone();
                let group = SharedString::from(format!("untracked-{ix}"));
                let (dir, name) = match path.rsplit_once('/') {
                    Some((dir, name)) => (format!("{dir}/"), name.to_string()),
                    None => (String::new(), path.clone()),
                };
                h_flex()
                    .id(("untracked", ix))
                    .group(group.clone())
                    .h_7()
                    .px_1p5()
                    .gap_1p5()
                    .rounded(cx.theme().radius)
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .text_sm()
                    .child(
                        div()
                            .w_3()
                            .flex_none()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().success)
                            .child("A"),
                    )
                    .child(ph_ui::file_icon_element(&name, gpui_kit::px(14.)))
                    .child(div().flex_none().child(name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(dir),
                    )
                    .child(
                        h_flex()
                            .gap_0p5()
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .child(
                                Button::new(("open-untracked", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::File)
                                    .tooltip("Open file")
                                    .accessibility_label("Open file")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ChangesPanelEvent::OpenFile {
                                            path: open.clone(),
                                            line: 0,
                                        });
                                    })),
                            )
                            .child(
                                Button::new(("keep-untracked", ix))
                                    .outline()
                                    .xsmall()
                                    .label("Keep")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.keep_path(keep.clone(), window, cx)
                                    })),
                            ),
                    )
            }))
            .into_any_element()
    }
}

/// `+12 −3`, in the git colours, small.
fn change_counts(added: usize, removed: usize, cx: &gpui_kit::App) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap_1()
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
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

impl Render for ChangesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match self.view {
            GitView::History => self
                .history
                .clone()
                .map(IntoElement::into_any_element)
                .unwrap_or_else(|| div().into_any_element()),
            GitView::Stashes => self
                .stashes
                .clone()
                .map(IntoElement::into_any_element)
                .unwrap_or_else(|| div().into_any_element()),
            GitView::Branch => self.render_branch(cx),
            GitView::Changes => match &self.conflict {
                Some(conflict) => conflict.clone().into_any_element(),
                None => self.render_changes(cx),
            },
        };
        v_flex()
            .size_full()
            .child(self.render_tabs(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl ChangesPanel {
    fn render_changes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let is_repo = !matches!(self.state, State::NotRepository | State::Loading);
        let review: AnyElement = match &self.state {
            State::Loading => div().into_any_element(),
            State::NotRepository => EmptyState::new("Not a git repository")
                .icon(Icons::GitCompare)
                .description(
                    "Reviewing changes needs git. Run `git init` in the project folder to start.",
                )
                .into_any_element(),
            State::Failed(message) => EmptyState::new("Could not read changes")
                .icon(IconName::CircleAlert)
                .description(message.clone())
                .into_any_element(),
            State::Loaded(changes) if changes.files.is_empty() && changes.untracked.is_empty() => {
                EmptyState::new("No changes")
                    .icon(Icons::GitCompare)
                    .description(if changes.staged_files > 0 {
                        "Everything is kept (staged). Commit when ready."
                    } else {
                        "Edits made by you or an agent appear here for review."
                    })
                    .into_any_element()
            }
            State::Loaded(_) => {
                // Drawn only where it shows (see `list`).
                if self.list_rows.is_empty() {
                    self.sync_rows();
                }
                let entity = cx.entity().downgrade();
                use gpui_kit::component::scroll::ScrollableElement as _;
                return div()
                    .relative()
                    .size_full()
                    .child(
                        gpui_kit::list(self.list.clone(), move |ix, _, cx| {
                            entity
                                .update(cx, |this, cx| this.render_list_item(ix, cx))
                                .unwrap_or_else(|_| div().into_any_element())
                        })
                        .size_full(),
                    )
                    .vertical_scrollbar(&self.list)
                    .into_any_element();
            }
        };
        v_flex()
            .id("git-changes")
            .size_full()
            .overflow_y_scroll()
            .when(is_repo, |this| {
                this.child(div().flex_none().child(self.commit_box.clone()))
                    .children(self.render_conflicts(cx))
                    .child(div().flex_none().child(self.render_header(cx)))
                    .children(self.render_notes_bar(cx))
            })
            .child(div().flex_none().child(review))
            .into_any_element()
    }
}

/// Asks for a review note on `line` of `path`; OK adds it to the panel.
fn ask_note(
    path: String,
    line: u32,
    code: String,
    window: &mut Window,
    cx: &mut Context<ChangesPanel>,
) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::input::{Input, InputState};
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("What should change here?"));
    let panel = cx.entity().downgrade();
    let focus = input.clone();
    let title = format!("Note on {path}:{line}");
    window.open_dialog(cx, move |dialog, _, _| {
        let (input, panel, path, code) = (input.clone(), panel.clone(), path.clone(), code.clone());
        dialog
            .title(title.clone())
            .w(gpui_kit::px(520.))
            .child(Input::new(&input))
            .footer(ph_ui::dialog_buttons("Add note", true))
            .on_ok(move |_, _, cx| {
                let text = input.read(cx).value().to_string();
                let note = crate::notes::Note {
                    path: path.clone(),
                    line,
                    code: code.clone(),
                    text,
                    from_review: false,
                };
                _ = panel.update(cx, |panel, cx| panel.add_note(note, cx));
                true
            })
    });
    focus.update(cx, |input, cx| input.focus(window, cx));
}
