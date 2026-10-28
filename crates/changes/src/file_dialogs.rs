//! Dialogs the Files tree opens for one file: its diff against the last
//! commit or another file, and its history (each commit's change to it).

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _, h_flex, notification::Notification, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, px,
};
use ph_git::FileChange;
use ph_git::log::Commit;

use ph_preview::diff::file_diff;

/// Shows `changes` in a dialog titled `title`; says so when there are none.
pub fn open_diff(
    title: impl Into<SharedString>,
    changes: Vec<FileChange>,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.into();
    if changes.is_empty() {
        window.push_notification(Notification::info(format!("{title}: no differences.")), cx);
        return;
    }
    let changes = Rc::new(changes);
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog.title(title.clone()).w(px(860.)).child(
            v_flex()
                .id("file-diff")
                .max_h(px(600.))
                .overflow_y_scroll()
                .gap_2()
                .children(changes.iter().map(|change| file_diff(change, cx))),
        )
    });
}

/// Runs `load` in the background and shows its diff, or why it failed.
pub fn load_diff(
    title: impl Into<SharedString>,
    load: impl FnOnce() -> anyhow::Result<Vec<FileChange>> + Send + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.into();
    let task = cx.background_spawn(async move { load() });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            _ = cx.update(|window, cx| match result {
                Ok(changes) => open_diff(title, changes, window, cx),
                Err(err) => {
                    window.push_notification(Notification::error(format!("{title}: {err:#}")), cx)
                }
            });
        })
        .detach();
}

/// The commits that changed `path` in the repository at `root`; picking
/// one shows what it changed.
pub fn open_history(root: PathBuf, path: String, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| FileHistory::new(root, path.clone(), cx));
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(format!("History of {name}"))
            .w(px(900.))
            .child(view.clone())
    });
}

/// How many commits the history lists.
const HISTORY_LIMIT: usize = 200;

struct FileHistory {
    root: PathBuf,
    path: String,
    commits: Option<Result<Vec<Commit>, SharedString>>,
    selected: Option<usize>,
    diff: Option<Result<Vec<FileChange>, SharedString>>,
    _load: Task<()>,
    _diff: Option<Task<()>>,
}

impl FileHistory {
    fn new(root: PathBuf, path: String, cx: &mut Context<Self>) -> Self {
        let (load_root, load_path) = (root.clone(), path.clone());
        let load = cx.background_spawn(async move {
            ph_git::files::file_log(&load_root, &load_path, HISTORY_LIMIT)
        });
        let task = cx.spawn(async move |this, cx| {
            let result = load
                .await
                .map_err(|err| SharedString::from(format!("{err:#}")));
            _ = this.update(cx, |this, cx| {
                let first = result.as_ref().is_ok_and(|commits| !commits.is_empty());
                this.commits = Some(result);
                if first {
                    this.select(0, cx);
                }
                cx.notify();
            });
        });
        Self {
            root,
            path,
            commits: None,
            selected: None,
            diff: None,
            _load: task,
            _diff: None,
        }
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(Ok(commits)) = &self.commits else {
            return;
        };
        let Some(commit) = commits.get(ix) else {
            return;
        };
        self.selected = Some(ix);
        self.diff = None;
        let (root, path, id) = (self.root.clone(), self.path.clone(), commit.id.clone());
        let load =
            cx.background_spawn(async move { ph_git::files::commit_diff(&root, &id, &path) });
        self._diff = Some(cx.spawn(async move |this, cx| {
            let result = load
                .await
                .map_err(|err| SharedString::from(format!("{err:#}")));
            _ = this.update(cx, |this, cx| {
                this.diff = Some(result);
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for FileHistory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let message = |text: SharedString| {
            div()
                .p_3()
                .text_sm()
                .text_color(muted)
                .child(text)
                .into_any_element()
        };
        let commits = match &self.commits {
            None => message("Reading the history…".into()),
            Some(Err(err)) => message(err.clone()),
            Some(Ok(commits)) if commits.is_empty() => {
                message("No commit has changed this file yet.".into())
            }
            Some(Ok(commits)) => v_flex()
                .gap_0p5()
                .children(commits.iter().enumerate().map(|(ix, commit)| {
                    let selected = self.selected == Some(ix);
                    v_flex()
                        .id(("history-commit", ix))
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .when(selected, |row| row.bg(theme.list_active))
                        .when(!selected, |row| row.hover(|row| row.bg(theme.list_hover)))
                        .child(div().text_sm().truncate().child(commit.subject.clone()))
                        .child(div().text_xs().text_color(muted).truncate().child(format!(
                            "{} · {} · {}",
                            commit.short(),
                            commit.author,
                            ph_ui::time::ago(commit.time)
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
                }))
                .into_any_element(),
        };
        let diff = match &self.diff {
            None if self.selected.is_some() => message("Loading…".into()),
            None => div().into_any_element(),
            Some(Err(err)) => message(err.clone()),
            Some(Ok(changes)) if changes.is_empty() => {
                message("No change to this file's text in that commit.".into())
            }
            Some(Ok(changes)) => v_flex()
                .gap_2()
                .children(changes.iter().map(|change| file_diff(change, cx)))
                .into_any_element(),
        };
        h_flex()
            .h(px(560.))
            .items_start()
            .gap_3()
            .child(
                div()
                    .id("history-commits")
                    .w(px(280.))
                    .flex_none()
                    .h_full()
                    .overflow_y_scroll()
                    .child(commits),
            )
            .child(
                div()
                    .id("history-diff")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(diff),
            )
    }
}
