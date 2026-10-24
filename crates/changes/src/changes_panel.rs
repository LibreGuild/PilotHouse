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
}

enum State {
    Loading,
    NotRepository,
    Loaded(WorkingChanges),
    Failed(SharedString),
}

/// The views of the Git tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitView {
    Changes,
    History,
    Stashes,
}

impl GitView {
    pub fn label(self) -> &'static str {
        match self {
            GitView::Changes => "Changes",
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
        cx.observe(&repo, |_, _, cx| cx.notify()).detach();
        let commit_box = cx.new(|cx| CommitBox::new(repo.clone(), window, cx));
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
                    Ok(Some(changes)) => State::Loaded(changes),
                    Ok(None) => State::NotRepository,
                    Err(err) => State::Failed(format!("{err:#}").into()),
                };
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
            _ => {}
        }
        cx.notify();
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

        v_flex()
            .flex_none()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                h_flex()
                    .id(("change-file", ix))
                    .px_2()
                    .py_1p5()
                    .gap_2()
                    .bg(cx.theme().muted.opacity(0.4))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&path) {
                            this.collapsed.insert(path.clone());
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall(),
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
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(dir),
                            )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("+{} −{}", file.added(), file.removed())),
                    )
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
            )
            .when(!collapsed, |this| {
                this.when(file.binary, |this| {
                    this.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Binary file changed."),
                    )
                })
                .children(
                    file.hunks
                        .iter()
                        .enumerate()
                        .map(|(hunk_ix, hunk)| self.render_hunk(ix, hunk_ix, hunk, cx)),
                )
            })
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

        let mono = cx.theme().mono_font_family.clone();
        v_flex()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_xs()
            .child(
                h_flex()
                    .px_2()
                    .py_0p5()
                    .gap_2()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_family(mono.clone())
                            .child(hunk.header.clone()),
                    )
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
            )
            .children(diff_view::hunk_lines(hunk, cx))
            .into_any_element()
    }

    fn render_untracked(&self, untracked: &[String], cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1()
            .child(
                div()
                    .pt_2()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child("New files"),
            )
            .children(untracked.iter().enumerate().map(|(ix, path)| {
                let keep = path.clone();
                let open = path.clone();
                h_flex()
                    .id(("untracked", ix))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .child(
                        Icon::new(Icons::Plus)
                            .xsmall()
                            .text_color(cx.theme().success),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(path.clone()))
                    .child(
                        Button::new(("open-untracked", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::File)
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
                    )
            }))
            .into_any_element()
    }
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
            State::Loaded(changes) => {
                let changes = changes.clone();
                v_flex()
                    .id("changes-list")
                    .px_3()
                    .pb_3()
                    .gap_2()
                    .children(
                        changes
                            .files
                            .iter()
                            .enumerate()
                            .map(|(ix, file)| self.render_file(ix, file, cx)),
                    )
                    .when(!changes.untracked.is_empty(), |this| {
                        this.child(self.render_untracked(&changes.untracked, cx))
                    })
                    .into_any_element()
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
            })
            .child(div().flex_none().child(review))
            .into_any_element()
    }
}
