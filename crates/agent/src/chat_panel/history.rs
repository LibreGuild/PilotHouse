//! Past chats.
//!
//! The clock button over the chat opens the chats last used, the newest
//! first, whatever their state (at work, waiting for a look, done): one
//! list, since it is looked through by what was done last. Each row has
//! its agent's mark, and when pointed at, what can be done with the chat
//! without opening it: mark it done, rename it, delete it. *Show all
//! chats* at its end puts the whole list where the chat is (the
//! transcript and the message box give way to it) until a chat is picked
//! or the list is left.
//!
//! What a chat's done-mark is, and renaming and deleting (which ask
//! first), belong to whoever shows the panel: it gives the mark
//! ([`ChatPanel::set_chat_marks`]) and acts on
//! [`ChatPanelEvent::ChatAction`]. A panel without them lists its chats
//! and nothing more.

use super::*;

/// How many chats the pop-up lists.
const RECENT: usize = 10;
/// The most the whole list draws (the newest): it is a list to pick from,
/// not an archive to page through; older ones are found by search.
const ALL: usize = 300;

/// What is asked of a chat from its row in Past chats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatAction {
    /// Mark it done, or not done when it is.
    ToggleDone,
    Rename,
    Delete,
}

/// Whether a chat is marked done, as whoever shows the panel keeps it.
pub type ChatMarks = Rc<dyn Fn(SessionId, &App) -> bool>;

/// A chat as its row shows it. Worked out while the panel draws, since the
/// pop-up's rows are drawn where the panel cannot be read.
struct Listed {
    session: Session,
    /// It is the chat in front.
    front: bool,
    /// What it is doing, when it is loaded in this run.
    state: Option<crate::chats::ChatState>,
    /// Marked done; `None` where the panel has no such mark (and its rows
    /// no buttons).
    done: Option<bool>,
}

impl ChatPanel {
    /// Gives Past chats its row actions: `done` says whether a chat is
    /// marked done, and [`ChatPanelEvent::ChatAction`] is acted on.
    pub fn set_chat_marks(&mut self, done: ChatMarks) {
        self.chat_marks = Some(done);
    }

    /// The chats changed (one was renamed, deleted, used): a list that
    /// shows is read again.
    pub fn sessions_changed(&mut self, cx: &mut Context<Self>) {
        if self.history_open || self.all_chats {
            self.load_history();
            cx.notify();
        }
    }

    /// Whether the whole list of chats shows where the chat is.
    pub fn shows_all_chats(&self) -> bool {
        self.all_chats
    }

    /// Reads the chats to list, the last used first. A panel about one
    /// thing (a pull request) lists the chats about it.
    fn load_history(&mut self) {
        let only = self
            .context
            .as_ref()
            .map(|context| context.cwd.to_string_lossy().into_owned());
        self.history = self
            .store
            .sessions(self.project.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|session| only.is_none() || session.cwd.as_ref() == only.as_ref())
            .collect();
    }

    fn listed(&self, session: &Session, cx: &App) -> Listed {
        let id = session.id;
        Listed {
            session: session.clone(),
            front: self.active_session_id(cx) == Some(id),
            state: self
                .chats
                .read(cx)
                .summary(id, cx)
                .map(|summary| summary.state),
            done: self.chat_marks.as_ref().map(|done| done(id, cx)),
        }
    }

    fn open_from_history(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.history_open = false;
        self.all_chats = false;
        cx.emit(ChatPanelEvent::SelectSession(Some(id)));
        cx.notify();
    }

    fn show_all_chats(&mut self, cx: &mut Context<Self>) {
        self.history_open = false;
        self.all_chats = true;
        self.load_history();
        cx.notify();
    }

    fn leave_all_chats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.all_chats = false;
        if self.terminal_session.is_none() {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    /// Past chats and a new chat: the two buttons at the end of the
    /// header's tabs.
    pub(super) fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.weak_entity();
        // Only while the pop-up is open: a closed one draws nothing.
        let recent: Rc<Vec<Listed>> = Rc::new(if self.history_open {
            self.history
                .iter()
                .take(RECENT)
                .map(|session| self.listed(session, cx))
                .collect()
        } else {
            Vec::new()
        });
        let more = self.history.len().saturating_sub(RECENT);
        h_flex()
            .flex_none()
            .gap_0p5()
            .child(
                Popover::new("chat-history-list")
                    .offset(ph_ui::POPUP_GAP)
                    .anchor(gpui_kit::Anchor::TopRight)
                    .open(self.history_open)
                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                        this.history_open = *open;
                        if *open {
                            this.load_history();
                        }
                        cx.notify();
                    }))
                    .trigger(
                        Button::new("chat-history")
                            .ghost()
                            .xsmall()
                            .icon(Icons::RotateCcwClock)
                            .tooltip("Past chats")
                            .accessibility_label("Past chats"),
                    )
                    .content(move |_, _, cx| render_recent(&panel, &recent, more, cx)),
            )
            .child(
                Button::new("chat-new")
                    .ghost()
                    .xsmall()
                    .icon(Icons::SquarePen)
                    .tooltip("New chat (⌘N)")
                    .accessibility_label("New chat")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.all_chats = false;
                        cx.emit(ChatPanelEvent::SelectSession(None))
                    })),
            )
    }

    /// Every chat, where the transcript and the message box were: the
    /// header stays, so the tabs and the way back are where they are.
    pub(super) fn render_all_chats(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = self.render_header(window, cx).into_any_element();
        let weak = cx.weak_entity();
        let count = self.history.len();
        let outside = self.cli_view.is_some() && self.context.is_none();
        let bar = h_flex()
            .flex_none()
            .h_9()
            .px_4()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child("All chats"),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count.to_string()),
            )
            .child(
                Button::new("all-chats-search")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Search)
                    .tooltip("Search chats (⌘⇧K)")
                    .accessibility_label("Search chats")
                    .on_click(|_, window, cx| {
                        // The workspace owns the search; this crate cannot
                        // name its action.
                        if let Ok(action) = cx.build_action("workspace::SearchChats", None) {
                            window.dispatch_action(action, cx);
                        }
                    }),
            )
            // An agent's own sessions here that did not start in this app
            // (in another terminal, another editor).
            .when(outside, |this| {
                let weak = weak.clone();
                this.child(
                    Button::new("all-chats-outside")
                        .ghost()
                        .xsmall()
                        .label("Started elsewhere")
                        .tooltip(ph_ui::tip(
                            "Go on with a session of an agent that did not start in this app",
                        ))
                        .dropdown_caret(true)
                        .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                            AGENTS
                                .iter()
                                .filter(|agent| agents::has_cli(agent.id))
                                .fold(menu, |menu, agent| {
                                    let weak = weak.clone();
                                    menu.item(PopupMenuItem::new(agent.name).on_click(
                                        move |_, _, cx| {
                                            _ = weak.update(cx, |this, cx| {
                                                this.all_chats = false;
                                                cx.emit(ChatPanelEvent::PickOutsideSession(
                                                    agent.id,
                                                ))
                                            });
                                        },
                                    ))
                                })
                        }),
                )
            })
            .child(
                Button::new("all-chats-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Back to the chat")
                    .accessibility_label("Back to the chat")
                    .on_click(cx.listener(|this, _, window, cx| this.leave_all_chats(window, cx))),
            );
        let rows: Vec<AnyElement> = self
            .history
            .iter()
            .take(ALL)
            .map(|session| row(&weak, &self.listed(session, cx), true, cx))
            .collect();
        v_flex()
            .size_full()
            .child(header)
            .child(bar)
            .child(if rows.is_empty() {
                div()
                    .flex_1()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No chats yet.")
                    .into_any_element()
            } else {
                v_flex()
                    .id("all-chats")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_3()
                    .children(rows)
                    .when(count > ALL, |this| {
                        this.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "The {ALL} last used are listed. Search finds the others."
                                )),
                        )
                    })
                    .test_support()
                    .into_any_element()
            })
            .into_any_element()
    }
}

/// The pop-up: the chats last used (`recent`), and the way to all of
/// them (`more` than those).
fn render_recent(
    panel: &WeakEntity<ChatPanel>,
    recent: &[Listed],
    more: usize,
    cx: &App,
) -> AnyElement {
    let weak = panel.clone();
    let rows: Vec<AnyElement> = recent
        .iter()
        .map(|listed| row(panel, listed, false, cx))
        .collect();
    v_flex()
        .w(ph_ui::rems_from_px(320.))
        .max_w_full()
        .gap_0p5()
        .map(|list| match rows.is_empty() {
            true => list.child(
                div()
                    .px_2()
                    .py_1p5()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No chats yet."),
            ),
            false => list.children(rows),
        })
        .child(div().h(px(1.)).my_0p5().bg(cx.theme().border))
        .child(
            h_flex()
                .id("history-show-all")
                .role(gpui_kit::accesskit::Role::Button)
                .aria_label("Show all chats")
                .h_7()
                .px_2()
                .gap_2()
                .rounded(cx.theme().radius)
                .cursor_pointer()
                .text_sm()
                .hover(|style| style.bg(cx.theme().list_hover))
                .on_click(move |_, _, cx| {
                    _ = weak.update(cx, |this, cx| this.show_all_chats(cx));
                })
                .child(div().flex_1().child("Show all chats"))
                .when(more > 0, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{more} more")),
                    )
                })
                .test_support(),
        )
        .into_any_element()
}

/// A chat in Past chats: its agent's mark, what it is doing when it is
/// doing something, its name; when pointed at, what can be done with it.
/// In the whole list (`dated`) it also says when it was last used.
fn row(panel: &WeakEntity<ChatPanel>, listed: &Listed, dated: bool, cx: &App) -> AnyElement {
    use crate::chats::ChatState;
    let theme = cx.theme();
    let Listed {
        session,
        front,
        state,
        done,
    } = listed;
    let (front, state, done) = (*front, *state, *done);
    let id = session.id;
    let agent = agents::find(&session.agent_id).unwrap_or(&AGENTS[0]);
    let busy = matches!(state, Some(ChatState::Working | ChatState::NeedsYou));
    let dot = |color: gpui_kit::Hsla| {
        div()
            .flex_none()
            .size(ph_ui::rems_from_px(7.))
            .rounded_full()
            .bg(color)
            .into_any_element()
    };
    // Said only for a chat something is going on in; one at rest has its
    // name and nothing before it.
    let mark = match state {
        Some(ChatState::Working) => Some(ph_ui::Loader::new().xsmall().into_any_element()),
        Some(ChatState::NeedsYou) => Some(dot(theme.warning)),
        Some(ChatState::Failed) => Some(dot(theme.danger)),
        Some(ChatState::Done) => Some(dot(theme.success)),
        Some(ChatState::Idle) | None => None,
    };
    let group = SharedString::from(format!("history-row-{id}"));
    let weak = panel.clone();
    let action = |name: &'static str, icon: Icons, tooltip: &'static str, action: ChatAction| {
        let (weak, session) = (weak.clone(), session.clone());
        Button::new(SharedString::from(format!("history-{name}-{id}")))
            .ghost()
            .xsmall()
            .icon(icon)
            .tooltip(tooltip)
            .accessibility_label(tooltip)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                _ = weak.update(cx, |this, cx| {
                    // What asks first (a name, "delete?") does so over
                    // the window, not under this pop-up.
                    if action != ChatAction::ToggleDone {
                        this.history_open = false;
                    }
                    cx.emit(ChatPanelEvent::ChatAction(
                        Box::new(session.clone()),
                        action,
                    ));
                    cx.notify();
                });
            })
    };
    // The row's right end is one place: when the chat was last used (in
    // the whole list), and over it, while the row is pointed at, what
    // can be done with it. So a title is cut at the same place in every
    // row, pointed at or not.
    let actions = done.map(|done| {
        h_flex()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .gap_0p5()
            .invisible()
            .group_hover(group.clone(), |style| style.visible())
            .child(match done {
                false => action("done", Icons::Check, "Mark as done", ChatAction::ToggleDone),
                true => action("done", Icons::Undo2, "Not done", ChatAction::ToggleDone),
            })
            .child(action(
                "rename",
                Icons::Pencil,
                "Rename",
                ChatAction::Rename,
            ))
            // Not under an agent that works in it.
            .when(!busy, |this| {
                this.child(action("delete", Icons::Trash, "Delete", ChatAction::Delete))
            })
    });
    let has_actions = actions.is_some();
    let end = h_flex()
        .relative()
        .flex_none()
        .h_full()
        .justify_end()
        // Room for the three buttons, whatever stands under them.
        .when(has_actions, |this| this.min_w(ph_ui::rems_from_px(72.)))
        .when(dated, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .when(has_actions, |this| {
                        this.group_hover(group.clone(), |style| style.invisible())
                    })
                    .child(ph_ui::time::age(session.updated_at)),
            )
        })
        .children(actions);
    let open = weak.clone();
    h_flex()
        .id(("history-row", id as u64))
        .role(gpui_kit::accesskit::Role::Button)
        .aria_label(SharedString::from(session.title.clone()))
        .group(group.clone())
        .h_8()
        .px_2()
        .gap_2()
        .rounded(theme.radius)
        .cursor_pointer()
        .text_sm()
        .when(front, |this| this.bg(theme.list_active))
        .hover(|style| style.bg(theme.list_hover))
        .on_click(move |_, _, cx| {
            _ = open.update(cx, |this, cx| this.open_from_history(id, cx));
        })
        .child(
            div()
                .flex_none()
                .child(agent.mark(ph_ui::rems_from_px(14.))),
        )
        .children(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                // A chat marked done steps back; it is still one click away.
                .when(done == Some(true), |this| {
                    this.text_color(theme.muted_foreground)
                })
                .child(session.title.clone()),
        )
        .child(end)
        .test_support()
        .into_any_element()
}
