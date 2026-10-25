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

use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, TitleBar, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, Styled as _, Subscription, Window, div,
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
use ph_files::{FindInProject, QuickOpen};

/// A hidden workspace's screen is closed after this long with nothing going on.
const UNLOAD_AFTER: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Which view a window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Workspaces,
    GitHub,
}

/// Owns the project list and the open [`WorkspaceScreen`]s, and routes workspace
/// actions to whichever project is active.
pub struct Workspace {
    focus_handle: FocusHandle,
    /// This window.
    handle: gpui_kit::AnyWindowHandle,
    /// The workspaces, newest first (the sidebar's order, and ⌃1–⌃9's).
    projects: Vec<Project>,
    /// The space for chats outside any workspace.
    chats_space: Option<Project>,
    view: View,
    /// The GitHub view, made when first shown.
    github: Option<Entity<GithubView>>,
    /// The sidebar: workspaces unfolded, those listing every chat, and those
    /// listing the chats PilotHouse started (*Runs*).
    pub(crate) expanded: HashSet<ProjectId>,
    pub(crate) show_all: HashSet<ProjectId>,
    pub(crate) runs_open: HashSet<ProjectId>,
    /// The chats of unfolded workspaces whose screen is closed, read again
    /// each second (a schedule may add one).
    sessions: HashMap<ProjectId, Vec<ph_store::sessions::Session>>,
    /// Spaces stay alive after switching away so their state survives.
    spaces: HashMap<ProjectId, Entity<WorkspaceScreen>>,
    active_project: Option<ProjectId>,
    /// The workspace shown before the one in front (Ctrl+Tab goes back).
    previous_project: Option<ProjectId>,
    sidebar_collapsed: bool,
    /// What each workspace is doing, read again every second.
    statuses: HashMap<ProjectId, WorkspaceStatus>,
    /// When each loaded but hidden workspace was last shown.
    hidden_since: HashMap<ProjectId, std::time::Instant>,
    _status_poll: gpui_kit::Task<()>,
    _subscriptions: Vec<Subscription>,
    /// The title bar's inbox and to-dos.
    pub(crate) inbox: Entity<crate::inbox::InboxView>,
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
        let projects = workspaces(store);
        let chats_space = store
            .chats_space(&ph_store::paths::scratch_root())
            .inspect_err(|err| tracing::error!("opening the Chats space failed: {err:#}"))
            .ok();
        let last_project: Option<ProjectId> = store.setting(keys::LAST_PROJECT);
        let sidebar_collapsed: bool = store.setting(keys::SIDEBAR_COLLAPSED);
        let expanded: Vec<ProjectId> = store.setting(keys::SIDEBAR_EXPANDED);

        // Follow OS light/dark changes while the preference is "System".
        let appearance_subscription = cx.observe_window_appearance(window, |_, window, cx| {
            if AppState::appearance(cx) == Appearance::System {
                ph_theme::apply_appearance(Appearance::System, Some(window), cx);
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
            chats_space,
            view: View::Workspaces,
            github: None,
            expanded: expanded.into_iter().collect(),
            show_all: HashSet::new(),
            runs_open: HashSet::new(),
            sessions: HashMap::new(),
            spaces: HashMap::new(),
            active_project: None,
            previous_project: None,
            sidebar_collapsed,
            statuses: HashMap::new(),
            hidden_since: HashMap::new(),
            // Status comes from chats and test runs outside this window; a
            // cheap look each second catches every change.
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
            inbox,
        };
        // Copies made for agents that are no longer needed go, quietly.
        let tidy_store = AppState::store(cx).clone();
        cx.background_spawn(async move {
            let removed = crate::housekeeping::tidy(&tidy_store);
            if removed > 0 {
                tracing::info!("removed {removed} unused working copies");
            }
        })
        .detach();
        // The workspace shown last, else the Chats space, so a first start
        // opens straight into a chat.
        let initial = project
            .or(last_project)
            .filter(|id| workspace.has_project(*id) || workspace.is_chats(*id))
            .or(workspace.chats_space.as_ref().map(|chats| chats.id))
            .or_else(|| workspace.projects.first().map(|project| project.id));
        if let Some(id) = initial {
            workspace.open_project(id, window, cx);
        }
        workspace
    }

    /// The workspaces (not the Chats space, nor GitHub repositories').
    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    /// The space for chats outside any workspace.
    pub fn chats_space(&self) -> Option<&Project> {
        self.chats_space.as_ref()
    }

    /// Whether `id` is the Chats space.
    pub fn is_chats(&self, id: ProjectId) -> bool {
        self.chats_space
            .as_ref()
            .is_some_and(|chats| chats.id == id)
    }

    pub fn view(&self) -> View {
        self.view
    }

    /// Shows the Workspaces or the GitHub view.
    pub fn set_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
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
        self.open_project(project, window, cx);
        if let Some(space) = self.spaces.get(&project) {
            space.update(cx, |space, cx| space.select_session(session, cx));
        }
    }

    /// A new chat in workspace (or Chats space) `project`.
    pub fn new_chat_in(&mut self, project: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        self.open_project(project, window, cx);
        if let Some(space) = self.spaces.get(&project) {
            space.update(cx, |space, cx| space.new_session(window, cx));
        }
    }

    /// The chats the sidebar lists for `id`: its screen's, else as last read.
    pub(crate) fn sessions_of(
        &self,
        id: ProjectId,
        cx: &gpui_kit::App,
    ) -> Vec<ph_store::sessions::Session> {
        match self.spaces.get(&id) {
            Some(space) => space.read(cx).sessions().to_vec(),
            None => self.sessions.get(&id).cloned().unwrap_or_default(),
        }
    }

    /// Folds or unfolds workspace `id` in the sidebar.
    pub(crate) fn toggle_expanded(&mut self, id: ProjectId, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
            self.read_sessions(cx);
        }
        self.save_expanded(cx);
        cx.notify();
    }

    fn save_expanded(&self, cx: &mut Context<Self>) {
        let mut ids: Vec<ProjectId> = self.expanded.iter().copied().collect();
        ids.sort_unstable();
        let _ = AppState::store(cx).set_setting(keys::SIDEBAR_EXPANDED, &ids);
    }

    /// Reads the chats of unfolded workspaces without a screen.
    fn read_sessions(&mut self, cx: &mut Context<Self>) -> bool {
        let store = AppState::store(cx).clone();
        let mut changed = false;
        let wanted: Vec<ProjectId> = self
            .expanded
            .iter()
            .copied()
            .filter(|id| !self.spaces.contains_key(id))
            .collect();
        self.sessions.retain(|id, _| wanted.contains(id));
        for id in wanted {
            let sessions = store.sessions(id).unwrap_or_default();
            if self.sessions.get(&id) != Some(&sessions) {
                self.sessions.insert(id, sessions);
                changed = true;
            }
        }
        changed
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
        if self.view == View::GitHub {
            let focused = self.github.clone().is_some_and(|github| {
                github.update(cx, |github, cx| github.focus_chat(window, cx))
            });
            if !focused {
                self.focus_handle.focus(window, cx);
            }
            return;
        }
        match self.active_space().cloned() {
            Some(space) => space.update(cx, |space, cx| space.focus_chat(window, cx)),
            None => self.focus_handle.focus(window, cx),
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
        // The Chats space stays in the main window.
        if self.is_chats(id) {
            return;
        }
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
                .or(self.chats_space.as_ref().map(|chats| chats.id))
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

    fn has_project(&self, id: ProjectId) -> bool {
        self.projects.iter().any(|project| project.id == id)
    }

    fn shown_by_another(&self, id: ProjectId, cx: &mut Context<Self>) -> bool {
        crate::windows::shown_elsewhere(id, self.handle, cx).is_some()
    }

    /// Reads the workspace list again (another window changed it).
    pub(crate) fn reload_projects(&mut self, cx: &mut Context<Self>) {
        let projects = workspaces(AppState::store(cx));
        let chats = self.chats_space.as_ref().map(|chats| chats.id);
        let kept =
            |id: &ProjectId| Some(*id) == chats || projects.iter().any(|project| project.id == *id);
        self.spaces.retain(|id, _| kept(id));
        self.hidden_since.retain(|id, _| kept(id));
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
            .chain(self.chats_space.as_ref())
            .map(|project| (project.id, WorkspaceStatus::of(project.id, cx)))
            .collect();
        let sessions_changed = self.read_sessions(cx);
        if statuses != self.statuses || sessions_changed {
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
            .chain(self.chats_space.as_ref())
            .find(|project| project.id == id)
            .cloned()?;
        let space = cx.new(|cx| WorkspaceScreen::new(project, window, cx));
        let subscription = cx.subscribe_in(
            &space,
            window,
            move |this, _, event: &WorkspaceScreenEvent, window, cx| match event {
                WorkspaceScreenEvent::SessionsChanged => cx.notify(),
                WorkspaceScreenEvent::Review { hosting, pull } => {
                    let (hosting, pull) = ((**hosting).clone(), (**pull).clone());
                    this.set_view(View::GitHub, window, cx);
                    let github = this.github(cx);
                    github.update(cx, |github, cx| github.review(hosting, pull, window, cx));
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
        let shown = !self.sidebar_collapsed;
        space.update(cx, |space, cx| space.set_sidebar_shown(shown, cx));
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
        if !self.is_chats(id) && self.expanded.insert(id) {
            self.save_expanded(cx);
        }
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
        self.inbox
            .update(cx, |inbox, cx| inbox.set_project(Some(id), cx));
        let store = AppState::store(cx);
        let _ = store.touch_project(id);
        let _ = store.set_setting(keys::LAST_PROJECT, &Some(id));
        cx.notify();
    }

    /// Adds the folder at `path` and opens it.
    pub fn add_project_path(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            }
            Err(err) => {
                window.push_notification(
                    Notification::error(format!("Could not open the folder: {err:#}")),
                    cx,
                );
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
        if !self.has_project(pick.project_id) && !self.is_chats(pick.project_id) {
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
        let Some(id) = self.active_project.filter(|id| !self.is_chats(*id)) else {
            return window.dispatch_action(Box::new(AddProject), cx);
        };
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
        let Some(project) = self
            .active_space()
            .map(|space| space.read(cx).project().clone())
            .filter(|project| project.is_workspace())
        else {
            return;
        };
        let workspace = cx.entity().downgrade();
        crate::rename_dialog::open(project.name.clone(), window, cx, move |name, window, cx| {
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
        });
    }

    /// Removes a project from PilotHouse (the folder on disk is untouched).
    pub fn remove_project(&mut self, id: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_project(id) {
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
        if self.expanded.remove(&id) {
            self.save_expanded(cx);
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
            let next = self
                .chats_space
                .as_ref()
                .map(|chats| chats.id)
                .or_else(|| self.projects.first().map(|project| project.id));
            if let Some(next) = next {
                self.open_project(next, window, cx);
            }
        }
        cx.notify();
    }

    fn with_active_space(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut WorkspaceScreen, &mut Window, &mut Context<WorkspaceScreen>),
    ) {
        if let Some(space) = self.active_space() {
            space.update(cx, |space, cx| update(space, window, cx));
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

    fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        command_palette::open(self, cx.entity(), window, cx);
    }

    fn new_session(&mut self, _: &NewSession, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == View::GitHub {
            if let Some(github) = self.github.clone() {
                github.update(cx, |github, cx| github.new_chat(window, cx));
            }
            return;
        }
        self.with_active_space(window, cx, |space, window, cx| {
            space.new_session(window, cx)
        });
    }

    fn switch_to(
        &mut self,
        action: &SwitchToWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.projects.get(action.index).map(|project| project.id) {
            self.open_project(id, window, cx);
        }
    }

    fn previous_workspace(
        &mut self,
        _: &PreviousWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        if let Some(id) = self.active_project {
            self.remove_project(id, window, cx);
        }
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar_collapsed(!self.sidebar_collapsed, cx);
    }

    fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        let _ = AppState::store(cx).set_setting(keys::SIDEBAR_COLLAPSED, &collapsed);
        for space in self.spaces.values() {
            space.update(cx, |space, cx| space.set_sidebar_shown(!collapsed, cx));
        }
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

    fn find_in_project(&mut self, _: &FindInProject, window: &mut Window, cx: &mut Context<Self>) {
        self.with_active_space(window, cx, |space, window, cx| {
            space.find_in_project(window, cx)
        });
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .active_space()
            .map(|space| space.read(cx).project().path.clone());
        settings_dialog::open(project, window, cx);
    }

    fn set_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        AppState::set_appearance(appearance, window, cx);
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let github = (self.view == View::GitHub)
            .then(|| self.github.clone())
            .flatten();
        let content = match (&github, self.active_space()) {
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
            (None, None) => div().flex_1().into_any_element(),
        };

        v_flex()
            .id("workspace")
            .key_context(WORKSPACE_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            // The app background; panels float on it as surfaces.
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::add_project))
            .on_action(cx.listener(Self::new_workspace))
            .on_action(cx.listener(Self::add_folder))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::new_session))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_context_panel))
            .on_action(cx.listener(Self::toggle_zoom))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::find_in_project))
            .on_action(cx.listener(|this, _: &ShowFiles, window, cx| {
                this.show_tab(ContextTab::Files, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowWorkspacesView, window, cx| {
                this.set_view(View::Workspaces, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowGitHubView, window, cx| {
                this.set_view(View::GitHub, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowChanges, window, cx| {
                this.show_tab(ContextTab::Changes, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowBoard, window, cx| {
                this.show_tab(ContextTab::Board, window, cx)
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
                if let Some(space) = this.active_space() {
                    space.update(cx, |space, cx| space.capture_to_chat(window, cx));
                }
            }))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(|this, _: &UseSystemAppearance, window, cx| {
                this.set_appearance(Appearance::System, window, cx)
            }))
            .on_action(cx.listener(|this, _: &UseLightAppearance, window, cx| {
                this.set_appearance(Appearance::Light, window, cx)
            }))
            .on_action(
                cx.listener(|_, _: &ShowUsage, window, cx| crate::usage_view::open(window, cx)),
            )
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
                if let Some(space) = this.active_space() {
                    let root = space.read(cx).project().path.clone();
                    crate::arch_map::open(space.downgrade(), root, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowParallelRuns, window, cx| {
                if let Some(space) = this.active_space() {
                    let (project, root) = {
                        let project = space.read(cx).project();
                        (project.id, project.path.clone())
                    };
                    crate::parallel::open_runs(space.downgrade(), project, root, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowInstructions, window, cx| {
                if let Some(space) = this.active_space() {
                    let root = space.read(cx).project().path.clone();
                    crate::instructions_view::open(root, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SearchChats, window, cx| {
                crate::chat_search::open(this.active_project_id(), window, cx)
            }))
            .on_action(cx.listener(Self::switch_to))
            .on_action(cx.listener(Self::previous_workspace))
            .on_action(cx.listener(Self::remove_workspace))
            .on_action(cx.listener(|this, _: &OpenInNewWindow, window, cx| {
                if let Some(id) = this.active_project {
                    this.open_in_new_window(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &UseDarkAppearance, window, cx| {
                this.set_appearance(Appearance::Dark, window, cx)
            }))
            .child(
                TitleBar::new()
                    .h(ph_theme::layout::TITLE_BAR_HEIGHT)
                    .bg(title_bar::background(
                        self.active_space()
                            .map(|space| space.read(cx).project())
                            .filter(|project| {
                                self.view == View::Workspaces && project.is_workspace()
                            }),
                        cx,
                    ))
                    .child(title_bar::render(self, window, cx)),
            )
            .child(
                h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    // The GitHub view draws its own sidebar.
                    .when(!self.sidebar_collapsed && github.is_none(), |this| {
                        this.child(sidebar::render(self, cx))
                    })
                    .child(content),
            )
            .child(status_bar::render(self, cx))
    }
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
