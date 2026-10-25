//! A workspace's chats while PilotHouse runs: its agents' processes and every
//! chat opened or started, held outside any window. The chat column
//! ([`crate::ChatPanel`]) only shows them, so a chat keeps going when its
//! screen is closed or rebuilt (another window, an unloaded workspace), and
//! a schedule can start one with no screen at all.

use std::collections::HashMap;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription};
use ph_store::Store;
use ph_store::projects::{Project, ProjectId};
use ph_store::sessions::SessionId;

use crate::agents::{self, AGENTS, AgentSpec};
use crate::connection::AgentConnection;
use crate::thread::{ChatThread, ThreadEvent, ThreadProject, ThreadStatus};

/// Most chats kept loaded; beyond this the least recently used idle ones
/// are let go (they load again from the store when opened).
const MAX_KEPT: usize = 24;

/// Settings key for the agent new chats start with.
pub(crate) const DEFAULT_AGENT_KEY: &str = "agent.default";

/// The agent new chats start with.
pub(crate) fn default_agent(store: &Store) -> &'static AgentSpec {
    let id: String = store.setting(DEFAULT_AGENT_KEY);
    agents::find(&id).unwrap_or(&AGENTS[0])
}

/// Every workspace's chats, by workspace.
#[derive(Default)]
pub struct ChatHub {
    chats: HashMap<ProjectId, Entity<WorkspaceChats>>,
}

impl Global for ChatHub {}

impl ChatHub {
    /// The chats of `project`, made on first use.
    pub fn chats(store: &Store, project: &Project, cx: &mut App) -> Entity<WorkspaceChats> {
        if let Some(chats) = Self::existing(project.id, cx) {
            return chats;
        }
        let chats = cx.new(|_| WorkspaceChats::new(store.clone(), ThreadProject::of(project)));
        cx.default_global::<Self>()
            .chats
            .insert(project.id, chats.clone());
        chats
    }

    pub fn existing(project_id: ProjectId, cx: &App) -> Option<Entity<WorkspaceChats>> {
        cx.try_global::<Self>()?.chats.get(&project_id).cloned()
    }

    /// Ends a removed workspace's chats (its agents stop).
    pub fn remove(project_id: ProjectId, cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.global_mut::<Self>().chats.remove(&project_id);
        }
    }
}

#[derive(Debug, Clone)]
pub enum WorkspaceChatsEvent {
    /// A chat was made (here or by its first message).
    SessionsChanged,
    /// A chat started with [`WorkspaceChats::start_background`] finished its
    /// turn; `reply` is the start of the agent's last message.
    BackgroundDone {
        session: SessionId,
        title: String,
        reply: String,
    },
}

/// What a workspace's chats are doing, for its status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatsActivity {
    /// Chats whose agent is starting or working.
    pub working: usize,
    /// Chats waiting for the user to answer a question.
    pub waiting: usize,
    /// Turns that ended while the workspace was not on screen.
    pub unseen: usize,
}

/// A loaded chat.
struct Kept {
    thread: Entity<ChatThread>,
    last_used: u64,
    _watches: [Subscription; 2],
}

pub struct WorkspaceChats {
    store: Store,
    project: ThreadProject,
    /// One process per agent, started on first use (and again if it exited).
    connections: HashMap<&'static str, Entity<AgentConnection>>,
    /// Chats opened or started, with their watches and when last used.
    threads: HashMap<SessionId, Kept>,
    /// Counts uses, to know which kept chat was used least lately.
    uses: u64,
    /// Turn-end watches of chats started in the background (each fires
    /// once: later turns are the user's).
    _background: Vec<Subscription>,
    /// The workspace is shown in a window.
    on_screen: bool,
    /// Chats whose turn ended while it was not.
    unseen: Vec<SessionId>,
}

impl EventEmitter<WorkspaceChatsEvent> for WorkspaceChats {}

impl WorkspaceChats {
    fn new(store: Store, project: ThreadProject) -> Self {
        Self {
            store,
            project,
            connections: HashMap::new(),
            threads: HashMap::new(),
            uses: 0,
            _background: Vec::new(),
            on_screen: false,
            unseen: Vec::new(),
        }
    }

    /// The workspace's folders changed: new chats and agents get them (a
    /// chat already started keeps its folders until its agent restarts).
    pub fn set_project(&mut self, project: &Project) {
        self.project = ThreadProject::of(project);
    }

    /// A new, unsent chat with `agent` (not kept until its first message).
    pub(crate) fn draft(
        &self,
        agent: &'static AgentSpec,
        cx: &mut Context<Self>,
    ) -> Entity<ChatThread> {
        let (store, project) = (self.store.clone(), self.project.clone());
        cx.new(|cx| ChatThread::draft(store, project, agent, cx))
    }

    /// The chat of `session`, loaded the first time.
    pub(crate) fn thread_for(
        &mut self,
        session: ph_store::sessions::Session,
        cx: &mut Context<Self>,
    ) -> Entity<ChatThread> {
        self.uses += 1;
        if let Some(kept) = self.threads.get_mut(&session.id) {
            kept.last_used = self.uses;
            return kept.thread.clone();
        }
        let id = session.id;
        let (store, project) = (self.store.clone(), self.project.clone());
        let thread = cx.new(|cx| ChatThread::open(store, project, session, cx));
        self.keep(id, thread.clone(), cx);
        thread
    }

    /// Keeps `thread` (now stored as `session`) loaded, so it keeps working
    /// off screen.
    pub(crate) fn keep(
        &mut self,
        session: SessionId,
        thread: Entity<ChatThread>,
        cx: &mut Context<Self>,
    ) {
        self.uses += 1;
        if let Some(kept) = self.threads.get_mut(&session) {
            kept.last_used = self.uses;
            return;
        }
        let watches = [
            // Status follows the chats' state.
            cx.observe(&thread, |_, _, cx| cx.notify()),
            cx.subscribe(&thread, move |this, _, event: &ThreadEvent, cx| {
                if let ThreadEvent::TurnEnded = event
                    && !this.on_screen
                    && !this.unseen.contains(&session)
                {
                    this.unseen.push(session);
                    cx.notify();
                }
            }),
        ];
        self.threads.insert(
            session,
            Kept {
                thread,
                last_used: self.uses,
                _watches: watches,
            },
        );
        self.let_go_of_idle(cx);
    }

    /// Past [`MAX_KEPT`], lets go of the least recently used chats that are
    /// idle (not working, not waiting, nothing unseen). A chat on screen
    /// keeps its own handle and is used again when shown.
    fn let_go_of_idle(&mut self, cx: &App) {
        while self.threads.len() > MAX_KEPT {
            let oldest = self
                .threads
                .iter()
                // Never the one just kept.
                .filter(|(id, kept)| {
                    let thread = kept.thread.read(cx);
                    kept.last_used != self.uses
                        && *thread.status() == ThreadStatus::Idle
                        && !thread.is_waiting(cx)
                        && !self.unseen.contains(id)
                })
                .min_by_key(|(_, kept)| kept.last_used)
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    self.threads.remove(&id);
                }
                None => break,
            }
        }
    }

    /// The workspace's process for `spec`, started on first use (and again
    /// if it exited).
    pub(crate) fn connection(
        &mut self,
        spec: &'static AgentSpec,
        cx: &mut Context<Self>,
    ) -> Entity<AgentConnection> {
        if let Some(connection) = self.connections.get(spec.id)
            && !connection.read(cx).has_exited()
        {
            return connection.clone();
        }
        let cwd = self.project.path.clone();
        let connection = cx.new(|cx| AgentConnection::start(spec, cwd, cx));
        self.connections.insert(spec.id, connection.clone());
        connection
    }

    /// Starts a chat named `title` with `prompt` and the default agent,
    /// shown nowhere (a scheduled run).
    /// [`WorkspaceChatsEvent::BackgroundDone`] says when the agent is done.
    pub fn start_background(
        &mut self,
        title: String,
        prompt: String,
        cx: &mut Context<Self>,
    ) -> Option<SessionId> {
        let agent = default_agent(&self.store);
        let thread = self.draft(agent, cx);
        let connection = self.connection(agent, cx);
        let blocks = vec![agent_client_protocol_schema::v1::ContentBlock::Text(
            agent_client_protocol_schema::v1::TextContent::new(prompt.clone()),
        )];
        thread.update(cx, |thread, cx| {
            thread.send(prompt, Vec::new(), title.clone(), blocks, connection, cx)
        });
        let session = thread.read(cx).session().map(|session| session.id)?;
        self.keep(session, thread.clone(), cx);
        let mut done = false;
        self._background.push(
            cx.subscribe(&thread, move |_, thread, event: &ThreadEvent, cx| {
                // Only the scheduled turn: once the user replies in the chat,
                // its turns are theirs, not the schedule's.
                if let ThreadEvent::TurnEnded = event
                    && !std::mem::replace(&mut done, true)
                {
                    let reply = thread.read(cx).last_reply().unwrap_or_default();
                    cx.emit(WorkspaceChatsEvent::BackgroundDone {
                        session,
                        title: title.clone(),
                        reply: reply.chars().take(400).collect(),
                    });
                }
            }),
        );
        cx.emit(WorkspaceChatsEvent::SessionsChanged);
        Some(session)
    }

    /// The workspace is (or is no longer) shown in a window. Showing it
    /// counts as seeing what finished: the chats whose turn ended while it
    /// was away are returned (and forgotten).
    pub fn set_on_screen(&mut self, on_screen: bool, cx: &mut Context<Self>) -> Vec<SessionId> {
        self.on_screen = on_screen;
        if !on_screen || self.unseen.is_empty() {
            return Vec::new();
        }
        cx.notify();
        std::mem::take(&mut self.unseen)
    }

    /// What the chats are doing now.
    pub fn activity(&self, cx: &App) -> ChatsActivity {
        let mut activity = ChatsActivity {
            unseen: self.unseen.len(),
            ..Default::default()
        };
        for kept in self.threads.values() {
            let thread = kept.thread.read(cx);
            if thread.is_waiting(cx) {
                activity.waiting += 1;
            } else if matches!(
                thread.status(),
                ThreadStatus::Working | ThreadStatus::Connecting
            ) {
                activity.working += 1;
            }
        }
        activity
    }

    /// The chat of `session` waits for the user to answer a question.
    pub fn is_waiting(&self, session: SessionId, cx: &App) -> bool {
        self.threads
            .get(&session)
            .is_some_and(|kept| kept.thread.read(cx).is_waiting(cx))
    }

    /// The chat of `session` is open and its agent is busy.
    pub fn is_working(&self, session: SessionId, cx: &App) -> bool {
        self.threads.get(&session).is_some_and(|kept| {
            matches!(
                kept.thread.read(cx).status(),
                ThreadStatus::Working | ThreadStatus::Connecting
            )
        })
    }

    /// Renames chat `session` (stored, and in its loaded thread).
    pub fn rename(
        &mut self,
        session: SessionId,
        title: &str,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.store.rename_session(session, title)?;
        if let Some(kept) = self.threads.get(&session) {
            kept.thread
                .update(cx, |thread, cx| thread.set_title(title, cx));
        }
        cx.emit(WorkspaceChatsEvent::SessionsChanged);
        Ok(())
    }

    /// Deletes chat `session` and its transcript. Refused while its agent
    /// works or waits for an answer, so nothing is cut off mid-turn.
    pub fn delete(&mut self, session: SessionId, cx: &mut Context<Self>) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_working(session, cx) && !self.is_waiting(session, cx),
            "the chat is still working"
        );
        self.store.delete_session(session)?;
        self.threads.remove(&session);
        self.unseen.retain(|id| *id != session);
        cx.emit(WorkspaceChatsEvent::SessionsChanged);
        Ok(())
    }
}
