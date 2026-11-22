//! The app whose lines Logcat shows: every app, the open project's, or one
//! of those installed, found by typing a part of its name. A phone has a
//! hundred apps or more, and the one wanted is seldom near the top of a
//! list of them.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div,
};

use crate::logcat::MINE;

/// An app was picked: `None` for every app, [`MINE`] for the open project's,
/// else a package.
pub(crate) struct AppPicked(pub Option<String>);

pub(crate) struct AppPicker {
    search: Entity<InputState>,
    /// The apps installed.
    packages: Vec<String>,
    /// The open project has apps of its own to offer.
    has_project: bool,
    /// What is picked now, as [`AppPicked`] says it.
    current: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AppPicked> for AppPicker {}

impl AppPicker {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find an app…"));
        let subscriptions =
            vec![
                cx.subscribe(&search, |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => cx.notify(),
                    // Enter takes the first app found.
                    InputEvent::PressEnter { .. } => {
                        if let Some(first) = this.found(cx).into_iter().next() {
                            cx.emit(AppPicked(Some(first)));
                        }
                    }
                    _ => {}
                }),
            ];
        Self {
            search,
            packages: Vec::new(),
            has_project: false,
            current: None,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_packages(&mut self, packages: Vec<String>, cx: &mut Context<Self>) {
        self.packages = packages;
        cx.notify();
    }

    pub(crate) fn set_has_project(&mut self, has_project: bool, cx: &mut Context<Self>) {
        self.has_project = has_project;
        cx.notify();
    }

    /// The picker opens, with `current` picked: an empty search, ready to
    /// be typed in.
    pub(crate) fn opened(
        &mut self,
        current: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.current = current;
        let search = self.search.clone();
        // Once it is drawn: there is nothing to focus before.
        window.defer(cx, move |window, cx| {
            search.update(cx, |search, cx| {
                search.set_value("", window, cx);
                search.focus(window, cx);
            })
        });
        cx.notify();
    }

    /// The installed apps whose name has what is typed in it, any case.
    fn found(&self, cx: &gpui_kit::App) -> Vec<String> {
        matching(&self.packages, &self.search.read(cx).value())
    }

    fn row(
        &self,
        id: SharedString,
        label: String,
        pick: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let picked = self.current == pick;
        h_flex()
            .id(id)
            .flex_none()
            .h_7()
            .px_2()
            .gap_2()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .text_sm()
            .hover(|style| style.bg(cx.theme().list_hover))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(picked, |this| {
                this.child(
                    Icon::new(IconName::Check)
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                )
            })
            // Findable in UI tests and by the automation socket.
            .test_support()
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(AppPicked(pick.clone()))))
    }
}

/// The names in `packages` with `typed` in them, any case; all of them
/// when nothing is typed.
fn matching(packages: &[String], typed: &str) -> Vec<String> {
    let typed = typed.trim().to_lowercase();
    packages
        .iter()
        .filter(|package| typed.is_empty() || package.to_lowercase().contains(&typed))
        .cloned()
        .collect()
}

impl Render for AppPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let searching = !self.search.read(cx).value().trim().is_empty();
        let found = self.found(cx);
        let none = found.is_empty();
        v_flex()
            .w(ph_ui::rems_from_px(300.))
            .gap_1()
            .child(
                Input::new(&self.search).small().prefix(
                    Icon::new(IconName::Search)
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                ),
            )
            .child(
                v_flex()
                    .id("logcat-app-list")
                    .max_h(ph_ui::rems_from_px(320.))
                    .overflow_y_scroll()
                    // The two that are no one app, unless one is searched for.
                    .when(!searching, |this| {
                        this.child(self.row("logcat-app-all".into(), "All apps".into(), None, cx))
                            .when(self.has_project, |this| {
                                this.child(self.row(
                                    "logcat-app-mine".into(),
                                    "This project".into(),
                                    Some(MINE.into()),
                                    cx,
                                ))
                            })
                            .child(div().flex_none().h_px().my_1().bg(cx.theme().border))
                    })
                    .children(found.into_iter().map(|package| {
                        self.row(
                            format!("logcat-app-{package}").into(),
                            package.clone(),
                            Some(package),
                            cx,
                        )
                    }))
                    .when(none, |this| {
                        this.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if searching {
                                    "No app of that name is installed."
                                } else {
                                    "No apps installed."
                                }),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_are_found_by_a_part_of_their_name() {
        let packages = vec![
            "com.example.app".to_string(),
            "com.Example.Notes".to_string(),
            "org.other".to_string(),
        ];
        assert_eq!(matching(&packages, ""), packages);
        assert_eq!(matching(&packages, " EXAMPLE "), packages[..2]);
        assert_eq!(matching(&packages, "notes"), packages[1..2]);
        assert!(matching(&packages, "missing").is_empty());
    }
}
