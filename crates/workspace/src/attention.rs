//! Telling the user when an agent needs them or has news, wherever they
//! are: a system notification when a chat starts waiting for an answer,
//! finishes or fails while not on screen, and (on macOS) the count of chats
//! waiting for an answer on the Dock icon.
//!
//! One watcher for the app, whatever the number of windows: it looks at the
//! chats every workspace keeps loaded, and the agents running in terminals,
//! once a second and acts on changes.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{App, Global, SharedString, SystemNotification, Task};
use ph_agent::{ChatHub, ChatState};
use ph_store::projects::ProjectId;
use ph_store::sessions::SessionId;

const TAG_PREFIX: &str = "chat-";
const TERMINAL_TAG_PREFIX: &str = "terminal-agent-";

/// Whose state is watched.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Watched {
    Chat(SessionId),
    /// An agent in a terminal, by pane.
    Terminal(String),
}

/// The watcher, kept for the app's life.
struct Attention {
    /// Each chat's (and terminal agent's) state at the last look.
    seen: HashMap<Watched, ChatState>,
    /// The badge on the Dock icon, as last set.
    badge: usize,
    _poll: Task<()>,
}

impl Global for Attention {}

/// Starts watching (once).
pub(crate) fn install(cx: &mut App) {
    if cx.has_global::<Attention>() {
        return;
    }
    // For the app's life.
    let poll = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            cx.update(look);
        }
    });
    cx.set_global(Attention {
        seen: HashMap::new(),
        badge: 0,
        _poll: poll,
    });
}

/// The workspace and terminal agent (pane) a notification's tag names.
pub(crate) fn terminal_of_tag(tag: &str) -> Option<(ProjectId, String)> {
    let (project, pane) = tag.strip_prefix(TERMINAL_TAG_PREFIX)?.split_once('-')?;
    Some((project.parse().ok()?, pane.to_string()))
}

/// The workspace and chat a notification's tag names.
pub(crate) fn chat_of_tag(tag: &str) -> Option<(ProjectId, SessionId)> {
    let (project, session) = tag.strip_prefix(TAG_PREFIX)?.split_once('-')?;
    Some((project.parse().ok()?, session.parse().ok()?))
}

fn look(cx: &mut App) {
    crate::terminal_agents::sync(cx);
    let mut notes = Vec::new();
    let mut now = HashMap::new();
    let mut waiting = 0;
    for (project, chats) in ChatHub::all(cx) {
        let chats = chats.read(cx);
        for (session, summary) in chats.live(cx) {
            if summary.state == ChatState::NeedsYou {
                waiting += 1;
            }
            let before = cx
                .global::<Attention>()
                .seen
                .get(&Watched::Chat(session))
                .copied()
                .unwrap_or(ChatState::Idle);
            if let Some(title) = news(before, summary.state, chats.is_viewing(session)) {
                notes.push(SystemNotification {
                    tag: SharedString::from(format!("{TAG_PREFIX}{project}-{session}")),
                    title: format!("{}: {title}", summary.title).into(),
                    body: summary.line.clone().unwrap_or_default().into(),
                    actions: Vec::new(),
                });
            }
            now.insert(Watched::Chat(session), summary.state);
        }
    }
    // Agents in terminals: watched when theirs is the terminal in front
    // and PilotHouse is.
    let app_active = cx.active_window().is_some();
    for agent in crate::terminal_agents::TerminalAgents::all(cx) {
        if agent.state == ChatState::NeedsYou {
            waiting += 1;
        }
        let key = Watched::Terminal(agent.pane.clone());
        let before = cx
            .global::<Attention>()
            .seen
            .get(&key)
            .copied()
            .unwrap_or(ChatState::Idle);
        let in_front = app_active
            && agent.terminal.upgrade().is_some_and(|terminal| {
                ph_terminal::TerminalHub::existing(agent.project, cx)
                    .is_some_and(|terminals| terminals.read(cx).active() == Some(&terminal))
            });
        if let Some(title) = news(before, agent.state, in_front) {
            let name = agent
                .title
                .clone()
                .unwrap_or_else(|| agent.agent.name.to_string());
            notes.push(SystemNotification {
                tag: SharedString::from(format!(
                    "{TERMINAL_TAG_PREFIX}{}-{}",
                    agent.project, agent.pane
                )),
                title: format!("{name}: {title}").into(),
                body: agent.line.clone().unwrap_or_default().into(),
                actions: Vec::new(),
            });
        }
        now.insert(key, agent.state);
    }
    let attention = cx.global_mut::<Attention>();
    attention.seen = now;
    let badge_changed = attention.badge != waiting;
    attention.badge = waiting;
    if badge_changed {
        set_badge(waiting);
    }
    for note in notes {
        cx.show_system_notification(note);
    }
}

/// What to say when a chat goes from `before` to `after`, if anything: a
/// question, or the end of a turn the user is not watching.
fn news(before: ChatState, after: ChatState, viewing: bool) -> Option<&'static str> {
    if before == after || viewing {
        return None;
    }
    match after {
        ChatState::NeedsYou => Some("needs your answer"),
        ChatState::Done if before == ChatState::Working => Some("finished"),
        ChatState::Failed if before == ChatState::Working => Some("stopped with an error"),
        _ => None,
    }
}

/// The number of chats waiting for an answer, on the Dock icon (none when
/// zero).
#[cfg(target_os = "macos")]
fn set_badge(count: usize) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    let app = NSApplication::sharedApplication(main);
    app.dockTile().setBadgeLabel(label.as_deref());
}

#[cfg(not(target_os = "macos"))]
fn set_badge(_: usize) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_news_is_announced() {
        use ChatState::*;
        assert_eq!(news(Working, Done, false), Some("finished"));
        assert_eq!(news(Working, NeedsYou, false), Some("needs your answer"));
        assert_eq!(news(Working, Failed, false), Some("stopped with an error"));
        assert_eq!(news(Working, Done, true), None, "the user is watching it");
        assert_eq!(news(Idle, Done, false), None, "news from before this run");
        assert_eq!(news(Done, Done, false), None);
    }

    #[test]
    fn tags_name_the_chat() {
        assert_eq!(chat_of_tag("chat-3-17"), Some((3, 17)));
        assert_eq!(chat_of_tag("todo-3"), None);
        assert_eq!(
            terminal_of_tag("terminal-agent-3-p17"),
            Some((3, "p17".to_string()))
        );
    }
}
