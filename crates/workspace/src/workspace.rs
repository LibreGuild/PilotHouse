//! The root view of a workspace window (the main one, or one a workspace
//! moved to; see [`crate::windows`]).

use std::collections::HashMap;

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
use crate::project_space::{ProjectSpace, ProjectSpaceEvent};
use crate::status::WorkspaceStatus;
use crate::{command_palette, settings_dialog, sidebar, status_bar, title_bar, welcome};
use ph_files::{FindInProject, QuickOpen};

/// A hidden workspace's screen is closed after this long with nothing going on.
const UNLOAD_AFTER: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Owns the project list and the open [`ProjectSpace`]s, and routes workspace
/// actions to whichever project is active.
pub struct Workspace {
    focus_handle: FocusHandle,
    /// This window.
    handle: gpui_kit::AnyWindowHandle,
    projects: Vec<Project>,
    /// Spaces stay alive after switching away so their state survives.
    spaces: HashMap<ProjectId, Entity<ProjectSpace>>,
    active_project: Option<ProjectId>,
    /// The workspace shown before the one in front (Ctrl+Tab goes back).
    previous_project: Option<ProjectId>,
    sidebar_collapsed: bool,
    /// What each workspace is doing, read again every second.
    statuses: HashMap<ProjectId, WorkspaceStatus>,
    /// When each loaded but hidden workspace was last shown.
    hidden_since: HashMap<ProjectId, std::time::Instant>,
    /// The workspace switcher's filter, and its keyboard highlight.
    pub(crate) switcher_search: Entity<gpui_kit::component::input::InputState>,
    pub(crate) switcher_highlight: usize,
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
        let projects = store.projects().unwrap_or_else(|err| {
            tracing::error!("loading projects failed: {err:#}");
            Vec::new()
        });
        let last_project: Option<ProjectId> = store.setting(keys::LAST_PROJECT);
        let sidebar_collapsed: bool = store.setting(keys::SIDEBAR_COLLAPSED);

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
        let switcher_search = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx).placeholder("Find a workspace")
        });
        let switcher_subscription = cx.subscribe(
            &switcher_search,
            |this, _, event: &gpui_kit::component::input::InputEvent, cx| {
                if let gpui_kit::component::input::InputEvent::Change = event {
                    this.switcher_highlight = 0;
                    cx.notify();
                }
            },
        );
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
            spaces: HashMap::new(),
            active_project: None,
            previous_project: None,
            sidebar_collapsed,
            statuses: HashMap::new(),
            hidden_since: HashMap::new(),
            switcher_search,
            switcher_highlight: 0,
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
                switcher_subscription,
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
        let initial = project
            .or(last_project)
            .filter(|id| workspace.projects.iter().any(|project| project.id == *id))
            .or_else(|| workspace.projects.first().map(|project| project.id));
        if let Some(id) = initial {
            workspace.open_project(id, window, cx);
        }
        workspace
    }

    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    pub fn active_project_id(&self) -> Option<ProjectId> {
        self.active_project
    }

    pub fn active_space(&self) -> Option<&Entity<ProjectSpace>> {
        self.active_project.and_then(|id| self.spaces.get(&id))
    }

    pub fn is_sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }

    /// Gives the keyboard to the chat of the workspace in front, else the
    /// window.
    pub(crate) fn focus_front(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_space().cloned() {
            Some(space) => space.update(cx, |space, cx| space.focus_chat(window, cx)),
            None => self.focus_handle.focus(window, cx),
        }
    }

    /// The switcher opened: no filter, typing goes there.
    pub(crate) fn switcher_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher_highlight = 0;
        let search = self.switcher_search.clone();
        window.defer(cx, move |window, cx| {
            search.update(cx, |search, cx| {
                search.set_value("", window, cx);
                search.focus(window, cx);
            })
        });
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

    fn has_project(&self, id: ProjectId) -> bool {
        self.projects.iter().any(|project| project.id == id)
    }

    fn shown_by_another(&self, id: ProjectId, cx: &mut Context<Self>) -> bool {
        crate::windows::shown_elsewhere(id, self.handle, cx).is_some()
    }

    /// Reads the workspace list again (another window changed it).
    pub(crate) fn reload_projects(&mut self, cx: &mut Context<Self>) {
        let Ok(projects) = AppState::store(cx).projects() else {
            return;
        };
        self.spaces
            .retain(|id, _| projects.iter().any(|project| project.id == *id));
        self.hidden_since
            .retain(|id, _| projects.iter().any(|project| project.id == *id));
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
        for id in idle {
            tracing::info!("closing the screen of idle workspace {id}");
            self.spaces.remove(&id);
            self.hidden_since.remove(&id);
        }
    }

    fn refresh_statuses(&mut self, cx: &mut Context<Self>) {
        self.unload_idle(UNLOAD_AFTER, cx);
        let statuses: HashMap<ProjectId, WorkspaceStatus> = self
            .projects
            .iter()
            .map(|project| (project.id, WorkspaceStatus::of(project.id, cx)))
            .collect();
        if statuses != self.statuses {
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
    ) -> Option<Entity<ProjectSpace>> {
        if let Some(space) = self.spaces.get(&id) {
            return Some(space.clone());
        }
        let project = self
            .projects
            .iter()
            .find(|project| project.id == id)
            .cloned()?;
        let space = cx.new(|cx| ProjectSpace::new(project, window, cx));
        let subscription = cx.subscribe_in(
            &space,
            window,
            move |this, space, event: &ProjectSpaceEvent, window, cx| match event {
                ProjectSpaceEvent::SessionsChanged => cx.notify(),
                ProjectSpaceEvent::RevealFiles => {
                    if this.sidebar_collapsed {
                        this.set_sidebar_collapsed(false, cx);
                    }
                    space.update(cx, |_, cx| cx.notify());
                }
                ProjectSpaceEvent::MakePrimary(path) => {
                    this.make_primary_folder(id, path, window, cx)
                }
                ProjectSpaceEvent::RemoveFolder(path) => {
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

    /// Picks folders and adds them to the workspace in front.
    fn add_folder(
        &mut self,
        _: &AddFolderToWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.active_project else {
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
        // Its agents and shells stop.
        ph_agent::ChatHub::remove(id, cx);
        ph_terminal::TerminalHub::remove(id, cx);
        ph_uitest::runner::TestRunHub::remove(id, cx);
        self.projects_changed(cx);
        crate::schedules::Automation::global(cx).update(cx, |automation, _| automation.forget(id));
        if self.active_project == Some(id) {
            self.active_project = None;
            if let Some(next) = self.projects.first().map(|project| project.id) {
                self.open_project(next, window, cx);
            }
        }
        cx.notify();
    }

    fn with_active_space(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut ProjectSpace, &mut Window, &mut Context<ProjectSpace>),
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
        let content = match self.active_space() {
            Some(space) => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(space.clone())
                .into_any_element(),
            None => welcome::render(window, cx).into_any_element(),
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
            .on_action(cx.listener(|this, _: &ShowFiles, _, cx| {
                if let Some(space) = this.active_space().cloned() {
                    space.update(cx, |space, cx| space.reveal_files(cx));
                }
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
                        self.active_space().map(|space| space.read(cx).project()),
                        cx,
                    ))
                    .child(title_bar::render(self, window, cx)),
            )
            .child(
                h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .when(!self.sidebar_collapsed, |this| {
                        this.child(sidebar::render(self, cx))
                    })
                    .child(content),
            )
            .child(status_bar::render(self, cx))
    }
}
