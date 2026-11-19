//! The tabs of a workspace's chat: its chats that are still open (one needs
//! the user, is being worked on, or waits for a look), each with its state.
//! Several agents at work in one workspace are a click apart, and what each
//! is doing shows without leaving for the Bridge.
//!
//! They are the workspace's cards of the Bridge, in a row: a chat marked
//! done there leaves the row, and marking one done here puts it under
//! *Done* there. They stand in the chat panel's own header, where a chat's
//! title would be (the panel asks for them when it draws): one row says
//! which chats there are, which is in front and what can be done with it.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::{WeakEntity, Window};
use ph_agent::ChatState;

use super::*;
use crate::bridge::OpenChat;

/// A tab is no wider than this (its title is cut).
const TAB_MAX: f32 = 220.;

/// The row of tabs of `screen`'s chat, for the chat panel's header.
pub(super) fn render(
    screen: &WeakEntity<WorkspaceScreen>,
    _: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let this = screen.upgrade()?;
    let (shell, project, front, open) = {
        let this = this.read(cx);
        let shell = this.shell.upgrade()?;
        let front = this.active_session;
        let open =
            crate::bridge::open_chats(shell.read(cx), this.project.id, &this.sessions, front, cx);
        // The tab of the chat in front is scrolled into view when the chat
        // changes, not on every draw: the row stays where it was dragged.
        if this.tabs_front.replace(front) != front
            && let Some(at) = open.iter().position(|chat| Some(chat.session.id) == front)
        {
            this.tabs_scroll.scroll_to_item(at);
        }
        (this.shell.clone(), this.project.id, front, open)
    };
    let scroll = this.read(cx).tabs_scroll.clone();
    let tabs: Vec<AnyElement> = open
        .iter()
        .map(|chat| chat_tab(chat, front, project, screen, &shell, cx))
        .collect();
    // A chat not sent yet is in front: it has no tab of its own to show as
    // chosen, so the row says so.
    let unsent = front.is_none().then(|| {
        h_flex()
            .id("chat-tab-new")
            .flex_none()
            .h_7()
            .px_2()
            .gap_1p5()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .text_sm()
            .child(Icon::new(Icons::SquarePen).xsmall())
            .child("New chat")
    });
    Some(
        h_flex()
            .id("chat-tabs")
            .w_full()
            .gap_0p5()
            .overflow_x_scroll()
            .track_scroll(&scroll)
            .children(tabs)
            .children(unsent)
            .test_support()
            .into_any_element(),
    )
}

fn chat_tab(
    chat: &OpenChat,
    front: Option<SessionId>,
    project: ph_store::projects::ProjectId,
    screen: &WeakEntity<WorkspaceScreen>,
    shell: &WeakEntity<crate::workspace::Workspace>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let id = chat.session.id;
    let state = chat.summary.state;
    let selected = front == Some(id);
    let busy = matches!(state, ChatState::Working | ChatState::NeedsYou);
    let title = chat.summary.title.clone();
    // Where it works too, when that is a folder of its own (a worktree):
    // the header's place for that went with its second row.
    let folder = chat
        .session
        .cwd
        .as_ref()
        .map(|cwd| format!(" · in {cwd}"))
        .unwrap_or_default();
    let tip = SharedString::from(format!(
        "{title} · {} · {}{folder}",
        state_label(state),
        chat.summary.agent.name
    ));
    let group = SharedString::from(format!("chat-tab-{id}"));
    // Put away from here: under Done on the Bridge, and out of the row.
    let done = (!busy && !chat.settled).then(|| {
        let shell = shell.clone();
        div()
            .flex_none()
            .invisible()
            .group_hover(group.clone(), |style| style.visible())
            .child(
                Button::new(("chat-tab-done", id as u64))
                    .ghost()
                    .xsmall()
                    .icon(Icons::Check)
                    .tooltip("Mark as done")
                    .accessibility_label("Mark as done")
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        _ = shell.update(cx, |shell, cx| shell.settle_all(&[(project, id)], cx));
                    }),
            )
    });
    let menu = {
        let (shell, session) = (shell.clone(), chat.session.clone());
        let marks = (chat.pinned, chat.settled);
        move |menu, _: &mut Window, _: &mut Context<gpui_kit::component::menu::PopupMenu>| {
            crate::bridge::chat_menu_items(menu, &shell, project, &session, marks, busy)
        }
    };
    let show = screen.clone();
    h_flex()
        .id(("chat-tab", id as u64))
        .group(group)
        .flex_none()
        .max_w(ph_ui::rems_from_px(TAB_MAX))
        .h_7()
        .pl_2()
        .pr_1()
        .gap_1p5()
        .rounded(theme.radius)
        .text_sm()
        .cursor_pointer()
        .map(|this| {
            if selected {
                this.bg(theme.secondary)
                    .text_color(theme.foreground)
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
            } else {
                this.text_color(theme.muted_foreground)
                    .hover(|style| style.bg(theme.muted))
            }
        })
        .aria_label(tip.clone())
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .on_click(move |_, window, cx| {
            _ = show.update(cx, |this, cx| {
                this.select_session(id, cx);
                this.focus_chat(window, cx);
            });
        })
        .child(state_mark(state, cx))
        .child(div().min_w_0().truncate().child(title))
        // A tab with nothing to press is as wide on the right as one with:
        // the titles line up.
        .child(match done {
            Some(done) => done.into_any_element(),
            None => div().flex_none().w_1().into_any_element(),
        })
        .test_support()
        .context_menu(menu)
        .into_any_element()
}

/// A chat's state in a word, for a tab's tooltip.
fn state_label(state: ChatState) -> &'static str {
    match state {
        ChatState::NeedsYou => "Needs you",
        ChatState::Working => "Working",
        ChatState::Failed => "Stopped with an error",
        ChatState::Done => "Finished, not looked at",
        ChatState::Idle => "Waits for a look",
    }
}

/// A chat's state before its title: at work, or a dot in the colour the
/// Bridge's columns have.
fn state_mark(state: ChatState, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = match state {
        ChatState::Working => {
            return div()
                .flex_none()
                .child(ph_ui::Loader::new().xsmall())
                .into_any_element();
        }
        ChatState::NeedsYou => theme.warning,
        ChatState::Failed => theme.danger,
        ChatState::Done => theme.success,
        ChatState::Idle => theme.muted_foreground.opacity(0.4),
    };
    div()
        .flex_none()
        .size(ph_ui::rems_from_px(7.))
        .rounded_full()
        .bg(color)
        .into_any_element()
}
