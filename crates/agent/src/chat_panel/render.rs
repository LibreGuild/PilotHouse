//! How the chat column is drawn: header, transcript or the empty state,
//! attachments and the composer with its pills.

use super::*;

/// From this width of the panel (pixels at the default interface size) the
/// tabs and the chat's own controls share one row.
const ONE_ROW: f32 = 760.;

impl ChatPanel {
    /// Where the chat's agent works, before its title: a workspace (in its
    /// colour, and the folder when not the main one), a repository, or
    /// the Chats space, which has no folder.
    fn render_place(&self, cx: &App) -> AnyElement {
        let project = &self.project;
        let folder = self.active.read(cx).folder().to_path_buf();
        let (icon, color, label, tip): (Option<Icons>, Option<gpui_kit::Hsla>, String, String) =
            if project.read_only {
                (
                    Some(Icons::Github),
                    None,
                    project.name.clone(),
                    format!("Reads {} in {}'s own copy", project.name, ph_brand::NAME),
                )
            } else if project.empty {
                (
                    Some(Icons::MessagesSquare),
                    None,
                    project.name.clone(),
                    "No folder yet: the agent works in a folder of this workspace's own".into(),
                )
            } else {
                let sub = (folder != project.path)
                    .then(|| {
                        folder
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .flatten();
                (
                    None,
                    Some(ph_ui::workspace_color(project.shade, cx.theme().is_dark())),
                    match sub {
                        Some(sub) => format!("{} › {sub}", project.name),
                        None => project.name.clone(),
                    },
                    format!("Works in {}", ph_ui::platform::display_path(&folder)),
                )
            };
        h_flex()
            .id("chat-place")
            .flex_none()
            .max_w(ph_ui::rems_from_px(220.))
            .h_6()
            .px_2()
            .gap_1p5()
            .rounded(cx.theme().radius)
            .bg(cx.theme().muted)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .children(icon.map(|icon| Icon::new(icon).xsmall()))
            .children(color.map(|color| {
                div()
                    .flex_none()
                    .size(ph_ui::rems_from_px(8.))
                    .rounded_full()
                    .bg(color)
            }))
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    /// The top of the chat. Where the panel is a workspace's chat it is two
    /// rows, each about one thing: which chats there are (their tabs, with
    /// past chats and a new one at the end), then the chat in front (its
    /// agent, title, what it has used, and a menu for what is done with it
    /// once in a while). Anywhere else (a review's chat, the quick chat) it
    /// is the second row alone, with the chat's place before its title.
    ///
    /// The two rows had eight small things in the second, and one row had
    /// no room for the tabs' names in a narrow panel.
    pub(super) fn render_header(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tabs = self.tabs.clone().and_then(|tabs| tabs(window, cx));
        let tabbed = tabs.is_some();
        // With the room, the tabs and the rest are one row (the tab in
        // front says the title); two rows are for a narrow panel, where
        // one would leave the tabs no room for their names.
        let (width, probe) = ph_ui::measure::width_of("chat-header-width", window, cx);
        let wide = tabbed
            && width.is_some_and(|width| ph_ui::measure::design_px(width, window) >= ONE_ROW);
        let side_of = self.side_of().cloned();
        // The session in front: a chat, or one in its CLI form (the chat
        // behind it is then not what the header is about).
        let cli = self
            .terminal_session
            .as_ref()
            .map(|shown| (shown.agent, SharedString::from(shown.session.title.clone())));
        let thread = self.active.read(cx);
        let usage = match cli {
            Some(_) => None,
            None => {
                crate::usage_meter::render(thread.usage(), cx).map(IntoElement::into_any_element)
            }
        };
        let (dot, label) = match (&cli, thread.status()) {
            // Its state is on its tab and its card, as the CLI reports it.
            (Some(_), _) => (cx.theme().muted_foreground.opacity(0.5), "In its CLI"),
            (None, ThreadStatus::Working) => (cx.theme().success, "Working"),
            (None, ThreadStatus::Connecting) => (cx.theme().warning, "Starting"),
            (None, ThreadStatus::Idle) if thread.has_started() => {
                (cx.theme().success.opacity(0.6), "Ready")
            }
            (None, ThreadStatus::Idle) => (cx.theme().muted_foreground.opacity(0.5), "Idle"),
        };
        let spec = cli.as_ref().map_or(thread.agent(), |(agent, _)| *agent);
        let shown_title = match &cli {
            Some((_, title)) => title.clone(),
            None => thread.title(),
        };
        let is_cli = cli.is_some();
        // The agent's mark with a dot for its state; the words are in the
        // tooltip, to keep the header small.
        let tip = format!("{} · {label}", spec.name);
        let agent = div()
            .id("chat-agent-state")
            .flex_none()
            .relative()
            .p_0p5()
            .aria_label(SharedString::from(tip.clone()))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .child(spec.mark(ph_ui::rems_from_px(16.)))
            .child(
                div()
                    .absolute()
                    .right(px(-1.))
                    .bottom(px(-1.))
                    .size(ph_ui::rems_from_px(7.))
                    .rounded_full()
                    .border_1()
                    .border_color(cx.theme().background)
                    .bg(dot),
            );
        let title = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_sm()
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .child(shown_title);
        let form = self.render_form_switch(spec, cx);
        // A chat's menu (how much to show, compact, hand on) is the
        // chat's: in its CLI those are the CLI's own commands.
        let more = (!is_cli).then(|| self.render_more(cx).into_any_element());
        let back = side_of.map(|parent| {
            Button::new("side-chat-back")
                .ghost()
                .xsmall()
                .icon(IconName::ArrowLeft)
                .tooltip(format!("Back to “{}”", parent.title))
                .accessibility_label("Back to the chat")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.side = None;
                    cx.emit(ChatPanelEvent::SelectSession(Some(parent.id)));
                }))
        });
        if wide {
            // One row: which chats, then this chat's agent, what it has
            // used and its menu, then the chats not among the tabs and a
            // new one.
            return v_flex().flex_none().relative().child(probe).child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .pl_2()
                    .pr_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .children(back)
                    .children(tabs.map(|tabs| div().flex_1().min_w_0().child(tabs)))
                    .child(agent)
                    .children(form)
                    .children(usage)
                    .children(more)
                    .when(self.history_menu, |this| {
                        this.child(self.render_history(cx))
                    }),
            );
        }
        // Which chats: the tabs, and at their end the chats not among
        // them and a new one.
        let which = tabs.map(|tabs| {
            h_flex()
                .flex_none()
                .h_9()
                .pl_2()
                .pr_3()
                .gap_2()
                .child(div().flex_1().min_w_0().child(tabs))
                .when(self.history_menu, |this| {
                    this.child(self.render_history(cx))
                })
        });
        let place = (!tabbed).then(|| self.render_place(cx));
        // This chat.
        let this_chat = h_flex()
            .flex_none()
            .h_9()
            .px_4()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .children(back)
            // In a workspace the agent's mark leads (the window's bar
            // names the place); elsewhere the place does, and the mark
            // follows the title as it did.
            .when(tabbed, |this| this.child(agent))
            .children(place)
            .child(title)
            .children(form)
            .children(usage)
            .children(more)
            .when(!tabbed && self.history_menu, |this| {
                this.child(self.render_history(cx))
            });
        v_flex()
            .flex_none()
            .relative()
            .child(probe)
            .children(which)
            .child(this_chat)
    }

    /// Which of the agent's accounts the session runs in, for an agent
    /// that has more than its own (Settings → Agents). Picked before
    /// something is said; after that it only says which, since the agent
    /// keeps the conversation in that account's files.
    fn render_account(&self, agent: &'static AgentSpec, cx: &Context<Self>) -> Option<AnyElement> {
        let profile = crate::profiles::Profiles::known().of(agent.id);
        if profile.accounts.is_empty() {
            return None;
        }
        let thread = self.active.read(cx);
        let current = thread.account().to_string();
        let name: SharedString = match profile.account(&current) {
            Some(account) => account.name.clone().into(),
            None if current.is_empty() => "Default".into(),
            // Removed in Settings since: the session is still in it.
            None => current.clone().into(),
        };
        if !thread.can_change_account() {
            return Some(
                div()
                    .id("session-account")
                    .flex_none()
                    .px_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "A session stays in the account it started in",
                        )
                        .build(window, cx)
                    })
                    .child(name)
                    .into_any_element(),
            );
        }
        let panel = cx.weak_entity();
        let accounts: Vec<(String, SharedString)> =
            std::iter::once((String::new(), SharedString::from("Default")))
                .chain(
                    profile
                        .accounts
                        .iter()
                        .map(|account| (account.id.clone(), account.name.clone().into())),
                )
                .collect();
        Some(
            Button::new("choose-account")
                .ghost()
                .xsmall()
                .label(name)
                .tooltip(format!("The {} account this session runs in", agent.name))
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    accounts.iter().fold(menu, |menu, (id, name)| {
                        let (panel, id) = (panel.clone(), id.clone());
                        menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(id == current)
                                .on_click(move |_, _, cx| {
                                    _ = panel.update(cx, |this, cx| {
                                        this.active
                                            .update(cx, |thread, cx| thread.set_account(&id, cx));
                                        cx.notify();
                                    });
                                }),
                        )
                    })
                })
                .into_any_element(),
        )
    }

    /// Chat | CLI: the session's two forms (see [`Form`]), where a session
    /// has both: a workspace's chat, with an agent that has a CLI. The
    /// conversation is the same; this picks what it is typed into.
    fn render_form_switch(
        &self,
        agent: &'static AgentSpec,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use gpui_kit::component::tab::{Tab, TabBar};
        if self.cli_view.is_none()
            || self.context.is_some()
            || self.project.read_only
            || !ph_brand::enabled(ph_brand::Feature::Terminal)
            || !agents::has_cli(agent.id)
        {
            return None;
        }
        let form = self.form();
        Some(
            TabBar::new("session-form")
                .segmented()
                .xsmall()
                .flex_none()
                .selected_index(usize::from(form == Form::Cli))
                .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                    let to = if *ix == 1 { Form::Cli } else { Form::Chat };
                    // Shown as it is until whoever switches it has.
                    if to != this.form() {
                        cx.emit(ChatPanelEvent::ShowAs(to));
                    }
                    cx.notify();
                }))
                .child(Tab::new().label("Chat"))
                .child(Tab::new().label("CLI"))
                .into_any_element(),
        )
    }

    /// What is done with a chat once in a while, in one menu: how much of
    /// the agent's work to show, making the chat shorter, and going on
    /// with another agent.
    fn render_more(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::view_mode::ViewMode;
        let thread = self.active.read(cx);
        let current_mode = ViewMode::current(cx);
        let current_agent = thread.agent().id;
        // Only a chat that has started and is at rest can be made shorter
        // or handed on.
        let rested = thread.has_started() && *thread.status() == ThreadStatus::Idle;
        let store = self.store.clone();
        let panel = cx.weak_entity();
        Button::new("chat-more")
            .ghost()
            .xsmall()
            .icon(IconName::Ellipsis)
            .tooltip("View, compact, continue with another agent")
            .accessibility_label("Chat menu")
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                let menu = ViewMode::ALL.into_iter().fold(
                    menu.label("How much of the agent's work to show"),
                    |menu, mode| {
                        let store = store.clone();
                        menu.item(
                            PopupMenuItem::new(mode.label())
                                .checked(mode == current_mode)
                                .on_click(move |_, window, cx| {
                                    mode.choose(&store, cx);
                                    window.refresh();
                                }),
                        )
                    },
                );
                if !rested {
                    return menu;
                }
                let compact = panel.clone();
                let menu = menu
                    .separator()
                    .item(
                        PopupMenuItem::new("Compact chat")
                            .icon(Icons::FoldVertical)
                            .on_click(move |_, window, cx| {
                                _ = compact.update(cx, |panel, cx| panel.compact(window, cx));
                            }),
                    )
                    .separator()
                    .label("Continue with");
                agents::AGENTS
                    .iter()
                    .filter(|agent| agent.id != current_agent)
                    .fold(menu, |menu, agent| {
                        let panel = panel.clone();
                        menu.item(
                            PopupMenuItem::new(agent.name).on_click(move |_, window, cx| {
                                _ = panel.update(cx, |panel, cx| panel.hand_off(agent, window, cx));
                            }),
                        )
                    })
            })
    }

    /// The IDE layout has no chat list: past chats are in this menu, with a
    /// new-chat button beside it.
    pub(super) fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (store, project_id) = (self.store.clone(), self.project.id);
        // A chat about something lists the chats about it.
        let only = self
            .context
            .as_ref()
            .map(|context| context.cwd.to_string_lossy().into_owned());
        let active = self.active.read(cx).session().map(|session| session.id);
        let panel = cx.weak_entity();
        let outside = self.cli_view.is_some() && self.context.is_none();
        h_flex()
            .flex_none()
            .gap_0p5()
            .child(
                Button::new("chat-history")
                    .ghost()
                    .xsmall()
                    .icon(Icons::RotateCcwClock)
                    .tooltip("Past chats")
                    .accessibility_label("Past chats")
                    .menu(gpui_kit::Anchor::TopRight, move |menu, window, cx| {
                        let sessions: Vec<_> = store
                            .sessions(project_id)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|session| {
                                only.is_none() || session.cwd.as_ref() == only.as_ref()
                            })
                            .collect();
                        // An agent's own sessions here that did not start
                        // in this app (in another terminal, another
                        // editor): picked from the agent's list.
                        let outside = outside.then(|| {
                            let panel = panel.clone();
                            PopupMenu::build(window, cx, move |menu, _, _| {
                                AGENTS
                                    .iter()
                                    .filter(|agent| agents::has_cli(agent.id))
                                    .fold(menu, |menu, agent| {
                                        let panel = panel.clone();
                                        menu.item(PopupMenuItem::new(agent.name).on_click(
                                            move |_, _, cx| {
                                                _ = panel.update(cx, |_, cx| {
                                                    cx.emit(ChatPanelEvent::PickOutsideSession(
                                                        agent.id,
                                                    ))
                                                });
                                            },
                                        ))
                                    })
                            })
                        });
                        let menu = match outside {
                            Some(agents) => menu
                                .item(PopupMenuItem::submenu("Started outside this app", agents))
                                .separator(),
                            None => menu,
                        };
                        if sessions.is_empty() {
                            return menu.label("No chats yet");
                        }
                        let menu = menu
                            .item(PopupMenuItem::new("Search chats (⌘⇧K)").on_click(
                                |_, window, cx| {
                                    // The workspace owns the search; this crate cannot name its action.
                                    if let Ok(action) =
                                        cx.build_action("workspace::SearchChats", None)
                                    {
                                        window.dispatch_action(action, cx);
                                    }
                                },
                            ))
                            .separator();
                        // What each is doing, for those loaded in this run:
                        // a chat at work is found (and gone to) without
                        // opening them all.
                        let chats = crate::chats::ChatHub::existing(project_id, cx);
                        sessions.into_iter().take(30).fold(menu, |menu, session| {
                            let panel = panel.clone();
                            let id = session.id;
                            let state = chats
                                .as_ref()
                                .and_then(|chats| chats.read(cx).summary(id, cx))
                                .map(|summary| summary.state);
                            let title = SharedString::from(session.title);
                            menu.item(
                                PopupMenuItem::element(move |_, cx| {
                                    history_row(state, title.clone(), cx)
                                })
                                .checked(active == Some(id))
                                .on_click(move |_, _, cx| {
                                    _ = panel.update(cx, |_, cx| {
                                        cx.emit(ChatPanelEvent::SelectSession(Some(id)))
                                    });
                                }),
                            )
                        })
                    }),
            )
            .child(
                Button::new("chat-new")
                    .ghost()
                    .xsmall()
                    .icon(Icons::SquarePen)
                    .tooltip("New chat (⌘N)")
                    .accessibility_label("New chat")
                    .on_click(
                        cx.listener(|_, _, _, cx| cx.emit(ChatPanelEvent::SelectSession(None))),
                    ),
            )
    }

    pub(super) fn render_transcript(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.active.read(cx).entries().is_empty() {
            // Something is being readied for a chat about to start: it
            // shows as the run it is, not as the starters with a line of
            // small print under them.
            if let Some(label) = self.busy.clone() {
                return self.render_preparing(label, cx);
            }
            return self.render_empty(cx);
        }
        let thread = self.active.clone();
        // A chat shown since the view mode changed measures its rows again.
        let mode = crate::view_mode::ViewMode::current(cx);
        thread.update(cx, |thread, cx| thread.set_view_mode(mode, cx));
        let scroller = thread.read(cx).scroller().clone();
        TranscriptList::new("chat-transcript", scroller, move |ix, _, cx| {
            thread.update(cx, |thread, cx| thread.prepare_entry(ix, cx));
            render_entry(&thread, ix, cx)
        })
        .into_any_element()
    }

    /// A chat about to start, while what it needs is readied (a repository
    /// cloned, a pull request checked out): what was asked for, as the
    /// message that began the run, and the step under way with the time it
    /// has taken, as the agent's own steps show.
    fn render_preparing(&self, label: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let agent = self.active.read(cx).agent();
        let time = self
            .busy_since
            .map(|since| crate::entry_view::short_duration(since.elapsed()));
        let first_clone = label.contains("Cloning");
        v_flex()
            .id("chat-preparing")
            .size_full()
            .items_center()
            .child(
                v_flex()
                    .w_full()
                    .max_w(
                        ph_theme::preferences::preferences(cx)
                            .chat_width
                            .max_width(),
                    )
                    .px_6()
                    .pt_6()
                    .gap_4()
                    .when_some(self.busy_title.clone(), |this, title| {
                        this.child(
                            h_flex().justify_end().child(
                                // As a message the user sent looks.
                                div()
                                    .max_w(gpui_kit::relative(0.85))
                                    .px_3p5()
                                    .py_2()
                                    .rounded(theme.radius_lg)
                                    .bg(theme.muted)
                                    .text_sm()
                                    .child(title),
                            ),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(agent.mark(ph_ui::rems_from_px(18.)))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .text_color(theme.muted_foreground)
                                            .child(agent.name),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(ph_ui::Loader::new().small())
                                    .child(
                                        div()
                                            .min_w_0()
                                            .child(ShimmerText::new(label).id("chat-preparing-step")),
                                    )
                                    .children(time.map(|time| {
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(time)
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if first_clone {
                                        "The repository is copied to this computer once, which takes a while for a large one. The agent starts by itself when it is here; later chats about it start at once."
                                    } else {
                                        "The agent starts by itself as soon as this is done."
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The setup checks for `agent`. They search the PATH, so they run in
    /// the background, at first and again when a few seconds old (an
    /// install shows up without a restart, and drawing never waits).
    pub(super) fn setup_checks(
        &mut self,
        agent: &'static AgentSpec,
        cx: &mut Context<Self>,
    ) -> Vec<setup::Check> {
        match &self.setup {
            Some((id, at, checks)) if *id == agent.id => {
                if at.elapsed() >= std::time::Duration::from_secs(5)
                    && self._setup_refresh.is_none()
                {
                    let checks = cx.background_spawn(async move { setup::checks(agent) });
                    self._setup_refresh = Some(cx.spawn(async move |panel, cx| {
                        let checks = checks.await;
                        _ = panel.update(cx, |panel, cx| {
                            panel._setup_refresh = None;
                            let changed = panel
                                .setup
                                .as_ref()
                                .is_none_or(|(id, _, old)| *id != agent.id || *old != checks);
                            panel.setup = Some((agent.id, std::time::Instant::now(), checks));
                            if changed {
                                cx.notify();
                            }
                        });
                    }));
                }
                checks.clone()
            }
            // The first time too, in the background: they wait for the
            // login shell's environment, which can take seconds on start.
            _ => {
                let stale = std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_secs(10))
                    .unwrap_or_else(std::time::Instant::now);
                self.setup = Some((agent.id, stale, Vec::new()));
                self.setup_checks(agent, cx)
            }
        }
    }

    /// Every agent's setup checks, for the first-run card: in the
    /// background, again when a few seconds old, and once more a few seconds
    /// after each look while the card is up, so an agent installed meanwhile
    /// is found without a click.
    fn welcome_checks(&mut self, cx: &mut Context<Self>) -> AgentChecks {
        let fresh = self
            .welcome
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < std::time::Duration::from_secs(5));
        if !fresh && self._welcome_refresh.is_none() {
            let checks = cx.background_spawn(async {
                AGENTS
                    .iter()
                    .map(|spec| (spec, setup::checks(spec)))
                    .collect::<Vec<_>>()
            });
            self._welcome_refresh = Some(cx.spawn(async move |panel, cx| {
                let checks = checks.await;
                let shown = panel.update(cx, |panel, cx| {
                    panel._welcome_refresh = None;
                    let changed = panel.welcome.as_ref().is_none_or(|(_, old)| *old != checks);
                    // One can start now: look for agents again, and the
                    // chat takes it (once per change, not every refresh).
                    if changed && checks.iter().any(|(_, checks)| setup::ready(checks)) {
                        crate::installed::recheck(cx);
                    }
                    panel.welcome = Some((std::time::Instant::now(), checks));
                    if changed {
                        cx.notify();
                    }
                });
                if shown.is_ok() {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(5))
                        .await;
                    // Drawn again: a card still up looks again.
                    _ = panel.update(cx, |_, cx| cx.notify());
                }
            }));
        }
        self.welcome
            .as_ref()
            .map(|(_, checks)| checks.clone())
            .unwrap_or_else(|| AGENTS.iter().map(|spec| (spec, Vec::new())).collect())
    }

    pub(super) fn render_empty(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // The first look for agents waits for the login shell: no agent to
        // name yet (it may not be installed).
        if crate::installed::looking(cx) && agents::installed().is_none() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(ph_ui::logo_mark(ph_ui::rems_from_px(32.)))
                .child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(ph_ui::Loader::new().xsmall())
                        .child("Looking for your coding agents…"),
                )
                .into_any_element();
        }
        // None installed: the agents to choose from, not what is missing.
        if agents::installed().is_some_and(|installed| installed.is_empty()) {
            let checks = self.welcome_checks(cx);
            let panel = cx.weak_entity();
            let recheck = Box::new(move |_: &mut Window, cx: &mut App| {
                _ = panel.update(cx, |panel, cx| {
                    panel.welcome = None;
                    cx.notify();
                });
                crate::installed::recheck(cx);
            }) as setup_view::Recheck;
            return v_flex()
                .id("welcome")
                .size_full()
                .items_center()
                .overflow_y_scroll()
                .px_6()
                .py_8()
                .child(
                    div()
                        .w_full()
                        .max_w(ph_ui::rems_from_px(600.))
                        .my_auto()
                        .child(setup_view::welcome(&checks, recheck, cx)),
                )
                .into_any_element();
        }
        let agent = self.active.read(cx).agent();
        let checks = self.setup_checks(agent, cx);
        let (title, subtitle): (SharedString, SharedString) = match &self.context {
            Some(context) => (
                context.title.clone().into(),
                context.subtitle.clone().into(),
            ),
            None => ("What are we building?".into(), {
                // A chat started in one of the workspace's folders says
                // which.
                let folder = self.active.read(cx).folder();
                let place = match folder.file_name() {
                    Some(name) if folder != self.project.path.as_path() => {
                        format!("{} › {}", self.project.name, name.to_string_lossy())
                    }
                    _ => self.project.name.clone(),
                };
                format!("{} in {place}", agent.name).into()
            }),
        };
        let panel = cx.weak_entity();
        if !setup::ready(&checks) {
            let recheck = {
                let panel = panel.clone();
                Box::new(move |_: &mut Window, cx: &mut App| {
                    _ = panel.update(cx, |panel, cx| {
                        panel.setup = None;
                        cx.notify();
                    });
                    crate::installed::recheck(cx);
                }) as setup_view::Recheck
            };
            // Agents that are installed, to start with one of them instead.
            let others: Vec<&'static AgentSpec> = agents::installed()
                .unwrap_or_default()
                .into_iter()
                .filter(|spec| spec.id != agent.id)
                .collect();
            return centered_scroll("chat-setup")
                .child(
                    v_flex()
                        .w_full()
                        .my_auto()
                        .items_center()
                        .gap_4()
                        .child(
                            div()
                                .w_full()
                                .max_w(ph_ui::rems_from_px(560.))
                                .child(setup_view::card(agent, &checks, Some(recheck), cx)),
                        )
                        .when(!others.is_empty(), |this| {
                            this.child(
                                v_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Or use an agent that is installed:"),
                                    )
                                    .child(h_flex().flex_wrap().justify_center().gap_2().children(
                                        others.into_iter().map(|spec| {
                                            let panel = panel.clone();
                                            Button::new(SharedString::from(format!(
                                                "use-{}",
                                                spec.id
                                            )))
                                            .small()
                                            .outline()
                                            .child(
                                                h_flex()
                                                    .gap_1p5()
                                                    .child(spec.mark(ph_ui::rems_from_px(14.)))
                                                    .child(format!("Use {}", spec.name)),
                                            )
                                            .on_click(move |_, window, cx| {
                                                _ = panel.update(cx, |panel, cx| {
                                                    panel.switch_agent(spec, window, cx)
                                                });
                                            })
                                        }),
                                    )),
                            )
                        }),
                )
                .into_any_element();
        }
        // The agent chosen for new chats went away: say which is used now.
        let fallback = crate::chats::default_fallback(&self.store)
            .filter(|(_, used)| used.id == agent.id)
            .map(|(chosen, used)| {
                format!(
                    "{} was not found, so new chats use {}.",
                    chosen.name, used.name
                )
            });
        // The first start downloads the adapter: say so, it takes a while.
        let first_start = checks
            .iter()
            .find(|check| {
                check.state == setup::CheckState::Note && check.title.starts_with("Adapter")
            })
            .map(|check| check.detail.clone());
        centered_scroll("chat-empty")
            .child(
                v_flex()
                    .w_full()
                    .my_auto()
                    .items_center()
                    .gap_6()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            // Whose chat this is: the agent's mark, not a decoration.
                            .child(div().mb_1().child(agent.mark(ph_ui::rems_from_px(32.))))
                            .child(
                                div()
                                    .text_xl()
                                    .text_center()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_center()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(subtitle),
                            )
                            .children(first_start.into_iter().chain(fallback).map(|note| {
                                div()
                                    .text_xs()
                                    .text_center()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(note)
                            })),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(
                                ph_theme::preferences::preferences(cx)
                                    .chat_width
                                    .max_width(),
                            )
                            .grid()
                            // Side by side with room for two, else one under the
                            // other: half of a narrow pane is too little for a card.
                            .grid_cols(if self.narrow { 1 } else { 2 })
                            .gap_2()
                            .children(self.render_starters(cx))
                            .when(self.context.is_none(), |this| {
                                this.children(
                                    if self.project.empty && self.project.extra_dirs.is_empty() {
                                        FOLDERLESS_SUGGESTIONS
                                    } else {
                                        SUGGESTIONS
                                    }
                                    .iter()
                                    .enumerate()
                                    .map(
                                        |(ix, (title, hint, prompt))| {
                                            let panel = panel.clone();
                                            v_flex()
                                                .id(("suggestion", ix))
                                                .gap_0p5()
                                                .px_3()
                                                .py_2p5()
                                                .rounded(cx.theme().radius_lg)
                                                .border_1()
                                                .border_color(cx.theme().border)
                                                .cursor_pointer()
                                                .hover(|style| {
                                                    style
                                                        .bg(cx.theme().muted)
                                                        .border_color(cx.theme().input)
                                                })
                                                .on_click(move |_, window, cx| {
                                                    let (prompt, title) =
                                                        (prompt.to_string(), title.to_string());
                                                    _ = panel.update(cx, |panel, cx| {
                                                        panel.send_text(
                                                            prompt,
                                                            Some(title),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                })
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                                                        .child(*title),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(*hint),
                                                )
                                        },
                                    ),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    /// The context's starters (a pull request's templates), as cards.
    fn render_starters(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(context) = &self.context else {
            return Vec::new();
        };
        (context.starters)()
            .into_iter()
            .enumerate()
            .map(|(ix, starter)| {
                let id = starter.id.clone();
                v_flex()
                    .id(("starter", ix))
                    .gap_0p5()
                    .px_3()
                    .py_2p5()
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().muted).border_color(cx.theme().input))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.busy.is_none() {
                            cx.emit(ChatPanelEvent::Starter(id.clone()))
                        }
                    }))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(starter.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(starter.hint.clone()),
                    )
                    .test_support()
                    .into_any_element()
            })
            .collect()
    }

    pub(super) fn render_attachments(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() {
            return None;
        }
        let chips = self.attachments.iter().enumerate().map(|(ix, attachment)| {
            let thumbnail = match attachment {
                Attachment::Image { path, .. } => Some(path.clone()),
                _ => None,
            };
            let preview: attachment_view::PreviewSource = {
                let attachment = attachment.clone();
                let roots = self.project.roots();
                Rc::new(move |_| attachment.preview(&roots))
            };
            let remove = Button::new(("remove-attachment", ix))
                .ghost()
                .xsmall()
                .icon(IconName::Close)
                .accessibility_label("Remove attachment")
                .on_click(cx.listener(move |this, _, _, cx| {
                    // Only the button's: the chip under it opens the
                    // preview on a click.
                    cx.stop_propagation();
                    if ix < this.attachments.len() {
                        this.attachments.remove(ix);
                        cx.notify();
                    }
                }));
            attachment_view::chip(
                ("attachment", ix),
                attachment.label(),
                attachment.icon(),
                thumbnail,
                preview,
                Some(remove.into_any_element()),
                cx,
            )
        });
        Some(
            h_flex()
                .flex_wrap()
                .gap_1p5()
                .px_3()
                .pt_3()
                .children(chips)
                .into_any_element(),
        )
    }

    /// Messages waiting for the agent to finish, each removable or editable;
    /// the first can interrupt the agent.
    /// While the turn has subagents: how many run, and on click each one's
    /// state, latest step and time; a row shows it in the transcript.
    fn render_subagents(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let thread = self.active.read(cx);
        let working = *thread.status() == ThreadStatus::Working;
        let subagents = thread.subagents();
        let busy = |tool: &crate::transcript::ToolEntry| working && tool.is_busy();
        let running = subagents.iter().filter(|(_, tool)| busy(tool)).count();
        if running == 0 && !(self.subagents_open && !subagents.is_empty()) {
            return None;
        }
        let label = match (running, subagents.len()) {
            (0, all) => format!("{all} subagent{} finished", if all == 1 { "" } else { "s" }),
            (running, all) if running == all => {
                format!(
                    "{running} subagent{} running",
                    if running == 1 { "" } else { "s" }
                )
            }
            (running, all) => format!("{running} of {all} subagents running"),
        };
        let open = self.subagents_open;
        let theme = cx.theme();
        let rows: Vec<AnyElement> = if open {
            subagents
                .iter()
                .map(|(ix, tool)| {
                    let ix = *ix;
                    let glyph: AnyElement = match tool.status {
                        _ if busy(tool) => ph_ui::Loader::new().xsmall().into_any_element(),
                        crate::transcript::ToolStatus::Failed => Icon::new(IconName::Close)
                            .xsmall()
                            .text_color(theme.danger)
                            .into_any_element(),
                        _ => Icon::new(IconName::Check)
                            .xsmall()
                            .text_color(theme.success)
                            .into_any_element(),
                    };
                    let elapsed = tool
                        .started
                        .filter(|_| busy(tool))
                        .map(|started| crate::entry_view::short_duration(started.elapsed()));
                    let steps = tool.children.len();
                    let thread = self.active.clone();
                    h_flex()
                        .id(("subagent-row", ix))
                        .gap_2()
                        .px_2()
                        .h_7()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.list_hover))
                        .text_xs()
                        .child(glyph)
                        .child(
                            div()
                                .flex_none()
                                .max_w(ph_ui::rems_from_px(220.))
                                .truncate()
                                .text_color(theme.foreground)
                                .child(tool.display_title()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .children(tool.last_step()),
                        )
                        .child(div().flex_none().text_color(theme.muted_foreground).child(
                            match elapsed {
                                Some(elapsed) => format!("{steps} steps · {elapsed}"),
                                None => format!("{steps} steps"),
                            },
                        ))
                        .on_click(move |_, _, cx| {
                            thread.update(cx, |thread, cx| thread.reveal(ix, cx))
                        })
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        Some(
            v_flex()
                .gap_0p5()
                .child(
                    h_flex()
                        .id("subagents-chip")
                        .gap_1p5()
                        .px_2()
                        .h_6()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.list_hover))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .when(running > 0, |this| {
                            this.child(ph_ui::Loader::new().xsmall())
                        })
                        .child(label)
                        .child(
                            Icon::new(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronUp
                            })
                            .xsmall(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.subagents_open = !this.subagents_open;
                            cx.notify();
                        })),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    fn render_queued(
        &self,
        queued: Vec<(usize, String)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let steer = self.active.read(cx).can_steer(cx);
        v_flex()
            .gap_1()
            .children(queued.into_iter().map(|(ix, text)| {
                let line = text.lines().next().unwrap_or_default().to_string();
                h_flex()
                    .id(("queued", ix))
                    .group("queued")
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary.opacity(0.6))
                    .text_xs()
                    .child(
                        Icon::new(Icons::Clock)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(line))
                    .when(ix == 0, |this| {
                        this.child(
                            Button::new(("send-queued-now", ix))
                                .ghost()
                                .xsmall()
                                .label("Send now")
                                .tooltip(if steer {
                                    "Add this to what the agent is doing now"
                                } else {
                                    "Stop the agent and send this now"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.active
                                        .update(cx, |thread, cx| thread.send_queued_now(cx))
                                })),
                        )
                    })
                    .child(
                        Button::new(("edit-queued", ix))
                            .ghost()
                            .xsmall()
                            .icon(Icons::Pencil)
                            .tooltip("Edit")
                            .accessibility_label("Edit the queued message")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_queued(ix, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("remove-queued", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Remove")
                            .accessibility_label("Remove the queued message")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active.update(cx, |thread, cx| {
                                    thread.unqueue(ix, cx);
                                })
                            })),
                    )
            }))
    }

    pub(super) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let thread = self.active.read(cx);
        let status = thread.status().clone();
        let busy = matches!(status, ThreadStatus::Connecting | ThreadStatus::Working);
        let agent = thread.agent();
        let options = thread.options().to_vec();
        let panel = cx.weak_entity();

        // No agent installed: the welcome card says how to get one, and
        // nothing can be sent until then.
        let no_agent = agents::installed().is_some_and(|installed| installed.is_empty());
        let status_line: Option<SharedString> = match status {
            _ if no_agent => Some("Install one of the coding agents above to start a chat.".into()),
            // An empty chat shows it in place of the transcript
            // (`render_preparing`); one with messages, here, with how long
            // it has taken.
            _ if self.busy.is_some() => self
                .busy
                .clone()
                .filter(|_| !thread.entries().is_empty())
                .map(|label| match self.busy_since {
                    Some(since) => format!(
                        "{label} {}",
                        crate::entry_view::short_duration(since.elapsed())
                    )
                    .into(),
                    None => label,
                }),
            _ if self.reading_tabs => Some("Reading the shared Chrome tabs…".into()),
            ThreadStatus::Connecting => Some(format!("Starting {}…", agent.name).into()),
            // The transcript's working row says it.
            ThreadStatus::Working | ThreadStatus::Idle => None,
        };
        let failed = self.failed.clone();

        let mode = options
            .iter()
            .find(|option| option.category == OptionCategory::Mode)
            .cloned();
        let effort = options
            .iter()
            .find(|option| option.category == OptionCategory::Effort)
            .cloned();
        let has_more = options
            .iter()
            .any(|option| option.category == OptionCategory::Other);
        let account = self.render_account(agent, cx);
        let pills = h_flex()
            .gap_1()
            .min_w_0()
            .overflow_hidden()
            .child(attach_button(panel.clone()))
            .child(model_button(agent, &options, panel.clone(), cx))
            .children(account)
            .when_some(effort, |this, effort| {
                this.child(option_pill(&effort, panel.clone()))
            })
            .when_some(mode, |this, mode| {
                this.child(option_pill(&mode, panel.clone()))
            })
            .when(has_more, |this| {
                this.child(more_options_button(&options, panel.clone()))
            });

        let has_text = !self.composer.read(cx).value().trim().is_empty();
        // An agent that does not stop when asked: Stop then ends it.
        let slow = self.active.read(cx).stop_is_slow();
        let stop = busy.then(|| {
            Button::new("stop-agent")
                .small()
                .rounded_full()
                .icon(IconName::Square)
                .map(|this| if slow { this.danger() } else { this.outline() })
                .tooltip(if slow {
                    "Not stopping. Press to end the agent's process"
                } else {
                    "Stop"
                })
                .accessibility_label(if slow { "Force stop" } else { "Stop" })
                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
        });
        // While the agent works, a message is queued: it goes when the turn ends.
        let send = (!no_agent && (!busy || has_text)).then(|| {
            Button::new("send-message")
                .small()
                .primary()
                .rounded_full()
                .icon(if busy {
                    Icons::ListPlus
                } else {
                    IconName::ArrowUp.into()
                })
                .tooltip(if busy {
                    "Queue (Enter): sent when the agent finishes"
                } else if cfg!(target_os = "macos") {
                    "Send (Enter). ⌘Enter sends and starts a new chat"
                } else {
                    "Send (Enter). Ctrl+Enter sends and starts a new chat"
                })
                .accessibility_label(if busy { "Queue" } else { "Send" })
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        });
        let action = h_flex().gap_1().children(stop).children(send);
        let queued: Vec<(usize, String)> = thread
            .queued()
            .iter()
            .enumerate()
            .map(|(ix, prompt)| (ix, prompt.display.clone()))
            .collect();

        div()
            .flex_none()
            .w_full()
            .flex()
            .justify_center()
            .px_5()
            .pb_4()
            .pt_1()
            .child(
                v_flex()
                    .w_full()
                    .max_w(
                        ph_theme::preferences::preferences(cx)
                            .chat_width
                            .max_width(),
                    )
                    .gap_1p5()
                    .when_some(status_line, |this, text| {
                        this.child(
                            div()
                                .px_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(ShimmerText::new(text).id("agent-status")),
                        )
                    })
                    .children(self.render_subagents(cx))
                    .when(!queued.is_empty(), |this| {
                        this.child(self.render_queued(queued, cx))
                    })
                    .when_some(failed, |this, reason| {
                        this.child(
                            h_flex()
                                .id("chat-prepare-failed")
                                .px_2()
                                .gap_1p5()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(Icon::new(Icons::TriangleAlert).xsmall())
                                .child(div().flex_1().min_w_0().child(reason)),
                        )
                    })
                    .child(
                        v_flex()
                            .relative()
                            .rounded(cx.theme().radius_lg)
                            .border_1()
                            .border_color(cx.theme().input)
                            .bg(cx.theme().background)
                            .shadow_md()
                            // While suggestions show, the arrows, Enter, Tab and Esc work them
                            // instead of the text.
                            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                                if this.mention.is_some() {
                                    this.move_mention(-1, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                                if this.mention.is_some() {
                                    this.move_mention(1, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                                if let Some(selected) =
                                    this.mention.as_ref().map(|menu| menu.selected)
                                {
                                    this.accept_mention(selected, window, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                                if let Some(selected) =
                                    this.mention.as_ref().map(|menu| menu.selected)
                                {
                                    this.accept_mention(selected, window, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            // ⇧Tab switches the agent's mode, as in Claude Code.
                            .capture_action(cx.listener(|this, _: &OutdentInline, _, cx| {
                                if this.cycle_mode(cx) {
                                    cx.stop_propagation();
                                }
                            }))
                            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                                if this.mention.take().is_some() {
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            }))
                            // An image on the clipboard (a screenshot) is
                            // attached; text pastes as usual.
                            .capture_action(cx.listener(|this, _: &Paste, window, cx| {
                                if this.paste_image(window, cx) {
                                    cx.stop_propagation();
                                }
                            }))
                            // Something dragged over the chat can drop here.
                            .drag_over::<ph_files::DraggedFiles>(|style, _, _, cx| {
                                style.border_color(cx.theme().ring)
                            })
                            .drag_over::<gpui_kit::ExternalPaths>(|style, _, _, cx| {
                                style.border_color(cx.theme().ring)
                            })
                            .children(self.render_mention_menu(cx))
                            .children(self.render_attachments(cx))
                            .child(
                                div().px_1().pt_1().child(
                                    Textarea::new(&self.composer)
                                        .appearance(false)
                                        .bordered(false),
                                ),
                            )
                            .child(
                                h_flex()
                                    .px_2()
                                    .pb_2()
                                    .gap_2()
                                    .child(div().flex_1().min_w_0().overflow_hidden().child(pills))
                                    .child(div().flex_none().child(action)),
                            ),
                    ),
            )
    }
}

impl Render for ChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.terminal_session.is_some() {
            return self.render_cli(window, cx);
        }
        let (narrow, probe) = ph_ui::measure::narrower_than("chat-width", 440., window, cx);
        self.narrow = narrow;
        v_flex()
            .relative()
            .size_full()
            .child(probe)
            // Files and images dropped anywhere on the chat are attached.
            .on_drop(
                cx.listener(|this, dragged: &ph_files::DraggedFiles, window, cx| {
                    this.attach_tree_files(dragged, window, cx)
                }),
            )
            .on_drop(
                cx.listener(|this, paths: &gpui_kit::ExternalPaths, window, cx| {
                    this.drop_external(paths, window, cx)
                }),
            )
            .child(self.render_header(window, cx))
            // Clipped: in a short pane the empty chat's welcome is taller
            // than the room and would draw over the header.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_transcript(cx)),
            )
            .children(self.render_terminal_offer(cx))
            .child(self.render_composer(cx))
            .into_any_element()
    }
}

impl ChatPanel {
    /// Over the composer when this agent's CLI has no editor mode here (see
    /// [`agents::chat_unsupported`]): the one way it does work, a click
    /// away.
    fn render_terminal_offer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let agent = self.active.read(cx).agent();
        if self.cli_view.is_none()
            || !agents::chat_unsupported(agent.id)
            || !ph_brand::enabled(ph_brand::Feature::Terminal)
            || self.project.read_only
        {
            return None;
        }
        Some(
            h_flex()
                .flex_none()
                .mx_6()
                .mb_2()
                .px_3()
                .py_2()
                .gap_3()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().warning.opacity(0.4))
                .bg(cx.theme().warning.opacity(0.08))
                .child(
                    Icon::new(Icons::Terminal)
                        .small()
                        .text_color(cx.theme().warning),
                )
                .child(div().flex_1().min_w_0().text_sm().child(format!(
                    "{} here has no chat mode. It works in its CLI, in this same place.",
                    agent.name
                )))
                .child(
                    Button::new("run-agent-in-terminal")
                        .primary()
                        .small()
                        .label(format!("Open {}'s CLI", agent.name))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ChatPanelEvent::ShowAs(Form::Cli))
                        })),
                )
                .into_any_element(),
        )
    }
}

/// The model button: the agent's glyph and the current model; opens the
/// picker with every agent's models and the effort setting.
pub(super) fn model_button(
    agent: &'static AgentSpec,
    options: &[AgentOption],
    panel: WeakEntity<ChatPanel>,
    cx: &App,
) -> impl IntoElement + use<> {
    let model = options
        .iter()
        .find(|option| option.category == OptionCategory::Model)
        .map(|option| option.current_name())
        .unwrap_or_else(|| agent.name.to_string());
    let label = h_flex()
        .gap_1p5()
        .child(agent.mark(ph_ui::rems_from_px(14.)))
        .child(
            // A long model name gives way, so the row's buttons all fit.
            div()
                .max_w(ph_ui::rems_from_px(150.))
                .truncate()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(model),
        )
        .child(
            Icon::new(IconName::ChevronDown)
                .xsmall()
                .text_color(cx.theme().muted_foreground),
        );
    Popover::new("model-picker")
        .offset(ph_ui::POPUP_GAP)
        .anchor(gpui_kit::Anchor::BottomLeft)
        .trigger(
            Button::new("choose-model")
                .ghost()
                .xsmall()
                .child(label)
                .tooltip("Agent and model")
                .accessibility_label("Agent and model"),
        )
        .on_open_change({
            let panel = panel.clone();
            // Each opening shows this chat's agent first.
            move |open, window, cx| {
                _ = panel.update(cx, |panel, cx| {
                    if *open {
                        panel.picker_opened(window, cx)
                    } else {
                        // The keyboard goes back to the message.
                        panel.focus_composer(window, cx)
                    }
                });
            }
        })
        .content(move |_, _, cx| match panel.upgrade() {
            Some(chat) => {
                let data = chat.read(cx).picker_data(cx);
                let search = chat.read(cx).picker_search.clone();
                model_picker::render(&data, &search, panel.clone(), cx)
            }
            None => div().into_any_element(),
        })
}

/// A compact toolbar button with a caret.
pub(super) fn pill(
    id: impl Into<gpui_kit::ElementId>,
    icon: impl Into<Icon>,
    label: impl Into<SharedString>,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(icon)
        .label(label)
        .dropdown_caret(true)
}

pub(super) fn attach_button(panel: WeakEntity<ChatPanel>) -> impl IntoElement {
    Button::new("attach")
        .ghost()
        .xsmall()
        .icon(IconName::Plus)
        .tooltip("Attach")
        .accessibility_label("Attach")
        .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, cx| {
            let capture = panel.clone();
            let logs = panel.clone();
            let mention = panel.clone();
            let parallel = panel.clone();
            let tabs = panel.clone();
            let has_tabs = panel.upgrade().is_some_and(|panel| {
                panel
                    .read(cx)
                    .live
                    .as_ref()
                    .is_some_and(|live| live.tabs.is_some())
            });
            menu.item(
                PopupMenuItem::new("Screenshot of browser or device")
                    .icon(Icons::Camera)
                    .on_click(move |_, window, cx| {
                        _ = capture.update(cx, |panel, cx| panel.attach_live(false, window, cx));
                    }),
            )
            .item(
                PopupMenuItem::new("Recent logs")
                    .icon(Icons::ScrollText)
                    .on_click(move |_, window, cx| {
                        _ = logs.update(cx, |panel, cx| panel.attach_live(true, window, cx));
                    }),
            )
            .when(has_tabs, |menu| {
                menu.item(
                    PopupMenuItem::new("Tabs you shared in Chrome")
                        .icon(Icons::Globe)
                        .on_click(move |_, window, cx| {
                            _ = tabs.update(cx, |panel, cx| {
                                panel.attach_shared_tabs(false, window, cx)
                            });
                        }),
                )
            })
            .item(
                PopupMenuItem::new("File (type @path)")
                    .icon(IconName::File)
                    .on_click(move |_, window, cx| {
                        _ = mention.update(cx, |panel, cx| {
                            panel.composer.update(cx, |composer, cx| {
                                let text = format!("{}@", composer.value());
                                composer.set_value(text, window, cx);
                            });
                            panel.focus_composer(window, cx);
                        });
                    }),
            )
            // Not in a build whose brand leaves parallel runs out.
            .when(ph_brand::enabled(ph_brand::Feature::ParallelRuns), |menu| {
                menu.separator().item(
                    PopupMenuItem::new("Run with several agents")
                        .icon(Icons::GitCompareArrows)
                        .on_click(move |_, _, cx| {
                            _ = parallel.update(cx, |panel, cx| {
                                let prompt = panel.composer.read(cx).value().trim().to_string();
                                cx.emit(ChatPanelEvent::RunParallel { prompt });
                            });
                        }),
                )
            })
        })
}

pub(super) fn option_icon(category: OptionCategory) -> Icons {
    match category {
        OptionCategory::Model => Icons::Sparkles,
        OptionCategory::Mode => Icons::ShieldCheck,
        OptionCategory::Effort => Icons::Brain,
        OptionCategory::Other => Icons::Settings2,
    }
}

pub(super) fn option_pill(option: &AgentOption, panel: WeakEntity<ChatPanel>) -> impl IntoElement {
    let option = option.clone();
    pill(
        SharedString::from(format!("option-{}", option.id)),
        option_icon(option.category),
        option.current_name(),
    )
    .tooltip(if option.category == OptionCategory::Mode {
        format!("{} (⇧Tab switches)", option.name)
    } else {
        option.name.clone()
    })
    .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
        option_menu(menu, &option, panel.clone())
    })
}

pub(super) fn option_menu(
    menu: PopupMenu,
    option: &AgentOption,
    panel: WeakEntity<ChatPanel>,
) -> PopupMenu {
    let choices: Vec<(String, String)> = match &option.kind {
        OptionKind::Select(choices) => choices
            .iter()
            .map(|choice| (choice.value.clone(), choice.name.clone()))
            .collect(),
        OptionKind::Toggle => vec![("true".into(), "On".into()), ("false".into(), "Off".into())],
    };
    choices.into_iter().fold(menu, |menu, (value, name)| {
        let panel = panel.clone();
        let id = option.id.clone();
        let checked = value == option.current;
        menu.item(
            PopupMenuItem::new(name)
                .checked(checked)
                .on_click(move |_, _, cx| {
                    _ = panel.update(cx, |panel, cx| panel.choose_option(&id, &value, cx));
                }),
        )
    })
}

pub(super) fn more_options_button(
    options: &[AgentOption],
    panel: WeakEntity<ChatPanel>,
) -> impl IntoElement {
    let others: Vec<AgentOption> = options
        .iter()
        .filter(|option| option.category == OptionCategory::Other)
        .cloned()
        .collect();
    Button::new("more-options")
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .tooltip("More options")
        .accessibility_label("More options")
        .menu(gpui_kit::Anchor::BottomLeft, move |menu, window, cx| {
            others.iter().fold(menu, |menu, option| {
                let option = option.clone();
                let panel = panel.clone();
                menu.submenu(
                    format!("{}: {}", option.name, option.current_name()),
                    window,
                    cx,
                    move |menu, _, _| option_menu(menu, &option, panel.clone()),
                )
            })
        })
}

/// A pane whose content sits in its middle and scrolls once it is taller
/// than the pane (the child takes `my_auto()`). Centring alone
/// (`justify_center`) cuts tall content off at both ends, out of reach: a
/// narrow chat beside a pull request, a large interface size.
fn centered_scroll(id: &'static str) -> gpui_kit::Stateful<gpui_kit::Div> {
    v_flex()
        .id(id)
        .size_full()
        .items_center()
        .overflow_y_scroll()
        .px_6()
        .py_6()
}

/// A chat in the Past chats menu: its title after a mark for what it is
/// doing (at work, waiting for an answer, stopped on an error, finished and
/// not looked at). One that is at rest, or not loaded in this run, has no
/// mark; its title lines up with the others'.
fn history_row(
    state: Option<crate::chats::ChatState>,
    title: SharedString,
    cx: &App,
) -> impl IntoElement + use<> {
    use crate::chats::ChatState;
    let theme = cx.theme();
    let dot = |color: gpui_kit::Hsla| {
        div()
            .size(ph_ui::rems_from_px(7.))
            .rounded_full()
            .bg(color)
            .into_any_element()
    };
    let (mark, about) = match state {
        Some(ChatState::Working) => (
            ph_ui::Loader::new().xsmall().into_any_element(),
            Some("working"),
        ),
        Some(ChatState::NeedsYou) => (dot(theme.warning), Some("needs you")),
        Some(ChatState::Failed) => (dot(theme.danger), Some("stopped")),
        Some(ChatState::Done) => (dot(theme.success), Some("finished")),
        Some(ChatState::Idle) | None => (div().into_any_element(), None),
    };
    h_flex()
        .gap_2()
        .child(
            h_flex()
                .flex_none()
                .w(ph_ui::rems_from_px(14.))
                .justify_center()
                .child(mark),
        )
        .child(
            div()
                .min_w_0()
                .max_w(ph_ui::rems_from_px(320.))
                .truncate()
                .child(title),
        )
        .children(about.map(|about| {
            div()
                .flex_none()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(about)
        }))
}
