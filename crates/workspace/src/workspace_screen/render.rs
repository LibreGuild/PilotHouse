//! How a workspace's screen is drawn: what each pane shows (the chat, with
//! what finished while away; the editor; each tool), the dock they are
//! arranged in, and the icon bar.

use super::*;

impl WorkspaceScreen {
    /// The chat, under what finished while the workspace was away.
    pub(super) fn render_chat(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(away) = &self.away else {
            return self.chat.clone().into_any_element();
        };
        v_flex()
            .size_full()
            .child(self.render_away(away, cx))
            .child(div().flex_1().min_h_0().child(self.chat.clone()))
            .into_any_element()
    }

    /// "While you were away": each chat that finished and the test run, one
    /// click to see it.
    pub(super) fn render_away(&self, away: &Away, cx: &mut Context<Self>) -> impl IntoElement {
        let chats = away.chats.iter().filter_map(|id| {
            let session = self.sessions.iter().find(|session| session.id == *id)?;
            let id = *id;
            Some(
                Button::new(("away-chat", id as u64))
                    .outline()
                    .xsmall()
                    .icon(Icons::MessageSquare)
                    .label(short(&session.title, 32))
                    .tooltip("Show this chat")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_session(id, cx);
                        this.dismiss_away(cx);
                    })),
            )
        });
        let tests = away.tests.then(|| {
            let failed = ph_uitest::runner::TestRunHub::existing(self.project.id, cx)
                .is_some_and(|runs| runs.read(cx).failed());
            Button::new("away-tests")
                .outline()
                .xsmall()
                .icon(Icons::FlaskConical)
                .label(if failed {
                    "UI test failed"
                } else {
                    "UI test run done"
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_tab(ContextTab::Tests, window, cx);
                    this.dismiss_away(cx);
                }))
        });
        h_flex()
            .id("while-away")
            .flex_none()
            .flex_wrap()
            .gap_1p5()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.5))
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child("While you were away"),
            )
            .children(chats)
            .children(tests)
            .child(div().flex_1())
            .child(
                Button::new("away-dismiss")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Dismiss")
                    .accessibility_label("Dismiss")
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_away(cx))),
            )
    }

    pub(super) fn dismiss_away(&mut self, cx: &mut Context<Self>) {
        self.away = None;
        cx.notify();
    }

    /// What pane `pane` shows. A tool's panel is made here the first time
    /// its pane is drawn (a saved layout may show tools nobody opened yet).
    pub(super) fn render_pane(
        &mut self,
        pane: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = match pane {
            Pane::Chat => self.render_chat(cx),
            Pane::Editor => match self.files.clone() {
                Some(files) => files.into_any_element(),
                None => div().into_any_element(),
            },
            Pane::Tool(tab) => {
                self.ensure_panel(tab, window, cx);
                self.render_tab_content(tab, cx)
            }
        };
        // Absolutely filled, so each panel gets an exact height (a
        // percentage height inside a flex item resolves too tall).
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .child(div().absolute().inset_0().child(content))
            .into_any_element()
    }

    /// The tools down the window's right edge: icons, with names in their
    /// tooltips (or under them, in Settings).
    pub(super) fn render_tool_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let space = cx.entity();
        let labels = ph_theme::preferences::preferences(cx).tool_labels;
        let changes = self
            .changes
            .get(&self.git_folder)
            .map(|changes| changes.read(cx).change_count())
            .unwrap_or_default();
        let (shown_tabs, more): (Vec<ContextTab>, Vec<ContextTab>) = AppState::enabled_tabs(cx)
            .into_iter()
            .partition(|tab| self.is_relevant(*tab));
        let more_menu = (!more.is_empty()).then(|| {
            let space = space.clone();
            Button::new("more-tools")
                .ghost()
                .small()
                .icon(IconName::Ellipsis)
                .tooltip("More tools")
                .accessibility_label("More tools")
                .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    more.iter().fold(menu, |menu, tab| {
                        let (space, tab) = (space.clone(), *tab);
                        menu.item(
                            gpui_kit::component::menu::PopupMenuItem::new(tab.label())
                                .icon(Icon::new(tab.icon()))
                                .on_click(move |_, window, cx| {
                                    space.update(cx, |space, cx| space.show_tab(tab, window, cx))
                                }),
                        )
                    })
                })
        });
        v_flex()
            .id("context-tabs")
            .flex_none()
            .w(tool_rail_width(labels))
            .h_full()
            .py_1()
            .gap_0p5()
            .items_center()
            .children(shown_tabs.into_iter().map(|tab| {
                let selected = self.shown.contains(&Pane::Tool(tab));
                let space = space.clone();
                let badge = (tab == ContextTab::Changes && changes > 0).then(|| {
                    div()
                        .absolute()
                        .top(px(3.))
                        .right(px(3.))
                        .min_w(px(14.))
                        .h(px(14.))
                        .px(px(3.))
                        .rounded_full()
                        .bg(cx.theme().secondary_active)
                        .text_color(cx.theme().foreground)
                        .text_size(px(9.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if changes > 99 {
                            "99+".to_string()
                        } else {
                            changes.to_string()
                        })
                });
                let hint = tab_shortcut(tab);
                v_flex()
                    .id(SharedString::from(format!("context-tab-{}", tab.key())))
                    .role(gpui_kit::accesskit::Role::Tab)
                    .aria_label(tab.label())
                    .relative()
                    .w(tool_rail_width(labels) - px(6.))
                    .when(labels, |this| this.py_1p5().gap_1())
                    .when(!labels, |this| this.h(px(34.)))
                    .items_center()
                    .justify_center()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .text_color(if selected {
                        cx.theme().foreground
                    } else {
                        cx.theme().muted_foreground
                    })
                    .when(selected, |this| this.bg(cx.theme().secondary_hover))
                    .when(!selected, |this| {
                        this.hover(|style| {
                            style
                                .bg(cx.theme().secondary_hover)
                                .text_color(cx.theme().foreground)
                        })
                    })
                    .when(!labels, |this| {
                        let label = format!("{} ({hint})", tab.label());
                        this.tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(label.clone())
                                .build(window, cx)
                        })
                    })
                    .on_click(move |_, window, cx| {
                        space.update(cx, |space, cx| space.show_tab(tab, window, cx))
                    })
                    .child(Icon::new(tab.icon()).size(px(18.)))
                    .when(labels, |this| {
                        this.child(
                            div()
                                .text_size(px(10.))
                                .when(selected, |this| {
                                    this.font_weight(gpui_kit::FontWeight::MEDIUM)
                                })
                                .child(tab.label()),
                        )
                    })
                    .children(badge)
                    // Findable by label in UI tests and automation.
                    .test_support()
            }))
            .children(more_menu)
            .into_any_element()
    }

    pub(super) fn render_tab_content(&self, tab: ContextTab, cx: &mut Context<Self>) -> AnyElement {
        // Files, Git and PRs need a folder; say so instead of an empty tool.
        if self.project.folders().is_empty()
            && matches!(
                tab,
                ContextTab::Files | ContextTab::Changes | ContextTab::Pulls
            )
        {
            return ph_ui::EmptyState::new("No folders in this workspace")
                .icon(Icons::FolderPlus)
                .description(
                    "Add a folder to see its files, Git changes, branches and pull requests. Chat, the board, the browser and devices work without one.",
                )
                .action(
                    Button::new("empty-add-folder")
                        .primary()
                        .icon(Icons::FolderPlus)
                        .label("Add a folder")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::AddFolderToWorkspace), cx)
                        }),
                )
                .into_any_element();
        }
        let panel = match tab {
            ContextTab::Files => self.file_tree.clone().map(IntoElement::into_any_element),
            ContextTab::Changes => self
                .changes
                .get(&self.git_folder)
                .cloned()
                .map(|panel| self.with_folder_picker(panel.into_any_element(), cx)),
            ContextTab::Board => self.board.clone().map(IntoElement::into_any_element),
            ContextTab::Terminal => self.terminal.clone().map(IntoElement::into_any_element),
            ContextTab::Browser => self.browser.clone().map(IntoElement::into_any_element),
            ContextTab::Devices => self.devices.clone().map(IntoElement::into_any_element),
            ContextTab::Runs => self.runs.clone().map(IntoElement::into_any_element),
            ContextTab::Pulls => self
                .pulls
                .get(&self.git_folder)
                .cloned()
                .map(|panel| self.with_folder_picker(panel.into_any_element(), cx)),
            ContextTab::Tests => self.tests.clone().map(IntoElement::into_any_element),
        };
        panel.unwrap_or_else(|| div().into_any_element())
    }

    /// Git and PRs work on one folder at a time: with several folders, a
    /// row on top says which and switches it.
    pub(super) fn with_folder_picker(
        &self,
        panel: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let roots = self.roots();
        let folders: Vec<(usize, String, bool)> = roots
            .list()
            .iter()
            .enumerate()
            .skip(usize::from(self.project.empty))
            .map(|(ix, root)| (ix, root.name.clone(), ph_git::is_repository(&root.path)))
            .collect();
        if folders.len() < 2 {
            return panel;
        }
        let current = folders
            .iter()
            .find(|(ix, ..)| *ix == self.git_folder)
            .map(|(_, name, _)| name.clone())
            .unwrap_or_default();
        let space = cx.entity().downgrade();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Folder")
                    .child(
                        Button::new("git-folder")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Folder)
                            .label(current)
                            .dropdown_caret(true)
                            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                                folders.iter().fold(menu, |menu, (ix, name, repo)| {
                                    let (space, ix) = (space.clone(), *ix);
                                    let label = if *repo {
                                        name.clone()
                                    } else {
                                        format!("{name} (no Git)")
                                    };
                                    menu.item(
                                        gpui_kit::component::menu::PopupMenuItem::new(label)
                                            .on_click(move |_, window, cx| {
                                                _ = space.update(cx, |space, cx| {
                                                    space.set_git_folder(ix, window, cx)
                                                });
                                            }),
                                    )
                                })
                            }),
                    ),
            )
            .child(div().flex_1().min_h_0().child(panel))
            .into_any_element()
    }
}

impl Render for WorkspaceScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.reveal_pending) {
            self.panes.show(Pane::Chat, window, cx);
        }
        // The panes sit on a rounded card between the sidebar and the tool
        // rail, set off from the window's chrome.
        h_flex()
            .size_full()
            .pr_1()
            .pb_2()
            .items_stretch()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(ph_ui::Surface::new().child(self.panes.area.clone())),
            )
            .child(self.render_tool_rail(cx))
    }
}

/// The tool icons' column: icons alone, or wider with their names.
pub(super) fn tool_rail_width(labels: bool) -> gpui_kit::Pixels {
    if labels { px(64.) } else { px(42.) }
}

/// The shortcut that shows `tab` (see `actions::bind_keys`).
pub(super) fn tab_shortcut(tab: ContextTab) -> String {
    let key = match tab {
        ContextTab::Files => 1,
        ContextTab::Changes => 2,
        ContextTab::Board => 3,
        ContextTab::Terminal => 4,
        ContextTab::Browser => 5,
        ContextTab::Devices => 6,
        ContextTab::Runs => 7,
        ContextTab::Pulls => 8,
        ContextTab::Tests => 9,
    };
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// `text` cut to `max` characters, with an ellipsis.
pub(super) fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}
