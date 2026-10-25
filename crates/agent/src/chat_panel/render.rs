//! How the chat column is drawn: header, transcript or the empty state,
//! attachments and the composer with its pills.

use super::*;

impl ChatPanel {
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let thread = self.active.read(cx);
        let (dot, label) = match thread.status() {
            ThreadStatus::Working => (cx.theme().success, "Working"),
            ThreadStatus::Connecting => (cx.theme().warning, "Starting"),
            ThreadStatus::Idle if thread.has_started() => {
                (cx.theme().success.opacity(0.6), "Ready")
            }
            ThreadStatus::Idle => (cx.theme().muted_foreground.opacity(0.5), "Idle"),
        };
        h_flex()
            .flex_none()
            .h_11()
            .px_4()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(thread.title()),
            )
            // The agent's mark with a dot for its state; the words are in the
            // tooltip, to keep the header small.
            .child({
                let tip = format!("{} · {label}", thread.agent().name);
                div()
                    .id("chat-agent-state")
                    .flex_none()
                    .relative()
                    .p_0p5()
                    .aria_label(SharedString::from(tip.clone()))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .child(thread.agent().mark(px(16.)))
                    .child(
                        div()
                            .absolute()
                            .right(px(-1.))
                            .bottom(px(-1.))
                            .size(px(7.))
                            .rounded_full()
                            .border_1()
                            .border_color(cx.theme().background)
                            .bg(dot),
                    )
            })
            .children(crate::usage_meter::render(thread.usage(), cx))
            .when(
                thread.has_started() && *thread.status() == ThreadStatus::Idle,
                |this| this.child(self.render_hand_off(cx)),
            )
            .when(self.history_menu, |this| {
                this.child(self.render_history(cx))
            })
    }

    /// "Continue with…": the chat goes on with another agent.
    pub(super) fn render_hand_off(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.active.read(cx).agent().id;
        let panel = cx.weak_entity();
        Button::new("chat-hand-off")
            .ghost()
            .xsmall()
            .icon(Icons::MoveUpRight)
            .tooltip("Continue this chat with another agent")
            .accessibility_label("Continue with another agent")
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                let menu = menu.label("Continue with…");
                agents::AGENTS
                    .iter()
                    .filter(|agent| agent.id != current)
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
                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                        let sessions: Vec<_> = store
                            .sessions(project_id)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|session| {
                                only.is_none() || session.cwd.as_ref() == only.as_ref()
                            })
                            .collect();
                        if sessions.is_empty() {
                            return menu.label("No chats yet");
                        }
                        let menu = menu
                            .item(PopupMenuItem::new("Search chats… (⌘⇧K)").on_click(
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
                        sessions.into_iter().take(30).fold(menu, |menu, session| {
                            let panel = panel.clone();
                            let id = session.id;
                            menu.item(
                                PopupMenuItem::new(session.title)
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
            return self.render_empty(cx);
        }
        let thread = self.active.clone();
        let scroller = thread.read(cx).scroller().clone();
        MessageScroller::new("chat-transcript", scroller, move |ix, _, cx| {
            thread.update(cx, |thread, cx| thread.prepare_entry(ix, cx));
            render_entry(&thread, ix, cx)
        })
        .size_full()
        .into_any_element()
    }

    /// The setup checks for `agent`; file-system lookups, so cached briefly.
    pub(super) fn setup_checks(&mut self, agent: &'static AgentSpec) -> Vec<setup::Check> {
        let fresh = self.setup.as_ref().is_some_and(|(id, at, _)| {
            *id == agent.id && at.elapsed() < std::time::Duration::from_secs(5)
        });
        if !fresh {
            self.setup = Some((agent.id, std::time::Instant::now(), setup::checks(agent)));
        }
        self.setup
            .as_ref()
            .map(|(_, _, checks)| checks.clone())
            .unwrap_or_default()
    }

    pub(super) fn render_empty(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let agent = self.active.read(cx).agent();
        let checks = self.setup_checks(agent);
        let (title, subtitle): (SharedString, SharedString) = match &self.context {
            Some(context) => (
                context.title.clone().into(),
                context.subtitle.clone().into(),
            ),
            None => (
                "What are we building?".into(),
                format!("{} in {}", agent.name, self.project.name).into(),
            ),
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
                }) as setup_view::Recheck
            };
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .px_6()
                .child(
                    div()
                        .w_full()
                        .max_w(gpui_kit::px(560.))
                        .child(setup_view::card(agent, &checks, Some(recheck), cx)),
                )
                .into_any_element();
        }
        // The first start downloads the adapter: say so, it takes a while.
        let first_start = checks
            .iter()
            .find(|check| {
                check.state == setup::CheckState::Note && check.title.starts_with("Adapter")
            })
            .map(|check| check.detail.clone());
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .px_6()
            .gap_6()
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size_12()
                            .mb_1()
                            .rounded(cx.theme().radius_lg)
                            .bg(cx.theme().info.opacity(0.14))
                            .text_color(cx.theme().info)
                            .child(Icon::new(Icons::Sparkles).size_6()),
                    )
                    .child(
                        div()
                            .text_xl()
                            .text_center()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(subtitle),
                    )
                    .children(first_start.map(|note| {
                        div()
                            .text_xs()
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
                    .grid_cols(2)
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
                            .map(|(ix, (title, hint, prompt))| {
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
                                        style.bg(cx.theme().muted).border_color(cx.theme().input)
                                    })
                                    .on_click(move |_, window, cx| {
                                        let (prompt, title) =
                                            (prompt.to_string(), title.to_string());
                                        _ = panel.update(cx, |panel, cx| {
                                            panel.send_text(prompt, Some(title), window, cx)
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
                            }),
                        )
                    }),
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
                let (roots, id, store) =
                    (self.project.roots(), self.project.id, self.store.clone());
                Rc::new(move |_| attachment.preview(&roots, id, &store))
            };
            let remove = Button::new(("remove-attachment", ix))
                .ghost()
                .xsmall()
                .icon(IconName::Close)
                .accessibility_label("Remove attachment")
                .on_click(cx.listener(move |this, _, _, cx| {
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

    pub(super) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let thread = self.active.read(cx);
        let status = thread.status().clone();
        let busy = matches!(status, ThreadStatus::Connecting | ThreadStatus::Working);
        let agent = thread.agent();
        let options = thread.options().to_vec();
        let panel = cx.weak_entity();

        let status_line: Option<SharedString> = match status {
            _ if self.busy.is_some() => self.busy.clone(),
            ThreadStatus::Connecting => Some(format!("Starting {}…", agent.name).into()),
            ThreadStatus::Working => Some(format!("{} is working…", agent.name).into()),
            ThreadStatus::Idle => None,
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
        let pills = h_flex()
            .gap_1()
            .min_w_0()
            .overflow_hidden()
            .child(attach_button(panel.clone()))
            .child(model_button(agent, &options, panel.clone(), cx))
            .when_some(effort, |this, effort| {
                this.child(option_pill(&effort, panel.clone()))
            })
            .when_some(mode, |this, mode| {
                this.child(option_pill(&mode, panel.clone()))
            })
            .when(has_more, |this| {
                this.child(more_options_button(&options, panel.clone()))
            });

        let action = if busy {
            Button::new("stop-agent")
                .small()
                .outline()
                .rounded_full()
                .icon(IconName::Square)
                .tooltip("Stop")
                .accessibility_label("Stop")
                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
        } else {
            Button::new("send-message")
                .small()
                .info()
                .rounded_full()
                .icon(IconName::ArrowUp)
                .tooltip("Send (Enter)")
                .accessibility_label("Send")
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };

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
                            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                                if this.mention.take().is_some() {
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            }))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().child(self.render_transcript(cx)))
            .child(self.render_composer(cx))
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
        .child(agent.mark(px(14.)))
        .child(
            // A long model name gives way, so the row's buttons all fit.
            div()
                .max_w(px(150.))
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
        .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
            let capture = panel.clone();
            let logs = panel.clone();
            let mention = panel.clone();
            let parallel = panel.clone();
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
            .item(
                PopupMenuItem::new("File… (type @path)")
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
            .separator()
            .item(
                PopupMenuItem::new("Run with several agents…")
                    .icon(Icons::GitCompareArrows)
                    .on_click(move |_, _, cx| {
                        _ = parallel.update(cx, |panel, cx| {
                            let prompt = panel.composer.read(cx).value().trim().to_string();
                            cx.emit(ChatPanelEvent::RunParallel { prompt });
                        });
                    }),
            )
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
    .tooltip(option.name.clone())
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
