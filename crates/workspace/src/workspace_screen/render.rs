//! How a workspace's screen is drawn: the chat's island (with what finished
//! while away) and the result pane's, with its row of names and what the
//! one in front shows (the review of the changes, the browser or a phone,
//! the terminal, the files and their editor, another tool).

use super::*;

impl WorkspaceScreen {
    /// The chat, under the tabs of the workspace's open chats and what
    /// finished while the workspace was away.
    pub(super) fn render_chat(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.render_chat_tabs(cx);
        if tabs.is_none() && self.away.is_none() {
            return self.chat.clone().cached(filled()).into_any_element();
        }
        v_flex()
            .size_full()
            .children(tabs)
            .children(self.away.as_ref().map(|away| self.render_away(away, cx)))
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
                Some(files) => files.cached(filled()).into_any_element(),
                None => div().into_any_element(),
            },
            // A tool this build leaves out never starts (a layout naming it
            // is not loaded; this is the last guard).
            Pane::Tool(tab) if !tab.in_build() => div().into_any_element(),
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

    /// The names over the result pane and, among the workspace's other
    /// tools, those under *More*.
    fn result_names(&self, cx: &App) -> (Vec<ResultTab>, Vec<ContextTab>) {
        let enabled = AppState::enabled_tabs(cx);
        let on = |tab: ContextTab| enabled.contains(&tab);
        let front = self.layout.result_open.then(|| self.layout.tab());
        let mut names = Vec::new();
        let git = on(ContextTab::Changes);
        let changes =
            git && (self.is_relevant(ContextTab::Changes) || front == Some(ResultTab::Git));
        if changes {
            names.push(ResultTab::Git);
        }
        if on(ContextTab::Browser) || on(ContextTab::Devices) {
            names.push(ResultTab::Preview);
        }
        if on(ContextTab::Terminal) {
            names.push(ResultTab::Terminal);
        }
        // An open file shows also where the Files tool is switched off.
        if (on(ContextTab::Files) && self.is_relevant(ContextTab::Files))
            || front == Some(ResultTab::Files)
            || self.has_open_files(cx)
        {
            names.push(ResultTab::Files);
        }
        let mut more = Vec::new();
        for tab in enabled {
            match ResultTab::of(tab) {
                // The one in front has a name of its own, beside the four.
                other @ ResultTab::Other(_) if front == Some(other) => names.push(other),
                ResultTab::Other(_) => more.push(tab),
                _ => {}
            }
        }
        // Git without a repository (as far as was seen when the folders
        // were looked at) has no name, but is not out of reach: a folder
        // may have become one since.
        if git && !changes && self.project.is_workspace() {
            more.push(ContextTab::Changes);
        }
        // Those the workspace has a use for lead the menu.
        more.sort_by_key(|tab| !self.is_relevant(*tab));
        (names, more)
    }

    /// The row over the result pane: what it can show, and what the tool
    /// in front adds (the browser or a phone; the tree beside the editor).
    fn render_result_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let space = cx.entity();
        let (names, more) = self.result_names(cx);
        let front = self.layout.tab();
        let changes = self
            .changes
            .get(&self.git_folder)
            .map(|changes| changes.read(cx).change_count())
            .unwrap_or_default();
        let tabs = names.into_iter().map(|tab| {
            let space = space.clone();
            let label = match tab {
                ResultTab::Git if changes > 0 => format!("{} {changes}", tab.label()),
                _ => tab.label().to_string(),
            };
            ph_ui::PillTab::new(
                format!("context-tab-{}", tab.key()),
                label,
                move |window, cx| {
                    space.update(cx, |space, cx| space.show_result_tab(tab, window, cx))
                },
            )
            .name(tab.label())
            .icon(Icon::new(tab.icon()))
            .selected(tab == front)
        });
        let more_menu = (!more.is_empty()).then(|| {
            let space = space.clone();
            Button::new("more-tools")
                .ghost()
                .small()
                .icon(IconName::Ellipsis)
                .tooltip("More tools")
                .accessibility_label("More tools")
                .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                    more.iter().fold(menu, |menu, tab| {
                        let (space, tab) = (space.clone(), *tab);
                        let item = gpui_kit::component::menu::PopupMenuItem::new(tab.label())
                            .icon(Icon::new(tab.icon()))
                            .on_click(move |_, window, cx| {
                                space.update(cx, |space, cx| space.show_tab(tab, window, cx))
                            });
                        menu.item(item)
                    })
                })
        });
        let enabled = AppState::enabled_tabs(cx);
        // Preview: the browser or a phone, when the build has both; the
        // one switch other panes have for two views of one thing (a
        // file's changes, unified or split).
        let preview = (front == ResultTab::Preview
            && enabled.contains(&ContextTab::Browser)
            && enabled.contains(&ContextTab::Devices))
        .then(|| {
            let space = space.clone();
            gpui_kit::component::tab::TabBar::new("preview-kind")
                .segmented()
                .small()
                .selected_index(usize::from(self.layout.tool == ContextTab::Devices))
                .on_click(move |ix: &usize, window, cx| {
                    let tool = if *ix == 1 {
                        ContextTab::Devices
                    } else {
                        ContextTab::Browser
                    };
                    space.update(cx, |space, cx| space.show_tab(tool, window, cx))
                })
                .child(gpui_kit::component::tab::Tab::new().label("Browser"))
                .child(gpui_kit::component::tab::Tab::new().label("Phone"))
                .into_any_element()
        });
        // Files: the tree makes way for the editor.
        let tree = (front == ResultTab::Files && self.has_tree(cx)).then(|| {
            let label = if self.layout.tree_open {
                "Hide the folders' tree"
            } else {
                "Show the folders' tree"
            };
            Button::new("toggle-file-tree")
                .ghost()
                .small()
                .icon(if self.layout.tree_open {
                    IconName::PanelLeftClose
                } else {
                    IconName::PanelLeftOpen
                })
                .tooltip(label)
                .accessibility_label(label)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_tree(cx)))
                .into_any_element()
        });
        h_flex()
            .id("context-tabs")
            .flex_none()
            .px_2()
            .py_1p5()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(ph_ui::PillTabs::new("result-tabs", tabs.collect())),
            )
            .children(more_menu)
            .children(preview)
            .children(tree)
            .into_any_element()
    }

    /// The Files tool is in this build and the workspace has folders to
    /// list.
    fn has_tree(&self, cx: &App) -> bool {
        self.project.is_workspace() && AppState::enabled_tabs(cx).contains(&ContextTab::Files)
    }

    /// What the result pane shows under its names.
    fn render_result(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::component::resizable::{h_resizable, resizable_panel};
        use ph_theme::layout::{SIDE_LIST_MAX, SIDE_LIST_MIN};
        match self.layout.tool {
            // The folders' tree and, beside it, the files open from it.
            ContextTab::Files => {
                let editor = self.render_pane(Pane::Editor, window, cx);
                if !(self.layout.tree_open && self.has_tree(cx)) {
                    return editor;
                }
                let tree = self.render_pane(Pane::Tool(ContextTab::Files), window, cx);
                h_resizable("files-split")
                    .with_state(&self.tree_split)
                    .child(
                        resizable_panel()
                            .size(ph_ui::scaled_px(240., window))
                            .size_range(SIDE_LIST_MIN..SIDE_LIST_MAX)
                            .child(tree),
                    )
                    .child(resizable_panel().child(editor))
                    .into_any_element()
            }
            tool => self.render_pane(Pane::Tool(tool), window, cx),
        }
    }

    /// The chat folded away: a strip that says what it is doing and brings
    /// it back.
    fn render_chat_strip(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let status = crate::status::WorkspaceStatus::of(self.project.id, cx);
        let theme = cx.theme();
        let state = if status.waiting > 0 {
            Some(theme.warning)
        } else if status.working > 0 {
            Some(theme.info)
        } else if status.unseen > 0 {
            Some(theme.success)
        } else {
            None
        };
        let label = format!(
            "Show the chat ({})",
            ph_ui::platform::shortcut("secondary-b")
        );
        div()
            .flex_none()
            .w(ph_ui::rems_from_px(44.))
            .h_full()
            .child(
                crate::frame::island(window, cx).child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .py_2()
                        .gap_2()
                        .child(
                            Button::new("unfold-chat")
                                .ghost()
                                .small()
                                .icon(Icons::MessagesSquare)
                                .tooltip(label.clone())
                                .accessibility_label(label)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.show_chat(window, cx)),
                                ),
                        )
                        .children(state.map(|color| {
                            div().size(ph_ui::rems_from_px(8.)).rounded_full().bg(color)
                        })),
                ),
            )
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
        // Each tool is drawn again only when it changes, not on every
        // keystroke in the editor next to it.
        fn cached<V: Render>(panel: &Option<Entity<V>>) -> Option<AnyElement> {
            panel
                .clone()
                .map(|panel| panel.cached(filled()).into_any_element())
        }
        let panel = match tab {
            ContextTab::Files => cached(&self.file_tree),
            ContextTab::Changes => self.changes.get(&self.git_folder).cloned().map(|panel| {
                self.with_folder_picker(panel.cached(filled()).into_any_element(), cx)
            }),
            ContextTab::Board => cached(&self.board),
            ContextTab::Terminal => cached(&self.terminal),
            ContextTab::Browser => cached(&self.browser),
            ContextTab::Devices => cached(&self.devices),
            ContextTab::Runs => cached(&self.runs),
            ContextTab::Pulls => self.pulls.get(&self.git_folder).cloned().map(|panel| {
                self.with_folder_picker(panel.cached(filled()).into_any_element(), cx)
            }),
            ContextTab::Tests => cached(&self.tests),
            ContextTab::Logcat => cached(&self.logcat),
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
        let folders: Vec<(usize, String, std::path::PathBuf)> = roots
            .list()
            .iter()
            .enumerate()
            .skip(usize::from(self.project.empty))
            .map(|(ix, root)| (ix, root.name.clone(), root.path.clone()))
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
                                // Looked up as the menu opens, not on every frame.
                                folders.iter().fold(menu, |menu, (ix, name, path)| {
                                    let (space, ix) = (space.clone(), *ix);
                                    let label = if ph_git::is_repository(path) {
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
        use gpui_kit::component::resizable::{h_resizable, resizable_panel};
        let gap = crate::frame::gap();
        let chat = (!self.layout.chat_folded).then(|| {
            crate::frame::island(window, cx)
                .child(self.render_chat(cx))
                .into_any_element()
        });
        let result = self.layout.result_open.then(|| {
            let bar = self.render_result_bar(cx);
            let content = self.render_result(window, cx);
            crate::frame::island(window, cx)
                .child(bar)
                .child(div().flex_1().min_h_0().child(content))
                .into_any_element()
        });
        let strip = self
            .layout
            .chat_folded
            .then(|| self.render_chat_strip(window, cx));
        let panes = match (chat, result) {
            // Side by side, the line between them dragged to share the
            // room: the chat starts with two fifths of it.
            (Some(chat), Some(result)) => {
                let room = window.viewport_size().width;
                // In a small window neither pane may ask for more than
                // its part: the two least widths together would not fit.
                let chat_min = ph_theme::layout::CHAT_MIN.min(room * 0.45);
                let result_min = ph_theme::layout::CONTEXT_PANEL_MIN.min(room * 0.4);
                let chat_width = match self.layout.chat_share {
                    // Where the line between them was last left.
                    Some(share) => (room * share).max(chat_min),
                    None => {
                        (room * 0.4).clamp(chat_min, ph_ui::scaled_px(640., window).max(chat_min))
                    }
                };
                let screen = cx.entity().downgrade();
                h_resizable(("space-split", self.project.id as u64))
                    .on_resize(move |state, window, cx| {
                        // Of the window's width, as the size it starts
                        // with is worked out above.
                        let room = f32::from(window.viewport_size().width);
                        let chat = state.read(cx).sizes().first().copied();
                        if let Some(chat) = chat.filter(|_| room > 0.) {
                            let share = f32::from(chat) / room;
                            _ = screen.update(cx, |screen, cx| screen.set_chat_share(share, cx));
                        }
                    })
                    .child(
                        resizable_panel()
                            .size(chat_width)
                            .size_range(chat_min..gpui_kit::Pixels::MAX)
                            .child(div().size_full().pr(gap / 2.).child(chat)),
                    )
                    .child(
                        resizable_panel()
                            .size_range(result_min..gpui_kit::Pixels::MAX)
                            .child(div().size_full().pl(gap / 2.).child(result)),
                    )
                    .into_any_element()
            }
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => div().into_any_element(),
        };
        h_flex()
            .size_full()
            .items_stretch()
            .pr_2()
            .pb(gap)
            .gap(gap)
            .children(strip)
            .child(div().flex_1().min_w_0().h_full().child(panes))
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

/// A cached tool's layout: it fills the pane it is drawn in.
fn filled() -> gpui_kit::StyleRefinement {
    gpui_kit::StyleRefinement::default().size_full()
}
