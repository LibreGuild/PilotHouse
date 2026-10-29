//! The editor's Changes tab: one file's changes, or all of them, as a diff
//! drawn side by side or in one column (gpui-component's diff viewer, with
//! syntax colours). The Git tab opens it; it reads the changes again when
//! asked, off the UI thread.

use std::sync::Arc;

use anyhow::Result;
use gpui_kit::component::diff::{Diff, DiffFile, DiffMode, DiffState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div,
};
use ph_ui::{EmptyState, Icons};

/// Reads the changes to show, as a patch (`git diff` output). Blocking.
pub type LoadDiff = Arc<dyn Fn() -> Result<String> + Send + Sync>;

pub struct DiffTab {
    title: SharedString,
    load: LoadDiff,
    state: Entity<DiffState>,
    /// Files in view (none: nothing changed).
    files: usize,
    error: Option<SharedString>,
    loading: bool,
    _load: Option<Task<()>>,
}

impl DiffTab {
    pub fn new(title: SharedString, load: LoadDiff, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| DiffState::new(Vec::new(), cx));
        let mut tab = Self {
            title,
            load,
            state,
            files: 0,
            error: None,
            loading: false,
            _load: None,
        };
        tab.reload(cx);
        tab
    }

    pub fn title(&self) -> SharedString {
        self.title.clone()
    }

    /// Shows other changes in this tab.
    pub fn show(&mut self, title: SharedString, load: LoadDiff, cx: &mut Context<Self>) {
        self.title = title;
        self.load = load;
        self.reload(cx);
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let load = self.load.clone();
        self.loading = true;
        let read = cx.background_spawn(async move {
            let patch = load()?;
            DiffFile::parse(&patch).map_err(|err| anyhow::anyhow!("{err}"))
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(files) => {
                        this.files = files.len();
                        this.error = None;
                        this.state
                            .update(cx, |state, cx| state.set_files(files, cx));
                    }
                    // Git's first line says what went wrong; the rest is
                    // usage text.
                    Err(err) => {
                        let text = format!("{err:#}");
                        let first = text.lines().next().unwrap_or_default().to_string();
                        this.error = Some(first.into());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for DiffTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let split = self.state.read(cx).mode() == DiffMode::Split;
        let body = if let Some(error) = self.error.clone() {
            EmptyState::new("Could not read the changes")
                .icon(Icons::CircleAlert)
                .description(error)
                .into_any_element()
        } else if self.files == 0 && !self.loading {
            EmptyState::new("No changes")
                .icon(Icons::GitCompare)
                .description("The files are as they were at the last commit.")
                .into_any_element()
        } else {
            // Side by side, long lines wrap so both sides stay in view.
            Diff::new(&self.state).soft_wrap(split).into_any_element()
        };
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .flex_none()
                    .h_9()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(Icon::new(Icons::GitCompare).small())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(self.title.clone()),
                    )
                    .child(
                        TabBar::new("diff-mode")
                            .segmented()
                            .xsmall()
                            .selected_index(usize::from(split))
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                let mode = if *ix == 1 {
                                    DiffMode::Split
                                } else {
                                    DiffMode::Unified
                                };
                                this.state.update(cx, |state, cx| state.set_mode(mode, cx));
                                cx.notify();
                            }))
                            .child(Tab::new().label("Unified"))
                            .child(Tab::new().label("Split")),
                    )
                    .child(
                        Button::new("diff-reload")
                            .ghost()
                            .xsmall()
                            .icon(Icons::RotateCw)
                            .loading(self.loading)
                            .tooltip("Read the changes again")
                            .accessibility_label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
    }
}
