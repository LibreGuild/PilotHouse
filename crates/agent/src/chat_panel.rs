//! The chat column: transcript and composer for the project's open chat.
//!
//! One `ChatPanel` per project. It owns the project's agent connections (one
//! process per agent, shared by its chats) and one [`ChatThread`] per chat
//! that has been opened. With no chat selected it shows a draft thread, which
//! becomes a stored chat on its first message.
//!
//! Split by concern: `mentions` completes `@` and `#`, `picker` chooses the
//! agent and model, `render` draws the column.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{
        Enter, Escape, IndentInline, InputEvent, InputState, MoveDown, MoveUp, Textarea,
        TextareaState,
    },
    menu::{PopupMenu, PopupMenuItem},
    message_scroller::MessageScroller,
    popover::Popover,
    shimmer::ShimmerText,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Focusable as _,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div, px,
};
use ph_store::Store;
use ph_store::projects::Project;
use ph_store::sessions::{Session, SessionId};
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::agents::{self, AGENTS, AgentSpec, Availability};
use crate::attachment_view;
use crate::attachments::{self, Attachment, LiveSources, PullBrief};
use crate::chats::{ChatHub, DEFAULT_AGENT_KEY, WorkspaceChats, default_agent};
use crate::connection::AgentConnection;
use crate::entry_view::render_entry;
use crate::mention_complete;
use crate::model_picker::{self, Pick, PickerData, PickerKey};
use crate::options::{self, AgentOption, OptionCategory, OptionKind};
use crate::review_card::ReviewCardEvent;
use crate::thread::{ChatThread, ThreadEvent, ThreadProject, ThreadStatus};
use crate::{setup, setup_view};

mod mentions;
mod picker;
mod render;

/// Setting key for the agent new chats start with.
/// Session titles are cut to this many characters.
const TITLE_CHARS: usize = 48;

#[derive(Debug, Clone)]
pub enum ChatPanelEvent {
    /// A chat was created (first message of a draft) or renamed.
    SessionsChanged,
    /// Open a project file (from a review finding).
    OpenFile { path: String, line: Option<u64> },
    /// Run a new review of the same pull request.
    ReviewAgain { review_id: i64 },
    /// Show this chat (from the history menu), or a new one (`None`).
    SelectSession(Option<SessionId>),
    /// Give `prompt` (the composer's text) to several agents in parallel.
    RunParallel { prompt: String },
    /// The chat in view finished a turn.
    TurnEnded,
    /// `@pr:` is being typed and no pull requests are known: load them and
    /// pass them in with [`ChatPanel::set_pulls`].
    WantPulls,
}

/// Starter prompts shown in an empty chat: title, hint, prompt.
const SUGGESTIONS: &[(&str, &str, &str)] = &[
    (
        "Explain this project",
        "How it is structured and where to start",
        "Explain how this project is structured: the main parts, how they fit together, and where a new contributor should start.",
    ),
    (
        "Find likely bugs",
        "Review recent changes for problems",
        "Review the uncommitted changes and the most recently changed files for likely bugs. List each issue with the file, the problem and a fix.",
    ),
    (
        "Plan a feature",
        "Break work into board cards",
        "Help me plan a feature. Ask me what it should do, then break it into small cards on the PilotHouse board.",
    ),
    (
        "Write tests",
        "Cover the most important code",
        "Find the most important untested code in this project and write focused tests for it.",
    ),
];

/// Starter prompts for a workspace without folders.
const FOLDERLESS_SUGGESTIONS: &[(&str, &str, &str)] = &[
    (
        "Think something through",
        "A plan, a spec or a decision",
        "Help me think something through. Ask me what it is, then help me turn it into a short written plan.",
    ),
    (
        "Check a website",
        "Look around it in the browser",
        "Open a website I name in the PilotHouse browser and tell me what you find: what works, what looks broken, and anything slow. Ask me which site first.",
    ),
    (
        "Plan work",
        "Break it into board cards",
        "Help me plan some work. Ask me what it is, then break it into small cards on the PilotHouse board.",
    ),
    (
        "Start something new",
        "A script or a small project here",
        "Help me start something new in this workspace's scratch folder. Ask me what I want to build, then make it step by step.",
    ),
];

pub struct ChatPanel {
    store: Store,
    project: ThreadProject,
    /// The workspace's agents and chats, which outlive this panel.
    chats: Entity<WorkspaceChats>,
    /// The chat on screen: a stored one, or a draft.
    active: Entity<ChatThread>,
    composer: Entity<TextareaState>,
    /// The model picker's search field.
    /// The agent whose models the open picker shows (`None`: this chat's).
    picker_view: Option<&'static AgentSpec>,
    /// The picker's filter, and which of the models it lists is highlighted
    /// for the keyboard.
    picker_search: Entity<InputState>,
    picker_highlight: usize,
    /// The mention suggestions open under the cursor.
    mention: Option<MentionMenu>,
    /// The project's files for `@` suggestions, and when they were listed.
    files: Option<(std::time::Instant, ph_files::FileList)>,
    /// Definitions in those files, for `@` suggestions.
    symbols: std::sync::Arc<Vec<ph_files::symbols::Symbol>>,
    _files: Option<Task<()>>,
    /// The workspace's open pull requests, for `@pr:` (set by the workspace).
    pulls: Vec<PullBrief>,
    /// Pull requests were asked for once ([`ChatPanelEvent::WantPulls`]).
    pulls_asked: bool,
    /// The current agent's setup checks, refreshed every few seconds.
    setup: Option<(&'static str, std::time::Instant, Vec<setup::Check>)>,
    /// Attached to the next message.
    attachments: Vec<Attachment>,
    /// Title for the chat the next send creates, instead of its text.
    next_title: Option<String>,
    /// Screenshots and logs from the project's browser and devices.
    live: Option<LiveSources>,
    /// The header lists past chats (when the window's sidebar is hidden).
    history_menu: bool,
    _subscriptions: Vec<Subscription>,
    _active_subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChatPanelEvent> for ChatPanel {}

impl ChatPanel {
    pub fn new(
        store: Store,
        project: Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask anything. @file, #12 for a card, @screenshot, @log")
                .rows(2)
                .submit_on_enter(true)
        });
        let subscription = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    // Start the agent while the user is still typing.
                    InputEvent::Focus => this.warm_up(cx),
                    InputEvent::Change => this.update_mention(cx),
                    InputEvent::Blur => {
                        this.mention = None;
                        cx.notify();
                    }
                    _ => {}
                }
            },
        );
        let picker_search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter models"));
        let picker_subscription =
            cx.subscribe(&picker_search, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.picker_highlight = 0;
                    cx.notify();
                }
            });
        // The chats may predate this panel (a schedule started one), so
        // they get the folders as they are now.
        let chats = ChatHub::chats(&store, &project, cx);
        chats.update(cx, |chats, _| chats.set_project(&project));
        let project = ThreadProject::of(&project);
        let agent = default_agent(&store);
        let draft = cx.new(|cx| ChatThread::draft(store.clone(), project.clone(), agent, cx));
        let mut panel = Self {
            store,
            project,
            chats,
            active: draft,
            composer,
            picker_view: None,
            picker_search,
            picker_highlight: 0,
            mention: None,
            files: None,
            symbols: Default::default(),
            _files: None,
            pulls: Vec::new(),
            pulls_asked: false,
            setup: None,
            attachments: Vec::new(),
            next_title: None,
            live: None,
            history_menu: false,
            _subscriptions: vec![subscription, picker_subscription],
            _active_subscriptions: Vec::new(),
        };
        panel.watch_active(cx);
        panel
    }

    /// Shows `session`, or a fresh draft for `None`.
    pub fn set_session(&mut self, session: Option<Session>, cx: &mut Context<Self>) {
        self.active = match session {
            Some(session) => self
                .chats
                .update(cx, |chats, cx| chats.thread_for(session, cx)),
            None => {
                let agent = default_agent(&self.store);
                let (store, project) = (self.store.clone(), self.project.clone());
                cx.new(|cx| ChatThread::draft(store, project, agent, cx))
            }
        };
        self.watch_active(cx);
        cx.notify();
    }

    /// The workspace's folders changed: new chats get them (a chat already
    /// started keeps the folders it began with until its agent restarts).
    pub fn set_project(&mut self, project: Project, cx: &mut Context<Self>) {
        self.chats
            .update(cx, |chats, _| chats.set_project(&project));
        self.project = ThreadProject::of(&project);
        self.files = None;
        if !self.active.read(cx).has_started() {
            self.set_session(None, cx);
        }
        cx.notify();
    }

    /// Scrolls the transcript to entry `ix` (a search result).
    pub fn reveal_entry(&mut self, ix: usize, cx: &mut Context<Self>) {
        let scroller = self.active.read(cx).scroller().clone();
        scroller.update(cx, |scroller, cx| {
            scroller.scroll_to_item(ix, cx);
        });
    }

    /// A message is being written (or something is attached to the next).
    pub fn has_draft(&self, cx: &App) -> bool {
        !self.composer.read(cx).value().trim().is_empty() || !self.attachments.is_empty()
    }

    /// Offers past chats in the header (the sidebar that lists them is hidden).
    pub fn set_history_menu(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.history_menu = shown;
        cx.notify();
    }

    /// The workspace's agents and chats this panel shows.
    pub fn chats(&self) -> &Entity<WorkspaceChats> {
        &self.chats
    }

    pub fn active_session_id(&self, cx: &App) -> Option<SessionId> {
        self.active.read(cx).session().map(|session| session.id)
    }

    /// Opens a new review chat working in `cwd` (the PR's worktree) and
    /// starts it: `display` is what the chat shows for the long `prompt`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_review(
        &mut self,
        review_id: i64,
        cwd: std::path::PathBuf,
        title: String,
        display: String,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = default_agent(&self.store);
        let store = self.store.clone();
        // A review works in its worktree alone.
        let project = ThreadProject {
            path: cwd,
            extra_dirs: Vec::new(),
            empty: false,
            ..self.project.clone()
        };
        self.active = cx.new(|cx| {
            let mut thread = ChatThread::draft(store, project, agent, cx);
            thread.set_review(review_id);
            thread
        });
        self.watch_active(cx);
        let connection = self.connection(agent, cx);
        let blocks = vec![agent_client_protocol_schema::v1::ContentBlock::Text(
            agent_client_protocol_schema::v1::TextContent::new(prompt),
        )];
        self.active.update(cx, |thread, cx| {
            thread.send(display, Vec::new(), title, blocks, connection, cx)
        });
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Starts a new chat named `title` with `prompt`, letting the agent use
    /// PilotHouse's browser, device and test tools without asking during
    /// that turn (recording a UI test).
    pub fn start_tool_task(
        &mut self,
        title: String,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_session(None, cx);
        self.active
            .update(cx, |thread, _| thread.allow_app_tools_for_turn());
        self.send_text(prompt, Some(title), window, cx);
    }

    /// Starts one chat per lane of a parallel run, each agent in its own
    /// worktree, all with `prompt`; shows the first. Returns each lane's
    /// session, in order.
    pub fn start_parallel(
        &mut self,
        lanes: Vec<(&'static AgentSpec, std::path::PathBuf)>,
        title: &str,
        prompt: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Option<SessionId>> {
        let mut sessions = Vec::new();
        let mut first = None;
        for (agent, worktree) in lanes {
            let store = self.store.clone();
            let project = ThreadProject {
                path: worktree,
                extra_dirs: Vec::new(),
                empty: false,
                ..self.project.clone()
            };
            let thread = cx.new(|cx| {
                let mut thread = ChatThread::draft(store, project, agent, cx);
                thread.set_parallel();
                thread
            });
            let connection = self.connection(agent, cx);
            let blocks = vec![agent_client_protocol_schema::v1::ContentBlock::Text(
                agent_client_protocol_schema::v1::TextContent::new(prompt.to_string()),
            )];
            let lane_title = format!("{title} · {}", agent.name);
            thread.update(cx, |thread, cx| {
                thread.send(
                    prompt.to_string(),
                    Vec::new(),
                    lane_title,
                    blocks,
                    connection,
                    cx,
                )
            });
            let session = thread.read(cx).session().map(|session| session.id);
            if let Some(id) = session {
                let thread = thread.clone();
                self.chats
                    .update(cx, |chats, cx| chats.keep(id, thread, cx));
            }
            sessions.push(session);
            first.get_or_insert(thread);
        }
        if let Some(first) = first {
            self.active = first;
            self.watch_active(cx);
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        cx.emit(ChatPanelEvent::SessionsChanged);
        self.focus_composer(window, cx);
        cx.notify();
        sessions
    }

    /// Code (or anything) to ask about, attached to the next message. With
    /// a `prompt` (and chat `title`) it is sent right away and the agent may
    /// read and search without asking, so the answer comes quickly; without
    /// one the composer is focused for the user's own question. A busy chat
    /// hands it to a new chat.
    pub fn ask_about(
        &mut self,
        attachment: Attachment,
        prompt: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.active.read(cx).status(),
            ThreadStatus::Working | ThreadStatus::Connecting
        ) {
            self.set_session(None, cx);
        }
        self.attachments.push(attachment);
        match prompt {
            Some((title, prompt)) => {
                self.active
                    .update(cx, |thread, _| thread.allow_reads_for_turn());
                self.send_text(prompt, Some(title), window, cx);
            }
            None => self.focus_composer(window, cx),
        }
        cx.notify();
    }

    /// Sends `prompt` as if the user typed it (used by the board). A new chat
    /// is named `title` rather than after the prompt text.
    pub fn send_text(
        &mut self,
        prompt: String,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer
            .update(cx, |composer, cx| composer.set_value(prompt, window, cx));
        self.next_title = title;
        self.send(window, cx);
    }

    /// The workspace's open pull requests, for `@pr:` mentions.
    pub fn set_pulls(&mut self, pulls: Vec<PullBrief>, cx: &mut Context<Self>) {
        self.pulls = pulls;
        if self.mention.is_some() {
            self.update_mention(cx);
        }
    }

    /// Adds an attachment (a capture, log lines…) to the next message.
    pub fn set_live_sources(&mut self, live: LiveSources) {
        self.live = Some(live);
    }

    /// Attaches the current browser/device screenshot or recent logs.
    fn attach_live(&mut self, logs: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let attachment = if logs {
            (live.logs)(cx)
        } else {
            (live.screenshot)(cx)
        };
        if let Some(attachment) = attachment {
            self.attach(attachment, window, cx);
        }
    }

    pub fn attach(&mut self, attachment: Attachment, window: &mut Window, cx: &mut Context<Self>) {
        if !self.attachments.contains(&attachment) {
            self.attachments.push(attachment);
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn watch_active(&mut self, cx: &mut Context<Self>) {
        self._active_subscriptions = vec![
            cx.observe(&self.active, |_, _, cx| cx.notify()),
            cx.subscribe(
                &self.active,
                |this, thread, event: &ThreadEvent, cx| match event {
                    ThreadEvent::SessionCreated(session) => {
                        let id = session.id;
                        this.chats
                            .update(cx, |chats, cx| chats.keep(id, thread, cx));
                        cx.emit(ChatPanelEvent::SessionsChanged);
                    }
                    ThreadEvent::TurnEnded => cx.emit(ChatPanelEvent::TurnEnded),
                    ThreadEvent::Review(ReviewCardEvent::OpenFile { path, line }) => {
                        cx.emit(ChatPanelEvent::OpenFile {
                            path: path.clone(),
                            line: *line,
                        })
                    }
                    ThreadEvent::Review(ReviewCardEvent::ReviewAgain { review_id }) => {
                        cx.emit(ChatPanelEvent::ReviewAgain {
                            review_id: *review_id,
                        })
                    }
                },
            ),
        ];
    }

    /// The workspace's process for `spec`.
    fn connection(
        &mut self,
        spec: &'static AgentSpec,
        cx: &mut Context<Self>,
    ) -> Entity<AgentConnection> {
        self.chats
            .update(cx, |chats, cx| chats.connection(spec, cx))
    }

    fn warm_up(&mut self, cx: &mut Context<Self>) {
        let spec = self.active.read(cx).agent();
        if !matches!(spec.availability(), Availability::Ready { .. }) {
            return;
        }
        let connection = self.connection(spec, cx);
        self.active
            .update(cx, |thread, cx| thread.warm_up(connection, cx));
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        if *self.active.read(cx).status() != ThreadStatus::Idle {
            return;
        }
        let title = self.next_title.take().unwrap_or_else(|| title_from(&text));

        // Explicit attachments plus `@file` / `#card` / `@screenshot` / `@log`
        // mentions in the text.
        let mut attached = std::mem::take(&mut self.attachments);
        let wanted = attachments::live_mentions(&text);
        let mut live = Vec::new();
        if let Some(sources) = &self.live {
            if wanted.screenshot {
                live.extend((sources.screenshot)(cx));
            }
            if wanted.logs {
                live.extend((sources.logs)(cx));
            }
            if wanted.tests {
                live.extend((sources.tests)(cx));
            }
        }
        live.extend(attachments::pull_mentions(&text, &self.pulls));
        let roots = self.project.roots();
        for mention in attachments::mentions(&text, &roots).into_iter().chain(live) {
            if !attached.contains(&mention) {
                attached.push(mention);
            }
        }
        let accepts_images = self.active.read(cx).accepts_images();
        let mut blocks = Vec::new();
        if !text.is_empty() {
            blocks.push(agent_client_protocol_schema::v1::ContentBlock::Text(
                agent_client_protocol_schema::v1::TextContent::new(text.clone()),
            ));
        }
        for attachment in &attached {
            blocks.extend(attachment.to_blocks(
                &roots,
                self.project.id,
                &self.store,
                accepts_images,
            ));
        }
        let summaries = attached
            .iter()
            .map(|attachment| attachment.summary(&roots, self.project.id, &self.store))
            .collect();

        let spec = self.active.read(cx).agent();
        let connection = self.connection(spec, cx);
        self.active.update(cx, |thread, cx| {
            thread.send(text, summaries, title, blocks, connection, cx)
        });
        // A chat in use stays loaded in the workspace (and its status shows).
        if let Some(id) = self.active_session_id(cx) {
            let thread = self.active.clone();
            self.chats
                .update(cx, |chats, cx| chats.keep(id, thread, cx));
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.active.update(cx, |thread, cx| thread.cancel(cx));
    }

    // ---- Rendering -----------------------------------------------------------
}

/// A chat title from the first line of the first message.
fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().count() > TITLE_CHARS {
        title.push('…');
    }
    if title.is_empty() {
        "New chat".into()
    } else {
        title
    }
}

/// Mention suggestions open in the composer.
struct MentionMenu {
    typing: mention_complete::Typing,
    items: Vec<mention_complete::Suggestion>,
    selected: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_use_the_first_line_and_are_capped() {
        assert_eq!(title_from("Fix the login bug\nmore"), "Fix the login bug");
        let long = "x".repeat(60);
        assert_eq!(title_from(&long).chars().count(), TITLE_CHARS + 1);
        assert_eq!(title_from("   "), "New chat");
    }
}
