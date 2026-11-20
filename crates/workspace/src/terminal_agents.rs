//! Coding agents run as their own CLI in a workspace terminal, followed by
//! PilotHouse. Their state shows on the terminal tab, on the Bridge
//! and in notifications, like a chat's. How PilotHouse knows, best first:
//!
//! 1. **Hooks** (Claude Code started from PilotHouse): per-launch settings
//!    (`claude --settings <file>`, nothing written to `~/.claude`) whose
//!    command, `pilothouse agent-hook`, passes each event to the app
//!    (`agent.event`): the prompt, each tool, the question, the last reply.
//! 2. **Program status** (OSC 7501, see `ph_terminal::program_status`):
//!    Codex, Antigravity and Claude Code report their state in their output,
//!    wherever they were started, even by hand.
//! 3. **The title** (Gemini CLI): ◇ ready, ✦ working, ✋ action required.
//!
//! [`sync`] looks at every terminal once a second for 2 and 3.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::{App, AppContext as _, Entity, Global, WeakEntity};
use ph_agent::ChatState;
use ph_agent::agents::AgentSpec;
use ph_store::paths;
use ph_store::projects::ProjectId;
use ph_store::sessions::SessionId;
use ph_terminal::{AgentBadge, Terminal, WorkspaceTerminals};
use serde_json::Value;

/// How much of a session file's end is read for the last reply.
const TRANSCRIPT_TAIL: u64 = 256 * 1024;

/// How PilotHouse follows an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Hooks,
    ProgramStatus,
    Title,
    /// Started from PilotHouse, saying nothing in its terminal (yet):
    /// known while its terminal is open, its session found in its files.
    Launched,
}

/// One agent CLI in a terminal.
pub(crate) struct TerminalAgent {
    /// Its id: in hook commands (`--pane`), else from its terminal.
    pub pane: String,
    pub source: Source,
    pub project: ProjectId,
    pub terminal: WeakEntity<Terminal>,
    pub agent: &'static AgentSpec,
    pub state: ChatState,
    /// The first thing asked, as its name.
    pub title: Option<String>,
    /// Whether what the program calls itself (its status' title, the
    /// title it gives its terminal) is its name: so for an agent started
    /// by hand in a terminal. One started as a chat's CLI is not named by
    /// its terminal: a chat named "zsh" says nothing.
    pub named_by_program: bool,
    /// What it is doing, asks, or said last.
    pub line: Option<String>,
    /// The agent's own session id and its session file.
    pub session_id: Option<String>,
    pub transcript: Option<PathBuf>,
    /// What it was started with beside the user's own environment (its
    /// account): where its files are is read from this.
    pub environment: Vec<(String, String)>,
    pub updated_at: i64,
    /// When PilotHouse first saw it (Unix milliseconds).
    pub since: i64,
    /// Its chat in the workspace's list (kind `terminal`), once it was
    /// asked something: it stays there after the terminal closes.
    pub session: Option<SessionId>,
    /// What [`remember`] last saved of it.
    saved: Saved,
    /// When its session file was last looked for (Unix milliseconds), and
    /// how many times in vain: each look waits twice as long as the last.
    looked_at: i64,
    looks: u32,
}

/// How often a working agent's chat is marked active in the store (its
/// row shows the live agent meanwhile; the last time is saved as it goes).
const TOUCH_EVERY: i64 = 30_000;

impl TerminalAgent {
    /// It was asked something: it has a chat.
    fn asked(&self) -> bool {
        self.title.is_some() || self.session_id.is_some() || self.state != ChatState::Idle
    }

    /// Whether its chat lacks something [`remember`] saves.
    fn unsaved(&self) -> bool {
        self.asked()
            && (self.session.is_none()
                || (self.title.is_some() && self.saved.title != self.title)
                || (self.session_id.is_some() && self.saved.session_id != self.session_id)
                || self.touch_due()
                || self.line_due())
    }

    /// When it did something since its chat was last marked active, at
    /// most every [`TOUCH_EVERY`].
    fn touch_due(&self) -> bool {
        self.updated_at > self.saved.at
            && (self.saved.at == 0 || self.updated_at - self.saved.at >= TOUCH_EVERY)
    }

    /// What it said at the end of a turn, not saved yet (hooks say; others
    /// are read from their file once it is gone).
    fn line_due(&self) -> bool {
        matches!(self.state, ChatState::Done | ChatState::Idle)
            && self.line.is_some()
            && self.saved.line != self.line
    }
}

/// What was saved of a terminal agent's chat, to save only changes.
#[derive(Default)]
struct Saved {
    title: Option<String>,
    session_id: Option<String>,
    at: i64,
    line: Option<String>,
}

#[derive(Default)]
pub(crate) struct TerminalAgents {
    agents: Vec<TerminalAgent>,
    /// Sessions resumed in a terminal whose agent has not said anything
    /// yet, by pane: they become its `session_id` once it does.
    resumed: HashMap<String, String>,
}

impl Global for TerminalAgents {}

impl TerminalAgents {
    /// Every agent followed, read without marking the list changed.
    fn list(cx: &App) -> &[TerminalAgent] {
        cx.try_global::<Self>()
            .map(|agents| agents.agents.as_slice())
            .unwrap_or_default()
    }

    /// The agents whose terminal is still open.
    pub(crate) fn all(cx: &App) -> Vec<&TerminalAgent> {
        cx.try_global::<Self>()
            .map(|agents| {
                agents
                    .agents
                    .iter()
                    .filter(|agent| agent.terminal.upgrade().is_some())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The agent running in `terminal`.
    pub(crate) fn of_terminal<'a>(
        terminal: &Entity<Terminal>,
        cx: &'a App,
    ) -> Option<&'a TerminalAgent> {
        cx.try_global::<Self>()?
            .agents
            .iter()
            .find(|agent| agent.terminal.upgrade().as_ref() == Some(terminal))
    }

    /// The agent running in `pane`.
    pub(crate) fn find<'a>(pane: &str, cx: &'a App) -> Option<&'a TerminalAgent> {
        cx.try_global::<Self>()?
            .agents
            .iter()
            .find(|agent| agent.pane == pane)
    }

    /// The agent running in a terminal for chat `session` (asked for each
    /// terminal chat the sidebar lists: nothing is collected).
    pub(crate) fn of_session(session: SessionId, cx: &App) -> Option<&TerminalAgent> {
        Self::list(cx)
            .iter()
            .find(|agent| agent.session == Some(session) && agent.terminal.upgrade().is_some())
    }

    /// The agent running agent `agent`'s own session `session`, if one is.
    pub(crate) fn of_cli_session<'a>(
        agent: &str,
        session: &str,
        cx: &'a App,
    ) -> Option<&'a TerminalAgent> {
        Self::list(cx).iter().find(|running| {
            running.agent.id == agent
                && running.session_id.as_deref() == Some(session)
                && running.terminal.upgrade().is_some()
        })
    }

    /// The user looked at agent `pane`: its news is seen.
    pub(crate) fn seen(pane: &str, cx: &mut App) {
        let Some(agent) = cx
            .default_global::<Self>()
            .agents
            .iter_mut()
            .find(|agent| agent.pane == pane && agent.state == ChatState::Done)
        else {
            return;
        };
        agent.state = ChatState::Idle;
        let terminal = agent.terminal.clone();
        _ = terminal.update(cx, |terminal, cx| terminal.acknowledge(cx));
    }
}

/// The id of an agent found from its terminal.
fn terminal_pane(terminal: &Entity<Terminal>) -> String {
    format!("t{}", terminal.entity_id())
}

/// The agent an OSC 7501 `app` (or a terminal's name) means.
fn agent_named(name: &str) -> Option<&'static AgentSpec> {
    let name = name.to_lowercase();
    let id = if name.contains("claude") {
        "claude"
    } else if name.contains("codex") {
        "codex"
    } else if name.contains("gemini") {
        "gemini"
    } else if name.contains("agy") || name.contains("antigravity") {
        "antigravity"
    } else if name.contains("cline") {
        "cline"
    } else {
        return None;
    };
    ph_agent::agents::find(id)
}

/// Gemini CLI's state from its title: `◇  Ready (folder)`,
/// `✦ …`/`⏲  Working…`, `✋  Action Required`.
fn title_state(title: &str) -> Option<ChatState> {
    match title.trim_start().chars().next()? {
        '\u{270B}' => Some(ChatState::NeedsYou),
        '\u{25C7}' => Some(ChatState::Idle),
        '\u{2726}' | '\u{23F2}' => Some(ChatState::Working),
        _ => None,
    }
}

/// What a terminal's agent is and does, read from the terminal.
struct Found {
    agent: &'static AgentSpec,
    source: Source,
    state: ChatState,
    line: Option<String>,
    title: Option<String>,
}

/// Reads agent `known` (its last state, if followed already) from
/// `terminal`: its program status, else a Gemini CLI's title.
fn read_terminal(terminal: &Terminal, known: Option<ChatState>) -> Option<Found> {
    use ph_terminal::program_status::ProgramState;
    let label = terminal.label().map(|label| label.to_string());
    if let Some(status) = terminal.program_status() {
        let agent = status
            .app
            .as_deref()
            .and_then(agent_named)
            .or_else(|| label.as_deref().and_then(agent_named))?;
        let state = match (terminal.badge(), status.state) {
            (Some(AgentBadge::Done), _) => ChatState::Done,
            (_, ProgramState::Working) => ChatState::Working,
            (_, ProgramState::Blocked) => ChatState::NeedsYou,
            (_, ProgramState::Error) => ChatState::Failed,
            _ => ChatState::Idle,
        };
        // Its own title, else the one it gave the terminal (Claude names it
        // after the session), without a leading status glyph.
        let terminal_title = terminal
            .program_title()
            .trim_start_matches(|c: char| !c.is_alphanumeric())
            .trim()
            .to_string();
        let title = status.title.clone().or_else(|| {
            (!terminal_title.is_empty() && Some(terminal_title.as_str()) != label.as_deref())
                .then_some(terminal_title)
        });
        return Some(Found {
            agent,
            source: Source::ProgramStatus,
            state,
            line: status.msg.clone(),
            title,
        });
    }
    if !label
        .as_deref()
        .is_some_and(|label| label.starts_with("Gemini CLI"))
    {
        return None;
    }
    let state = match (known, title_state(terminal.program_title())?) {
        // Ready after work is news until looked at.
        (Some(ChatState::Working), ChatState::Idle) => ChatState::Done,
        (Some(ChatState::Done), ChatState::Idle) if terminal.badge() == Some(AgentBadge::Done) => {
            ChatState::Done
        }
        (_, state) => state,
    };
    Some(Found {
        agent: agent_named("gemini")?,
        source: Source::Title,
        state,
        line: None,
        title: None,
    })
}

/// Finds agents from what their terminals say (program status, Gemini's
/// title) and brings their state up to date. Called once a second.
///
/// The list is only written when something changed: taking it to write
/// marks it changed, and every view that shows it is drawn again.
pub(crate) fn sync(cx: &mut App) {
    // A hooked agent's news is seen once its tab was looked at (that
    // clears the tab's dot).
    let seen_news: Vec<String> = TerminalAgents::list(cx)
        .iter()
        .filter(|agent| {
            agent.source == Source::Hooks
                && agent.state == ChatState::Done
                && agent
                    .terminal
                    .upgrade()
                    .is_some_and(|terminal| terminal.read(cx).badge() != Some(AgentBadge::Done))
        })
        .map(|agent| agent.pane.clone())
        .collect();
    if !seen_news.is_empty() {
        for agent in &mut cx.default_global::<TerminalAgents>().agents {
            if seen_news.contains(&agent.pane) {
                agent.state = ChatState::Idle;
            }
        }
    }
    let mut seen_panes = Vec::new();
    for (project, terminals) in ph_terminal::TerminalHub::all(cx) {
        let list = terminals.read(cx).terminals().to_vec();
        for terminal in list {
            let pane = terminal_pane(&terminal);
            let agents = TerminalAgents::list(cx);
            let hooked = agents.iter().any(|agent| {
                agent.source == Source::Hooks
                    && agent.terminal.upgrade().as_ref() == Some(&terminal)
            });
            if hooked {
                continue;
            }
            let known = agents.iter().find(|agent| agent.pane == pane);
            let Some(found) = read_terminal(terminal.read(cx), known.map(|agent| agent.state))
            else {
                continue;
            };
            seen_panes.push(pane.clone());
            if found.source == Source::Title {
                let badge = match found.state {
                    ChatState::Working => Some(AgentBadge::Working),
                    ChatState::NeedsYou => Some(AgentBadge::NeedsYou),
                    ChatState::Done => Some(AgentBadge::Done),
                    _ => None,
                };
                terminal.update(cx, |terminal, cx| terminal.set_badge(badge, cx));
            }
            if TerminalAgents::list(cx)
                .iter()
                .find(|agent| agent.pane == pane)
                .is_some_and(|agent| !found.changes(agent))
            {
                continue;
            }
            let agents = cx.default_global::<TerminalAgents>();
            match agents.agents.iter_mut().find(|agent| agent.pane == pane) {
                Some(agent) => {
                    if agent.state != found.state {
                        // Asked something: its session is worth looking
                        // for again soon.
                        agent.looks = 0;
                    }
                    if agent.state != found.state
                        || (found.line.is_some() && agent.line != found.line)
                    {
                        agent.updated_at = now();
                    }
                    agent.source = found.source_for(agent);
                    agent.state = found.state;
                    // A program's line is as it says now (none once done).
                    if found.line.is_some() || found.source == Source::ProgramStatus {
                        agent.line = found.line;
                    }
                    // What the program calls itself names an agent
                    // started by hand. A session started from a chat is
                    // named as chats are: it keeps the chat's name, or
                    // takes the first thing asked (its hooks, its file).
                    if found.title.is_some() && agent.named_by_program {
                        agent.title = found.title;
                    }
                }
                None => {
                    let session_id = agents.resumed.remove(&pane);
                    agents.agents.push(TerminalAgent {
                        pane,
                        source: found.source,
                        project,
                        terminal: terminal.downgrade(),
                        agent: found.agent,
                        state: found.state,
                        title: found.title,
                        named_by_program: true,
                        line: found.line,
                        session_id,
                        transcript: None,
                        // Started by hand: in the user's own environment.
                        environment: Vec::new(),
                        updated_at: now(),
                        since: now(),
                        session: None,
                        saved: Saved::default(),
                        looked_at: 0,
                        looks: 0,
                    })
                }
            }
        }
    }
    // Agents whose terminal went, or whose program stopped saying, go
    // (their chats stay in the list, with what they said last).
    let stays = |agent: &TerminalAgent| {
        agent.terminal.upgrade().is_some()
            && (matches!(agent.source, Source::Hooks | Source::Launched)
                || seen_panes.contains(&agent.pane))
    };
    let gone: Vec<Gone> = TerminalAgents::list(cx)
        .iter()
        .filter(|agent| !stays(agent))
        .map(|agent| Gone {
            session: agent.session,
            agent: agent.agent.id,
            cli: agent.session_id.clone(),
            transcript: agent.transcript.clone(),
            at: (agent.updated_at > agent.saved.at).then_some(agent.updated_at),
        })
        .collect();
    if !gone.is_empty() {
        cx.default_global::<TerminalAgents>()
            .agents
            .retain(|agent| stays(agent));
        let store = crate::AppState::store(cx).clone();
        for gone in gone {
            let Some(session) = gone.session else {
                continue;
            };
            // When it last did something, saved at most every so often
            // while it ran.
            if let Some(at) = gone.at {
                let _ = store.touch_session(session, at);
            }
            keep_last_reply(session, gone.agent, gone.cli, gone.transcript, cx);
        }
    }
    remember(cx);
    find_sessions(cx);
}

/// An agent no longer followed, and what is kept of it.
struct Gone {
    session: Option<SessionId>,
    agent: &'static str,
    cli: Option<String>,
    transcript: Option<PathBuf>,
    /// When it last did something, if not saved yet.
    at: Option<i64>,
}

impl Found {
    /// How `agent` is followed from now: one started from PilotHouse stays
    /// followed while its terminal is open, even once it stops saying.
    fn source_for(&self, agent: &TerminalAgent) -> Source {
        if agent.source == Source::Launched {
            Source::Launched
        } else {
            self.source
        }
    }

    /// Whether this says anything new of `agent`.
    fn changes(&self, agent: &TerminalAgent) -> bool {
        let line_changes = (self.line.is_some() || self.source == Source::ProgramStatus)
            && agent.line != self.line;
        let title_changes = self.title.is_some() && agent.title != self.title;
        self.source_for(agent) != agent.source
            || self.state != agent.state
            || line_changes
            || title_changes
    }
}

/// Saves each agent that was asked something as a chat of its workspace
/// (kind `terminal`): its title, its own session (to resume it) and when it
/// last did something, so it stays in the list once its terminal closes. A
/// session resumed in a terminal goes on in the chat it had.
fn remember(cx: &mut App) {
    // Most seconds nothing is new: the list is then left unmarked.
    if !TerminalAgents::list(cx).iter().any(TerminalAgent::unsaved) {
        return;
    }
    let store = crate::AppState::store(cx).clone();
    let mut agents = std::mem::take(&mut cx.default_global::<TerminalAgents>().agents);
    let mut changed = Vec::new();
    for agent in &mut agents {
        if !agent.unsaved() {
            continue;
        }
        let title = agent
            .title
            .clone()
            .unwrap_or_else(|| format!("{} in a terminal", agent.agent.name));
        let id = match agent.session {
            Some(id) => id,
            None => {
                let earlier = agent.session_id.as_ref().and_then(|cli| {
                    store
                        .sessions(agent.project)
                        .ok()?
                        .into_iter()
                        .find(|session| {
                            session.kind == "terminal"
                                && session.agent_id == agent.agent.id
                                && session.acp_session_id.as_deref() == Some(cli.as_str())
                        })
                });
                let id = match earlier {
                    // Resumed: it goes on under the name it had.
                    Some(session) => {
                        if agent.title.is_none() {
                            agent.title = Some(session.title.clone());
                            agent.saved.title = agent.title.clone();
                        }
                        session.id
                    }
                    None => {
                        let cwd = agent
                            .terminal
                            .upgrade()
                            .map(|terminal| terminal.read(cx).cwd().to_string_lossy().into_owned());
                        let Ok(session) =
                            store.create_session(agent.project, &title, agent.agent.id)
                        else {
                            continue;
                        };
                        let _ = store.set_session_place(session.id, "terminal", cwd.as_deref());
                        session.id
                    }
                };
                agent.session = Some(id);
                changed.push(agent.project);
                id
            }
        };
        if agent.title.is_some() && agent.saved.title != agent.title {
            let _ = store.rename_session(id, &title);
            agent.saved.title = agent.title.clone();
            changed.push(agent.project);
        }
        if agent.session_id.is_some() && agent.saved.session_id != agent.session_id {
            let _ = store.set_session_acp_id(id, agent.session_id.as_deref());
            agent.saved.session_id = agent.session_id.clone();
        }
        if agent.touch_due() {
            let _ = store.touch_session(id, agent.updated_at);
            agent.saved.at = agent.updated_at;
            changed.push(agent.project);
        }
        if agent.line_due() {
            let _ = store.set_session_line(id, agent.line.as_deref());
            agent.saved.line = agent.line.clone();
            changed.push(agent.project);
        }
    }
    let state = cx.default_global::<TerminalAgents>();
    // Agents found meanwhile (none: this runs on the UI thread) stay.
    agents.append(&mut state.agents);
    state.agents = agents;
    for project in changed {
        crate::session_changes::note(project, cx);
    }
}

/// Once an agent's terminal closed, keeps what its session said with its
/// chat: the last thing it said (under its title), and what was asked and
/// answered, for Search chats. Read from its session file (found by its id
/// in its folder when not known), off the UI thread.
fn keep_last_reply(
    session: SessionId,
    agent: &'static str,
    cli: Option<String>,
    transcript: Option<PathBuf>,
    cx: &mut App,
) {
    let Some(format) = ph_agent::cli_transcript::Format::of(agent) else {
        return;
    };
    let store = crate::AppState::store(cx).clone();
    let project = store
        .session(session)
        .ok()
        .flatten()
        .map(|row| row.project_id);
    cx.spawn(async move |cx| {
        let job_store = store.clone();
        let line = cx
            .background_spawn(async move {
                let path = transcript.or_else(|| {
                    let row = job_store.session(session).ok().flatten()?;
                    // Its files are where its account keeps them.
                    let environment = ph_agent::profiles::Profiles::load(&job_store)
                        .of(agent)
                        .environment_for(&row.account);
                    let (cwd, cli) = (PathBuf::from(row.cwd?), cli?);
                    ph_agent::cli_sessions::sessions_in(agent, &cwd, &environment)
                        .into_iter()
                        .find(|past| past.id == cli)?
                        .path
                })?;
                let said = ph_agent::cli_transcript::said(format, &path);
                let line = ph_agent::cli_transcript::first_line_of_last_reply(&said);
                // For Search: the start of each message is enough, and keeps
                // the write (under the store's lock) short for a long session.
                const KEPT_CHARS: usize = 8_000;
                let messages: Vec<(&str, String)> = said
                    .into_iter()
                    .map(|(role, text)| {
                        let text: String = text.chars().take(KEPT_CHARS).collect();
                        (role, serde_json::json!({ "text": text }).to_string())
                    })
                    .collect();
                let _ = job_store.replace_messages(session, &messages);
                if line.is_some() {
                    let _ = job_store.set_session_line(session, line.as_deref());
                }
                Some(())
            })
            .await;
        let (Some(()), Some(project)) = (line, project) else {
            return;
        };
        cx.update(|cx| crate::session_changes::note(project, cx));
    })
    .detach();
}

/// For agents whose own session is not known yet: looks for it in their
/// files (off the UI thread; 30 seconds after the last look, then twice as
/// long each time up to 10 minutes, again soon once it is asked something),
/// the newest in their folder since they started that no other agent and no
/// chat of the workspace has, so their chat is saved and can be resumed and
/// read once the terminal closes. One never asked anything has none, so it
/// is not saved.
fn find_sessions(cx: &mut App) {
    const FIRST: i64 = 30_000;
    const LONGEST: i64 = 600_000;
    let now = now();
    let due = |agent: &TerminalAgent| {
        let wait = (FIRST << agent.looks.min(5)).min(LONGEST);
        agent.session_id.is_none() && now - agent.looked_at >= wait
    };
    if !TerminalAgents::list(cx).iter().any(due) {
        return;
    }
    let mut jobs = Vec::new();
    let store = crate::AppState::store(cx).clone();
    let state = cx.default_global::<TerminalAgents>();
    let taken: Vec<String> = state
        .agents
        .iter()
        .filter_map(|agent| agent.session_id.clone())
        .collect();
    for agent in &mut state.agents {
        if !due(agent) {
            continue;
        }
        agent.looked_at = now;
        agent.looks = agent.looks.saturating_add(1);
        let Some(terminal) = agent.terminal.upgrade() else {
            continue;
        };
        jobs.push((
            agent.pane.clone(),
            agent.agent.id,
            agent.project,
            agent.since,
            terminal,
            agent.environment.clone(),
        ));
    }
    for (pane, agent_id, project, since, terminal, environment) in jobs {
        let cwd = terminal.read(cx).cwd().to_path_buf();
        let (taken, store) = (taken.clone(), store.clone());
        cx.spawn(async move |cx| {
            let found = cx
                .background_spawn(async move {
                    // The workspace's chats with this agent (ACP sessions
                    // are its CLI's own) are not this terminal's.
                    let chats: Vec<String> = store
                        .sessions(project)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|chat| chat.kind != "terminal" && chat.agent_id == agent_id)
                        .filter_map(|chat| chat.acp_session_id)
                        .collect();
                    ph_agent::cli_sessions::sessions_in(agent_id, &cwd, &environment)
                        .into_iter()
                        // Sessions keep their file's time; slack for the clock.
                        .find(|session| {
                            session.updated_at >= since - 5_000
                                && !taken.contains(&session.id)
                                && !chats.contains(&session.id)
                        })
                })
                .await;
            let Some(found) = found else {
                return;
            };
            cx.update(|cx| {
                if let Some(agent) = cx
                    .default_global::<TerminalAgents>()
                    .agents
                    .iter_mut()
                    .find(|agent| agent.pane == pane && agent.session_id.is_none())
                {
                    agent.session_id = Some(found.id);
                    if agent.transcript.is_none() {
                        agent.transcript = found.path;
                    }
                    // Agents that do not say what they were asked (Gemini).
                    if agent.title.is_none() && !found.title.is_empty() {
                        agent.title = Some(found.title);
                    }
                    if found.updated_at > agent.updated_at {
                        agent.updated_at = found.updated_at;
                    }
                }
            });
        })
        .detach();
    }
}

/// Answers `agent.event` from `pilothouse agent-hook`, and clears the
/// hook settings of an earlier run. The views that show agents read them
/// from the list, which `apply` marks changed: each window's next status
/// look (once a second) draws them again, so a burst of events (many a
/// second while an agent works) costs one redraw.
pub(crate) fn install(cx: &mut App) {
    let _ = std::fs::remove_dir_all(settings_dir());
    ph_control::ControlServer::handle(
        "agent.event",
        |params, cx| {
            let pane = params["pane"].as_str().unwrap_or_default().to_string();
            apply(&pane, &params["event"], cx);
            Ok(Value::Null)
        },
        cx,
    );
}

fn settings_dir() -> PathBuf {
    paths::data_dir().join("terminal-agents")
}

/// What an agent's CLI is started with.
pub(crate) struct Launch<'a> {
    pub agent_id: &'a str,
    pub project: ProjectId,
    /// Where it works.
    pub cwd: PathBuf,
    /// One of its own sessions to go on with.
    pub resume: Option<&'a str>,
    /// The chat this CLI is the other form of: what it does is kept with
    /// that chat, instead of a chat of its own. `named` says the chat has
    /// its name already, which the first thing asked here must not change.
    pub chat: Option<(SessionId, bool)>,
    /// The model and the mode to start with, as the agent names them in a
    /// chat (a session that changes form keeps both).
    pub model: Option<&'a str>,
    pub mode: Option<&'a str>,
}

/// The command that starts an agent's CLI: its program, the session to go
/// on with, the model and mode asked for, and `more` (hooks, PilotHouse's
/// own tools), each already written as the shell takes it.
fn command(
    start: &Launch,
    profile: &ph_agent::profiles::Profile,
    more: &[String],
) -> anyhow::Result<String> {
    let name = |id: &str| {
        ph_agent::agents::find(id).map_or_else(|| id.to_string(), |agent| agent.name.to_string())
    };
    let usual = ph_agent::agents::cli_of(start.agent_id)
        .ok_or_else(|| anyhow::anyhow!("{} has no CLI", name(start.agent_id)))?;
    // The user's own command for it (Settings → Agents), as they typed it:
    // a shell reads it, as it would in their terminal.
    let program = match profile.command.trim() {
        "" => usual,
        command => command,
    };
    // Only what could be a flag's value: these are typed into a shell.
    let plain = |value: &&str| {
        !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-:/[]".contains(c))
    };
    let model = start
        .model
        .filter(plain)
        .filter(|model| *model != "default");
    let mode = start.mode.filter(plain);
    let mut words = vec![program.to_string()];
    match start.agent_id {
        "claude" => {
            if let Some(session) = start.resume {
                words.extend(["--resume".into(), session.to_string()]);
            }
            if let Some(model) = model {
                words.extend(["--model".into(), model.to_string()]);
            }
            // The modes a chat and the CLI name alike; the plain one
            // ("default", the CLI's "manual") is what it starts in anyway.
            if let Some(mode) = mode.filter(|mode| {
                [
                    "acceptEdits",
                    "plan",
                    "bypassPermissions",
                    "auto",
                    "dontAsk",
                ]
                .contains(mode)
            }) {
                words.extend(["--permission-mode".into(), mode.to_string()]);
            }
        }
        "codex" => {
            if let Some(session) = start.resume {
                words.extend(["resume".into(), session.to_string()]);
            }
            if let Some(model) = model {
                words.extend(["--model".into(), model.to_string()]);
            }
        }
        "gemini" => {
            if let Some(session) = start.resume {
                words.extend(["--resume".into(), session.to_string()]);
            }
            if let Some(model) = model {
                words.extend(["--model".into(), model.to_string()]);
            }
        }
        "antigravity" => {
            if let Some(session) = start.resume {
                words.extend(["--conversation".into(), session.to_string()]);
            }
        }
        "cline" => {
            if let Some(session) = start.resume {
                words.extend(["--id".into(), session.to_string()]);
            }
        }
        _ => anyhow::bail!("{} cannot run in a terminal", name(start.agent_id)),
    }
    // What the user adds to every start of it, each word kept whole.
    words.extend(profile.arguments.iter().map(|argument| {
        let plain = !argument.is_empty()
            && argument
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-:/=@+,".contains(c));
        if plain {
            argument.clone()
        } else {
            quoted(Path::new(argument))
        }
    }));
    words.extend(more.iter().cloned());
    Ok(words.join(" "))
}

/// PilotHouse's own tools (the browser, devices, tests) for an agent's CLI,
/// as the flags it takes MCP servers by: a chat gets them when its session
/// is set up, and a session's CLI form must not have less. `file` is where
/// a CLI that reads its servers from a file finds them.
fn own_tools(agent_id: &str, cwd: &Path, project: ProjectId, file: &Path) -> Vec<String> {
    let Some((program, args, env)) = ph_agent::own_tools_command(cwd, agent_id, project) else {
        return Vec::new();
    };
    match agent_id {
        "claude" => {
            let servers = serde_json::json!({ "mcpServers": { "pilothouse": {
                "command": program,
                "args": args,
                "env": env.into_iter().collect::<HashMap<String, String>>(),
            } } });
            match serde_json::to_vec_pretty(&servers)
                .ok()
                .and_then(|json| std::fs::write(file, json).ok())
            {
                Some(()) => vec!["--mcp-config".into(), quoted(file)],
                None => Vec::new(),
            }
        }
        // Settings given for this run only (`-c key=value`, the value as
        // TOML): nothing is written to the user's own configuration.
        "codex" => {
            let text = |value: &str| serde_json::Value::from(value).to_string();
            let list = |values: &[String]| {
                let items: Vec<String> = values.iter().map(|value| text(value)).collect();
                format!("[{}]", items.join(","))
            };
            let mut flags = vec![
                "-c".to_string(),
                quoted(Path::new(&format!(
                    "mcp_servers.pilothouse.command={}",
                    text(&program.to_string_lossy())
                ))),
                "-c".to_string(),
                quoted(Path::new(&format!(
                    "mcp_servers.pilothouse.args={}",
                    list(&args)
                ))),
            ];
            for (name, value) in env {
                flags.push("-c".into());
                flags.push(quoted(Path::new(&format!(
                    "mcp_servers.pilothouse.env.{name}={}",
                    text(&value)
                ))));
            }
            flags
        }
        // The others read their servers from their own settings.
        _ => Vec::new(),
    }
}

/// Starts an agent's CLI in a new terminal of `terminals`, followed by
/// PilotHouse.
pub(crate) fn launch(
    start: Launch,
    terminals: &Entity<WorkspaceTerminals>,
    cx: &mut App,
) -> anyhow::Result<Entity<Terminal>> {
    let agent = ph_agent::agents::find(start.agent_id)
        .ok_or_else(|| anyhow::anyhow!("{} is not a known agent", start.agent_id))?;
    let start = Launch {
        resume: start.resume.filter(|session| is_session_id(session)),
        ..start
    };
    let pane = format!(
        "p{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or_default()
    );
    std::fs::create_dir_all(settings_dir())?;
    let mut more = Vec::new();
    // Claude Code tells PilotHouse what it does through hooks, set for
    // this run only; the others are followed by what they say in the
    // terminal (program status, title).
    let hooked = start.agent_id == "claude";
    if hooked {
        let settings = settings_dir().join(format!("{pane}.json"));
        std::fs::write(
            &settings,
            serde_json::to_vec_pretty(&hook_settings(&hook_command(&pane)?))?,
        )?;
        more.extend(["--settings".to_string(), quoted(&settings)]);
    }
    more.extend(own_tools(
        start.agent_id,
        &start.cwd,
        start.project,
        &settings_dir().join(format!("{pane}.mcp.json")),
    ));
    // As Settings → Agents says it runs, in the account its chat is in.
    let store = crate::AppState::store(cx).clone();
    let chat = start
        .chat
        .and_then(|(chat, _)| store.session(chat).ok().flatten());
    let profile = ph_agent::profiles::Profiles::load(&store).of(start.agent_id);
    let environment =
        profile.environment_for(chat.as_ref().map_or("", |chat| chat.account.as_str()));
    let command = command(&start, &profile, &more)?;
    let label = free_label(agent.name, terminals, cx);
    let terminal = terminals
        .update(cx, |terminals, cx| {
            terminals.run_program(&label, &command, start.cwd.clone(), &environment, cx)
        })
        .ok_or_else(|| anyhow::anyhow!("Could not open a terminal"))?;
    // A chat that has its name keeps it; its row is there already.
    let (session, title) = match start.chat {
        Some((id, named)) => (
            Some(id),
            named.then(|| chat.map(|chat| chat.title)).flatten(),
        ),
        None => (None, None),
    };
    // Known from the start, even if it never says how it is doing in its
    // output (Cline): its chat is saved once its session is found. (Agents
    // whose terminal closed are left to `sync`, which keeps what they
    // said.)
    cx.default_global::<TerminalAgents>()
        .agents
        .push(TerminalAgent {
            pane: if hooked {
                pane
            } else {
                terminal_pane(&terminal)
            },
            source: if hooked {
                Source::Hooks
            } else {
                Source::Launched
            },
            project: start.project,
            terminal: terminal.downgrade(),
            agent,
            state: ChatState::Idle,
            title: title.clone(),
            named_by_program: false,
            line: None,
            session_id: start.resume.map(str::to_string),
            transcript: None,
            environment,
            updated_at: now(),
            since: now(),
            session,
            saved: Saved {
                title,
                session_id: session.and(start.resume.map(str::to_string)),
                ..Saved::default()
            },
            looked_at: 0,
            looks: 0,
        });
    Ok(terminal)
}

/// Finds the session file of the agent in `terminal`: the one its hooks
/// name, the one it resumed, or the newest in its folder since it started.
/// The looking on disk is the returned job's, to run off the UI thread.
pub(crate) fn session_file(
    terminal: &WeakEntity<Terminal>,
    cx: &App,
) -> Option<ph_agent::cli_transcript::FindJob> {
    let terminal = terminal.upgrade()?;
    let agent = TerminalAgents::of_terminal(&terminal, cx)?;
    if let Some(path) = agent.transcript.clone() {
        return Some(Box::new(move || Some(path)));
    }
    let (agent_id, cwd) = (agent.agent.id, terminal.read(cx).cwd().to_path_buf());
    let (session_id, since) = (agent.session_id.clone(), agent.since);
    let environment = agent.environment.clone();
    Some(Box::new(move || {
        let sessions = ph_agent::cli_sessions::sessions_in(agent_id, &cwd, &environment);
        if let Some(session) = session_id
            .as_ref()
            .and_then(|id| sessions.iter().find(|session| &session.id == id))
            // A resumed session the user went on from (`/clear`) is older.
            .filter(|session| {
                !sessions
                    .iter()
                    .any(|newer| newer.updated_at > session.updated_at && newer.updated_at >= since)
            })
        {
            return session.path.clone();
        }
        // Sessions keep their file's time; a little slack for the clock.
        sessions
            .into_iter()
            .find(|session| session.updated_at >= since - 5_000)
            .and_then(|session| session.path)
    }))
}

/// `name`, or `name 2`, … : the first not taken by a terminal.
fn free_label(name: &str, terminals: &Entity<WorkspaceTerminals>, cx: &App) -> String {
    let taken: Vec<String> = terminals
        .read(cx)
        .terminals()
        .iter()
        .filter_map(|terminal| terminal.read(cx).label().map(|label| label.to_string()))
        .collect();
    (1..)
        .map(|n| match n {
            1 => name.to_string(),
            n => format!("{name} {n}"),
        })
        .find(|label| !taken.contains(label))
        .unwrap_or_else(|| name.to_string())
}

/// The hook command: this program, told where the app listens and which
/// terminal the agent runs in.
fn hook_command(pane: &str) -> anyhow::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!(
        "{} agent-hook --socket {} --pane {pane}",
        quoted(&exe),
        quoted(&paths::control_socket_path())
    ))
}

/// Claude Code settings with a hook on every event PilotHouse follows.
fn hook_settings(command: &str) -> Value {
    let hook =
        serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]);
    let tool_hook = serde_json::json!([{ "matcher": "*", "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]);
    serde_json::json!({
        "hooks": {
            "SessionStart": hook,
            "UserPromptSubmit": hook,
            "PreToolUse": tool_hook,
            "PostToolUse": tool_hook,
            "Notification": hook,
            "Stop": hook,
            "SessionEnd": hook,
        }
    })
}

/// `path` quoted for the shell it is typed into: in single quotes, where
/// sh, zsh, bash, fish and PowerShell expand neither `$` nor backticks. A
/// quote in it is written as each reads it: `'\''` (POSIX shells, fish),
/// `''` (PowerShell). Command Prompt has no single quotes: double ones.
fn quoted(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) {
        let (shell, _) = ph_env::default_shell();
        if shell
            .file_stem()
            .is_some_and(|name| name.eq_ignore_ascii_case("cmd"))
        {
            return format!("\"{text}\"");
        }
        return format!("'{}'", text.replace('\'', "''"));
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// An id safe to put on a command line (Cline's have `_`).
fn is_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// A hook event from agent `pane`.
fn apply(pane: &str, event: &Value, cx: &mut App) {
    let Some(agent) = cx
        .default_global::<TerminalAgents>()
        .agents
        .iter_mut()
        .find(|agent| agent.pane == pane)
    else {
        return;
    };
    if let Some(session) = event["session_id"].as_str().filter(|id| is_session_id(id)) {
        agent.session_id = Some(session.to_string());
    }
    if let Some(path) = event["transcript_path"].as_str() {
        agent.transcript = Some(PathBuf::from(path));
    }
    agent.updated_at = now();
    match event["hook_event_name"].as_str().unwrap_or_default() {
        "UserPromptSubmit" => {
            let prompt = event["prompt"].as_str().unwrap_or_default();
            if agent.title.is_none() {
                agent.title = first_line(prompt);
            }
            agent.state = ChatState::Working;
            agent.line = None;
        }
        "PreToolUse" => {
            agent.state = ChatState::Working;
            agent.line = Some(tool_line(
                event["tool_name"].as_str().unwrap_or("a tool"),
                &event["tool_input"],
            ));
        }
        "PostToolUse" => agent.state = ChatState::Working,
        "Notification" => {
            let message = event["message"].as_str().unwrap_or_default();
            // Claude also says it is idle after a while; only a question
            // needs the user.
            if !message.to_lowercase().contains("waiting for your input") {
                agent.state = ChatState::NeedsYou;
                agent.line = first_line(message);
            }
        }
        "Stop" => {
            agent.state = ChatState::Done;
            match event["last_assistant_message"]
                .as_str()
                .and_then(first_line)
            {
                Some(line) => agent.line = Some(line),
                // Read from its session file off the UI thread (its last
                // 256 KB, parsed line by line).
                None => {
                    if let Some(path) = agent.transcript.clone() {
                        let pane = pane.to_string();
                        cx.spawn(async move |cx| {
                            let Some(line) =
                                cx.background_spawn(async move { last_reply(&path) }).await
                            else {
                                return;
                            };
                            cx.update(|cx| {
                                if let Some(agent) = cx
                                    .default_global::<TerminalAgents>()
                                    .agents
                                    .iter_mut()
                                    .find(|agent| agent.pane == pane)
                                {
                                    agent.line = Some(line);
                                }
                            });
                        })
                        .detach();
                    }
                    return finish(pane, cx);
                }
            }
        }
        "SessionEnd" => agent.state = ChatState::Idle,
        _ => {}
    }
    finish(pane, cx);
}

/// Shows agent `pane`'s state on its terminal tab.
fn finish(pane: &str, cx: &mut App) {
    let Some(agent) = cx
        .default_global::<TerminalAgents>()
        .agents
        .iter()
        .find(|agent| agent.pane == pane)
    else {
        return;
    };
    let badge = match agent.state {
        ChatState::Working => Some(AgentBadge::Working),
        ChatState::NeedsYou => Some(AgentBadge::NeedsYou),
        ChatState::Done => Some(AgentBadge::Done),
        ChatState::Failed | ChatState::Idle => None,
    };
    let terminal = agent.terminal.clone();
    _ = terminal.update(cx, |terminal, cx| terminal.set_badge(badge, cx));
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
}

/// "Bash: cargo test", "Edit: src/main.rs".
fn tool_line(tool: &str, input: &Value) -> String {
    let detail = [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ]
    .iter()
    .find_map(|key| input[*key].as_str())
    .and_then(first_line);
    match detail {
        Some(detail) => format!("{tool}: {detail}"),
        None => tool.to_string(),
    }
}

/// The first line of the last reply in a Claude Code session file.
fn last_reply(path: &Path) -> Option<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(TRANSCRIPT_TAIL)))
        .ok()?;
    // Bytes: the seek can land inside a character.
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail);
    tail.lines().rev().find_map(|line| {
        let row: Value = serde_json::from_str(line).ok()?;
        if row["type"] != "assistant" {
            return None;
        }
        row["message"]["content"]
            .as_array()?
            .iter()
            .rev()
            .find_map(|block| {
                (block["type"] == "text")
                    .then(|| block["text"].as_str())
                    .flatten()
            })
            .and_then(first_line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start<'a>(agent_id: &'a str, resume: Option<&'a str>) -> Launch<'a> {
        Launch {
            agent_id,
            project: 1,
            cwd: PathBuf::from("/work"),
            resume,
            chat: None,
            model: None,
            mode: None,
        }
    }

    #[test]
    fn each_agents_cli_goes_on_with_its_session_its_own_way() {
        let id = "0199a213-81c0-7800-8aa1-bbab2a035a53";
        let usual = ph_agent::profiles::Profile::default();
        for (agent, fresh, resumed) in [
            ("claude", "claude", format!("claude --resume {id}")),
            ("codex", "codex", format!("codex resume {id}")),
            ("gemini", "gemini", format!("gemini --resume {id}")),
            ("antigravity", "agy", format!("agy --conversation {id}")),
            ("cline", "cline", format!("cline --id {id}")),
        ] {
            assert_eq!(command(&start(agent, None), &usual, &[]).unwrap(), fresh);
            assert_eq!(
                command(&start(agent, Some(id)), &usual, &[]).unwrap(),
                resumed
            );
        }
        assert!(command(&start("nobody", None), &usual, &[]).is_err());

        // The user's own command for it, and what they add to every start.
        let mut theirs = ph_agent::profiles::Profile::default();
        theirs.command = "claude-beta".into();
        theirs.arguments = vec![
            "--verbose".into(),
            "--append-system-prompt".into(),
            "Be brief; it's late".into(),
        ];
        assert_eq!(
            command(
                &start("claude", Some(id)),
                &theirs,
                &["--settings 'h.json'".to_string()]
            )
            .unwrap(),
            if cfg!(windows) {
                format!(
                    "claude-beta --resume {id} --verbose --append-system-prompt 'Be brief; it''s late' --settings 'h.json'"
                )
            } else {
                format!(
                    "claude-beta --resume {id} --verbose --append-system-prompt 'Be brief; it'\\''s late' --settings 'h.json'"
                )
            }
        );
    }

    #[test]
    fn a_session_keeps_its_model_and_mode_in_its_cli() {
        let with = |agent, model, mode| {
            command(
                &Launch {
                    model,
                    mode,
                    ..start(agent, None)
                },
                &ph_agent::profiles::Profile::default(),
                &["--settings 'hooks.json'".to_string()],
            )
            .unwrap()
        };
        assert_eq!(
            with("claude", Some("opus"), Some("plan")),
            "claude --model opus --permission-mode plan --settings 'hooks.json'"
        );
        // The plain mode is what the CLI starts in; "default" is no model.
        assert_eq!(
            with("claude", Some("default"), Some("default")),
            "claude --settings 'hooks.json'"
        );
        assert_eq!(
            with("codex", Some("gpt-5.6-luna"), Some("read-only")),
            "codex --model gpt-5.6-luna --settings 'hooks.json'"
        );
        // Nothing that the shell would read as more than a value.
        assert_eq!(
            with("claude", Some("opus; rm -rf ~"), Some("plan && true")),
            "claude --settings 'hooks.json'"
        );
        // An agent whose CLI takes neither.
        assert_eq!(
            with("cline", Some("x"), Some("plan")),
            "cline --settings 'hooks.json'"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn quotes_paths_for_the_shell() {
        assert_eq!(quoted(Path::new("/a b/$x`y`")), "'/a b/$x`y`'");
        assert_eq!(quoted(Path::new("/it's")), "'/it'\\''s'");
    }

    #[test]
    fn hooks_call_back_with_the_pane() {
        let settings = hook_settings("\"/app/pilothouse\" agent-hook --pane p1");
        let stop = &settings["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(stop["type"], "command");
        assert!(stop["command"].as_str().unwrap().contains("--pane p1"));
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "*");
    }

    #[test]
    fn reads_the_last_reply_and_tool_lines() {
        let dir = std::env::temp_dir().join(format!("ph-term-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.jsonl");
        std::fs::write(
            &file,
            [
                r#"{"type":"user","message":{"content":"hi"}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done.\nAll good."}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(last_reply(&file).as_deref(), Some("Done."));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            tool_line("Bash", &serde_json::json!({"command": "cargo test\n--all"})),
            "Bash: cargo test"
        );
        assert!(!is_session_id("x; rm -rf /"));
        assert_eq!(
            title_state("\u{25C7}  Ready (app)   "),
            Some(ChatState::Idle)
        );
        assert_eq!(
            title_state("\u{270B}  Action Required (app)"),
            Some(ChatState::NeedsYou)
        );
        assert_eq!(title_state("zsh"), None);
        assert_eq!(agent_named("agy").map(|spec| spec.id), Some("antigravity"));
    }
}
