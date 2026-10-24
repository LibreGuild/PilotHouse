//! The chat column: transcript and composer for the project's open chat.
//!
//! One `ChatPanel` per project. It owns the project's agent connections (one
//! process per agent, shared by its chats) and one [`ChatThread`] per chat
//! that has been opened. With no chat selected it shows a draft thread, which
//! becomes a stored chat on its first message.

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

    /// Looks for a mention being typed and suggests what it could be.
    fn update_mention(&mut self, cx: &mut Context<Self>) {
        let composer = self.composer.read(cx);
        let typing = mention_complete::typing_at(&composer.value(), composer.cursor());
        let Some(typing) = typing else {
            if self.mention.take().is_some() {
                cx.notify();
            }
            return;
        };
        // Files are listed in the background, again after a minute.
        let stale = self
            .files
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > std::time::Duration::from_secs(60));
        if typing.trigger == '@' && stale && self._files.is_none() {
            let roots = self.project.roots();
            self._files = Some(cx.spawn(async move |this, cx| {
                let (files, symbols) = cx
                    .background_spawn(async move {
                        let files = roots.index();
                        let symbols = ph_files::symbols::index(&roots, &files);
                        (files, symbols)
                    })
                    .await;
                _ = this.update(cx, |this, cx| {
                    this.symbols = std::sync::Arc::new(symbols);
                    this.files = Some((std::time::Instant::now(), files));
                    this._files = None;
                    this.update_mention(cx);
                });
            }));
        }
        let empty = ph_files::FileList::default();
        let files = self
            .files
            .as_ref()
            .map(|(_, files)| files)
            .unwrap_or(&empty);
        let cards: Vec<(i64, String)> = if typing.trigger == '#' {
            self.store
                .cards(self.project.id)
                .unwrap_or_default()
                .into_iter()
                .map(|card| (card.id, card.title))
                .collect()
        } else {
            Vec::new()
        };
        if typing.trigger == '@' && typing.query.starts_with("pr") && !self.pulls_asked {
            self.pulls_asked = true;
            cx.emit(ChatPanelEvent::WantPulls);
        }
        let pulls: Vec<(u64, String)> = self
            .pulls
            .iter()
            .map(|pull| (pull.number, pull.title.clone()))
            .collect();
        let extra = mention_complete::Extra {
            symbols: &self.symbols,
            pulls: &pulls,
        };
        let items = mention_complete::suggestions(&typing, files, &cards, &extra);
        let selected = self
            .mention
            .as_ref()
            .filter(|menu| menu.typing.range.start == typing.range.start)
            .map(|menu| menu.selected.min(items.len().saturating_sub(1)))
            .unwrap_or_default();
        self.mention = (!items.is_empty()).then_some(MentionMenu {
            typing,
            items,
            selected,
        });
        cx.notify();
    }

    /// Puts suggestion `ix` in place of the mention being typed.
    fn accept_mention(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.mention.take() else {
            return;
        };
        let Some(item) = menu.items.get(ix) else {
            return;
        };
        let (range, insert) = (menu.typing.range.clone(), item.insert.clone());
        self.composer.update(cx, |composer, cx| {
            composer.set_selected_range(range, cx);
            composer.replace(insert, window, cx);
        });
        cx.notify();
    }

    fn move_mention(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.mention {
            let count = menu.items.len() as isize;
            menu.selected = (menu.selected as isize + by).rem_euclid(count.max(1)) as usize;
            cx.notify();
        }
    }

    fn render_mention_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.mention.as_ref()?;
        Some(
            v_flex()
                .id("mention-menu")
                .absolute()
                .bottom_full()
                .left_0()
                .mb_1()
                .w(px(420.))
                .max_w_full()
                .p_1()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .shadow_lg()
                .children(menu.items.iter().enumerate().map(|(ix, item)| {
                    h_flex()
                        .id(("mention-item", ix))
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(cx.theme().radius)
                        .cursor_pointer()
                        .when(ix == menu.selected, |this| this.bg(cx.theme().list_active))
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.accept_mention(ix, window, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(div().flex_none().text_sm().child(item.label.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(item.detail.clone()),
                        )
                }))
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("↑↓ to pick · Enter or Tab to insert · Esc to close"),
                ),
        )
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

    fn choose_agent(&mut self, spec: &'static AgentSpec, cx: &mut Context<Self>) {
        let _ = self.store.set_setting(DEFAULT_AGENT_KEY, &spec.id);
        self.active
            .update(cx, |thread, cx| thread.set_agent(spec, cx));
        cx.notify();
    }

    fn choose_option(&mut self, option_id: &str, value: &str, cx: &mut Context<Self>) {
        self.active
            .update(cx, |thread, cx| thread.set_option(option_id, value, cx));
    }

    /// What the model picker lists: this chat's agent live, the others from
    /// their cached options with remembered choices applied.
    fn picker_data(&self, cx: &App) -> PickerData {
        let thread = self.active.read(cx);
        let active = thread.agent();
        // This chat's agent first, then the others in their usual order.
        let ordered =
            std::iter::once(active).chain(AGENTS.iter().filter(|spec| spec.id != active.id));
        let agents = ordered
            .map(|spec| {
                let options = if spec.id == active.id {
                    thread.options().to_vec()
                } else {
                    options::with_preferences(
                        options::cached(&self.store, spec.id),
                        &options::preferences(&self.store, spec.id),
                    )
                };
                (spec, options)
            })
            .collect();
        PickerData {
            active,
            started: thread.has_started(),
            agents,
            viewing: self.picker_view.unwrap_or(active),
            query: self.picker_search.read(cx).value().to_string(),
            highlight: self.picker_highlight,
        }
    }

    /// A choice made in the model picker.
    pub(crate) fn pick(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active.read(cx).agent();
        match pick {
            Pick::Model {
                agent,
                option,
                value,
            } if agent.id == active.id => self.choose_option(&option, &value, cx),
            Pick::Model {
                agent,
                option,
                value,
            } => {
                // Applied when that agent's session starts.
                options::remember(&self.store, agent.id, &option, &value);
                self.switch_agent(agent, window, cx);
                self.picker_view = None;
            }
            Pick::Agent(agent) => {
                self.switch_agent(agent, window, cx);
                self.picker_view = None;
            }
            Pick::View(agent) => {
                self.picker_view = Some(agent);
                self.picker_highlight = 0;
                cx.notify();
            }
        }
    }

    /// The picker opened: this chat's agent, no filter, typing goes there.
    fn picker_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_view = None;
        self.picker_highlight = 0;
        let search = self.picker_search.clone();
        window.defer(cx, move |window, cx| {
            search.update(cx, |search, cx| {
                search.set_value("", window, cx);
                search.focus(window, cx);
            })
        });
    }

    /// The picker's keys: ↑/↓ move the highlight, Enter picks it, Tab shows
    /// the next agent.
    pub(crate) fn picker_key(
        &mut self,
        key: PickerKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let data = self.picker_data(cx);
        let models = model_picker::listed_models(&data);
        match key {
            PickerKey::Up => {
                self.picker_highlight = self.picker_highlight.saturating_sub(1);
            }
            PickerKey::Down => {
                self.picker_highlight =
                    (self.picker_highlight + 1).min(models.len().saturating_sub(1));
            }
            PickerKey::Pick => {
                if let Some(pick) = models.into_iter().nth(self.picker_highlight) {
                    self.pick(pick, window, cx);
                }
            }
            PickerKey::NextAgent => {
                let ix = data
                    .agents
                    .iter()
                    .position(|(agent, _)| agent.id == data.viewing.id)
                    .unwrap_or_default();
                let next = data.agents[(ix + 1) % data.agents.len()].0;
                self.pick(Pick::View(next), window, cx);
            }
        }
        cx.notify();
    }

    /// Continues this chat with `agent` in a new chat: the new agent gets a
    /// recap of this one with the first message, and the composer offers a
    /// message to send (or edit).
    fn hand_off(&mut self, agent: &'static AgentSpec, window: &mut Window, cx: &mut Context<Self>) {
        let (recap, from, title) = {
            let thread = self.active.read(cx);
            (
                thread.handoff_recap(cx),
                thread.agent().name,
                thread.title(),
            )
        };
        let Some(recap) = recap else { return };
        let _ = self.store.set_setting(DEFAULT_AGENT_KEY, &agent.id);
        self.set_session(None, cx);
        self.choose_agent(agent, cx);
        let origin = format!(
            "Continuing “{title}” from {from}. {} gets a recap with your first message.",
            agent.name
        );
        self.active
            .update(cx, |thread, cx| thread.continue_from(recap, origin, cx));
        self.next_title = Some(format!("{title} · {}", agent.name));
        self.composer.update(cx, |composer, cx| {
            composer.set_value("Continue where the previous agent left off.", window, cx)
        });
        cx.emit(ChatPanelEvent::SessionsChanged);
        self.warm_up(cx);
        self.focus_composer(window, cx);
    }

    /// Uses `agent` for this chat, or for a new one if this chat already
    /// started (an agent cannot change mid-conversation).
    fn switch_agent(
        &mut self,
        agent: &'static AgentSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active.read(cx).has_started() {
            let _ = self.store.set_setting(DEFAULT_AGENT_KEY, &agent.id);
            self.set_session(None, cx);
            cx.emit(ChatPanelEvent::SessionsChanged);
        } else {
            self.choose_agent(agent, cx);
        }
        // Start it now, so its models fill in.
        self.warm_up(cx);
        self.focus_composer(window, cx);
    }

    // ---- Rendering -----------------------------------------------------------

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(
                h_flex()
                    .flex_none()
                    .gap_1p5()
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(cx.theme().muted)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().size_1p5().rounded_full().bg(dot))
                    .child(thread.agent().mark(px(12.)))
                    .child(format!("{} · {label}", thread.agent().name)),
            )
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
    fn render_hand_off(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
    fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (store, project_id) = (self.store.clone(), self.project.id);
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
                        let sessions = store.sessions(project_id).unwrap_or_default();
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

    fn render_transcript(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.active.read(cx).entries().is_empty() {
            return self.render_empty(cx);
        }
        let thread = self.active.clone();
        let scroller = thread.read(cx).scroller().clone();
        MessageScroller::new("chat-transcript", scroller, move |ix, _, cx| {
            render_entry(&thread, ix, cx)
        })
        .size_full()
        .into_any_element()
    }

    /// The setup checks for `agent`; file-system lookups, so cached briefly.
    fn setup_checks(&mut self, agent: &'static AgentSpec) -> Vec<setup::Check> {
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

    fn render_empty(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let agent = self.active.read(cx).agent();
        let checks = self.setup_checks(agent);
        let subtitle: SharedString = format!("{} in {}", agent.name, self.project.name).into();
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
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("What are we building?"),
                    )
                    .child(
                        div()
                            .text_sm()
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
                    .children(
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
                                    let (prompt, title) = (prompt.to_string(), title.to_string());
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
                    ),
            )
            .into_any_element()
    }

    fn render_attachments(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let thread = self.active.read(cx);
        let status = thread.status().clone();
        let busy = matches!(status, ThreadStatus::Connecting | ThreadStatus::Working);
        let agent = thread.agent();
        let options = thread.options().to_vec();
        let panel = cx.weak_entity();

        let status_line: Option<SharedString> = match status {
            ThreadStatus::Connecting => Some(format!("Starting {}…", agent.name).into()),
            ThreadStatus::Working => Some(format!("{} is working…", agent.name).into()),
            ThreadStatus::Idle => None,
        };

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
fn model_button(
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
                if *open {
                    _ = panel.update(cx, |panel, cx| panel.picker_opened(window, cx));
                }
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
fn pill(
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

fn attach_button(panel: WeakEntity<ChatPanel>) -> impl IntoElement {
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

fn option_icon(category: OptionCategory) -> Icons {
    match category {
        OptionCategory::Model => Icons::Sparkles,
        OptionCategory::Mode => Icons::ShieldCheck,
        OptionCategory::Effort => Icons::Brain,
        OptionCategory::Other => Icons::Settings2,
    }
}

fn option_pill(option: &AgentOption, panel: WeakEntity<ChatPanel>) -> impl IntoElement {
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

fn option_menu(menu: PopupMenu, option: &AgentOption, panel: WeakEntity<ChatPanel>) -> PopupMenu {
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

fn more_options_button(options: &[AgentOption], panel: WeakEntity<ChatPanel>) -> impl IntoElement {
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
