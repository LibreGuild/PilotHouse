//! The root view of a window (the main one, or one a workspace moved to;
//! see [`crate::windows`]).
//!
//! A window shows one of two views, picked in the title bar:
//! - **Workspaces**: the sidebar lists the Chats space (chats outside any
//!   workspace) and every workspace with its chats; the screen of the one in
//!   front fills the rest.
//! - **GitHub** ([`crate::github_view`]): pull requests across repositories,
//!   reviews and the repositories' chats, apart from any workspace.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, TitleBar, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyView, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, Styled as _, Subscription, Window,
    div,
};
use ph_store::projects::{Project, ProjectId};
use ph_theme::Appearance;

use crate::actions::*;
use crate::app_state::{AppState, keys};
use crate::context_panel::ContextTab;
use crate::github_view::GithubView;
use crate::status::WorkspaceStatus;
use crate::workspace_screen::{WorkspaceScreen, WorkspaceScreenEvent};
use crate::{command_palette, settings_dialog, sidebar, status_bar, title_bar};
use ph_files::{FindInProject, GoToSymbol, QuickOpen};

/// A hidden workspace's screen is closed after this long with nothing going on.
const UNLOAD_AFTER: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// What a window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// The Bridge: every task of every workspace, by where it stands.
    #[default]
    Bridge,
    /// A workspace's screen (the one in front), or the Chats space's.
    Workspaces,
    /// Reviews: pull requests across repositories and their agent reviews.
    GitHub,
    /// One of the organisation's own (`ph_custom::views`), by its id.
    Custom(&'static str),
}

/// Owns the project list and the open [`WorkspaceScreen`]s, and routes workspace
/// actions to whichever project is active.
pub struct Workspace {
    focus_handle: FocusHandle,
    /// This window.
    handle: gpui_kit::AnyWindowHandle,
    /// The workspaces, newest first (the sidebar's order, and ⌃1–⌃9's).
    projects: Vec<Project>,
    /// The workspace for chats that need no folder, as this window knows
    /// it: it has a place of its own in the column of icons, over the
    /// user's workspaces (kept here, not asked of the store while drawing).
    chats: Option<ProjectId>,
    view: View,
    /// What the Bridge shows.
    bridge: crate::bridge::BridgeState,
    /// The GitHub view, made when first shown.
    github: Option<Entity<GithubView>>,
    /// The organisation's own views (`ph_custom::views`), each made when
    /// first shown.
    custom_views: HashMap<&'static str, AnyView>,
    /// Pinned chats, as last read (another window may pin one too).
    pub(crate) pinned: Vec<ph_store::sessions::Session>,
    /// Chats marked done.
    settled: HashSet<ph_store::sessions::SessionId>,
    /// What the agent CLIs in terminals looked like at the last check.
    terminal_agents_seen: u64,
    /// The changes to chats made away from their screen it has seen.
    session_changes_seen: crate::session_changes::Seen,
    /// Status checks so far (one a second).
    status_ticks: u32,
    /// The chats of the workspaces whose screen is closed, read again
    /// every few seconds (a schedule may add one).
    sessions: HashMap<ProjectId, Rc<[ph_store::sessions::Session]>>,
    /// Spaces stay alive after switching away so their state survives.
    spaces: HashMap<ProjectId, Entity<WorkspaceScreen>>,
    active_project: Option<ProjectId>,
    /// The workspace shown before the one in front (Ctrl+Tab goes back).
    previous_project: Option<ProjectId>,
    /// Reviews' list of repositories is hidden.
    sidebar_collapsed: bool,
    /// What each workspace is doing, read again every second.
    statuses: HashMap<ProjectId, WorkspaceStatus>,
    /// When each loaded but hidden workspace was last shown.
    hidden_since: HashMap<ProjectId, std::time::Instant>,
    _status_poll: gpui_kit::Task<()>,
    /// The every-few-seconds read of the stored chats, while it runs.
    _sessions_poll: Option<gpui_kit::Task<()>>,
    _subscriptions: Vec<Subscription>,
    /// The status bar follows the devices once something uses them.
    _devices: Option<Subscription>,
    /// The title bar's inbox and to-dos.
    pub(crate) inbox: Entity<crate::inbox::InboxView>,
    /// The branch, pull request and checks of the folders tasks work in
    /// (shared by the windows), for the cards and the title bar.
    facts: Entity<crate::task_facts::TaskFacts>,
}

impl Workspace {
    /// The main window's root, showing the workspace shown last.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_showing(None, window, cx)
    }

    /// A window's root showing workspace `project` (else the one shown last).
    pub fn new_showing(
        project: Option<ProjectId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = AppState::store(cx);
        // Scratch folders left behind by a moved data folder come back
        // first, so chats can start in them.
        if let Err(err) = store.heal_scratch_folders(&ph_store::paths::scratch_root()) {
            tracing::warn!("checking scratch folders failed: {err:#}");
        }
        // The workspace for chats that need no folder is always there
        // (made now when there is none): a quick chat's home, and where a
        // first start opens straight into a chat.
        let scratch = store
            .scratch_workspace(&ph_store::paths::scratch_root())
            .inspect_err(|err| tracing::error!("making the Chats workspace failed: {err:#}"))
            .ok()
            .map(|scratch| scratch.id);
        let projects = workspaces(store);
        // No workspace of the user's own yet.
        let first_start = projects.iter().all(|project| Some(project.id) == scratch);
        let last_project: Option<ProjectId> = store.setting(keys::LAST_PROJECT);
        let sidebar_collapsed: bool = store.setting(keys::SIDEBAR_COLLAPSED);
        let front: String = store.setting(keys::FRONT_VIEW);

        // Follow OS light/dark changes while the preference is "System".
        let appearance_subscription = cx.observe_window_appearance(window, |_, window, cx| {
            if AppState::appearance(cx) == Appearance::System {
                ph_theme::apply_appearance(&Appearance::System, Some(window), cx);
            }
        });

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let weak = cx.weak_entity();
        let inbox = cx.new(|cx| crate::inbox::InboxView::new(weak, window, cx));
        let inbox_subscription = cx.observe(&inbox, |_, _, cx| cx.notify());
        // A closed menu or popover can leave nothing with the keyboard, and
        // then no shortcut works until something is clicked: the chat in
        // front takes it back.
        let focus_subscription =
            cx.on_focus_lost(window, |this, window, cx| this.focus_front(window, cx));
        // Schedules run app-wide; the inbox shows what they did.
        let automation = crate::schedules::Automation::global(cx);
        let automation_subscription = cx.subscribe(
            &automation,
            |this, _, _: &crate::schedules::AutomationChanged, cx| this.refresh_inbox(cx),
        );
        let mut workspace = Self {
            focus_handle,
            handle: window.window_handle(),
            projects,
            chats: scratch,
            view: View::Bridge,
            bridge: Default::default(),
            github: None,
            custom_views: HashMap::new(),
            pinned: Vec::new(),
            // Chats deleted since (their ids can be used again) are dropped.
            settled: {
                let store = AppState::store(cx);
                store
                    .setting::<Vec<ph_store::sessions::SessionId>>(keys::SIDEBAR_SETTLED)
                    .into_iter()
                    .filter(|id| store.session(*id).ok().flatten().is_some())
                    .collect()
            },
            terminal_agents_seen: 0,
            session_changes_seen: crate::session_changes::current(cx),
            status_ticks: 0,
            sessions: HashMap::new(),
            spaces: HashMap::new(),
            active_project: None,
            previous_project: None,
            sidebar_collapsed,
            statuses: HashMap::new(),
            hidden_since: HashMap::new(),
            // Status comes from chats and test runs outside this window; a
            // cheap look each second catches every change.
            _sessions_poll: None,
            _status_poll: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                    if this
                        .update(cx, |this: &mut Self, cx| this.refresh_statuses(cx))
                        .is_err()
                    {
                        return;
                    }
                }
            }),
            _subscriptions: vec![
                appearance_subscription,
                inbox_subscription,
                automation_subscription,
                focus_subscription,
            ],
            _devices: None,
            inbox,
            facts: {
                let store = AppState::store(cx).clone();
                crate::task_facts::hub(&store, cx)
            },
        };
        // What a card says of its branch and pull request shows when found.
        let facts = workspace.facts.clone();
        cx.observe(&facts, |_, _, cx| cx.notify()).detach();
        // A window that closes stops asking about its folders.
        cx.on_release(move |this, cx| {
            let window = this.handle.window_id();
            facts.update(cx, |facts, _| facts.forget(window));
        })
        .detach();
        workspace.want_facts(cx);
        // Copies made for agents that are no longer needed go, quietly.
        let tidy_store = AppState::store(cx).clone();
        cx.background_spawn(async move {
            let removed = crate::housekeeping::tidy(&tidy_store);
            if removed > 0 {
                tracing::info!("removed {removed} unused working copies");
            }
        })
        .detach();
        // The workspace shown last is ready behind whatever is in front,
        // else the newest one.
        let initial = project
            .or(last_project)
            .filter(|id| workspace.has_project(*id))
            .or_else(|| workspace.projects.first().map(|project| project.id));
        if let Some(id) = initial {
            workspace.open_project(id, window, cx);
        }
        // In front: the workspace this window was opened for; else what was
        // in front last. Someone who has workspaces and comes from a
        // version without the Bridge meets it first; with no workspace yet
        // the chat is the place to be.
        let bridge = match front.as_str() {
            "workspace" => false,
            "bridge" => true,
            // The chat, on a first start: its only workspace is the one
            // for chats, and the Bridge would have nothing to show.
            _ => !first_start && !workspace.projects.is_empty(),
        };
        if project.is_none() && bridge {
            workspace.view = View::Bridge;
            if let Some(space) = workspace.active_space().cloned() {
                space.update(cx, |space, cx| space.set_visible(false, cx));
            }
            // Opening the workspace behind noted it as what is in front,
            // and its chat may have taken the keyboard: neither holds. A
            // chat that is not drawn would keep every shortcut from the
            // window until something was clicked.
            let _ = AppState::store(cx).set_setting(keys::FRONT_VIEW, &front_key(View::Bridge));
            workspace.focus_handle.focus(window, cx);
        }
        // The Bridge lists the chats of spaces whose screen is not open
        // from the store, from the start.
        workspace.read_sessions(cx);
        workspace
    }

    /// Says which folders this window's tasks work in: each workspace's
    /// first folder, and the folder of each open chat that has its own (a
    /// worktree). Their branch and pull request are found for the cards.
    fn want_facts(&mut self, cx: &mut Context<Self>) {
        let mut folders = Vec::new();
        for project in &self.projects {
            folders.extend(project.folders().into_iter().next());
            for session in self.sessions_of(project.id, cx).iter() {
                if let Some(cwd) = &session.cwd
                    && !self.is_settled(session.id)
                {
                    folders.push(std::path::PathBuf::from(cwd));
                }
            }
        }
        folders.sort();
        folders.dedup();
        let window = self.handle.window_id();
        self.facts
            .update(cx, |facts, cx| facts.want(window, folders, cx));
    }

    /// The workspaces (not the Chats space, nor GitHub repositories').
    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    pub fn view(&self) -> View {
        self.view
    }

    /// What the Bridge shows.
    pub(crate) fn bridge(&self) -> &crate::bridge::BridgeState {
        &self.bridge
    }

    /// Narrows the Bridge to one workspace's tasks (`None`: all of them).
    pub(crate) fn set_bridge_filter(&mut self, filter: Option<ProjectId>, cx: &mut Context<Self>) {
        self.bridge.filter = filter;
        cx.notify();
    }

    /// Lists every card of a Bridge column, or its newest again.
    pub(crate) fn toggle_bridge_all(&mut self, column: &'static str, cx: &mut Context<Self>) {
        if !self.bridge.all.remove(column) {
            self.bridge.all.insert(column);
        }
        cx.notify();
    }

    /// A view's entry in the left column was pressed (the Bridge,
    /// Reviews, one of the organisation's own).
    pub(crate) fn show(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(view, window, cx);
        self.focus_front(window, cx);
    }

    /// Opens what a card of the Bridge stands for.
    pub(crate) fn open_task_card(
        &mut self,
        project: ProjectId,
        target: &crate::bridge::Target,
        github: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::bridge::Target;
        match target {
            Target::Chat(session) if github => self.show_session(*session, window, cx),
            Target::Chat(session) => self.open_chat(project, *session, window, cx),
            // The terminal takes the keyboard itself: what is typed next
            // is the answer the agent waits for.
            Target::Terminal(pane) => {
                return self.open_terminal_agent(project, pane, false, window, cx);
            }
            // Its chat is beside the pull request.
            Target::Preparing(_) => self.set_view(View::GitHub, window, cx),
        }
        self.focus_front(window, cx);
    }

    /// Opens a task with the review of its workspace's changes beside it.
    pub(crate) fn review_task(
        &mut self,
        project: ProjectId,
        session: ph_store::sessions::SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_chat(project, session, window, cx);
        if let Some(space) = self.spaces.get(&project).cloned() {
            space.update(cx, |space, cx| {
                space.show_tab(crate::bridge::REVIEW_TOOL, window, cx)
            });
        }
    }

    /// Whether `id` is the workspace for chats that need no folder: it is
    /// not removed and takes no folders (see `Store::scratch_workspace`).
    pub(crate) fn is_scratch(&self, id: ProjectId, cx: &gpui_kit::App) -> bool {
        AppState::store(cx).is_scratch(id)
    }

    /// The workspace for chats that need no folder, shown as *Chats* over
    /// the user's workspaces in the column of icons.
    pub(crate) fn chats_place(&self) -> Option<&Project> {
        let chats = self.chats?;
        self.projects.iter().find(|project| project.id == chats)
    }

    /// The user's workspaces, in the order the column of icons shows them:
    /// all but the one for chats that need no folder.
    pub(crate) fn listed(&self) -> impl Iterator<Item = &Project> {
        self.projects
            .iter()
            .filter(|project| Some(project.id) != self.chats)
    }

    /// The workspace for chats that need no folder ("Chats"); this
    /// window's list takes it up when it was made elsewhere.
    pub(crate) fn scratch(&mut self, cx: &mut Context<Self>) -> Option<ProjectId> {
        let store = AppState::store(cx).clone();
        let project = store
            .scratch_workspace(&ph_store::paths::scratch_root())
            .inspect_err(|err| tracing::error!("making the Chats workspace failed: {err:#}"))
            .ok()?;
        self.chats = Some(project.id);
        if !self.has_project(project.id) {
            self.projects = workspaces(&store);
            self.projects_changed(cx);
            cx.notify();
        }
        Some(project.id)
    }

    /// Brings the workspace for chats that need no folder forward.
    pub fn show_scratch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(scratch) = self.scratch(cx) {
            self.open_project(scratch, window, cx);
            self.focus_front(window, cx);
        }
    }

    /// The screen to draw while a workspace is what the window shows.
    fn screen_in_front(&self) -> Option<&Entity<WorkspaceScreen>> {
        (self.view == View::Workspaces)
            .then_some(self.active_project)
            .flatten()
            .and_then(|id| self.spaces.get(&id))
    }

    /// Shows the Bridge, the workspace last in front, Reviews, or one of
    /// the organisation's views.
    pub fn set_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        // Nothing was in front yet to come back to.
        let view = match view {
            View::Workspaces if self.active_project.is_none() => View::Bridge,
            view => view,
        };
        // A build whose brand leaves the GitHub view out stays where it is.
        if view == View::GitHub && !ph_brand::enabled(ph_brand::Feature::GitHub) {
            return;
        }
        // An organisation's view, made the first time it is shown; an id no
        // view has (one since removed) changes nothing.
        if let View::Custom(id) = view
            && !self.custom_views.contains_key(id)
        {
            let Some(custom) = ph_custom::views()
                .into_iter()
                .find(|custom| custom.id == id)
            else {
                return;
            };
            let context = ph_custom::CustomContext {
                store: AppState::store(cx).clone(),
            };
            let built = (custom.build)(&context, window, cx);
            self.custom_views.insert(custom.id, built);
        }
        self.view = view;
        if view == View::GitHub {
            let (github, folders) = (self.github(cx), self.folders());
            github.update(cx, |github, cx| github.shown(folders, cx));
        }
        // Only a shown workspace streams browser and device frames.
        if let Some(space) = self.active_space().cloned() {
            space.update(cx, |space, cx| {
                space.set_visible(view == View::Workspaces, cx)
            });
        }
        let _ = AppState::store(cx).set_setting(keys::FRONT_VIEW, &front_key(view));
        self.focus_front(window, cx);
        cx.notify();
    }

    /// The GitHub view, made the first time.
    pub fn github(&mut self, cx: &mut Context<Self>) -> Entity<GithubView> {
        if let Some(github) = &self.github {
            return github.clone();
        }
        let (weak, folders) = (cx.weak_entity(), self.folders());
        let github = cx.new(|cx| GithubView::new(weak, folders, cx));
        let shown = !self.sidebar_collapsed;
        github.update(cx, |github, cx| github.set_sidebar_shown(shown, cx));
        self._subscriptions
            .push(cx.observe(&github, |_, _, cx| cx.notify()));
        self.github = Some(github.clone());
        github
    }

    /// Each workspace's name and folders (for the GitHub view's
    /// repositories).
    fn folders(&self) -> Vec<(String, Vec<std::path::PathBuf>)> {
        self.projects
            .iter()
            .map(|project| (project.name.clone(), project.folders()))
            .collect()
    }

    /// The GitHub view, if it was shown.
    pub fn github_view(&self) -> Option<&Entity<GithubView>> {
        self.github.as_ref()
    }

    /// Shows chat `session` where it lives: its workspace or the Chats
    /// space, or the GitHub view for a repository's.
    pub fn show_session(
        &mut self,
        session: ph_store::sessions::SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = AppState::store(cx).clone();
        let Some(found) = store.session(session).ok().flatten() else {
            return;
        };
        let Some(project) = store.project(found.project_id).ok().flatten() else {
            return;
        };
        if project.kind == ph_store::projects::ProjectKind::GitHub {
            if !ph_brand::enabled(ph_brand::Feature::GitHub) {
                return;
            }
            self.set_view(View::GitHub, window, cx);
            let github = self.github(cx);
            github.update(cx, |github, cx| {
                github.show_session(project, session, window, cx)
            });
            return;
        }
        self.open_chat(project.id, session, window, cx);
    }

    /// Shows chat `session` of workspace (or Chats space) `project`.
    pub fn open_chat(
        &mut self,
        project: ProjectId,
        session: ph_store::sessions::SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An agent CLI's chat whose terminal is still open: the terminal.
        if let Some(pane) = crate::terminal_agents::TerminalAgents::of_session(session, cx)
            .map(|agent| agent.pane.clone())
        {
            return self.open_terminal_agent(project, &pane, true, window, cx);
        }
        // A workspace made since this window last read them (by the quick
        // chat, by another window): it is read now, not found missing.
        if !self.has_project(project) {
            self.reload_projects(cx);
        }
        // Shown in another window: the chat opens there, in the window
        // that comes forward (not only the workspace, with whatever chat
        // it had in front).
        if let Some((handle, other)) = crate::windows::shown_elsewhere(project, self.handle, cx) {
            // Deferred: that window may be this one's caller.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    window.activate_window();
                    other.update(cx, |other, cx| {
                        other.open_chat(project, session, window, cx)
                    });
                });
            });
            return;
        }
        self.open_project(project, window, cx);
        if let Some(space) = self.spaces.get(&project) {
            space.update(cx, |space, cx| space.select_session(session, cx));
        }
    }

    /// Shows the terminal agent `pane` runs in (of workspace `project`),
    /// its news seen.
    pub(crate) fn open_terminal_agent(
        &mut self,
        project: ProjectId,
        pane: &str,
        transcript: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view != View::Workspaces {
            self.set_view(View::Workspaces, window, cx);
        }
        self.open_project(project, window, cx);
        let terminal = crate::terminal_agents::TerminalAgents::find(pane, cx)
            .and_then(|agent| agent.terminal.upgrade());
        if let (Some(space), Some(terminal)) = (self.spaces.get(&project).cloned(), terminal) {
            space.update(cx, |space, cx| {
                space.show_terminal(&terminal, window, cx);
                if transcript {
                    space.show_transcript(&terminal, window, cx);
                }
            });
        }
        crate::terminal_agents::TerminalAgents::seen(pane, cx);
        cx.notify();
    }

    /// A new chat in workspace (or Chats space) `project`.
    /// Puts what the user sent from Chrome in a composer: the chat on screen
    /// (the workspace on screen, else the Chats space), or a new chat there.
    /// Says where, for the extension to tell the user.
    pub(crate) fn receive_from_browser(
        &mut self,
        new_chat: bool,
        attachments: Vec<ph_agent::Attachment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        if self.view != View::Workspaces {
            self.set_view(View::Workspaces, window, cx);
        }
        let project = self
            .active_project
            .or_else(|| self.scratch(cx))
            .ok_or_else(|| anyhow::anyhow!("Open a workspace in {} first.", ph_brand::NAME))?;
        self.open_project(project, window, cx);
        let space = self
            .spaces
            .get(&project)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("The workspace could not be opened."))?;
        let name = space.read(cx).project().name.clone();
        // A read-only chat on screen (a closed terminal's): a new one.
        let new_chat = new_chat || !space.read(cx).chat_takes_messages(cx);
        space.update(cx, |space, cx| {
            if new_chat {
                space.new_session(window, cx);
            }
            space.attach_to_chat(attachments, window, cx);
        });
        cx.notify();
        Ok(if new_chat {
            format!("a new chat in “{name}”")
        } else {
            format!("the chat in “{name}”")
        })
    }

    /// Drafts a message about `attachment` (`prompt` in the composer, for
    /// the user to change and send) in the chat on screen.
    pub(crate) fn draft_from_browser(
        &mut self,
        attachment: ph_agent::Attachment,
        prompt: &str,
        new_chat: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        if self.view != View::Workspaces {
            self.set_view(View::Workspaces, window, cx);
        }
        let project = self
            .active_project
            .or_else(|| self.scratch(cx))
            .ok_or_else(|| anyhow::anyhow!("Open a workspace in {} first.", ph_brand::NAME))?;
        self.open_project(project, window, cx);
        let space = self
            .spaces
            .get(&project)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("The workspace could not be opened."))?;
        let name = space.read(cx).project().name.clone();
        let new_chat = new_chat || !space.read(cx).chat_takes_messages(cx);
        space.update(cx, |space, cx| {
            if new_chat {
                space.new_session(window, cx);
            }
            space.draft_in_chat(attachment, prompt, window, cx)
        });
        Ok(if new_chat {
            format!("a new chat in “{name}”")
        } else {
            format!("the chat in “{name}”")
        })
    }

    /// An agent started using workspace `project`'s built-in browser: its
    /// Browser tab comes into view, so the user sees the testing. Another
    /// workspace on screen stays on screen, and a Browser tool the user
    /// switched off (Settings → Features) stays off. Says whether it showed.
    pub(crate) fn show_browser_for_agent(
        &mut self,
        project: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(space) = self.spaces.get(&project).cloned() else {
            return false;
        };
        if !AppState::enabled_tabs(cx).contains(&ContextTab::Browser) {
            return false;
        }
        space.update(cx, |space, cx| {
            space.show_tab(ContextTab::Browser, window, cx)
        });
        cx.notify();
        true
    }

    pub fn new_chat_in(&mut self, project: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        // A workspace made since this window last read them (by the quick
        // chat): read now.
        if !self.has_project(project) {
            self.reload_projects(cx);
        }
        self.open_project(project, window, cx);
        if let Some(space) = self.spaces.get(&project) {
            space.update(cx, |space, cx| space.new_session(window, cx));
        }
    }

    /// The chats the sidebar lists for `id`: its screen's, else as last read
    /// (shared, not copied: the sidebar asks on each draw).
    pub(crate) fn sessions_of(
        &self,
        id: ProjectId,
        cx: &gpui_kit::App,
    ) -> Rc<[ph_store::sessions::Session]> {
        match self.spaces.get(&id) {
            Some(space) => space.read(cx).sessions().clone(),
            None => self
                .sessions
                .get(&id)
                .cloned()
                .unwrap_or_else(|| Rc::new([])),
        }
    }

    /// The workspaces without a screen: their chats are read from the
    /// store (a screen lists its own).
    fn sessions_wanted(&self) -> Vec<ProjectId> {
        self.projects
            .iter()
            .map(|project| project.id)
            .filter(|id| !self.spaces.contains_key(id))
            .collect()
    }

    /// Reads the chats of the workspaces without a screen, and of the
    /// Chats space: the Bridge lists every task, loaded or not. At once,
    /// for what the user just did; the look every few seconds is
    /// [`Self::poll_sessions`].
    fn read_sessions(&mut self, cx: &mut Context<Self>) -> bool {
        let store = AppState::store(cx).clone();
        let mut changed = self.read_pinned(cx);
        let wanted = self.sessions_wanted();
        self.sessions.retain(|id, _| wanted.contains(id));
        for id in wanted {
            let sessions = store.sessions(id).unwrap_or_default();
            if self.sessions.get(&id).map(|known| &known[..]) != Some(&sessions[..]) {
                self.sessions.insert(id, sessions.into());
                changed = true;
            }
        }
        changed
    }

    /// Reads what [`Self::read_sessions`] reads off the UI thread and takes
    /// it when it is there. Nobody waits for the look every few seconds
    /// (for chats a schedule or another window changed), and a query that
    /// waits behind a writer must not hold the window up.
    fn poll_sessions(&mut self, cx: &mut Context<Self>) {
        if self._sessions_poll.is_some() {
            return;
        }
        let store = AppState::store(cx).clone();
        let wanted = self.sessions_wanted();
        let read = cx.background_spawn(async move {
            let pinned: Vec<ph_store::sessions::SessionId> = store.setting(keys::SIDEBAR_PINNED);
            let pinned: Vec<_> = pinned
                .iter()
                .filter_map(|id| store.session(*id).ok().flatten())
                .collect();
            let sessions: Vec<_> = wanted
                .into_iter()
                .map(|id| (id, store.sessions(id).unwrap_or_default()))
                .collect();
            (pinned, sessions)
        });
        self._sessions_poll = Some(cx.spawn(async move |this, cx| {
            let (pinned, sessions) = read.await;
            _ = this.update(cx, |this, cx| {
                this._sessions_poll = None;
                let mut changed = pinned != this.pinned;
                this.pinned = pinned;
                let wanted = this.sessions_wanted();
                this.sessions.retain(|id, _| wanted.contains(id));
                for (id, sessions) in sessions {
                    // A screen opened meanwhile lists its own.
                    if !wanted.contains(&id) {
                        continue;
                    }
                    if this.sessions.get(&id).map(|known| &known[..]) != Some(&sessions[..]) {
                        this.sessions.insert(id, sessions.into());
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            });
        }));
    }

    /// Re-reads the pinned chats (dropping ids whose chat is gone).
    fn read_pinned(&mut self, cx: &mut Context<Self>) -> bool {
        let store = AppState::store(cx);
        let ids: Vec<ph_store::sessions::SessionId> = store.setting(keys::SIDEBAR_PINNED);
        let pinned: Vec<_> = ids
            .iter()
            .filter_map(|id| store.session(*id).ok().flatten())
            .collect();
        let changed = pinned != self.pinned;
        self.pinned = pinned;
        changed
    }

    pub(crate) fn is_pinned(&self, id: ph_store::sessions::SessionId) -> bool {
        self.pinned.iter().any(|session| session.id == id)
    }

    /// Chat `id` was marked done.
    pub(crate) fn is_settled(&self, id: ph_store::sessions::SessionId) -> bool {
        self.settled.contains(&id)
    }

    /// Marks chat `id` (of `project`) done, its news seen; or not done.
    pub(crate) fn toggle_settled(
        &mut self,
        project: ProjectId,
        id: ph_store::sessions::SessionId,
        cx: &mut Context<Self>,
    ) {
        if self.settled.remove(&id) {
            self.save_settled(cx);
            cx.notify();
        } else {
            self.settle_all(&[(project, id)], cx);
        }
    }

    /// Marks every one of `chats` (each with its workspace) done, whatever
    /// it was: its news counts as seen. (A chat marked done before that
    /// finished another turn since is put away again, not taken back out.)
    pub(crate) fn settle_all(
        &mut self,
        chats: &[(ProjectId, ph_store::sessions::SessionId)],
        cx: &mut Context<Self>,
    ) {
        for (project, id) in chats {
            self.settled.insert(*id);
            if let Some(chats) = ph_agent::ChatHub::existing(*project, cx) {
                chats.update(cx, |chats, cx| chats.settle(*id, cx));
            }
            // One in its agent's CLI: what it finished counts as seen.
            let pane = crate::terminal_agents::TerminalAgents::of_session(*id, cx)
                .map(|agent| agent.pane.clone());
            if let Some(pane) = pane {
                crate::terminal_agents::TerminalAgents::seen(&pane, cx);
            }
        }
        self.save_settled(cx);
        // The dots and counts follow at once, not at the next look.
        self.refresh_statuses(cx);
        cx.notify();
    }

    fn save_settled(&self, cx: &gpui_kit::App) {
        let mut ids: Vec<_> = self.settled.iter().copied().collect();
        ids.sort();
        if let Err(err) = AppState::store(cx).set_setting(keys::SIDEBAR_SETTLED, &ids) {
            tracing::warn!("saving the chats marked done failed: {err:#}");
        }
    }

    /// Pins chat `id` to the top of the sidebar, or unpins it.
    pub(crate) fn toggle_pin(&mut self, id: ph_store::sessions::SessionId, cx: &mut Context<Self>) {
        let store = AppState::store(cx);
        let mut ids: Vec<ph_store::sessions::SessionId> = store.setting(keys::SIDEBAR_PINNED);
        if let Some(at) = ids.iter().position(|pinned| *pinned == id) {
            ids.remove(at);
        } else {
            ids.push(id);
        }
        if let Err(err) = store.set_setting(keys::SIDEBAR_PINNED, &ids) {
            tracing::warn!("saving the pinned chats failed: {err:#}");
        }
        self.read_pinned(cx);
        cx.notify();
    }

    /// The chats of `project`, loading them when its screen is not open.
    fn chats_of(
        &self,
        project: ProjectId,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<ph_agent::WorkspaceChats>> {
        let project = self
            .projects
            .iter()
            .find(|candidate| candidate.id == project)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the workspace is gone"))?;
        let store = AppState::store(cx).clone();
        Ok(ph_agent::ChatHub::chats(&store, &project, cx))
    }

    /// Asks for a new name for chat `session` of `project`.
    pub(crate) fn rename_chat(
        &mut self,
        project: ProjectId,
        session: &ph_store::sessions::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, workspace) = (session.id, cx.entity().downgrade());
        crate::rename_dialog::open(
            "Rename chat",
            session.title.clone(),
            window,
            cx,
            move |title, window, cx| {
                let title = title.trim().to_string();
                if title.is_empty() {
                    return;
                }
                _ = workspace.update(cx, |workspace, cx| {
                    let renamed = workspace.chats_of(project, cx).and_then(|chats| {
                        chats.update(cx, |chats, cx| chats.rename(id, &title, cx))
                    });
                    // One in its agent's CLI is called that there too.
                    if renamed.is_ok() {
                        crate::terminal_agents::TerminalAgents::renamed(id, &title, cx);
                    }
                    if let Err(err) = renamed {
                        window.push_notification(
                            Notification::error(format!("Could not rename the chat: {err:#}")),
                            cx,
                        );
                    }
                    workspace.read_sessions(cx);
                    cx.notify();
                });
            },
        );
    }

    /// Asks before deleting chat `session` of `project` with its transcript.
    pub(crate) fn delete_chat(
        &mut self,
        project: ProjectId,
        session: &ph_store::sessions::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, workspace) = (session.id, cx.entity().downgrade());
        let title = session.title.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            let workspace = workspace.clone();
            dialog
                .title("Delete chat?")
                .w(ph_ui::scaled_px(420., window))
                .child(format!(
                    "“{title}” and its transcript will be deleted. This cannot be undone."
                ))
                .footer(ph_ui::dialog_buttons("Delete", true))
                .on_ok(move |_, window, cx| {
                    _ = workspace.update(cx, |workspace, cx| {
                        workspace.delete_chat_now(project, id, window, cx)
                    });
                    true
                })
        });
    }

    fn delete_chat_now(
        &mut self,
        project: ProjectId,
        id: ph_store::sessions::SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The chat on screen gives way to a new one first.
        if let Some(space) = self.spaces.get(&project).cloned()
            && space.read(cx).active_session() == Some(id)
        {
            space.update(cx, |space, cx| space.new_session(window, cx));
        }
        let deleted = self
            .chats_of(project, cx)
            .and_then(|chats| chats.update(cx, |chats, cx| chats.delete(id, cx)));
        // Its agent's CLI, if it shows as one, goes with it.
        if deleted.is_ok()
            && let Some(space) = self.spaces.get(&project).cloned()
        {
            space.update(cx, |space, cx| space.close_cli(id, cx));
        }
        if let Err(err) = deleted {
            window.push_notification(
                Notification::error(format!("Could not delete the chat: {err:#}")),
                cx,
            );
            return;
        }
        if self.is_pinned(id) {
            self.toggle_pin(id, cx);
        }
        // Its id can be a new chat's later: that one is not done.
        if self.settled.remove(&id) {
            self.save_settled(cx);
        }
        self.read_sessions(cx);
        cx.notify();
    }

    pub fn active_project_id(&self) -> Option<ProjectId> {
        self.active_project
    }

    pub fn active_space(&self) -> Option<&Entity<WorkspaceScreen>> {
        self.active_project.and_then(|id| self.spaces.get(&id))
    }

    pub fn is_sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }

    /// Gives the keyboard to the chat in front (the workspace's, or the
    /// GitHub view's), else the window.
    pub(crate) fn focus_front(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let View::Custom(_) = self.view {
            self.focus_handle.focus(window, cx);
            return;
        }
        if self.view == View::GitHub {
            let focused = self.github.clone().is_some_and(|github| {
                github.update(cx, |github, cx| github.focus_chat(window, cx))
            });
            if !focused {
                self.focus_handle.focus(window, cx);
            }
            return;
        }
        // The screen that is drawn: one hidden behind "no workspace yet"
        // cannot take the keyboard (no shortcut would reach the window).
        let taken = self
            .screen_in_front()
            .cloned()
            .is_some_and(|space| space.update(cx, |space, cx| space.focus_chat(window, cx)));
        if !taken {
            self.focus_handle.focus(window, cx);
        }
    }

    /// This window has a screen for workspace `id`.
    pub fn has_space(&self, id: ProjectId) -> bool {
        self.spaces.contains_key(&id)
    }

    /// Moves workspace `id` to a window of its own. Its chats, shells and
    /// test runs carry on; files with unsaved changes must be saved first,
    /// since the editor is rebuilt there.
    pub fn open_in_new_window(
        &mut self,
        id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(space) = self.spaces.get(&id) {
            let unsaved = space.read(cx).unsaved_files(cx);
            if unsaved > 0 {
                window.push_notification(
                    Notification::warning(format!(
                        "Save or close the {unsaved} changed file{} first.",
                        if unsaved == 1 { "" } else { "s" }
                    ))
                    .title("Open in new window"),
                    cx,
                );
                return;
            }
            space.update(cx, |space, cx| space.set_visible(false, cx));
        }
        self.spaces.remove(&id);
        self.hidden_since.remove(&id);
        if self.active_project == Some(id) {
            self.active_project = None;
            let next = self
                .previous_project
                .filter(|previous| *previous != id && self.has_project(*previous))
                .or_else(|| {
                    self.projects
                        .iter()
                        .map(|project| project.id)
                        .find(|other| *other != id && !self.shown_by_another(*other, cx))
                });
            if let Some(next) = next {
                self.open_project(next, window, cx);
            }
        }
        cx.notify();
        cx.defer(move |cx| {
            if let Err(err) = crate::windows::open_window(Some(id), cx) {
                tracing::error!("opening a window failed: {err:#}");
            }
        });
    }

    pub(crate) fn has_project(&self, id: ProjectId) -> bool {
        self.projects.iter().any(|project| project.id == id)
    }

    fn shown_by_another(&self, id: ProjectId, cx: &mut Context<Self>) -> bool {
        crate::windows::shown_elsewhere(id, self.handle, cx).is_some()
    }

    /// Reads the workspace list again (another window changed it).
    pub(crate) fn reload_projects(&mut self, cx: &mut Context<Self>) {
        crate::attention::forget_names(cx);
        let store = AppState::store(cx);
        let projects = workspaces(store);
        let kept = |id: &ProjectId| projects.iter().any(|project| project.id == *id);
        self.spaces.retain(|id, _| kept(id));
        self.hidden_since.retain(|id, _| kept(id));
        self.chats = projects
            .iter()
            .map(|project| project.id)
            .find(|id| store.is_scratch(*id));
        self.projects = projects;
        cx.notify();
    }

    /// Tells the other windows the workspace list changed.
    fn projects_changed(&self, cx: &mut Context<Self>) {
        let me = cx.entity_id();
        for other in crate::windows::all(cx) {
            if other.entity_id() != me {
                other.update(cx, |other, cx| other.reload_projects(cx));
            }
        }
    }

    /// Runs Git again in every open workspace (network drives changed).
    pub(crate) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        for space in self.spaces.values() {
            space.update(cx, |space, cx| space.refresh_git(cx));
        }
        cx.notify();
    }

    /// What workspace `id` is doing (as of the last second).
    pub fn status(&self, id: ProjectId) -> WorkspaceStatus {
        self.statuses.get(&id).copied().unwrap_or_default()
    }

    /// Closes the screens of workspaces hidden for `hidden_for` (every
    /// second: [`UNLOAD_AFTER`]) with nothing going on and nothing to lose,
    /// so memory stays flat however many are visited. Their chats and
    /// shells live on; opening one again rebuilds its screen from what was
    /// saved.
    pub fn unload_idle(&mut self, hidden_for: std::time::Duration, cx: &mut Context<Self>) {
        let idle: Vec<ProjectId> = self
            .hidden_since
            .iter()
            .filter(|(id, since)| {
                let status = self.status(**id);
                since.elapsed() >= hidden_for
                    && status.working == 0
                    && status.waiting == 0
                    && self
                        .spaces
                        .get(id)
                        .is_some_and(|space| space.read(cx).can_unload(cx))
            })
            .map(|(id, _)| *id)
            .collect();
        let closed = !idle.is_empty();
        for id in idle {
            tracing::info!("closing the screen of idle workspace {id}");
            self.spaces.remove(&id);
            self.hidden_since.remove(&id);
        }
        // The sidebar lists their chats from the store from now on.
        if closed {
            self.read_sessions(cx);
            cx.notify();
        }
    }

    fn refresh_statuses(&mut self, cx: &mut Context<Self>) {
        self.unload_idle(UNLOAD_AFTER, cx);
        let statuses: HashMap<ProjectId, WorkspaceStatus> = self
            .projects
            .iter()
            .map(|project| (project.id, WorkspaceStatus::of(project.id, cx)))
            .collect();
        // The store, for chats of spaces without a screen (changed by
        // schedules or another window): every few seconds, not each one.
        self.status_ticks = self.status_ticks.wrapping_add(1);
        if self.status_ticks.is_multiple_of(5) {
            self.poll_sessions(cx);
            self.want_facts(cx);
        }
        // The sidebar lists agent CLIs in terminals: redrawn when they change.
        let agents = {
            use std::hash::{Hash as _, Hasher as _};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for agent in crate::terminal_agents::TerminalAgents::all(cx) {
                (&agent.pane, agent.state as u8, &agent.title, &agent.line).hash(&mut hasher);
            }
            hasher.finish()
        };
        let agents_changed = std::mem::replace(&mut self.terminal_agents_seen, agents) != agents;
        // Chats saved meanwhile away from their screen (terminal agents',
        // quick chats): every screen when too many changed to tell which.
        let saved = crate::session_changes::since(&mut self.session_changes_seen, cx);
        let screens: Vec<_> = match &saved {
            Some(projects) => projects
                .iter()
                .filter_map(|project| self.spaces.get(project).cloned())
                .collect(),
            None => self.spaces.values().cloned().collect(),
        };
        for space in screens {
            space.update(cx, |space, cx| space.reload_sessions(cx));
        }
        let any = saved.is_none_or(|projects| !projects.is_empty());
        let sessions_changed = any && self.read_sessions(cx);
        if statuses != self.statuses || sessions_changed || agents_changed {
            self.statuses = statuses;
            cx.notify();
        }
    }

    /// Gives workspace `id` colour `color` (`None`: the one from its id).
    pub fn set_color(&mut self, id: ProjectId, color: Option<u8>, cx: &mut Context<Self>) {
        if let Err(err) = AppState::store(cx).set_project_color(id, color) {
            tracing::warn!("saving the workspace colour failed: {err:#}");
            return;
        }
        if let Some(project) = self.projects.iter_mut().find(|project| project.id == id) {
            project.color = color;
        }
        if let Some(space) = self.spaces.get(&id) {
            space.update(cx, |space, cx| space.set_color(color, cx));
        }
        self.projects_changed(cx);
        cx.notify();
    }

    /// The space of workspace `id`, made (but not shown) if it has none yet.
    pub(crate) fn ensure_space(
        &mut self,
        id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<WorkspaceScreen>> {
        if let Some(space) = self.spaces.get(&id) {
            return Some(space.clone());
        }
        let project = self
            .projects
            .iter()
            .find(|project| project.id == id)
            .cloned()?;
        let shell = cx.weak_entity();
        let space = cx.new(|cx| WorkspaceScreen::new(project, shell, window, cx));
        let subscription = cx.subscribe_in(
            &space,
            window,
            move |this, _, event: &WorkspaceScreenEvent, window, cx| match event {
                WorkspaceScreenEvent::SessionsChanged => cx.notify(),
                WorkspaceScreenEvent::Agent { hosting, request } => {
                    let (hosting, request) = ((**hosting).clone(), request.clone());
                    if matches!(
                        request,
                        ph_pulls::AgentRequest::Fix { .. } | ph_pulls::AgentRequest::Address { .. }
                    ) {
                        this.fix_pull(hosting, request, window, cx);
                        return;
                    }
                    // Reviews run in the GitHub view, which the brand can
                    // leave out.
                    if !ph_brand::enabled(ph_brand::Feature::GitHub) {
                        return;
                    }
                    this.set_view(View::GitHub, window, cx);
                    let github = this.github(cx);
                    github.update(cx, |github, cx| github.agent(hosting, request, window, cx));
                }
                WorkspaceScreenEvent::OpenFindings(_) | WorkspaceScreenEvent::ReviewAgain(_)
                    if !ph_brand::enabled(ph_brand::Feature::GitHub) => {}
                WorkspaceScreenEvent::OpenFindings(review_id) => {
                    this.set_view(View::GitHub, window, cx);
                    let github = this.github(cx);
                    github.update(cx, |github, cx| {
                        github.open_findings(*review_id, window, cx)
                    });
                }
                WorkspaceScreenEvent::ReviewAgain(review_id) => {
                    this.set_view(View::GitHub, window, cx);
                    let github = this.github(cx);
                    github.update(cx, |github, cx| github.review_again(*review_id, window, cx));
                }
                WorkspaceScreenEvent::ShowSession(session) => {
                    this.show_session(*session, window, cx)
                }
                WorkspaceScreenEvent::MakePrimary(path) => {
                    this.make_primary_folder(id, path, window, cx)
                }
                WorkspaceScreenEvent::RemoveFolder(path) => {
                    this.remove_folder_path(id, path, window, cx)
                }
            },
        );
        self._subscriptions.push(subscription);
        self.spaces.insert(id, space.clone());
        Some(space)
    }

    /// Reads the inbox and to-dos again (after an agent added one).
    pub fn refresh_inbox(&self, cx: &mut Context<Self>) {
        self.inbox.update(cx, |inbox, cx| inbox.refresh(cx));
    }

    /// Makes `id` the active project, creating its space on first open. A
    /// workspace shown in another window is shown there instead.
    pub fn open_project(&mut self, id: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((handle, other)) = crate::windows::shown_elsewhere(id, self.handle, cx) {
            // Deferred: that window may be this one's caller.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    window.activate_window();
                    other.update(cx, |other, cx| other.open_project(id, window, cx));
                });
            });
            return;
        }
        if self.ensure_space(id, window, cx).is_none() {
            return;
        }
        self.view = View::Workspaces;
        crate::windows::set_shown_in(id, self.handle, cx);
        // Only the shown project streams browser and device frames.
        if let Some(previous) = self.active_project.filter(|previous| *previous != id)
            && let Some(space) = self.spaces.get(&previous)
        {
            space.update(cx, |space, cx| space.set_visible(false, cx));
            self.hidden_since
                .insert(previous, std::time::Instant::now());
        }
        self.hidden_since.remove(&id);
        if let Some(space) = self.spaces.get(&id) {
            space.update(cx, |space, cx| space.set_visible(true, cx));
        }
        if self.active_project != Some(id) {
            self.previous_project = self.active_project;
        }
        self.active_project = Some(id);
        let store = AppState::store(cx);
        let _ = store.touch_project(id);
        let _ = store.set_setting(keys::LAST_PROJECT, &Some(id));
        let _ = store.set_setting(keys::FRONT_VIEW, &front_key(View::Workspaces));
        cx.notify();
    }

    /// Adds the folder at `path` and opens it.
    /// Opens folder `path` as a workspace (adding it the first time); its
    /// id, unless it could not be added.
    pub fn add_project_path(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ProjectId> {
        match AppState::store(cx).add_project(path) {
            Ok(project) => {
                if !self
                    .projects
                    .iter()
                    .any(|existing| existing.id == project.id)
                {
                    self.projects.insert(0, project.clone());
                    self.projects_changed(cx);
                }
                self.open_project(project.id, window, cx);
                Some(project.id)
            }
            Err(err) => {
                window.push_notification(
                    Notification::error(format!("Could not open the folder: {err:#}")),
                    cx,
                );
                None
            }
        }
    }

    /// Opens a workspace and the file at `path` (absolute, in one of its
    /// folders) in its Files tab.
    pub fn open_project_file(
        &mut self,
        id: ProjectId,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_project(id, window, cx);
        if let Some(space) = self.spaces.get(&id) {
            space.update(cx, |space, cx| {
                if let Some(relative) = space.roots().path_of(path) {
                    space.open_file(&relative, None, window, cx);
                }
            });
        }
    }

    /// Opens the chat a search result is in, at its message.
    pub fn show_message(
        &mut self,
        pick: crate::chat_search::Pick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_project(pick.project_id) {
            // A GitHub repository's chat.
            return self.show_session(pick.session_id, window, cx);
        }
        self.open_project(pick.project_id, window, cx);
        if let Some(space) = self.spaces.get(&pick.project_id) {
            space.update(cx, |space, cx| {
                space.show_message(pick.session_id, pick.message_id, cx)
            });
        }
    }

    /// A new workspace without folders, opened.
    fn new_workspace(&mut self, _: &NewWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        let name = format!("Workspace {}", self.projects.len() + 1);
        match AppState::store(cx).create_empty_workspace(&name, &ph_store::paths::scratch_root()) {
            Ok(project) => {
                self.projects.insert(0, project.clone());
                self.projects_changed(cx);
                self.open_project(project.id, window, cx);
            }
            Err(err) => window.push_notification(
                Notification::error(format!("Could not make the workspace: {err:#}")),
                cx,
            ),
        }
    }

    /// Picks folders and adds them to the workspace in front (from the
    /// Chats space, which has none, a folder opens as a workspace).
    fn add_folder(
        &mut self,
        _: &AddFolderToWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // From the workspace for chats that need no folder, a folder
        // opens as a workspace of its own: that one stays without.
        let Some(id) = self.active_project.filter(|id| !self.is_scratch(*id, cx)) else {
            return window.dispatch_action(Box::new(AddProject), cx);
        };
        self.bring_forward(window, cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add to workspace".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            _ = this.update_in(cx, |this, window, cx| {
                for path in paths {
                    this.add_folder_path(id, &path, window, cx);
                }
            });
        })
        .detach();
    }

    /// Adds `path` to workspace `id`, updating its space in place.
    pub fn add_folder_path(
        &mut self,
        id: ProjectId,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match AppState::store(cx).add_workspace_folder(id, path) {
            Ok(project) => self.workspace_changed(project, window, cx),
            Err(err) => window.push_notification(
                Notification::error(format!("Could not add the folder: {err:#}")),
                cx,
            ),
        }
    }

    /// Takes `path` out of workspace `id` (the folder on disk is untouched).
    pub fn remove_folder_path(
        &mut self,
        id: ProjectId,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match AppState::store(cx).remove_workspace_folder(
            id,
            path,
            &ph_store::paths::scratch_root(),
        ) {
            Ok(project) => self.workspace_changed(project, window, cx),
            Err(err) => window.push_notification(
                Notification::error(format!("Could not remove the folder: {err:#}")),
                cx,
            ),
        }
    }

    /// Makes `path` the workspace's primary folder (where agents start).
    pub fn make_primary_folder(
        &mut self,
        id: ProjectId,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match AppState::store(cx).make_primary_folder(id, path) {
            Ok(project) => self.workspace_changed(project, window, cx),
            Err(err) => window.push_notification(Notification::warning(format!("{err:#}")), cx),
        }
    }

    /// Shows a workspace's new folders (or name) everywhere.
    fn workspace_changed(&mut self, project: Project, window: &mut Window, cx: &mut Context<Self>) {
        crate::attention::forget_names(cx);
        if let Some(existing) = self
            .projects
            .iter_mut()
            .find(|existing| existing.id == project.id)
        {
            *existing = project.clone();
        }
        if let Some(space) = self.spaces.get(&project.id) {
            space.update(cx, |space, cx| space.set_project(project, window, cx));
        }
        self.projects_changed(cx);
        cx.notify();
    }

    fn rename_workspace(
        &mut self,
        _: &RenameWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bring_forward(window, cx);
        let Some(project) = self
            .active_space()
            .map(|space| space.read(cx).project().clone())
            .filter(|project| project.is_workspace())
        else {
            return;
        };
        let workspace = cx.entity().downgrade();
        crate::rename_dialog::open(
            "Rename workspace",
            project.name.clone(),
            window,
            cx,
            move |name, window, cx| {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                if let Err(err) = AppState::store(cx).rename_project(project.id, &name) {
                    tracing::warn!("renaming the workspace failed: {err:#}");
                    return;
                }
                let renamed = Project {
                    name,
                    ..project.clone()
                };
                _ = workspace.update(cx, |workspace, cx| {
                    workspace.workspace_changed(renamed, window, cx)
                });
            },
        );
    }

    /// Removes a project from PilotHouse (the folder on disk is untouched;
    /// its chats go with it).
    pub fn remove_project(&mut self, id: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_project(id) {
            return;
        }
        // Its chats would have nowhere to be found.
        if self.is_scratch(id, cx) {
            window.push_notification(
                Notification::info(
                    "Chats holds the chats that need no folder, so it stays. \
                     Delete its chats one by one from their menus.",
                ),
                cx,
            );
            return;
        }
        if let Err(err) = AppState::store(cx).remove_project(id) {
            window.push_notification(
                Notification::error(format!("Could not remove the workspace: {err:#}")),
                cx,
            );
            return;
        }
        self.projects.retain(|project| project.id != id);
        self.spaces.remove(&id);
        self.hidden_since.remove(&id);
        if self.bridge.filter == Some(id) {
            self.bridge.filter = None;
        }
        // Its agents and shells stop.
        ph_agent::ChatHub::remove(id, cx);
        ph_terminal::TerminalHub::remove(id, cx);
        ph_uitest::runner::TestRunHub::remove(id, cx);
        ph_browser::BrowserHub::remove(id, cx);
        self.projects_changed(cx);
        crate::schedules::Automation::global(cx).update(cx, |automation, _| automation.forget(id));
        if self.active_project == Some(id) {
            self.active_project = None;
            // Back to the Bridge, where the others are: no other workspace
            // is the obvious one to show instead. One is ready behind it
            // all the same (the one shown before, else the Chats space), so
            // a new chat and the other commands have somewhere to go.
            let behind = self
                .previous_project
                .filter(|previous| *previous != id && self.has_project(*previous))
                .or_else(|| {
                    self.projects
                        .iter()
                        .map(|project| project.id)
                        .find(|other| *other != id)
                });
            if let Some(behind) = behind {
                self.open_project(behind, window, cx);
            }
            self.set_view(View::Bridge, window, cx);
        }
        cx.notify();
    }

    /// Runs `update` on the workspace last in front, bringing its screen
    /// forward first: what a command does there is not done unseen behind
    /// the Bridge or Reviews.
    /// Updates `agent`'s CLI with its own updater, in a terminal of the
    /// workspace in front (shown, so it is seen to run and to fail), and
    /// looks again a while after for whether it is still behind.
    pub(crate) fn update_agent_cli(
        &mut self,
        agent: &'static ph_agent::agents::AgentSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = crate::agent_updates::update_command(agent.id) else {
            return;
        };
        if self.active_space().is_none() {
            window.push_notification(
                Notification::info("Open a workspace first: the update runs in its terminal."),
                cx,
            );
            return;
        }
        self.with_active_space(window, cx, |space, window, cx| {
            space.run_in_terminal(&format!("Update {}", agent.name), command, window, cx)
        });
        // The updater does not say when it is done: its CLI is asked its
        // version until it is the newest, so the status bar follows.
        crate::agent_updates::watch(agent.id, cx);
    }

    fn with_active_space(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut WorkspaceScreen, &mut Window, &mut Context<WorkspaceScreen>),
    ) {
        let Some(space) = self.active_space().cloned() else {
            return;
        };
        self.bring_forward(window, cx);
        space.update(cx, |space, cx| update(space, window, cx));
    }

    /// Shows the workspace last in front where the Bridge or Reviews is: a
    /// command that acts on "this workspace" is not carried out on one
    /// nobody is looking at.
    fn bring_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view != View::Workspaces && self.active_project.is_some() {
            self.set_view(View::Workspaces, window, cx);
        }
    }

    // ---- Action handlers -------------------------------------------------

    fn add_project(&mut self, _: &AddProject, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open as workspace".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            _ = this.update_in(cx, |this, window, cx| {
                this.add_project_path(&path, window, cx)
            });
        })
        .detach();
    }

    pub(crate) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        command_palette::open(self, cx.entity(), window, cx);
    }

    fn new_session(&mut self, _: &NewSession, window: &mut Window, cx: &mut Context<Self>) {
        // An organisation's view has no chats: the new one is the
        // workspace's, so show it.
        if let View::Custom(_) = self.view {
            self.set_view(View::Workspaces, window, cx);
        }
        if self.view == View::GitHub {
            if let Some(github) = self.github.clone() {
                github.update(cx, |github, cx| github.new_chat(window, cx));
            }
            return;
        }
        // From the Bridge: in the workspace it is narrowed to, else the
        // one last in front (its *New task* asks which).
        if self.view == View::Bridge
            && let Some(id) = self.bridge.filter.filter(|id| self.has_project(*id))
        {
            return self.new_chat_in(id, window, cx);
        }
        self.with_active_space(window, cx, |space, window, cx| {
            space.new_session_asked(window, cx)
        });
    }

    /// Asks the new task's name, then makes its branch and worktree off the
    /// UI thread, opens it as a workspace and runs its setup.
    fn new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        let Some(space) = self.active_space().cloned() else {
            return;
        };
        self.bring_forward(window, cx);
        let (root, parent_name) = {
            let space = space.read(cx);
            let folders = space.project().folders();
            (
                folders.get(space.git_folder()).cloned(),
                space.project().name.clone(),
            )
        };
        self.ask_for_task(root, parent_name, window, cx);
    }

    /// Asks for a task of workspace `id` on a branch of its own (from the
    /// Bridge, where no workspace is in front): in its first folder that is
    /// a repository.
    pub(crate) fn new_task_in(
        &mut self,
        id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.iter().find(|project| project.id == id) else {
            return;
        };
        let root = project
            .folders()
            .into_iter()
            .find(|folder| ph_git::is_repository(folder));
        let name = project.name.clone();
        self.ask_for_task(root, name, window, cx);
    }

    /// Asks the task's name, then makes its branch and worktree from
    /// `root` and opens it as a workspace beside `parent_name`'s.
    fn ask_for_task(
        &mut self,
        root: Option<std::path::PathBuf>,
        parent_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = root.filter(|root| ph_git::is_repository(root)) else {
            window.push_notification(
                Notification::error("A task needs a workspace whose folder is a Git repository."),
                cx,
            );
            return;
        };
        let workspace = cx.entity().downgrade();
        crate::rename_dialog::ask(
            "New task",
            "Create",
            String::new(),
            window,
            cx,
            move |name, window, cx| {
                let (root, workspace, parent_name) =
                    (root.clone(), workspace.clone(), parent_name.clone());
                let worktrees = ph_store::paths::copies_dir().join("tasks");
                let made = {
                    let (root, name) = (root.clone(), name.clone());
                    cx.background_spawn(
                        async move { crate::tasks::create(&root, &name, &worktrees) },
                    )
                };
                window
                    .spawn(cx, async move |cx| {
                        let made = made.await;
                        _ = workspace.update_in(cx, |workspace, window, cx| match made {
                            Ok(made) => {
                                workspace.open_task(&root, &parent_name, &name, made, window, cx)
                            }
                            Err(err) => window.push_notification(
                                Notification::error(format!("Could not make the task: {err:#}")),
                                cx,
                            ),
                        });
                    })
                    .detach();
            },
        );
    }

    /// Fixes findings of a pull request, or addresses the comments on one
    /// (`request`), in a new task on its head: from a workspace's checkout
    /// of the repository when there is one, else the GitHub view's clone.
    /// The task's chat starts with the work.
    pub fn fix_pull(
        &mut self,
        hosting: ph_pulls::Hosting,
        request: ph_pulls::AgentRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = AppState::store(cx).clone();
        let (pull, findings) = match request {
            ph_pulls::AgentRequest::Fix { pull, findings } => (pull, Some(findings)),
            ph_pulls::AgentRequest::Address { pull } => (pull, None),
            _ => return,
        };
        // Which folder holds the repository is asked of Git, folder by
        // folder: in the background, as Git is slow to start on Windows.
        let folders: Vec<(PathBuf, String)> = self
            .projects
            .iter()
            .flat_map(|project| {
                project
                    .folders()
                    .into_iter()
                    .map(|folder| (folder, project.name.clone()))
            })
            .collect();
        let number = pull.number;
        let name = format!("PR {number} fix");
        let work = {
            let store = store.clone();
            cx.background_spawn(async move {
                let (root, parent_name) = anyhow::Context::context(pull_checkout(&hosting, folders),
                    "Open the repository in a workspace (or review the pull request first) to fix it in a task.",
                )?;
                let sha = ph_pulls::hosting::fetch_head(&root, &hosting, number)?;
                let worktrees = ph_store::paths::copies_dir().join("tasks");
                let made = crate::tasks::create_for_pull(&root, number, &sha, &worktrees)?;
                // Someone else's settings must not act on the agent.
                let theirs = !pull.user.login.eq_ignore_ascii_case(&hosting.account.login);
                let removed = if theirs {
                    ph_pulls::clones::sanitize_worktree(&made.path)
                } else {
                    Vec::new()
                };
                let fork = pull
                    .head
                    .label
                    .split_once(':')
                    .is_some_and(|(owner, _)| !owner.eq_ignore_ascii_case(&hosting.repo.owner));
                let push = if fork {
                    format!(
                        "Its branch is on {}'s fork: push there ({}) if they let maintainers edit.",
                        pull.head.label.split(':').next().unwrap_or_default(),
                        pull.head.name
                    )
                } else {
                    format!(
                        "Push with `git push {} HEAD:{}` when done.",
                        hosting.remote, pull.head.name
                    )
                };
                let (prompt, display) = match findings {
                    Some(findings) => {
                        let labels: Vec<&str> = findings.iter().map(|f| f.label.as_str()).collect();
                        (
                            ph_review::prompt::fix_prompt(&findings),
                            format!("Fix {} of #{number} “{}”. {push}", labels.join(", "), pull.title),
                        )
                    }
                    None => {
                        let comments = ph_review::github::comments_to_address(
                            &store,
                            &crate::review::pull_ref(&hosting, number),
                        )?;
                        if comments.is_empty() {
                            anyhow::bail!("No comments to address on #{number}.");
                        }
                        (
                            ph_review::prompt::address_prompt(number, &comments),
                            format!(
                                "Address the {} review comments on #{number} “{}”. {push}",
                                comments.len(),
                                pull.title
                            ),
                        )
                    }
                };
                let display = if removed.is_empty() {
                    display
                } else {
                    format!(
                        "{display} (The pull request's agent settings are taken out of this task so they cannot act on the agent: {}.)",
                        removed.join(", ")
                    )
                };
                anyhow::Ok((root, parent_name, made, prompt, display))
            })
        };
        let workspace = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = work.await;
                _ = workspace.update_in(cx, |workspace, window, cx| match result {
                    Ok((root, parent_name, made, prompt, display)) => {
                        let path = made.path.clone();
                        workspace.open_task(&root, &parent_name, &name, made, window, cx);
                        let space = workspace
                            .spaces
                            .values()
                            .find(|space| space.read(cx).project().path == path)
                            .cloned();
                        if let Some(space) = space {
                            space.update(cx, |space, cx| {
                                space.ask_chat(name.clone(), display, prompt, window, cx)
                            });
                        }
                    }
                    Err(err) => window.push_notification(
                        Notification::error(format!("Could not make the task: {err:#}")),
                        cx,
                    ),
                });
            })
            .detach();
    }

    /// Opens a task just made: a workspace on its worktree, named after it,
    /// with its setup running.
    fn open_task(
        &mut self,
        root: &std::path::Path,
        parent_name: &str,
        name: &str,
        made: crate::tasks::Created,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = AppState::store(cx).clone();
        // Only the task's own workspace is renamed and remembered: never the
        // one in front should the worktree not open.
        let Some(project) = self.add_project_path(&made.path, window, cx) else {
            return;
        };
        let title = format!("{parent_name} · {}", name.trim());
        if store.rename_project(project, &title).is_ok() {
            self.reload_projects(cx);
        }
        let port = crate::tasks::free_port(&store);
        let task = crate::tasks::Task {
            project,
            root: root.to_path_buf(),
            branch: made.branch,
            base: made.base,
            start: made.start,
            path: made.path,
            port,
            extras: made.extras,
        };
        if let Err(err) = crate::tasks::remember(&store, task) {
            tracing::warn!("saving the task failed: {err:#}");
        }
        let setup = crate::tasks::config(root)
            .ok()
            .and_then(|config| config.setup)
            .unwrap_or_default();
        let command = if setup.trim().is_empty() {
            // The number itself: every shell echoes that the same way.
            crate::tasks::setup_command(&format!("echo \"Port for this task: {port}\""), port)
        } else if ph_store::paths::is_untrusted_copy(root) {
            // Someone else's repository says what to run: it is typed as a
            // comment, to read first and then run by hand.
            let remark = if cfg!(windows)
                && ph_env::default_shell()
                    .0
                    .file_stem()
                    .is_some_and(|name| name.eq_ignore_ascii_case("cmd"))
            {
                "rem"
            } else {
                "#"
            };
            format!(
                "{remark} pilothouse.toml asks to run this; read it, then run it yourself: {}",
                setup.replace(['\n', '\r'], " ")
            )
        } else {
            crate::tasks::setup_command(&setup, port)
        };
        if let Some(space) = self.spaces.get(&project).cloned() {
            space.update(cx, |space, cx| {
                space.run_in_terminal("Setup", &command, window, cx)
            });
        }
        cx.notify();
    }

    /// Asks, then removes the task in front: its worktree (refused with
    /// changes not committed), its branch when merged, and its workspace.
    fn finish_task(&mut self, _: &FinishTask, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.active_project else {
            return;
        };
        self.bring_forward(window, cx);
        let store = AppState::store(cx).clone();
        let Some(task) = crate::tasks::of(&store, project) else {
            return;
        };
        let checked = {
            let task = task.clone();
            cx.background_spawn(async move { crate::tasks::is_merged(&task) })
        };
        let workspace = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let merged = checked.await;
                _ = workspace.update_in(cx, |_, window, cx| {
                    let description = format!(
                        "Removes the worktree at {} and this workspace. {}",
                        ph_ui::platform::display_path(&task.path),
                        if merged {
                            format!(
                                "{} is merged into {}, so it is deleted too.",
                                task.branch, task.base
                            )
                        } else {
                            format!("The branch {} stays.", task.branch)
                        }
                    );
                    let workspace = cx.entity().downgrade();
                    window.open_alert_dialog(cx, move |dialog, _, _| {
                        let (task, workspace, store) =
                            (task.clone(), workspace.clone(), store.clone());
                        dialog
                            .confirm()
                            .title("Finish the task?")
                            .description(description.clone())
                            .button_props(
                                gpui_kit::component::dialog::DialogButtonProps::default()
                                    .ok_text("Finish task"),
                            )
                            .on_ok(move |_, window, cx| {
                                let (task, workspace, store) =
                                    (task.clone(), workspace.clone(), store.clone());
                                let done = {
                                    let task = task.clone();
                                    cx.background_spawn(async move {
                                        crate::tasks::finish(&task, merged)
                                    })
                                };
                                window
                                    .spawn(cx, async move |cx| {
                                        let done = done.await;
                                        _ = workspace.update_in(cx, |workspace, window, cx| {
                                            if let Err(err) = &done
                                                && task.path.exists()
                                            {
                                                // The sentence for the user, without Git's own.
                                                window.push_notification(
                                                    Notification::error(err.to_string()),
                                                    cx,
                                                );
                                                return;
                                            }
                                            if let Err(err) = done {
                                                window.push_notification(err.to_string(), cx);
                                            }
                                            let _ = crate::tasks::forget(&store, task.project);
                                            workspace.remove_project(task.project, window, cx);
                                        });
                                    })
                                    .detach();
                                true
                            })
                    });
                });
            })
            .detach();
    }

    fn side_chat(&mut self, _: &SideChat, window: &mut Window, cx: &mut Context<Self>) {
        if self.view != View::Workspaces {
            return;
        }
        self.with_active_space(window, cx, |space, window, cx| space.side_chat(window, cx));
    }

    fn switch_to(
        &mut self,
        action: &SwitchToWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // By its place among the user's workspaces, as the column of
        // icons numbers them (Chats has a key of its own).
        let id = self.listed().nth(action.index).map(|project| project.id);
        if let Some(id) = id {
            self.open_project(id, window, cx);
        }
    }

    fn previous_workspace(
        &mut self,
        _: &PreviousWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // From the Bridge or Reviews, the one shown before is the one
        // still behind them.
        if self.view != View::Workspaces && self.active_project.is_some() {
            return self.bring_forward(window, cx);
        }
        let previous = self
            .previous_project
            .filter(|id| self.projects.iter().any(|project| project.id == *id));
        if let Some(id) = previous {
            self.open_project(id, window, cx);
        }
    }

    fn remove_workspace(
        &mut self,
        _: &RemoveWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.active_project else {
            return;
        };
        // Its chats go with it: said first, and asked, when it has any.
        let chats = self.sessions_of(id, cx).len();
        if chats == 0 || self.is_scratch(id, cx) {
            return self.remove_project(id, window, cx);
        }
        let name = self
            .projects
            .iter()
            .find(|project| project.id == id)
            .map(|project| project.name.clone())
            .unwrap_or_default();
        let workspace = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, _| {
            let workspace = workspace.clone();
            dialog
                .title(format!("Remove {name}?"))
                .w(ph_ui::scaled_px(460., window))
                .child(format!(
                    "Its {} deleted with it. The folders on disk are not touched.",
                    if chats == 1 {
                        "chat is".to_string()
                    } else {
                        format!("{chats} chats are")
                    }
                ))
                .footer(ph_ui::dialog_buttons("Remove", true))
                .on_ok(move |_, window, cx| {
                    _ = workspace
                        .update(cx, |workspace, cx| workspace.remove_project(id, window, cx));
                    true
                })
        });
    }

    /// ⌘B: folds what is at the left of the view in front away, or brings
    /// it back: a workspace's chat, Reviews' list. The key was the
    /// sidebar's, and still makes room the same way.
    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        match self.view {
            View::Workspaces => {
                self.with_active_space(window, cx, |space, _, cx| space.toggle_chat(cx));
                // The composer is gone with the chat, or back with it.
                self.focus_front(window, cx);
            }
            View::GitHub => self.set_sidebar_collapsed(!self.sidebar_collapsed, cx),
            View::Bridge | View::Custom(_) => {}
        }
    }

    fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        let _ = AppState::store(cx).set_setting(keys::SIDEBAR_COLLAPSED, &collapsed);
        if let Some(github) = &self.github {
            github.update(cx, |github, cx| github.set_sidebar_shown(!collapsed, cx));
        }
        cx.notify();
    }

    fn toggle_context_panel(
        &mut self,
        _: &ToggleContextPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_active_space(window, cx, |space, window, cx| {
            space.toggle_context_panel(window, cx)
        });
    }

    fn toggle_zoom(&mut self, _: &ToggleZoom, window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_space(window, cx, |space, window, cx| {
            space.toggle_zoom(window, cx)
        });
        self.focus_front(window, cx);
    }

    fn reset_layout(&mut self, _: &ResetLayout, window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_space(window, cx, |space, window, cx| {
            space.reset_layout(window, cx)
        });
    }

    fn show_tab(&mut self, tab: ContextTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.view != View::Workspaces {
            self.set_view(View::Workspaces, window, cx);
        }
        self.with_active_space(window, cx, |space, window, cx| {
            space.show_tab(tab, window, cx)
        });
    }

    fn quick_open(&mut self, _: &QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_space(window, cx, |space, window, cx| space.quick_open(window, cx));
    }

    fn go_to_symbol(&mut self, _: &GoToSymbol, window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_space(window, cx, |space, window, cx| {
            space.go_to_symbol(window, cx)
        });
    }

    fn find_in_project(&mut self, _: &FindInProject, window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_space(window, cx, |space, window, cx| {
            space.find_in_project(window, cx)
        });
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .active_space()
            .map(|space| space.read(cx).project().path.clone());
        settings_dialog::open(
            project,
            settings_dialog::Page::First,
            cx.entity().downgrade(),
            window,
            cx,
        );
    }

    fn open_storage_settings(
        &mut self,
        _: &OpenStorageSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self
            .active_space()
            .map(|space| space.read(cx).project().path.clone());
        settings_dialog::open(
            project,
            settings_dialog::Page::Storage,
            cx.entity().downgrade(),
            window,
            cx,
        );
    }

    fn open_chrome_extension_settings(
        &mut self,
        _: &ph_browser::OpenChromeExtensionSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !ph_brand::enabled(ph_brand::Feature::ChromeExtension) {
            return;
        }
        let project = self
            .active_space()
            .map(|space| space.read(cx).project().path.clone());
        settings_dialog::open(
            project,
            settings_dialog::Page::Chrome,
            cx.entity().downgrade(),
            window,
            cx,
        );
    }

    fn open_github_settings(
        &mut self,
        _: &OpenGitHubSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self
            .active_space()
            .map(|space| space.read(cx).project().path.clone());
        settings_dialog::open(
            project,
            settings_dialog::Page::GitHub,
            cx.entity().downgrade(),
            window,
            cx,
        );
    }

    /// A new chat, its message begun, to design a theme with the agent:
    /// each change shows at once (the `theme_*` tools).
    pub(crate) fn design_theme(
        &mut self,
        _: &DesignTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        if self.view != View::Workspaces {
            self.set_view(View::Workspaces, window, cx);
        }
        let from = ph_theme::themes::current(cx).name.clone();
        let prompt = format!(
            "Let's design a {app} theme together, starting from {from}. Show each change live with the theme tools, and save it when I say so. I'd like ",
            app = ph_brand::NAME,
        );
        self.with_active_space(window, cx, move |space, window, cx| {
            space.new_session(window, cx);
            space.begin_message(&prompt, window, cx);
        });
    }
}

/// The light (or dark) version of the theme in use: its partner, else
/// Modern Light (or Modern Dark).
fn switch_mode(dark: bool, cx: &mut gpui_kit::App) {
    let current = ph_theme::themes::current(cx);
    if current.mode.is_dark() == dark && AppState::appearance(cx) != Appearance::System {
        return;
    }
    let name = if current.mode.is_dark() == dark {
        current.name.to_string()
    } else {
        ph_theme::themes::partner(&current.name, current.mode.is_dark()).to_string()
    };
    AppState::set_appearance(Appearance::Theme(name), cx);
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._devices.is_none()
            && let Some(devices) = ph_devices::DeviceManager::existing(cx)
        {
            self._devices = Some(cx.observe(&devices, |_, _, cx| cx.notify()));
        }
        let github = (self.view == View::GitHub)
            .then(|| self.github.clone())
            .flatten();
        // An organisation's view is an island of the frame, as wide as the
        // window allows (it has no list beside it).
        let custom = match self.view {
            View::Custom(id) => self.custom_views.get(id).cloned(),
            _ => None,
        };
        let content = match (&github, self.screen_in_front()) {
            _ if custom.is_some() => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .pr_2()
                .pb(crate::frame::gap())
                .child(crate::frame::island(window, cx).children(custom.clone()))
                .into_any_element(),
            (Some(github), _) => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(github.clone())
                .into_any_element(),
            (None, Some(space)) => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(space.clone())
                .into_any_element(),
            // The Bridge, also when there is no workspace to show.
            (None, None) => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(crate::bridge::render(self, window, cx))
                .into_any_element(),
        };

        v_flex()
            .id("workspace")
            .key_context(WORKSPACE_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            // The frame's colour; the islands sit on it.
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::add_project))
            .on_action(cx.listener(Self::new_workspace))
            .on_action(cx.listener(Self::add_folder))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::new_session))
            .on_action(cx.listener(Self::side_chat))
            .on_action(cx.listener(Self::new_task))
            .on_action(cx.listener(Self::finish_task))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_context_panel))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                this.with_active_space(window, cx, |space, window, cx| {
                    space.toggle_terminal(window, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &ShowLogcat, window, cx| {
                this.show_tab(ContextTab::Logcat, window, cx)
            }))
            .on_action(cx.listener(Self::toggle_zoom))
            .on_action(cx.listener(Self::reset_layout))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::go_to_symbol))
            .on_action(cx.listener(Self::find_in_project))
            .on_action(cx.listener(|this, _: &ShowFiles, window, cx| {
                this.show_tab(ContextTab::Files, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowWorkspacesView, window, cx| {
                this.set_view(View::Bridge, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ShowChatsView, window, cx| this.show_scratch(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowGitHubView, window, cx| {
                this.set_view(View::GitHub, window, cx)
            }))
            .on_action(cx.listener(|this, show: &ph_custom::ShowView, window, cx| {
                if let Some(custom) = ph_custom::views()
                    .into_iter()
                    .find(|custom| custom.id == show.id)
                {
                    this.set_view(View::Custom(custom.id), window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ShowChanges, window, cx| {
                this.show_tab(ContextTab::Changes, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowTerminal, window, cx| {
                this.show_tab(ContextTab::Terminal, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowBrowser, window, cx| {
                this.show_tab(ContextTab::Browser, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowDevices, window, cx| {
                this.show_tab(ContextTab::Devices, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowRuns, window, cx| {
                this.show_tab(ContextTab::Runs, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowPulls, window, cx| {
                this.show_tab(ContextTab::Pulls, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowTests, window, cx| {
                this.show_tab(ContextTab::Tests, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CaptureToChat, window, cx| {
                this.bring_forward(window, cx);
                if let Some(space) = this.active_space() {
                    space.update(cx, |space, cx| space.capture_to_chat(window, cx));
                }
            }))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_github_settings))
            .on_action(cx.listener(Self::open_storage_settings))
            .on_action(cx.listener(Self::open_chrome_extension_settings))
            .on_action(cx.listener(|_, _: &UseSystemAppearance, _, cx| {
                AppState::set_appearance(Appearance::System, cx)
            }))
            .on_action(cx.listener(|_, _: &UseLightAppearance, _, cx| switch_mode(false, cx)))
            .on_action(cx.listener(Self::design_theme))
            .on_action(
                cx.listener(|_, _: &ShowUsage, window, cx| crate::usage_view::open(window, cx)),
            )
            .on_action(cx.listener(|_, _: &NewPresentation, window, cx| {
                crate::presentations::new_dialog(window, cx)
            }))
            .on_action(cx.listener(|_, _: &OpenPresentation, window, cx| {
                crate::presentations::open_dialog(window, cx)
            }))
            .on_action(
                cx.listener(|_, _: &ShowShortcuts, window, cx| crate::shortcuts::open(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowNetworkDrives, window, cx| {
                let drive = this
                    .active_space()
                    .and_then(|space| space.read(cx).network_drive().cloned());
                crate::network_drives::open(drive, window, cx)
            }))
            .on_action(cx.listener(|_, _: &ShowUtilities, window, cx| {
                crate::utilities::open(None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowSchedules, window, cx| {
                if let Some(id) = this.active_project_id() {
                    crate::schedules::view::open(cx.entity().downgrade(), id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowArchitecture, window, cx| {
                this.bring_forward(window, cx);
                if let Some(space) = this.active_space() {
                    let root = space.read(cx).project().path.clone();
                    crate::arch_map::open(space.downgrade(), root, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowParallelRuns, window, cx| {
                this.bring_forward(window, cx);
                if let Some(space) = this.active_space() {
                    let (project, root) = {
                        let project = space.read(cx).project();
                        (project.id, project.path.clone())
                    };
                    crate::parallel::open_runs(space.downgrade(), project, root, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowInstructions, window, cx| {
                this.bring_forward(window, cx);
                if let Some(space) = this.active_space() {
                    let root = space.read(cx).project().path.clone();
                    crate::instructions_view::open(root, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SearchChats, window, cx| {
                // Beside a workspace the search starts with its chats (one
                // click widens it); from the Bridge, Reviews or the Chats
                // space it is of every chat.
                let workspace = (this.view == View::Workspaces)
                    .then(|| this.active_project_id())
                    .flatten();
                crate::chat_search::open(workspace, window, cx)
            }))
            .on_action(cx.listener(Self::switch_to))
            .on_action(cx.listener(Self::previous_workspace))
            .on_action(cx.listener(Self::remove_workspace))
            .on_action(cx.listener(|this, _: &OpenInNewWindow, window, cx| {
                this.bring_forward(window, cx);
                if let Some(id) = this.active_project {
                    this.open_in_new_window(id, window, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &UseDarkAppearance, _, cx| switch_mode(true, cx)))
            // A gradient or picture behind the frame, when the user chose
            // one, then the frame's own light: first, so everything is
            // drawn over them.
            .children(crate::frame::layers(self, cx))
            .child({
                let bar = TitleBar::new()
                    .h(ph_theme::layout::TITLE_BAR_HEIGHT)
                    // Clear, with no line under it: the bar, the columns of
                    // icons and the status bar are one frame, painted once
                    // behind them all (`frame::layers`).
                    .bg(gpui_kit::transparent_black())
                    .border_b_0();
                // On macOS the bar starts after the window's buttons. On
                // Windows and Linux those are on the right, and the bar
                // would start a fixed 12 px in: the app's mark then stood
                // right of the line the icons under it are on. It starts at
                // the window's edge there, and the mark is centred over the
                // column (`title_bar::app_icon`).
                let bar = if cfg!(target_os = "macos") {
                    bar
                } else {
                    bar.pl_0()
                };
                bar.child(title_bar::render(self, window, cx))
            })
            .child(
                h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    // Dragging the edge of Reviews' list resizes it.
                    .on_drag_move(cx.listener(
                        |_, event: &gpui_kit::DragMoveEvent<sidebar::SidebarEdge>, window, cx| {
                            // The list starts after the column of icons.
                            let rail = ph_ui::scaled_px(
                                crate::rail::width_px(crate::rail::labels(cx)),
                                window,
                            );
                            sidebar::drag_width(
                                event.event.position.x - event.bounds.left() - rail,
                                cx,
                            );
                        },
                    ))
                    .child(crate::rail::render_places(self, cx))
                    .child(content),
            )
            .child(status_bar::render(self, cx))
    }
}

/// How the view in front is remembered between runs: the Bridge, or a
/// workspace (which one is `LAST_PROJECT`). Reviews and an organisation's
/// views are not come back to: they are places to visit.
fn front_key(view: View) -> &'static str {
    match view {
        View::Workspaces => "workspace",
        _ => "bridge",
    }
}

/// Where to make a task for `hosting`'s pull requests: the first of
/// `folders` (with its workspace's name) whose Git remote is the
/// repository, else the GitHub view's clone of it. Runs Git: off the UI
/// thread.
fn pull_checkout(
    hosting: &ph_pulls::Hosting,
    folders: Vec<(PathBuf, String)>,
) -> Option<(PathBuf, String)> {
    let wanted = (
        hosting.account.host.to_ascii_lowercase(),
        hosting.repo.owner.to_ascii_lowercase(),
        hosting.repo.name.to_ascii_lowercase(),
    );
    let local = folders.into_iter().find(|(folder, _)| {
        ph_git::remote::hosted_repo(folder).is_some_and(|hosted| {
            (
                hosted.host.to_ascii_lowercase(),
                hosted.owner.to_ascii_lowercase(),
                hosted.name.to_ascii_lowercase(),
            ) == wanted
        })
    });
    let clone = ph_pulls::clones::RepoId::of(hosting).clone_dir();
    local.or_else(|| {
        clone
            .join(".git")
            .exists()
            .then(|| (clone, hosting.repo.name.clone()))
    })
}

/// The workspaces in the store, newest first: an order that stays put as
/// they are used (the sidebar's, and ⌃1–⌃9's).
fn workspaces(store: &ph_store::Store) -> Vec<Project> {
    let mut projects: Vec<Project> = store
        .projects()
        .unwrap_or_else(|err| {
            tracing::error!("loading workspaces failed: {err:#}");
            Vec::new()
        })
        .into_iter()
        .filter(Project::is_workspace)
        .collect();
    projects.sort_by_key(|project| std::cmp::Reverse(project.id));
    projects
}
