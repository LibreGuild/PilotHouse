//! One chat: its transcript and its conversation with an agent.
//!
//! A thread can start as a **draft** (no stored session yet). Focusing the
//! composer warms it up: the agent starts and the ACP session is created, so
//! the model and mode pickers are filled before the first message. The stored
//! session is created on the first send.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ph_store::usage::{SessionUsage, UsageReport};

use crate::transcript_scroll::TranscriptScroll;
use agent_client_protocol_schema::v1::{
    CancelNotification, ContentBlock, EnvVariable, McpServer, McpServerStdio, PromptRequest,
    PromptResponse, RequestPermissionRequest, SessionUpdate, StopReason, TextContent,
};
use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, SharedString, Subscription, Task,
};
use ph_store::Store;
use ph_store::projects::ProjectId;
use ph_store::sessions::Session;
use serde_json::{Value, json};

use crate::agents::{self, AgentSpec};
use crate::connection::{AgentConnection, Ready, SignInMethod};
use crate::options::{self, AgentOption};
use crate::permission::{ApprovalInbox, PermissionChoice, PermissionRequest};
use crate::review_card::ReviewCardEvent;
use crate::transcript::{AttachmentSummary, Entry, PlanItem, ToolEntry, ToolKindLabel, block_text};

/// How much earlier conversation is replayed to a fresh agent session.
const HISTORY_PREAMBLE_CHARS: usize = 8_000;
/// Work the agent starts on its own (Claude Code following up on a
/// background command or subagent) counts as done after this long without
/// an update and with no tool call running…
const BACKGROUND_QUIET: Duration = Duration::from_secs(8);
/// …or after this long without any update at all.
const BACKGROUND_SILENT: Duration = Duration::from_secs(90);

/// A message written while the agent was busy, sent when it is free.
#[derive(Clone)]
pub struct QueuedPrompt {
    pub display: String,
    pub attachments: Vec<AttachmentSummary>,
    pub title: String,
    pub blocks: Vec<ContentBlock>,
    /// Its entry, when the transcript already shows (and stored) it: it
    /// was steered into a turn that had just ended.
    pub shown_at: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ThreadStatus {
    Idle,
    /// Starting the agent or its session for a prompt.
    Connecting,
    /// The agent is working on a prompt.
    Working,
}

/// Facts about the project the thread belongs to.
#[derive(Clone)]
pub struct ThreadProject {
    pub id: ProjectId,
    pub name: String,
    /// Where the agent works: the primary folder (or a worktree, or the
    /// scratch folder of a workspace without folders).
    pub path: PathBuf,
    /// The workspace's other folders, given to the agent as extra roots.
    pub extra_dirs: Vec<PathBuf>,
    /// The workspace has no folders: `path` is its scratch folder.
    pub empty: bool,
    /// A GitHub repository's chat (the GitHub view): it reads and explains
    /// pull requests in PilotHouse's own copy, and never changes files.
    pub read_only: bool,
}

impl ThreadProject {
    pub fn of(project: &ph_store::projects::Project) -> Self {
        Self {
            id: project.id,
            name: project.name.clone(),
            path: project.path.clone(),
            extra_dirs: project.extra_folders.clone(),
            empty: project.empty,
            read_only: project.kind == ph_store::projects::ProjectKind::GitHub,
        }
    }

    /// The folders as Files names them, for mentions and attachments.
    pub fn roots(&self) -> ph_files::roots::Roots {
        let folders: Vec<_> =
            std::iter::once((self.path.clone(), self.empty.then(|| "Scratch".to_string())))
                .chain(self.extra_dirs.iter().map(|dir| (dir.clone(), None)))
                .collect();
        ph_files::roots::Roots::new(&folders)
    }

    /// Told to the agent with a chat's first message: the workspace's other
    /// folders, or that it has none.
    pub fn workspace_note(&self) -> Option<String> {
        if self.read_only {
            return Some(format!(
                "(Note from PilotHouse: this chat is about a GitHub repository, in PilotHouse's own copy of it at {}. It is \
                 read-only: read, search and run read-only commands such as git diff, log and show, but never edit files, \
                 commit, push or install anything. Changes are made from a workspace.)",
                self.path.display()
            ));
        }
        if self.empty && self.extra_dirs.is_empty() {
            return Some(format!(
                "(Workspace note from PilotHouse: this workspace has no project folder. You are working in its own scratch \
                 folder, {}, which you may use for any files you make. The user may just want to talk, or to use the \
                 browser and devices through the PilotHouse tools.)",
                self.path.display()
            ));
        }
        if self.extra_dirs.is_empty() {
            return None;
        }
        let roots = self.roots();
        let folders: Vec<String> = roots
            .list()
            .iter()
            .map(|root| format!("- {}: {}", root.name, root.path.display()))
            .collect();
        Some(format!(
            "(Workspace note from PilotHouse: this workspace has several folders, often separate Git repositories. You \
             work in the first; the others are open to you too:\n{}\nIn this chat a path like `{}/src/main.rs` names a file \
             in the folder of that name.)",
            folders.join("\n"),
            roots.list()[1].name
        ))
    }
}

#[derive(Debug, Clone)]
pub enum ThreadEvent {
    /// The agent finished a turn (or it stopped or failed).
    TurnEnded,
    /// The first send created the stored session.
    SessionCreated(Session),
    /// A review card asks to open a file, or for a new review run.
    Review(ReviewCardEvent),
}

/// What a review chat's turn is for, read from the agent's reply when the
/// turn ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewTurn {
    /// A round of the chat's review: its findings.
    Round,
    /// Whether the author is right about finding `finding_id` (stored).
    Claim { finding_id: i64 },
    /// A walkthrough, saved as run `run_id`.
    Walkthrough { run_id: i64 },
}

/// The agent session id, once set up; shared so concurrent callers await one setup.
type SetupTask = Shared<Task<Result<String, SharedString>>>;

pub struct ChatThread {
    store: Store,
    project: ThreadProject,
    /// `None` while the chat is a draft.
    session: Option<Session>,
    agent: &'static AgentSpec,
    connection: Option<Entity<AgentConnection>>,
    acp_session_id: Option<String>,
    setup: Option<SetupTask>,
    /// What the agent offers (model, mode, effort…); from the cache until the session exists.
    options: Vec<AgentOption>,
    /// Ignore streamed history while the agent replays a resumed session.
    replaying: bool,
    /// Loading a session this chat has no transcript of: its replay is the
    /// history.
    importing: bool,
    /// Whether the running agent accepts images.
    accepts_images: bool,
    entries: Vec<Entry>,
    /// `entries[..saved]` are in the store.
    saved: usize,
    status: ThreadStatus,
    scroller: Entity<TranscriptScroll>,
    /// The PR review this chat runs, if it is a review chat.
    review_id: Option<i64>,
    /// What the turn under way is for (see [`ReviewTurn`]).
    review_turn: Option<ReviewTurn>,
    /// A lane of a parallel run: the agent works in its own worktree
    /// (`project.path`), remembered with the session.
    parallel: bool,
    /// The chat works in a folder of the workspace other than the primary
    /// one (Files: New Chat Here), kept with the session.
    in_folder: bool,
    /// Read-only tools run without asking during the current turn (a quick
    /// question about code). Review chats always allow them.
    reads_this_turn: bool,
    /// PilotHouse's browser, device and test tools run without asking
    /// during the current turn (recording or repairing a UI test).
    app_tools_this_turn: bool,
    /// Stored message ids of user entries, by entry index, so a checkpoint
    /// taken after the message was saved can be added to it.
    user_message_ids: HashMap<usize, i64>,
    /// A checkpoint restore in progress.
    restoring: bool,
    /// Told to the agent with the next message (files were restored, so
    /// what it read earlier may be stale).
    note_for_agent: Option<String>,
    /// Tokens, cost and context so far, as the agent reported them.
    usage: Option<SessionUsage>,
    /// When the current turn (or background work) started.
    turn_started: Option<Instant>,
    /// When the agent last sent anything.
    /// (`_quiet` ticks while a turn runs, and watches background work.)
    last_update: Instant,
    /// The agent works without a prompt of ours (see [`BACKGROUND_QUIET`]).
    background: bool,
    /// The agent's slash commands: name and description.
    commands: Vec<(String, String)>,
    /// Messages to send once the agent is free, oldest first.
    queued: VecDeque<QueuedPrompt>,
    /// Runs of steps the user opened, by their first tool call's id.
    open_runs: std::collections::HashSet<String>,
    /// The view mode the rows were last measured for.
    view_mode: crate::view_mode::ViewMode,
    /// Send the next queued message as soon as the current turn stops.
    send_when_stopped: bool,
    /// The next prompt sent is already this user entry (see
    /// [`QueuedPrompt::shown_at`]).
    resend: Option<usize>,
    _turn: Option<Task<()>>,
    _quiet: Option<Task<()>>,
    _cards: Vec<Subscription>,
}

impl EventEmitter<ThreadEvent> for ChatThread {}

impl ChatThread {
    /// A new, unsaved chat with `agent`.
    pub fn draft(
        store: Store,
        project: ThreadProject,
        agent: &'static AgentSpec,
        cx: &mut Context<Self>,
    ) -> Self {
        let options = options::with_preferences(
            options::cached(&store, agent.id),
            &options::preferences(&store, agent.id),
        );
        let scroller = cx.new(|cx| TranscriptScroll::new(0, cx));
        Self {
            store,
            project,
            session: None,
            agent,
            connection: None,
            acp_session_id: None,
            setup: None,
            options,
            replaying: false,
            importing: false,
            accepts_images: true,
            entries: Vec::new(),
            saved: 0,
            status: ThreadStatus::Idle,
            scroller,
            review_id: None,
            review_turn: None,
            parallel: false,
            in_folder: false,
            reads_this_turn: false,
            app_tools_this_turn: false,
            user_message_ids: HashMap::new(),
            restoring: false,
            note_for_agent: None,
            usage: None,
            turn_started: None,
            last_update: Instant::now(),
            background: false,
            commands: Vec::new(),
            queued: VecDeque::new(),
            open_runs: Default::default(),
            view_mode: Default::default(),
            send_when_stopped: false,
            resend: None,
            _turn: None,
            _quiet: None,
            _cards: Vec::new(),
        }
    }

    /// Makes this draft a PR review chat (the review run already exists):
    /// its first turn's reply is read as the round's findings.
    pub fn set_review(&mut self, review_id: i64) {
        self.review_id = Some(review_id);
        self.review_turn = Some(ReviewTurn::Round);
    }

    /// The next turn's reply is read as `turn` (an author's claim checked,
    /// a walkthrough). Reads run without asking, as in a review.
    pub fn set_review_turn(&mut self, turn: ReviewTurn) {
        self.review_turn = Some(turn);
    }

    pub fn review_id(&self) -> Option<i64> {
        self.review_id
    }

    /// Makes this draft a parallel-run lane working in its worktree.
    pub fn set_parallel(&mut self) {
        self.parallel = true;
    }

    /// Makes this draft work in its project path for good (a folder picked
    /// in Files), so the chat goes on there when reopened.
    pub fn keep_folder(&mut self) {
        self.in_folder = true;
    }

    /// Where the agent works.
    pub fn folder(&self) -> &std::path::Path {
        &self.project.path
    }

    /// Lets the next turn read and search without asking (cleared when the
    /// turn ends).
    pub fn allow_reads_for_turn(&mut self) {
        self.reads_this_turn = true;
    }

    /// Lets the next turn read and use PilotHouse's browser, device and
    /// test tools without asking (cleared when the turn ends).
    pub fn allow_app_tools_for_turn(&mut self) {
        self.reads_this_turn = true;
        self.app_tools_this_turn = true;
    }

    /// Forwards a review card's requests (open a file, review again).
    fn watch_card(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        if let Entry::Review { card, .. } = entry {
            self._cards
                .push(cx.subscribe(card, |_, _, event: &ReviewCardEvent, cx| {
                    cx.emit(ThreadEvent::Review(event.clone()))
                }));
        }
    }

    /// After a review chat's turn: reads the agent's reply for what the turn
    /// was for (see [`ReviewTurn`]) and saves it; a round's findings show
    /// as a card.
    fn record_review(&mut self, cx: &mut Context<Self>) {
        let Some(turn) = self.review_turn.take() else {
            return;
        };
        let reply = self
            .entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                Entry::Agent { text, .. } => Some(text.clone()),
                Entry::User { .. } => Some(String::new()),
                _ => None,
            })
            .unwrap_or_default();
        match turn {
            ReviewTurn::Round => self.record_round(reply, cx),
            ReviewTurn::Claim { finding_id } => {
                let notice = match (
                    ph_review::prompt::parse_claim(&reply),
                    self.store.pr_finding(finding_id).ok().flatten(),
                ) {
                    (Some((agree, draft)), Some(mut finding)) => {
                        finding.claim = if agree { "agree" } else { "disagree" }.into();
                        finding.draft_reply = draft;
                        match self.store.update_finding(&finding) {
                            Ok(()) => format!(
                                "{} {} a reply is drafted under the finding, to post or change.",
                                finding.label,
                                if agree {
                                    "holds up for the author:"
                                } else {
                                    "still stands:"
                                }
                            ),
                            Err(err) => format!("Could not save the answer: {err:#}"),
                        }
                    }
                    _ => "No answer block in that reply (pilothouse-claim).".into(),
                };
                self.push(
                    Entry::Notice {
                        text: notice,
                        is_error: false,
                    },
                    cx,
                );
                cx.emit(ThreadEvent::TurnEnded);
            }
            ReviewTurn::Walkthrough { run_id } => {
                let notice = match ph_review::walkthrough::parse(&reply) {
                    Some(walkthrough) => {
                        let data = serde_json::to_string(&walkthrough).unwrap_or_default();
                        match self
                            .store
                            .finish_pr_run(run_id, &walkthrough.gist.what, &data)
                        {
                            Ok(()) => {
                                "The walkthrough is ready in the pull request's Walkthrough tab."
                                    .to_string()
                            }
                            Err(err) => format!("Could not save the walkthrough: {err:#}"),
                        }
                    }
                    None => "No walkthrough block in that reply (pilothouse-walkthrough).".into(),
                };
                self.push(
                    Entry::Notice {
                        text: notice,
                        is_error: false,
                    },
                    cx,
                );
            }
        }
    }

    /// A round's findings: built from the previous round's off the UI
    /// thread (git, files), saved, and shown as the card.
    fn record_round(&mut self, reply: String, cx: &mut Context<Self>) {
        let Some(review_id) = self.review_id else {
            return;
        };
        let Ok(Some(review)) = self.store.pr_review(review_id) else {
            return;
        };
        let store = self.store.clone();
        let worktree = self.project.path.clone();
        let agent = self.agent.name;
        let work = cx.background_spawn(async move {
            ph_review::rounds::finish(&store, &review, &reply, &worktree, agent)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some(_)) => {
                        let entry = Entry::review(review_id, cx);
                        this.watch_card(&entry, cx);
                        this.push(entry, cx);
                    }
                    Ok(None) => this.push(
                        Entry::Notice {
                            text: "No findings block in that reply. Ask the agent to finish with the pilothouse-review block to see the findings here.".into(),
                            is_error: false,
                        },
                        cx,
                    ),
                    Err(err) => this.push(
                        Entry::Notice {
                            text: format!("Could not save the review: {err:#}"),
                            is_error: true,
                        },
                        cx,
                    ),
                }
                this.persist(cx);
                cx.emit(ThreadEvent::TurnEnded);
            });
        })
        .detach();
    }

    /// A stored chat with its transcript.
    pub fn open(
        store: Store,
        project: ThreadProject,
        session: Session,
        cx: &mut Context<Self>,
    ) -> Self {
        let agent = agents::find(&session.agent_id).unwrap_or(&agents::AGENTS[0]);
        let entries: Vec<Entry> = store
            .messages(session.id)
            .unwrap_or_default()
            .iter()
            .filter_map(|message| Entry::from_stored(&message.body, cx))
            .collect();
        // A review chat works in its own worktree.
        let mut project = project;
        if let Some(cwd) = session
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .filter(|cwd| cwd.is_dir())
        {
            project.path = cwd;
        }
        let review_id = store
            .pr_review_for_session(session.id)
            .ok()
            .flatten()
            .map(|review| review.id);
        let mut thread = Self::draft(store, project, agent, cx);
        thread.review_id = review_id;
        for entry in &entries {
            thread.watch_card(entry, cx);
        }
        thread.saved = entries.len();
        thread.usage = thread.store.session_usage(session.id).ok().flatten();
        thread.scroller = cx.new(|cx| TranscriptScroll::new(entries.len(), cx));
        thread.entries = entries;
        thread.session = Some(session);
        thread
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// After the chat was renamed (stored already).
    pub fn set_title(&mut self, title: &str, cx: &mut Context<Self>) {
        if let Some(session) = &mut self.session {
            session.title = title.to_string();
            cx.notify();
        }
    }

    pub fn title(&self) -> SharedString {
        self.session
            .as_ref()
            .map(|session| SharedString::from(session.title.clone()))
            .unwrap_or_else(|| "New chat".into())
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Shows a history read from elsewhere (a terminal agent's session
    /// file): the first `keep` entries stay as they are, the rest become
    /// `entries`. Nothing is stored.
    pub fn show_history(&mut self, keep: usize, entries: Vec<Entry>, cx: &mut Context<Self>) {
        let keep = keep.min(self.entries.len());
        let old = self.entries.len();
        self.entries.truncate(keep);
        let added = entries.len();
        self.entries.extend(entries);
        self.scroller.update(cx, |scroller, cx| {
            scroller.splice(keep..old, added, cx);
        });
        cx.notify();
    }

    /// Readies entry `ix` to be drawn (see [`Entry::prepare`]).
    pub fn prepare_entry(&mut self, ix: usize, cx: &mut App) {
        if let Some(entry) = self.entries.get_mut(ix) {
            entry.prepare(cx);
        }
    }

    pub fn scroller(&self) -> &Entity<TranscriptScroll> {
        &self.scroller
    }

    /// The agent waits for the user to answer a question (a permission).
    pub fn is_waiting(&self, cx: &gpui_kit::App) -> bool {
        self.entries.iter().rev().any(|entry| match entry {
            Entry::Permission(request) => request.read(cx).is_pending(),
            _ => false,
        })
    }

    /// The question the agent waits on, if any (a permission request's
    /// title).
    pub fn pending_question(&self, cx: &App) -> Option<String> {
        self.entries.iter().rev().find_map(|entry| match entry {
            Entry::Permission(request) if request.read(cx).is_pending() => {
                Some(request.read(cx).title.to_string())
            }
            _ => None,
        })
    }

    /// The last turn ended in an error (the agent failed or exited).
    pub fn failed(&self) -> bool {
        self.status == ThreadStatus::Idle
            && matches!(
                self.entries.last(),
                Some(Entry::Notice { is_error: true, .. } | Entry::SignIn { .. })
            )
    }

    /// The text of the agent's newest message.
    pub fn last_reply(&self) -> Option<String> {
        self.entries.iter().rev().find_map(|entry| match entry {
            Entry::Agent { text, .. } => Some(text.clone()),
            _ => None,
        })
    }

    pub fn status(&self) -> &ThreadStatus {
        &self.status
    }

    /// What the agent is doing, for the transcript's working row: how long
    /// it has been at it, and the step under way.
    pub fn activity(&self) -> Option<(Duration, Option<String>)> {
        if self.status == ThreadStatus::Idle {
            return None;
        }
        let elapsed = self.turn_started.map(|at| at.elapsed()).unwrap_or_default();
        let step = self.entries.iter().rev().find_map(|entry| match entry {
            Entry::User { .. } => Some(None),
            Entry::Tool(tool) if tool.is_running() => {
                let inner = tool.children.iter().rev().find(|child| child.is_running());
                Some(Some(match inner {
                    Some(child) => format!("{} › {}", tool.display_title(), child.display_title()),
                    None => tool.display_title(),
                }))
            }
            _ => None,
        });
        let step = step.flatten().or_else(|| match self.entries.last() {
            Some(Entry::Thought { .. }) => Some("Thinking".to_string()),
            Some(Entry::Agent { .. }) => Some("Writing".to_string()),
            _ => None,
        });
        Some((elapsed, step))
    }

    /// The commands the agent offers (`/name`), with their descriptions.
    pub fn commands(&self) -> &[(String, String)] {
        &self.commands
    }

    /// The agent works on its own, without a prompt of ours.
    pub fn is_background(&self) -> bool {
        self.background
    }

    pub fn queued(&self) -> &VecDeque<QueuedPrompt> {
        &self.queued
    }

    /// Keeps a message for when the agent is free.
    pub fn queue(&mut self, prompt: QueuedPrompt, cx: &mut Context<Self>) {
        self.queued.push_back(prompt);
        cx.notify();
    }

    pub fn unqueue(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<QueuedPrompt> {
        let prompt = self.queued.remove(ix);
        cx.notify();
        prompt
    }

    /// Stops the current turn and sends the first queued message right away.
    pub fn send_queued_now(&mut self, cx: &mut Context<Self>) {
        if self.queued.is_empty() {
            return;
        }
        if self.status == ThreadStatus::Idle {
            self.send_next_queued(cx);
            return;
        }
        if self.can_steer(cx) {
            self.steer_next_queued(cx);
            return;
        }
        self.send_when_stopped = true;
        self.cancel(cx);
    }

    /// Whether a message can join the turn under way instead of waiting
    /// for it (the agent supports steering).
    pub fn can_steer(&self, cx: &App) -> bool {
        self.status == ThreadStatus::Working
            && self.acp_session_id.is_some()
            && self.connection.as_ref().is_some_and(|connection| {
                matches!(connection.read(cx).ready().peek(), Some(Ok(ready)) if ready.steering)
            })
    }

    /// Adds the next queued message to the turn under way. Should the
    /// turn end first, it goes as a new prompt.
    fn steer_next_queued(&mut self, cx: &mut Context<Self>) {
        let (Some(connection), Some(session_id)) =
            (self.connection.clone(), self.acp_session_id.clone())
        else {
            return;
        };
        let Some(next) = self.queued.pop_front() else {
            return;
        };
        let user_ix = self.entries.len();
        self.push(
            Entry::User {
                text: next.display.clone(),
                attachments: next.attachments.clone(),
                checkpoint: None,
            },
            cx,
        );
        self.persist(cx);
        let ready = connection.read(cx).ready();
        let params = json!({
            "sessionId": session_id,
            "prompt": next.blocks,
            "_meta": { "steering": { "idleBehavior": "promptRequired" } },
        });
        cx.spawn(async move |this, cx| {
            let result = async {
                let ready = ready.await?;
                ready
                    .process
                    .request("_session/steering", params)
                    .await
                    .map_err(|err| SharedString::from(format!("{err:#}")))
            }
            .await;
            _ = this.update(cx, |this, cx| match result {
                Ok(reply) if reply["outcome"] == "promptRequired" => {
                    // The turn ended meanwhile: send it as the next prompt.
                    // Its message is in the transcript (and stored) already.
                    this.queued.push_front(QueuedPrompt {
                        shown_at: Some(user_ix),
                        ..next
                    });
                    this.send_next_queued(cx);
                }
                Ok(_) => {}
                Err(err) => {
                    this.queued.push_front(next);
                    this.push(
                        Entry::Notice {
                            text: format!("Could not add the message to the turn: {err}"),
                            is_error: true,
                        },
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn send_next_queued(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        if self.status != ThreadStatus::Idle {
            return;
        }
        if let Some(next) = self.queued.pop_front() {
            self.resend = next.shown_at;
            self.send(
                next.display,
                next.attachments,
                next.title,
                next.blocks,
                connection,
                cx,
            );
        }
    }

    pub fn agent(&self) -> &'static AgentSpec {
        self.agent
    }

    pub fn options(&self) -> &[AgentOption] {
        &self.options
    }

    pub fn accepts_images(&self) -> bool {
        self.accepts_images
    }

    /// Whether the agent session is running (the agent can no longer change).
    pub fn has_started(&self) -> bool {
        self.acp_session_id.is_some() || self.setup.is_some() || !self.entries.is_empty()
    }

    /// Changes the agent. Only allowed before the agent session started.
    pub fn set_agent(&mut self, agent: &'static AgentSpec, cx: &mut Context<Self>) {
        if self.has_started() || self.agent.id == agent.id {
            return;
        }
        if let Some(session) = &mut self.session {
            if self.store.set_session_agent(session.id, agent.id).is_err() {
                return;
            }
            session.agent_id = agent.id.to_string();
        }
        self.agent = agent;
        self.connection = None;
        self.options = options::with_preferences(
            options::cached(&self.store, agent.id),
            &options::preferences(&self.store, agent.id),
        );
        cx.notify();
    }

    /// Starts the agent session ahead of the first message, so pickers fill
    /// in and the first reply comes sooner. Errors surface on send instead.
    pub fn warm_up(&mut self, connection: Entity<AgentConnection>, cx: &mut Context<Self>) {
        if self.acp_session_id.is_none() {
            let setup = self.ensure_session(connection, cx);
            cx.background_spawn(async move {
                let _ = setup.await;
            })
            .detach();
        }
    }

    /// Picks `value` for option `option_id`, now and for future sessions.
    pub fn set_option(&mut self, option_id: &str, value: &str, cx: &mut Context<Self>) {
        options::remember(&self.store, self.agent.id, option_id, value);
        let Some(option) = self
            .options
            .iter_mut()
            .find(|option| option.id == option_id)
        else {
            return;
        };
        option.current = value.to_string();
        let option = option.clone();
        cx.notify();

        let (Some(connection), Some(session_id)) = (&self.connection, &self.acp_session_id) else {
            return; // Applied when the session starts.
        };
        let ready = connection.read(cx).ready();
        let (method, params) = option.set_request(session_id, value);
        cx.spawn(async move |this, cx| {
            let result = async {
                let ready = ready.await?;
                ready
                    .process
                    .request(method, params)
                    .await
                    .map_err(|err| SharedString::from(format!("{err:#}")))
            }
            .await;
            _ = this.update(cx, |this, cx| match result {
                Ok(response) => this.update_options(response.get("configOptions"), cx),
                Err(err) => this.push(
                    Entry::Notice {
                        text: format!("Could not change {}: {err}", option.name),
                        is_error: true,
                    },
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Sends a prompt. `display` is what the user typed; `blocks` is what the
    /// agent receives (it may carry files, cards and screenshots).
    pub fn send(
        &mut self,
        display: String,
        attachments: Vec<AttachmentSummary>,
        title: String,
        blocks: Vec<ContentBlock>,
        connection: Entity<AgentConnection>,
        cx: &mut Context<Self>,
    ) {
        if self.status != ThreadStatus::Idle {
            return;
        }
        let resend = self.resend.take();
        if self.session.is_none() {
            match self
                .store
                .create_session(self.project.id, &title, self.agent.id)
            {
                Ok(mut session) => {
                    if let Some(acp_id) = &self.acp_session_id {
                        let _ = self.store.set_session_acp_id(session.id, Some(acp_id));
                        session.acp_session_id = Some(acp_id.clone());
                    }
                    if let Some(review_id) = self.review_id {
                        let cwd = self.project.path.to_string_lossy().to_string();
                        let _ = self
                            .store
                            .set_session_place(session.id, "review", Some(&cwd));
                        let _ = self.store.set_pr_review_session(review_id, session.id);
                        session.kind = "review".into();
                        session.cwd = Some(cwd);
                    } else if self.parallel {
                        let cwd = self.project.path.to_string_lossy().to_string();
                        let _ = self
                            .store
                            .set_session_place(session.id, "parallel", Some(&cwd));
                        session.kind = "parallel".into();
                        session.cwd = Some(cwd);
                    } else if self.project.read_only || self.in_folder {
                        // A GitHub chat is about the pull request its folder
                        // holds; a chat started in a folder stays there.
                        let cwd = self.project.path.to_string_lossy().to_string();
                        let _ = self.store.set_session_place(session.id, "chat", Some(&cwd));
                        session.cwd = Some(cwd);
                    }
                    cx.emit(ThreadEvent::SessionCreated(session.clone()));
                    self.session = Some(session);
                }
                Err(err) => {
                    self.push(
                        Entry::Notice {
                            text: format!("Could not save the chat: {err:#}"),
                            is_error: true,
                        },
                        cx,
                    );
                    return;
                }
            }
        }

        // A chat whose agent session cannot be resumed gets a recap of the past.
        let needs_recap = self.acp_session_id.is_none()
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.acp_session_id.is_none())
            && !self.entries.is_empty();
        let preamble = needs_recap.then(|| self.history_preamble(cx)).flatten();

        let first_message = !self
            .entries
            .iter()
            .any(|entry| matches!(entry, Entry::User { .. }));
        let label = display.clone();
        let shown = resend.filter(|ix| matches!(self.entries.get(*ix), Some(Entry::User { .. })));
        let user_ix = match shown {
            Some(ix) => ix,
            None => {
                self.push(
                    Entry::User {
                        text: display,
                        attachments,
                        checkpoint: None,
                    },
                    cx,
                );
                self.persist(cx);
                self.entries.len() - 1
            }
        };
        // The files as they are before the agent touches them, taken while
        // the agent session starts. Reviews and GitHub chats only read.
        let checkpoint = (self.review_id.is_none() && !self.project.read_only).then(|| {
            let root = self.project.path.clone();
            cx.background_spawn(async move { ph_git::checkpoint::create(&root, &label) })
        });
        self.status = ThreadStatus::Connecting;
        self.turn_started = Some(Instant::now());
        self.background = false;
        // Redraws the working row's clock each second until the turn ends.
        self._quiet = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let working = this.update(cx, |this, cx| {
                    let working = this.status != ThreadStatus::Idle;
                    if working {
                        this.changed_last(cx);
                    }
                    working
                });
                if !working.unwrap_or(false) {
                    return;
                }
            }
        }));
        cx.notify();

        let mut prompt = Vec::new();
        if let Some(preamble) = preamble {
            prompt.push(ContentBlock::Text(TextContent::new(preamble)));
        }
        if let Some(note) = self.note_for_agent.take() {
            prompt.push(ContentBlock::Text(TextContent::new(note)));
        }
        // A chat's first message says what the workspace is made of.
        if first_message && let Some(note) = self.project.workspace_note() {
            prompt.push(ContentBlock::Text(TextContent::new(note)));
        }
        prompt.extend(blocks);

        let setup = self.ensure_session(connection.clone(), cx);
        let ready = connection.read(cx).ready();
        self._turn = Some(cx.spawn(async move |this, cx| {
            let result: Result<StopReason, SharedString> = async {
                let session_id = setup.await?;
                let ready = ready.await?;
                // Never let the agent start before the snapshot is taken.
                if let Some(checkpoint) = checkpoint {
                    match checkpoint.await {
                        Ok(Some(commit)) => {
                            _ = this
                                .update(cx, |this, cx| this.set_checkpoint(user_ix, commit, cx));
                        }
                        Ok(None) => {}
                        Err(err) => tracing::warn!("checkpoint failed: {err:#}"),
                    }
                }
                _ = this.update(cx, |this, cx| {
                    this.status = ThreadStatus::Working;
                    cx.notify();
                });
                let request = PromptRequest::new(session_id, prompt);
                let response = ready
                    .process
                    .request("session/prompt", to_value(&request))
                    .await
                    .map_err(|err| SharedString::from(format!("{err:#}")))?;
                // Token totals, when the agent reports them (not in the stable
                // schema yet, so read from the raw reply).
                if let Some(report) = response.get("usage").and_then(turn_usage) {
                    _ = this.update(cx, |this, cx| this.record_usage(report, cx));
                }
                let response: PromptResponse = serde_json::from_value(response)
                    .map_err(|err| SharedString::from(format!("Unexpected prompt reply: {err}")))?;
                Ok(response.stop_reason)
            }
            .await;
            _ = this.update(cx, |this, cx| this.finish_turn(result, cx));
        }));
    }

    /// Asks the agent to stop the current turn (or its own background work).
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        // Background work has no turn of ours to end: it stops here. As
        // after a stopped turn, the queue waits unless "send now" asked.
        let background = self.background;
        if background {
            self.end_background(false, cx);
        }
        let send_after = background && std::mem::take(&mut self.send_when_stopped);
        let (Some(connection), Some(session_id)) = (&self.connection, &self.acp_session_id) else {
            if send_after {
                self.send_next_queued(cx);
            }
            return;
        };
        let ready = connection.read(cx).ready();
        let params = to_value(&CancelNotification::new(session_id.clone()));
        cx.spawn(async move |this, cx| {
            if let Ok(ready) = ready.await {
                let _ = ready.process.notify("session/cancel", params);
            }
            // Only after the cancel went, or it would stop this prompt too.
            if send_after {
                _ = this.update(cx, |this, cx| this.send_next_queued(cx));
            }
        })
        .detach();
        self.cancel_pending_permissions(cx);
    }

    /// Runs an agent-handled sign-in, then asks the user to retry.
    pub fn sign_in(&mut self, method: SignInMethod, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let task = connection.update(cx, |connection, cx| {
            connection.sign_in(method.id.clone(), cx)
        });
        self.push(
            Entry::Notice {
                text: format!(
                    "Signing in with {}… finish in your browser if it opens.",
                    method.name
                ),
                is_error: false,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = task.await;
            _ = this.update(cx, |this, cx| {
                let text = match result {
                    Ok(()) => "Signed in. Send your message again.".to_string(),
                    Err(err) => err.to_string(),
                };
                // A failed session setup is forgotten, so the next send retries.
                this.setup = None;
                this.push(
                    Entry::Notice {
                        text,
                        is_error: false,
                    },
                    cx,
                );
            });
        })
        .detach();
    }

    /// Applies one streamed update from the agent. `parent` is the tool call
    /// of the subagent that sent it, if one did.
    pub fn apply_update(
        &mut self,
        update: SessionUpdate,
        parent: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match &update {
            SessionUpdate::ConfigOptionUpdate(update) => {
                let raw = serde_json::to_value(&update.config_options).unwrap_or(Value::Null);
                self.update_options(Some(&raw), cx);
                return;
            }
            SessionUpdate::CurrentModeUpdate(update) => {
                if let Some(mode) = self
                    .options
                    .iter_mut()
                    .find(|option| option.category == options::OptionCategory::Mode)
                {
                    mode.current = update.current_mode_id.0.to_string();
                    cx.notify();
                }
                return;
            }
            SessionUpdate::AvailableCommandsUpdate(update) => {
                self.commands = update
                    .available_commands
                    .iter()
                    .map(|command| (command.name.clone(), command.description.clone()))
                    .collect();
                return;
            }
            SessionUpdate::UsageUpdate(update) => {
                self.record_usage(
                    UsageReport {
                        cost: update
                            .cost
                            .as_ref()
                            .map(|cost| (cost.amount, cost.currency.clone())),
                        context: Some((update.used, update.size)),
                        ..Default::default()
                    },
                    cx,
                );
                return;
            }
            // A resumed session replays its history; the transcript is already local.
            _ if self.replaying => return,
            // Taking in a session's history: its user messages too.
            SessionUpdate::UserMessageChunk(chunk) if self.importing => {
                let Some(text) = block_text(&chunk.content) else {
                    return;
                };
                match self.entries.last_mut() {
                    Some(Entry::User { text: known, .. }) => {
                        known.push_str(&text);
                        self.changed_last(cx);
                    }
                    _ => self.push(
                        Entry::User {
                            text,
                            attachments: Vec::new(),
                            checkpoint: None,
                        },
                        cx,
                    ),
                }
                return;
            }
            _ => {}
        }
        self.last_update = Instant::now();
        // New work, not a late update to the turn that just ended.
        let starts_work = matches!(
            update,
            SessionUpdate::AgentMessageChunk(_)
                | SessionUpdate::AgentThoughtChunk(_)
                | SessionUpdate::ToolCall(_)
        );
        if starts_work
            && !self.importing
            && self.status == ThreadStatus::Idle
            && self.connection.is_some()
        {
            self.start_background(cx);
        }
        // A subagent's steps go under the tool call that started it.
        if let Some(ix) = parent.as_deref().and_then(|parent| self.tool_index(parent)) {
            if let Entry::Tool(tool) = &mut self.entries[ix] {
                match &update {
                    SessionUpdate::ToolCall(call) => tool.child_call(call),
                    SessionUpdate::ToolCallUpdate(update) => tool.child_update(update),
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let Some(text) = block_text(&chunk.content) {
                            tool.push_agent_text(&text);
                        }
                    }
                    _ => return,
                }
            }
            self.changed(ix, cx);
            return;
        }
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                let Some(text) = block_text(&chunk.content) else {
                    return;
                };
                if let Some(Entry::Agent {
                    text: existing,
                    view,
                }) = self.entries.last_mut()
                {
                    existing.push_str(&text);
                    if let Some(view) = view {
                        view.update(cx, |view, cx| view.push_str(&text, cx));
                    }
                    self.changed_last(cx);
                } else {
                    let entry = Entry::agent(text, cx);
                    self.push(entry, cx);
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                let Some(text) = block_text(&chunk.content) else {
                    return;
                };
                if let Some(Entry::Thought { text: existing, .. }) = self.entries.last_mut() {
                    existing.push_str(&text);
                    self.changed_last(cx);
                } else {
                    self.push(
                        Entry::Thought {
                            text,
                            expanded: false,
                        },
                        cx,
                    );
                }
            }
            SessionUpdate::ToolCall(call) => {
                let tool = ToolEntry::from_call(&call);
                match self.tool_index(&tool.id) {
                    Some(ix) => {
                        self.entries[ix] = Entry::Tool(tool);
                        self.changed(ix, cx);
                    }
                    None => self.push(Entry::Tool(tool), cx),
                }
            }
            SessionUpdate::ToolCallUpdate(update) => {
                if let Some(ix) = self.tool_index(&update.tool_call_id.0) {
                    if let Entry::Tool(tool) = &mut self.entries[ix] {
                        tool.apply(&update);
                    }
                    self.changed(ix, cx);
                }
            }
            SessionUpdate::Plan(plan) => {
                let items: Vec<PlanItem> = plan
                    .entries
                    .iter()
                    .map(|item| PlanItem {
                        content: item.content.clone(),
                        status: (&item.status).into(),
                    })
                    .collect();
                // One plan per turn: replace it as the agent revises it.
                let turn_start = self
                    .entries
                    .iter()
                    .rposition(|entry| matches!(entry, Entry::User { .. }))
                    .unwrap_or(0);
                let existing = self.entries[turn_start..]
                    .iter()
                    .position(|entry| matches!(entry, Entry::Plan(_)))
                    .map(|offset| turn_start + offset);
                match existing {
                    Some(ix) => {
                        self.entries[ix] = Entry::Plan(items);
                        self.changed(ix, cx);
                    }
                    None => self.push(Entry::Plan(items), cx),
                }
            }
            _ => {}
        }
    }

    /// Shows a permission question inline and in the approval inbox.
    pub fn request_permission(
        &mut self,
        request: RequestPermissionRequest,
        respond: impl FnOnce(Option<String>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let title = request
            .tool_call
            .fields
            .title
            .as_ref()
            .map(|title| {
                ToolEntry {
                    title: title.clone(),
                    ..ToolEntry::placeholder()
                }
                .display_title()
            })
            .or_else(|| {
                self.tool_index(&request.tool_call.tool_call_id.0)
                    .and_then(|ix| match &self.entries[ix] {
                        Entry::Tool(tool) => Some(tool.display_title()),
                        _ => None,
                    })
            })
            .unwrap_or_else(|| "Use a tool".to_string());
        // A review, a quick question or a GitHub chat only reads: let it,
        // without waking anyone up.
        if self.review_id.is_some()
            || self.review_turn.is_some()
            || self.reads_this_turn
            || self.project.read_only
        {
            let known =
                self.tool_index(&request.tool_call.tool_call_id.0)
                    .and_then(|ix| match &self.entries[ix] {
                        Entry::Tool(tool) => Some(tool.clone()),
                        _ => None,
                    });
            use agent_client_protocol_schema::v1::ToolKind;
            let kind =
                request
                    .tool_call
                    .fields
                    .kind
                    .or(known.as_ref().map(|tool| match tool.kind {
                        ToolKindLabel::Read => ToolKind::Read,
                        ToolKindLabel::Search => ToolKind::Search,
                        ToolKindLabel::Think => ToolKind::Think,
                        ToolKindLabel::Execute => ToolKind::Execute,
                        ToolKindLabel::Edit => ToolKind::Edit,
                        ToolKindLabel::Delete => ToolKind::Delete,
                        ToolKindLabel::Move => ToolKind::Move,
                        _ => ToolKind::Other,
                    }));
            let raw_input = request.tool_call.fields.raw_input.clone();
            let allow = request.options.iter().find(|option| {
                matches!(
                    option.kind,
                    agent_client_protocol_schema::v1::PermissionOptionKind::AllowOnce
                )
            });
            // Antigravity can only go on with every permission for the rest
            // of the turn: that is the user's to give, never automatic.
            let escalates = title.contains("with every permission");
            if let Some(allow) = allow
                && !escalates
                && (crate::review_policy::allows(kind, raw_input.as_ref(), &title)
                    || (self.app_tools_this_turn
                        && crate::review_policy::is_app_tool(kind, &title, raw_input.as_ref())))
            {
                respond(Some(allow.option_id.0.to_string()), cx);
                return;
            }
            // A GitHub chat never changes files: refused without asking.
            let reject = request.options.iter().find(|option| {
                matches!(
                    option.kind,
                    agent_client_protocol_schema::v1::PermissionOptionKind::RejectOnce
                )
            });
            if self.project.read_only
                && matches!(
                    kind,
                    Some(ToolKind::Edit | ToolKind::Delete | ToolKind::Move)
                )
                && let Some(reject) = reject
            {
                respond(Some(reject.option_id.0.to_string()), cx);
                self.push(
                    Entry::Notice {
                        text: format!(
                            "Refused “{title}”: chats in the GitHub view only read. Make changes from a workspace."
                        ),
                        is_error: false,
                    },
                    cx,
                );
                return;
            }
        }
        let source = format!("{} · {}", self.project.name, self.title());
        let choices = request
            .options
            .iter()
            .map(PermissionChoice::from_option)
            .collect();
        // Approving a plan: the plan goes in the card, to read before
        // answering.
        let plan = (request.tool_call.fields.kind
            == Some(agent_client_protocol_schema::v1::ToolKind::SwitchMode))
        .then(|| {
            let mut tool = self
                .tool_index(&request.tool_call.tool_call_id.0)
                .and_then(|ix| match &self.entries[ix] {
                    Entry::Tool(tool) => Some(tool.clone()),
                    _ => None,
                })
                .unwrap_or_else(ToolEntry::placeholder);
            tool.apply(&request.tool_call);
            tool.output
        })
        .filter(|plan| !plan.trim().is_empty())
        .map(|plan| cx.new(|cx| crate::reply::Reply::new(&plan, cx)));
        let permission = cx.new(|_| {
            let mut permission =
                PermissionRequest::new(source.into(), title.into(), choices, respond);
            permission.plan = plan;
            permission
        });
        self.add_request(permission, cx);
    }

    /// The subagents of the latest turn, with their entry index.
    pub fn subagents(&self) -> Vec<(usize, &ToolEntry)> {
        let turn = self
            .entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::User { .. }))
            .map_or(0, |user| user + 1);
        self.entries[turn..]
            .iter()
            .enumerate()
            .filter_map(|(at, entry)| match entry {
                Entry::Tool(tool) if tool.is_subagent() => Some((turn + at, tool)),
                _ => None,
            })
            .collect()
    }

    /// Shows entry `ix` (scrolled to, its details open).
    pub fn reveal(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(Entry::Tool(tool)) = self.entries.get_mut(ix)
            && !tool.expanded
        {
            tool.expanded = true;
        }
        // In a closed run, open it.
        if crate::view_mode::ViewMode::current(cx) == crate::view_mode::ViewMode::Normal
            && let Some(run) = crate::transcript::run_of(&self.entries, ix)
            && let Some(key) = crate::transcript::run_tools(&self.entries, run.clone())
                .first()
                .map(|tool| tool.id.clone())
        {
            self.open_runs.insert(key);
            self.scroller
                .update(cx, |scroller, cx| scroller.remeasure_items(run, cx));
        }
        self.changed(ix, cx);
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
    }

    /// Whether the agent's tool call `id` is in this chat (or one of its
    /// subagents').
    pub fn has_tool_call(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| match entry {
            Entry::Tool(tool) => tool.id == id || tool.children.iter().any(|child| child.id == id),
            _ => false,
        })
    }

    /// The agent asks a question with a form: a card to answer in the chat
    /// and the inbox.
    pub fn ask(
        &mut self,
        form: crate::form::Form,
        respond: impl FnOnce(crate::permission::Reply, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let source = format!("{} · {}", self.project.name, self.title());
        let question = cx.new(|_| PermissionRequest::question(source.into(), form, respond));
        self.add_request(question, cx);
    }

    /// Shows a question in the chat and the inbox; its card is measured
    /// again as it is answered (here or in the inbox).
    fn add_request(&mut self, request: Entity<PermissionRequest>, cx: &mut Context<Self>) {
        ApprovalInbox::add(&request, cx);
        cx.observe(&request, |thread, request, cx| {
            if let Some(ix) = thread
                .entries
                .iter()
                .rposition(|entry| matches!(entry, Entry::Permission(known) if *known == request))
            {
                thread.changed(ix, cx);
            }
        })
        .detach();
        self.push(Entry::Permission(request), cx);
    }

    /// The agent process died: end the turn with its last words.
    pub fn agent_exited(&mut self, message: SharedString, cx: &mut Context<Self>) {
        self.acp_session_id = None;
        self.reads_this_turn = false;
        self.app_tools_this_turn = false;
        self.setup = None;
        self.connection = None;
        if self.status != ThreadStatus::Idle {
            self.finish_turn(Err(message), cx);
        }
    }

    pub fn toggle_expanded(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.entries.get_mut(ix) {
            Some(Entry::Tool(tool)) => tool.expanded = !tool.expanded,
            Some(Entry::Thought { expanded, .. }) => *expanded = !*expanded,
            _ => return,
        }
        self.changed(ix, cx);
    }

    /// Sets up the agent session once: resume the stored one if the agent can,
    /// else start a new one; then apply the user's remembered options.
    fn ensure_session(
        &mut self,
        connection: Entity<AgentConnection>,
        cx: &mut Context<Self>,
    ) -> SetupTask {
        if let Some(setup) = &self.setup {
            return setup.clone();
        }
        self.connection = Some(connection.clone());
        let ready = connection.read(cx).ready();
        let cwd = self.project.path.clone();
        let agent = self.agent;
        let resume_id = self
            .session
            .as_ref()
            .and_then(|session| session.acp_session_id.clone());
        let prefs = options::preferences(&self.store, agent.id);
        let mcp = pilothouse_mcp_server(&cwd, agent.id, self.project.id);
        // The servers set up in Settings → MCP & skills, for every agent.
        let extra_mcp = crate::mcp_config::for_session(&self.store);
        let extra_dirs = self.project.extra_dirs.clone();

        let task = cx.spawn(async move |this, cx| -> Result<String, SharedString> {
            let ready: Ready = match ready.await {
                Ok(ready) => ready,
                Err(err) => {
                    let _ = this.update(cx, |this, cx| this.offer_sign_in(&err, &[], cx));
                    return Err(err);
                }
            };
            let mcp_servers: Vec<McpServer> = mcp.into_iter().chain(extra_mcp).collect();
            let extra: Vec<String> = extra_dirs
                .iter()
                .map(|dir| dir.to_string_lossy().into_owned())
                .collect();

            // Resume when possible; fall back to a fresh session.
            let mut response: Option<(String, Value)> = None;
            if let (Some(resume_id), true) = (&resume_id, ready.load_session) {
                // The agent plays the session back. PilotHouse keeps its own
                // transcript, so the replay is dropped, except for a chat
                // that has none: one resumed from the agent's own sessions
                // (a CLI's), which takes it as its history.
                // Listen for the replay, which comes before the answer.
                let weak = this.clone();
                let replay_id = resume_id.clone();
                let _ = this.update(cx, |this, cx| {
                    this.replaying = !this.entries.is_empty();
                    this.importing = !this.replaying;
                    connection.update(cx, |connection, _| {
                        connection.register_thread(replay_id, weak)
                    });
                });
                let mut params =
                    json!({ "sessionId": resume_id, "cwd": cwd, "mcpServers": mcp_servers });
                if !extra.is_empty() {
                    params["additionalDirectories"] = json!(extra);
                }
                if let Ok(result) = ready.process.request("session/load", params).await {
                    response = Some((resume_id.clone(), result));
                }
                let _ = this.update(cx, |this, cx| {
                    this.replaying = false;
                    this.importing = false;
                    this.persist(cx);
                });
            }
            let (session_id, result) = match response {
                Some(found) => found,
                None => {
                    let mut params = json!({ "cwd": cwd, "mcpServers": mcp_servers });
                    if !extra.is_empty() {
                        params["additionalDirectories"] = json!(extra);
                    }
                    let result = match ready.process.request("session/new", params).await {
                        Ok(result) => result,
                        Err(err) => {
                            let err =
                                SharedString::from(format!("Could not start a session: {err:#}"));
                            let methods = ready.sign_in.clone();
                            let _ =
                                this.update(cx, |this, cx| this.offer_sign_in(&err, &methods, cx));
                            return Err(err);
                        }
                    };
                    let id = result
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| SharedString::from("The agent did not return a session id"))?
                        .to_string();
                    (id, result)
                }
            };

            let offered = options::parse_session_options(&result);
            // Apply remembered choices the agent did not pick itself.
            for option in &offered {
                if let Some(value) = prefs
                    .get(&option.id)
                    .filter(|value| **value != option.current)
                {
                    let (method, params) = option.set_request(&session_id, value);
                    let _ = ready.process.request(method, params).await;
                }
            }
            let weak = this.clone();
            let id = session_id.clone();
            let _ = this.update(cx, |this, cx| {
                this.acp_session_id = Some(id.clone());
                this.accepts_images = ready.images;
                if !offered.is_empty() {
                    options::cache(&this.store, agent.id, &offered);
                    this.options = options::with_preferences(offered, &prefs);
                }
                if let Some(session) = &mut this.session {
                    session.acp_session_id = Some(id.clone());
                    let _ = this.store.set_session_acp_id(session.id, Some(&id));
                }
                connection.update(cx, |connection, _| connection.register_thread(id, weak));
                cx.notify();
            });
            Ok(session_id)
        });
        let setup = task.shared();
        self.setup = Some(setup.clone());
        setup
    }

    /// When setup failed for lack of a login, offer the agent's sign-in methods.
    fn offer_sign_in(
        &mut self,
        error: &SharedString,
        methods: &[SignInMethod],
        cx: &mut Context<Self>,
    ) {
        self.setup = None;
        if needs_login(error) && !methods.is_empty() {
            self.push(
                Entry::SignIn {
                    message: error.to_string(),
                    methods: methods.to_vec(),
                },
                cx,
            );
        }
    }

    fn update_options(&mut self, raw: Option<&Value>, cx: &mut Context<Self>) {
        let Some(raw) = raw else {
            return;
        };
        let mut options = options::parse_options(raw);
        if options.is_empty() {
            return;
        }
        // A model or mode from the older lists stays unless replaced.
        let older = self.options.iter().filter(|option| {
            option.source != options::OptionSource::ConfigOption
                && !options.iter().any(|new| new.category == option.category)
        });
        options.extend(older.cloned().collect::<Vec<_>>());
        options::cache(&self.store, self.agent.id, &options);
        self.options = options;
        cx.notify();
    }

    /// The agent sent updates with no turn of ours running: Claude Code
    /// carries on by itself after background work. Shown as working until it
    /// goes quiet.
    fn start_background(&mut self, cx: &mut Context<Self>) {
        self.background = true;
        self.status = ThreadStatus::Working;
        self.turn_started = Some(Instant::now());
        let started = self.entries.len();
        self._quiet = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let done = this.update(cx, |this, cx| {
                    let quiet = this.last_update.elapsed();
                    let running = this.entries[started.min(this.entries.len())..]
                        .iter()
                        .any(|entry| matches!(entry, Entry::Tool(tool) if tool.is_running()));
                    let done = !this.background
                        || (quiet > BACKGROUND_QUIET && !running)
                        || quiet > BACKGROUND_SILENT;
                    if done && this.background {
                        this.end_background(true, cx);
                    }
                    cx.notify();
                    done
                });
                if done.unwrap_or(true) {
                    return;
                }
            }
        }));
        cx.notify();
    }

    /// The agent's own work is over (`send_queued`: it ended by itself, so
    /// a queued message goes now).
    fn end_background(&mut self, send_queued: bool, cx: &mut Context<Self>) {
        self.background = false;
        self.status = ThreadStatus::Idle;
        self.turn_started = None;
        self.changed_last(cx);
        self.persist(cx);
        cx.emit(ThreadEvent::TurnEnded);
        cx.notify();
        if send_queued {
            self.send_next_queued(cx);
        }
    }

    fn finish_turn(&mut self, result: Result<StopReason, SharedString>, cx: &mut Context<Self>) {
        cx.emit(ThreadEvent::TurnEnded);
        self.status = ThreadStatus::Idle;
        self.turn_started = None;
        // The working row goes and the turn's summary comes.
        self.changed_last(cx);
        self.reads_this_turn = false;
        self.app_tools_this_turn = false;
        self.cancel_pending_permissions(cx);
        let notice = |text: &str, is_error| Entry::Notice {
            text: text.to_string(),
            is_error,
        };
        let result_kind = result.as_ref().ok().cloned();
        let result_failed = result.is_err();
        // A turn that did not finish has no answer to read; what is said
        // next is conversation.
        if !matches!(result_kind, Some(StopReason::EndTurn)) {
            self.review_turn = None;
        }
        match result {
            Ok(StopReason::EndTurn) => self.record_review(cx),
            Ok(StopReason::Cancelled) => self.push(notice("Stopped.", false), cx),
            Ok(StopReason::MaxTokens) => {
                self.push(notice("The agent hit its output limit.", true), cx)
            }
            Ok(StopReason::Refusal) => {
                self.push(notice("The agent declined to continue.", true), cx)
            }
            Ok(_) => {}
            // A sign-in card was already shown for login problems.
            Err(_) if matches!(self.entries.last(), Some(Entry::SignIn { .. })) => {}
            Err(message) => {
                let text = if needs_login(&message) {
                    format!("{message}\n\n{}", self.agent.login_hint)
                } else {
                    message.to_string()
                };
                self.push(notice(&text, true), cx);
            }
        }
        self.persist(cx);
        cx.notify();
        // Messages written meanwhile go next: after a finished turn, or at
        // once when the user asked to stop for one.
        let stopped = matches!(result_kind, Some(StopReason::Cancelled)) || result_failed;
        if !stopped || std::mem::take(&mut self.send_when_stopped) {
            self.send_next_queued(cx);
        }
    }

    fn cancel_pending_permissions(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<_> = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Permission(request) if request.read(cx).is_pending() => {
                    Some(request.clone())
                }
                _ => None,
            })
            .collect();
        for request in pending {
            request.update(cx, |request, cx| request.cancel(cx));
        }
    }

    /// Writes entries that are not stored yet (drafts store nothing).
    fn persist(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        for (ix, entry) in self.entries.iter().enumerate().skip(self.saved) {
            let Some((role, body)) = entry.to_stored(cx) else {
                continue;
            };
            match self.store.append_message(session.id, role, &body) {
                Ok(id) if matches!(entry, Entry::User { .. }) => {
                    self.user_message_ids.insert(ix, id);
                }
                Ok(_) => {}
                Err(err) => tracing::warn!("saving chat message failed: {err:#}"),
            }
        }
        self.saved = self.entries.len();
    }

    /// Records the checkpoint taken before user message `ix`.
    fn set_checkpoint(&mut self, ix: usize, commit: String, cx: &mut Context<Self>) {
        let Some(Entry::User { checkpoint, .. }) = self.entries.get_mut(ix) else {
            return;
        };
        *checkpoint = Some(commit);
        if let (Some(id), Some((_, body))) = (
            self.user_message_ids.get(&ix),
            self.entries[ix].to_stored(cx),
        ) && let Err(err) = self.store.update_message(*id, &body)
        {
            tracing::warn!("saving the checkpoint failed: {err:#}");
        }
        cx.notify();
    }

    pub fn usage(&self) -> Option<&SessionUsage> {
        self.usage.as_ref()
    }

    /// Counts an agent's usage report for this chat (kept in memory for a
    /// draft, stored once the chat is).
    fn record_usage(&mut self, mut report: UsageReport, cx: &mut Context<Self>) {
        report.agent_session = self
            .acp_session_id
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or_default();
        match &self.session {
            Some(session) => match self.store.record_usage(session.id, &report) {
                Ok(usage) => self.usage = Some(usage),
                Err(err) => tracing::warn!("saving usage failed: {err:#}"),
            },
            None => {
                let usage = self.usage.get_or_insert_with(SessionUsage::default);
                if report.context.is_some() {
                    usage.context = report.context;
                }
            }
        }
        cx.notify();
    }

    /// Whether files can be restored now: not while the agent works on
    /// them, nor during another restore.
    pub fn can_restore(&self) -> bool {
        self.status == ThreadStatus::Idle && !self.restoring
    }

    /// Puts the project's files back to `checkpoint`, noting it in the chat
    /// with a way to undo. `message` is the user message the checkpoint was
    /// taken before; `None` when undoing a restore.
    pub fn restore_checkpoint(
        &mut self,
        checkpoint: String,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.can_restore() {
            return;
        }
        self.restoring = true;
        cx.notify();
        let root = self.project.path.clone();
        let work =
            cx.background_spawn(async move { ph_git::checkpoint::restore(&root, &checkpoint) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            _ = this.update(cx, |this, cx| {
                this.restoring = false;
                let entry = match result {
                    Ok(restored) => {
                        this.note_for_agent = Some(
                            "(Note from PilotHouse: the user restored the project's files to an earlier checkpoint. \
                             Files may differ from what you read or wrote since; read them again before relying on them.)"
                                .to_string(),
                        );
                        Entry::Restored {
                            text: restore_summary(&restored, message.as_deref()),
                            undo: Some(restored.undo),
                        }
                    }
                    Err(err) => Entry::Notice {
                        text: format!("Could not restore the files: {err:#}"),
                        is_error: true,
                    },
                };
                this.push(entry, cx);
                this.persist(cx);
            });
        })
        .detach();
    }

    /// Earlier conversation, for an agent session that could not be resumed.
    fn history_preamble(&self, cx: &App) -> Option<String> {
        let history = self.recent_history(cx)?;
        Some(format!(
            "Context: this continues an earlier conversation in PilotHouse. Here is the recent history:\n\n{history}\n\n---\n"
        ))
    }

    /// What another agent needs to take this chat over: who had it and
    /// what was said and done.
    pub fn handoff_recap(&self, cx: &App) -> Option<String> {
        let history = self.recent_history(cx)?;
        Some(format!(
            "Context: you are taking over work the user started with another coding agent ({}) in PilotHouse, \
             in the chat “{}”. Here is its recent history:\n\n{history}\n\n---\n\
             The project's files are as that agent left them; check their current state before relying on \
             the history. Then continue with the user's message below.\n",
            self.agent.name,
            self.title()
        ))
    }

    /// What a side chat's agent needs: this chat's recent history, and that
    /// the user asks something aside.
    pub fn side_recap(&self, cx: &App) -> Option<String> {
        let history = self.recent_history(cx)?;
        Some(format!(
            "Context: the user steps aside from the chat “{}” in PilotHouse to ask something about it. \
             Here is its recent history:\n\n{history}\n\n---\n\
             Answer the question below. Nothing said here goes back to that chat, and do not change \
             files unless the user asks.\n",
            self.title()
        ))
    }

    /// Starts this draft as a continuation: `note` goes to the agent with
    /// the first message, and the chat says where it came from.
    pub fn continue_from(&mut self, note: String, origin: String, cx: &mut Context<Self>) {
        self.note_for_agent = Some(note);
        self.push(
            Entry::Notice {
                text: origin,
                is_error: false,
            },
            cx,
        );
    }

    /// The transcript as plain lines, cut to the most recent part.
    fn recent_history(&self, cx: &App) -> Option<String> {
        let mut lines = Vec::new();
        for entry in &self.entries {
            match entry {
                Entry::User { text, .. } => lines.push(format!("User: {text}")),
                Entry::Agent { text, .. } => lines.push(format!("Assistant: {text}")),
                Entry::Tool(tool) => lines.push(format!("(tool: {})", tool.title)),
                Entry::Permission(request) => {
                    lines.push(format!("(permission: {})", request.read(cx).summary()))
                }
                _ => {}
            }
        }
        let mut history = lines.join("\n");
        if history.trim().is_empty() {
            return None;
        }
        if history.len() > HISTORY_PREAMBLE_CHARS {
            let mut cut = history.len() - HISTORY_PREAMBLE_CHARS;
            while !history.is_char_boundary(cut) {
                cut += 1;
            }
            history = format!("…{}", &history[cut..]);
        }
        Some(history)
    }

    fn tool_index(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::Tool(tool) if tool.id == id))
    }

    fn push(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.entries.push(entry);
        self.scroller.update(cx, |scroller, cx| {
            scroller.append(1, cx);
        });
        // A new step can make a run of the ones before it, or grow one:
        // its first row's summary and the others' heights change.
        let last = self.entries.len() - 1;
        if let Some(run) = crate::transcript::run_of(&self.entries, last) {
            self.scroller.update(cx, |scroller, cx| {
                scroller.remeasure_items(run.start..last, cx);
            });
        }
        cx.notify();
    }

    fn changed_last(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.entries.len().checked_sub(1) {
            self.changed(ix, cx);
        }
    }

    /// Row `ix` changed height (streamed text, expanded tool, new status);
    /// in a run, the run's first row (its summary) changes with it.
    fn changed(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows = crate::transcript::run_of(&self.entries, ix).map_or(ix..ix + 1, |run| {
            run.start.min(ix)..(ix + 1).max(run.start + 1)
        });
        self.scroller.update(cx, |scroller, cx| {
            scroller.remeasure_items(rows, cx);
        });
        cx.notify();
    }

    /// Shown in `mode`: every row is measured again if it changed.
    pub fn set_view_mode(&mut self, mode: crate::view_mode::ViewMode, cx: &mut Context<Self>) {
        if self.view_mode != mode {
            self.view_mode = mode;
            self.scroller
                .update(cx, |scroller, cx| scroller.remeasure(cx));
        }
    }

    /// Whether the run whose first tool call is `key` is open.
    pub fn is_run_open(&self, key: &str) -> bool {
        self.open_runs.contains(key)
    }

    /// Opens or closes the run entry `ix` belongs to.
    pub fn toggle_run(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(run) = crate::transcript::run_of(&self.entries, ix) else {
            return;
        };
        let Some(key) = crate::transcript::run_tools(&self.entries, run.clone())
            .first()
            .map(|tool| tool.id.clone())
        else {
            return;
        };
        if !self.open_runs.remove(&key) {
            self.open_runs.insert(key);
        }
        self.scroller.update(cx, |scroller, cx| {
            scroller.remeasure_items(run, cx);
        });
        cx.notify();
    }
}

/// Whether an error looks like "you need to log in first".
fn needs_login(message: &str) -> bool {
    let lower = message.to_lowercase();
    [
        "auth",
        "login",
        "log in",
        "sign in",
        "api key",
        "credential",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn to_value(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The PilotHouse MCP server, offered to every new agent session so agents
/// can use the board, the browser and devices. It is this binary in `mcp` mode.
/// `project_id` names the workspace outright: the agent may work in a copy
/// (a review, a parallel lane) that no workspace's folder holds.
fn pilothouse_mcp_server(
    project_path: &std::path::Path,
    agent_id: &str,
    project_id: ProjectId,
) -> Option<McpServer> {
    let exe = std::env::current_exe().ok()?;
    let mut env = Vec::new();
    if let Some(dir) = std::env::var_os(ph_store::paths::DATA_DIR_ENV) {
        env.push(EnvVariable::new(
            ph_store::paths::DATA_DIR_ENV,
            dir.to_string_lossy().to_string(),
        ));
    }
    Some(McpServer::Stdio(
        McpServerStdio::new("pilothouse", exe)
            .args(vec![
                "mcp".into(),
                "--project".into(),
                project_path.to_string_lossy().to_string(),
                "--agent".into(),
                agent_id.to_string(),
                "--workspace".into(),
                project_id.to_string(),
            ])
            .env(env),
    ))
}

/// The prompt reply's `usage` (running totals for the agent session).
fn turn_usage(usage: &Value) -> Option<UsageReport> {
    let number = |key: &str| usage.get(key).and_then(Value::as_u64);
    let report = UsageReport {
        input_tokens: number("inputTokens"),
        output_tokens: number("outputTokens"),
        cached_tokens: number("cachedReadTokens"),
        ..Default::default()
    };
    (report.input_tokens.is_some() || report.output_tokens.is_some()).then_some(report)
}

/// What a restore did, for the chat: "Restored 2 files and removed 1
/// created since “Fix the login…”." (`message` is `None` for an undo.)
fn restore_summary(restored: &ph_git::checkpoint::Restored, message: Option<&str>) -> String {
    let files = |n: usize| format!("{n} file{}", if n == 1 { "" } else { "s" });
    let Some(message) = message else {
        return match restored.restored + restored.removed {
            0 => "Undid the restore; the files already matched.".to_string(),
            changed => format!("Undid the restore ({} changed back).", files(changed)),
        };
    };
    let mut short: String = message.chars().take(60).collect();
    if message.chars().count() > 60 {
        short.push('…');
    }
    let message = short;
    let said = match (restored.restored, restored.removed) {
        (0, 0) => format!("The files already matched how they were before “{message}”."),
        (0, removed) => format!("Removed {} created since “{message}”.", files(removed)),
        (written, 0) => format!("Restored {} to before “{message}”.", files(written)),
        (written, removed) => format!(
            "Restored {} and removed {} created since “{message}”.",
            files(written),
            files(removed)
        ),
    };
    match restored.kept {
        0 => said,
        kept => format!(
            "{said} Kept {} that may be new: the checkpoint could not hold every untracked file.",
            files(kept)
        ),
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;

    #[test]
    fn restore_summaries_say_what_changed() {
        let done = |restored, removed| ph_git::checkpoint::Restored {
            restored,
            removed,
            kept: 0,
            undo: String::new(),
        };
        assert_eq!(
            restore_summary(&done(0, 1), Some("Add hello")),
            "Removed 1 file created since “Add hello”."
        );
        assert_eq!(
            restore_summary(&done(2, 0), Some("Fix")),
            "Restored 2 files to before “Fix”."
        );
        assert_eq!(
            restore_summary(&done(1, 3), Some("x")),
            "Restored 1 file and removed 3 files created since “x”."
        );
        assert!(
            restore_summary(&done(0, 0), Some(&"a".repeat(80)))
                .contains(&format!("{}…", "a".repeat(60)))
        );
        assert_eq!(
            restore_summary(&done(1, 1), None),
            "Undid the restore (2 files changed back)."
        );
    }
}
