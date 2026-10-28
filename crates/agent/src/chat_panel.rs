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

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{
        Enter, Escape, IndentInline, InputEvent, InputState, MoveDown, MoveUp, OutdentInline,
        Textarea, TextareaState,
    },
    menu::{PopupMenu, PopupMenuItem},
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
use crate::transcript_scroll::TranscriptList;
use crate::{setup, setup_view};

mod mentions;
mod picker;
mod render;

/// The composer's hint in a workspace's chat.
const PLACEHOLDER: &str = "Ask anything. @file, #12 for a card, @screenshot, @log";

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
    /// Show the findings of the pull request review `review_id` is of.
    OpenFindings { review_id: i64 },
    /// Show this chat (from the history menu), or a new one (`None`).
    SelectSession(Option<SessionId>),
    /// Give `prompt` (the composer's text) to several agents in parallel.
    RunParallel { prompt: String },
    /// The chat in view finished a turn.
    TurnEnded,
    /// `@pr:` is being typed and no pull requests are known: load them and
    /// pass them in with [`ChatPanel::set_pulls`].
    WantPulls,
    /// A starter of the panel's [`ChatContext`] was picked (its id).
    Starter(String),
}

/// What a chat panel is for when it is not a workspace's plain chat: the
/// GitHub view's chat about one pull request.
#[derive(Clone)]
pub struct ChatContext {
    /// Shown in an empty chat.
    pub title: String,
    pub subtitle: String,
    /// Picked from the empty chat ([`ChatPanelEvent::Starter`]); read each
    /// time it is drawn, so edits elsewhere (Settings) show at once.
    pub starters: Rc<dyn Fn() -> Vec<Starter>>,
    pub placeholder: String,
    /// Where new chats work. The history lists the chats that worked here.
    pub cwd: std::path::PathBuf,
    /// Makes `cwd` ready (clones, checks out) before a new chat's first
    /// message; `label` is shown meanwhile.
    pub prepare: Option<Prepare>,
}

/// Starts readying a chat's folder; the task ends when it is ready.
pub type PrepareFn = Rc<dyn Fn(&mut App) -> Task<anyhow::Result<()>>>;

/// A step that readies a chat's folder, shown as `label` while it runs.
#[derive(Clone)]
pub struct Prepare {
    pub label: String,
    pub run: PrepareFn,
}

/// One of an empty chat's suggestions.
#[derive(Clone)]
pub struct Starter {
    pub id: String,
    pub title: String,
    pub hint: String,
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
    /// What the panel is for, when not a workspace's plain chat.
    context: Option<ChatContext>,
    /// Something readying the chat (a clone), or why it failed.
    busy: Option<SharedString>,
    /// The list of the turn's subagents is open above the composer.
    subagents_open: bool,
    /// The side chat in front and the chat it stepped aside from.
    side: Option<(WeakEntity<ChatThread>, Session)>,
    /// New chats sent off with ⌘Enter, held until they are stored and the
    /// workspace's chats keep them.
    sent_off: Vec<(Entity<ChatThread>, Subscription)>,
    failed: Option<SharedString>,
    _prepare: Option<Task<()>>,
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
        crate::view_mode::ViewMode::load(&store, cx);
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(PLACEHOLDER)
                .auto_grow(2, 6)
                .submit_on_enter(true)
        });
        let subscription = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::PressEnter {
                        shift: false,
                        secondary: true,
                    } => this.send_and_start_new(window, cx),
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
            context: None,
            busy: None,
            subagents_open: false,
            sent_off: Vec::new(),
            side: None,
            failed: None,
            _prepare: None,
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
                let (store, project) = (self.store.clone(), self.draft_project());
                cx.new(|cx| ChatThread::draft(store, project, agent, cx))
            }
        };
        self.failed = None;
        self.watch_active(cx);
        cx.notify();
    }

    /// A new chat whose agent works in `folder`, a folder of the workspace.
    pub fn new_chat_in(
        &mut self,
        folder: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = default_agent(&self.store);
        let project = ThreadProject {
            path: folder,
            ..self.draft_project()
        };
        let store = self.store.clone();
        self.active = cx.new(|cx| {
            let mut thread = ChatThread::draft(store, project, agent, cx);
            thread.keep_folder();
            thread
        });
        self.failed = None;
        self.watch_active(cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Attaches `attachment` and puts `text` in an empty composer, to edit
    /// and send.
    pub fn draft_about(
        &mut self,
        attachment: Attachment,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composer.read(cx).value().trim().is_empty() {
            self.composer.update(cx, |composer, cx| {
                composer.set_value(text.to_string(), window, cx)
            });
        }
        self.attach(attachment, window, cx);
    }

    /// Where a new chat works: the context's folder, else the project's.
    fn draft_project(&self) -> ThreadProject {
        match &self.context {
            Some(context) => ThreadProject {
                path: context.cwd.clone(),
                extra_dirs: Vec::new(),
                empty: false,
                ..self.project.clone()
            },
            None => self.project.clone(),
        }
    }

    /// Makes this panel the chat about something (`None`: a plain chat).
    /// A draft in view moves to the context's folder.
    pub fn set_context(
        &mut self,
        context: Option<ChatContext>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = context
            .as_ref()
            .map(|context| context.placeholder.clone())
            .unwrap_or_else(|| PLACEHOLDER.to_string());
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholder(placeholder, window, cx)
        });
        self.context = context;
        self.busy = None;
        self.failed = None;
        self._prepare = None;
        if !self.active.read(cx).has_started() {
            self.set_session(None, cx);
        }
        cx.notify();
    }

    /// Shows `label` over the composer while the owner readies something
    /// for this chat (`None` when done).
    pub fn set_busy(&mut self, label: Option<String>, cx: &mut Context<Self>) {
        self.busy = label.map(Into::into);
        self.failed = None;
        cx.notify();
    }

    /// Says why readying the chat failed.
    pub fn set_failed(&mut self, reason: String, cx: &mut Context<Self>) {
        self.busy = None;
        self.failed = Some(reason.into());
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

    /// Opens a new chat working in the context's folder and starts it with
    /// `prompt` (a template's request); the chat shows `display` for it.
    /// The chat may read without asking.
    pub fn start_prompt(
        &mut self,
        title: String,
        display: String,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = None;
        self.failed = None;
        self.set_session(None, cx);
        let agent = self.active.read(cx).agent();
        self.active
            .update(cx, |thread, _| thread.allow_reads_for_turn());
        let connection = self.connection(agent, cx);
        let blocks = vec![agent_client_protocol_schema::v1::ContentBlock::Text(
            agent_client_protocol_schema::v1::TextContent::new(prompt),
        )];
        self.active.update(cx, |thread, cx| {
            thread.send(display, Vec::new(), title, blocks, connection, cx)
        });
        if let Some(id) = self.active_session_id(cx) {
            let thread = self.active.clone();
            self.chats
                .update(cx, |chats, cx| chats.keep(id, thread, cx));
        }
        self.focus_composer(window, cx);
        cx.notify();
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
        self.start_review_turn(
            Some(review_id),
            Some(crate::thread::ReviewTurn::Round),
            None,
            cwd,
            title,
            display,
            prompt,
            window,
            cx,
        )
    }

    /// Starts a chat that asks `prompt` about a pull request, working in
    /// its checkout `cwd` and only reading.
    pub fn start_question(
        &mut self,
        cwd: std::path::PathBuf,
        title: String,
        display: String,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_review_turn(None, None, None, cwd, title, display, prompt, window, cx)
    }

    /// Starts a review chat for `turn` (a round of review `review_id`, an
    /// author's claim, a walkthrough; none for a plain question), with
    /// agent `agent` (the default when none), working in `cwd`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_review_turn(
        &mut self,
        review_id: Option<i64>,
        turn: Option<crate::thread::ReviewTurn>,
        agent: Option<&'static AgentSpec>,
        cwd: std::path::PathBuf,
        title: String,
        display: String,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = None;
        self.failed = None;
        let agent = agent.unwrap_or_else(|| default_agent(&self.store));
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
            match (review_id, turn) {
                (Some(review_id), Some(crate::thread::ReviewTurn::Round)) => {
                    thread.set_review(review_id)
                }
                (_, Some(turn)) => thread.set_review_turn(turn),
                (_, None) => thread.allow_reads_for_turn(),
            }
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
        let viewing = self.active.read(cx).session().map(|session| session.id);
        self.chats
            .update(cx, |chats, cx| chats.set_viewing(viewing, cx));
        self._active_subscriptions = vec![
            cx.observe(&self.active, |_, _, cx| cx.notify()),
            cx.subscribe(
                &self.active,
                |this, thread, event: &ThreadEvent, cx| match event {
                    ThreadEvent::SessionCreated(session) => {
                        let id = session.id;
                        this.chats.update(cx, |chats, cx| {
                            chats.keep(id, thread, cx);
                            chats.set_viewing(Some(id), cx);
                        });
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
                    ThreadEvent::Review(ReviewCardEvent::OpenFindings { review_id }) => {
                        cx.emit(ChatPanelEvent::OpenFindings {
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

    /// Starts the agent and its session now (a resumed session is loaded,
    /// with its history), rather than when the composer is first focused.
    pub fn start_agent(&mut self, cx: &mut Context<Self>) {
        self.warm_up(cx);
    }

    fn warm_up(&mut self, cx: &mut Context<Self>) {
        // A folder still to be made (a clone) has nothing to start in yet.
        if self
            .context
            .as_ref()
            .is_some_and(|context| !context.cwd.is_dir())
        {
            return;
        }
        let spec = self.active.read(cx).agent();
        if !matches!(spec.availability(), Availability::Ready { .. }) {
            return;
        }
        let connection = self.connection(spec, cx);
        self.active
            .update(cx, |thread, cx| thread.warm_up(connection, cx));
    }

    /// Steps aside from this chat: a new chat with the same agent that has
    /// read this one's recent history, to ask something without adding to
    /// it. The header leads back.
    pub fn side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (recap, agent, title, session) = {
            let thread = self.active.read(cx);
            (
                thread.side_recap(cx),
                thread.agent(),
                thread.title(),
                thread.session().cloned(),
            )
        };
        let (Some(recap), Some(session)) = (recap, session) else {
            return;
        };
        self.set_session(None, cx);
        self.choose_agent(agent, cx);
        let origin = format!(
            "A side chat about “{title}”: {} has read it, and nothing here goes back to it.",
            agent.name
        );
        self.active
            .update(cx, |thread, cx| thread.continue_from(recap, origin, cx));
        self.side = Some((self.active.downgrade(), session));
        self.next_title = Some(format!("Aside: {title}"));
        cx.emit(ChatPanelEvent::SessionsChanged);
        self.warm_up(cx);
        self.focus_composer(window, cx);
    }

    /// The chat a side chat in front stepped aside from.
    pub(super) fn side_of(&self) -> Option<&Session> {
        self.side
            .as_ref()
            .filter(|(side, _)| *side == self.active.downgrade())
            .map(|(_, session)| session)
    }

    /// Sends, then opens a fresh draft: the chat goes on by itself (its
    /// card in the sidebar shows how it does) while the next one starts.
    fn send_and_start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sent_from = self.active.clone();
        let queued_before = sent_from.read(cx).queued().len();
        self.send(window, cx);
        // Not when the message is still waiting (a folder being readied)
        // or went into the queue of a working chat.
        let started = sent_from.read(cx).has_started();
        let queued = sent_from.read(cx).queued().len() > queued_before;
        if self.busy.is_none() && started && !queued && self.active == sent_from {
            // The workspace's chats keep it from here: now if it is stored
            // already, else once its agent session exists (until then
            // nothing else holds it).
            if let Some(id) = sent_from.read(cx).session().map(|session| session.id) {
                self.chats
                    .update(cx, |chats, cx| chats.keep(id, sent_from.clone(), cx));
                cx.emit(ChatPanelEvent::SessionsChanged);
            } else {
                let watch = cx.subscribe(&sent_from, |this, thread, event: &ThreadEvent, cx| {
                    if let ThreadEvent::SessionCreated(session) = event {
                        let id = session.id;
                        this.chats
                            .update(cx, |chats, cx| chats.keep(id, thread.clone(), cx));
                        this.sent_off.retain(|(held, _)| *held != thread);
                        cx.emit(ChatPanelEvent::SessionsChanged);
                    }
                });
                self.sent_off.push((sent_from.clone(), watch));
            }
            self.set_session(None, cx);
            self.composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        if self.busy.is_some() {
            return;
        }
        // While the agent works, the message waits its turn.
        if *self.active.read(cx).status() != ThreadStatus::Idle {
            self.send_prepared(window, cx);
            return;
        }
        // A new chat about something readies its folder first (clones the
        // repository, checks the pull request out), then sends.
        if !self.active.read(cx).has_started()
            && let Some(prepare) = self
                .context
                .as_ref()
                .and_then(|context| context.prepare.clone())
        {
            self.busy = Some(prepare.label.clone().into());
            self.failed = None;
            let task = (prepare.run)(cx);
            self._prepare = Some(cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                _ = this.update_in(cx, |this, window, cx| {
                    this.busy = None;
                    match result {
                        Ok(()) => {
                            // A fresh draft: one warmed up before the folder
                            // existed would start in the wrong place.
                            this.set_session(None, cx);
                            this.send_prepared(window, cx);
                        }
                        Err(err) => this.failed = Some(format!("{err:#}").into()),
                    }
                    cx.notify();
                });
            }));
            cx.notify();
            return;
        }
        self.send_prepared(window, cx);
    }

    /// [`Self::send`] once the chat's folder is ready.
    fn send_prepared(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
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
            if *thread.status() == ThreadStatus::Idle {
                thread.send(text, summaries, title, blocks, connection, cx)
            } else {
                thread.queue(
                    crate::thread::QueuedPrompt {
                        display: text,
                        attachments: summaries,
                        title,
                        blocks,
                        shown_at: None,
                    },
                    cx,
                )
            }
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

    /// Puts a queued message back in the composer to change it.
    fn edit_queued(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.active.update(cx, |thread, cx| thread.unqueue(ix, cx)) else {
            return;
        };
        self.composer.update(cx, |composer, cx| {
            composer.set_value(prompt.display, window, cx)
        });
        self.focus_composer(window, cx);
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
