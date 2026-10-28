//! Stashes: put the current changes aside, look at them, bring them back.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Task, Window, div,
};
use ph_git::stash::{self, Stash};
use ph_git::{FileChange, parse_diff};

use crate::branch_picker::ago;
use crate::repo::Repo;
use ph_preview::diff as diff_view;

pub struct StashView {
    repo: Entity<Repo>,
    message: Entity<InputState>,
    stashes: Vec<Stash>,
    open: Option<(usize, Vec<FileChange>)>,
    _load: Option<Task<()>>,
}

impl StashView {
    pub fn new(repo: Entity<Repo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message =
            cx.new(|cx| InputState::new(window, cx).placeholder("Describe the stash (optional)"));
        cx.observe(&repo, |this, _, cx| this.reload(cx)).detach();
        let mut view = Self {
            repo,
            message,
            stashes: Vec::new(),
            open: None,
            _load: None,
        };
        view.reload(cx);
        view
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let root = self.repo.read(cx).root().to_path_buf();
        let read = cx.background_spawn(async move { stash::list(&root) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let stashes = read.await.unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                if this.stashes != stashes {
                    this.stashes = stashes;
                    this.open = None;
                    cx.notify();
                }
            });
        }));
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message.read(cx).value().to_string();
        self.message
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.repo.update(cx, |repo, cx| {
            repo.run(
                "Stashing",
                move |root| stash::push(root, &message).map(|_| "Changes stashed".into()),
                cx,
            )
        });
    }

    fn act(&mut self, index: usize, action: Action, cx: &mut Context<Self>) {
        self.repo.update(cx, |repo, cx| match action {
            Action::Apply => repo.run(
                "Applying stash",
                move |root| stash::apply(root, index).map(|_| "Stash applied".into()),
                cx,
            ),
            Action::Pop => repo.run(
                "Popping stash",
                move |root| stash::pop(root, index).map(|_| "Stash applied and removed".into()),
                cx,
            ),
            Action::Drop => repo.run(
                "Dropping stash",
                move |root| stash::drop(root, index).map(|_| "Stash dropped".into()),
                cx,
            ),
        });
    }

    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|(open, _)| *open == index) {
            self.open = None;
            cx.notify();
            return;
        }
        let root = self.repo.read(cx).root().to_path_buf();
        let read = cx.background_spawn(async move { stash::show(&root, index) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let patch = read.await.unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                this.open = Some((index, parse_diff(&patch)));
                cx.notify();
            });
        }));
    }

    fn render_stash(&self, stash: &Stash, cx: &mut Context<Self>) -> AnyElement {
        let index = stash.index;
        let files = self
            .open
            .as_ref()
            .filter(|(open, _)| *open == index)
            .map(|(_, files)| files.clone());
        let button =
            |id: &'static str, label: &'static str, action: Action, cx: &mut Context<Self>| {
                Button::new((id, index))
                    .xsmall()
                    .ghost()
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.act(index, action, cx)))
            };
        v_flex()
            .flex_none()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .id(("stash", index))
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(index, cx)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(stash.message.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("stash@{{{index}}} · {}", ago(stash.created))),
                            ),
                    )
                    .child(button("stash-apply", "Apply", Action::Apply, cx))
                    .child(button("stash-pop", "Pop", Action::Pop, cx))
                    .child(button("stash-drop", "Drop", Action::Drop, cx)),
            )
            .when_some(files, |this, files| {
                this.child(
                    v_flex()
                        .p_2()
                        .gap_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .children(files.iter().map(|file| diff_view::file_diff(file, cx))),
                )
            })
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum Action {
    Apply,
    Pop,
    Drop,
}

impl Render for StashView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("stash-view")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.message).small()))
                    .child(
                        Button::new("stash-save")
                            .small()
                            .outline()
                            .icon(IconName::Inbox)
                            .label("Stash changes")
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
            .when(self.stashes.is_empty(), |this| {
                this.child(
                    div()
                        .py_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No stashes. Stashing puts your changes (new files too) aside for later."),
                )
            })
            .children(self.stashes.clone().iter().map(|stash| self.render_stash(stash, cx)))
    }
}
