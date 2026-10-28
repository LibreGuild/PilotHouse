//! Everything shown for one open workspace: the chat, the editor (while a
//! file is open) and the tools (Files, Git, the board…), each a pane of a
//! dockable layout the user arranges (see [`dock`]), and the icon bar that
//! shows a tool. The window's sidebar lists its chats.
//!
//! The Chats space (chats outside any workspace) has a screen too: the chat
//! and the tools that need no folder.
//!
//! A `WorkspaceScreen` is the screen of a workspace. Its running work (chats,
//! shells, test runs) belongs to the workspace and outlives the screen (see
//! [`ph_agent::chats`], [`ph_terminal::TerminalHub`],
//! [`ph_uitest::runner`]), so a space can be rebuilt without stopping any.
//!
//! Tool panels (board, terminal, ...) are created the first time their tool
//! is shown. A workspace that never opens the terminal never starts a shell.
//!
//! The code is split by concern: `panels` makes the tool panels, `to_chat`
//! sends what they produce to the chat, `dock` arranges the panes, `render`
//! draws the screen.

use gpui_kit::TestSupportExt as _;
use std::collections::{HashMap, HashSet};

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use ph_agent::{Attachment, ChatPanel, ChatPanelEvent, LiveSources, WorkspaceChatsEvent};
use ph_board::{BoardPanel, BoardPanelEvent};
use ph_browser::BrowserPanel;
use ph_changes::{BranchPicker, ChangesPanel, ChangesPanelEvent, Repo, RepoEvent};
use ph_devices::DevicesPanel;
use ph_files::{FileTree, FilesPanel, FilesPanelEvent, ProjectWatcher};
use ph_media::ToChat;
use ph_pulls::{PullsPanel, PullsPanelEvent};
use ph_runs::{RunsPanel, RunsPanelEvent};
use ph_store::projects::{Project, ProjectUiState};
use ph_store::sessions::{Session, SessionId};
use ph_terminal::TerminalPanel;
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::app_state::AppState;
use crate::context_panel::ContextTab;
use crate::detect::{Stack, detect};

mod dock;
mod panels;
mod render;
mod to_chat;

use dock::{Pane, Panes};

pub(crate) use panels::briefs;

/// Emitted when something the sidebar or title bar shows has changed, or
/// for what the window does (folders, reviews in the GitHub view).
#[derive(Clone)]
pub enum WorkspaceScreenEvent {
    SessionsChanged,
    /// Make this folder the workspace's primary one.
    MakePrimary(std::path::PathBuf),
    /// Take this folder out of the workspace.
    RemoveFolder(std::path::PathBuf),
    /// Work for an agent about a pull request (in the GitHub view, or a
    /// task for fixes).
    Agent {
        hosting: Box<ph_pulls::Hosting>,
        request: ph_pulls::AgentRequest,
    },
    /// Show the findings of review `review_id`.
    OpenFindings(i64),
    /// Run agent review `review_id` again on its PR's latest head.
    ReviewAgain(i64),
    /// Show chat `id`, wherever it is (a review in the GitHub view).
    ShowSession(SessionId),
}

/// What a workspace's folders hold, for choosing the tools it shows.
#[derive(Debug, Clone, Default)]
struct Traits {
    stacks: Vec<Stack>,
    /// Some folder is a Git repository.
    git: bool,
    /// Some folder's repository has a hosted remote (pull requests).
    remote: bool,
    has_tests: bool,
    /// The network drive the primary folder is on, if any.
    network: Option<ph_remote::drives::NetworkDrive>,
}

/// What finished while the workspace was away, shown above the chat until
/// dismissed.
#[derive(Debug, Clone, Default)]
struct Away {
    chats: Vec<SessionId>,
    tests: bool,
}

pub struct WorkspaceScreen {
    project: Project,
    sessions: Vec<Session>,
    active_session: Option<SessionId>,
    /// The tool shown last (for ⌘J, ⌘⇧J and the saved layout).
    context_tab: ContextTab,
    /// The panes on screen: the displayed tab of a group in view.
    shown: HashSet<Pane>,
    /// The dockable layout and its panes.
    panes: Panes,
    /// The chat is to be brought into view on the next frame.
    reveal_pending: bool,
    /// Saves the layout once a change has settled.
    save_dock_task: Option<gpui_kit::Task<()>>,
    away: Option<Away>,
    /// What the workspace's folders hold, once looked at.
    traits: Option<Traits>,
    /// The folder the Git and PRs tabs and the status bar's branch are for
    /// (an index into the workspace's roots).
    git_folder: usize,
    /// Each folder's repository model, Git tab, PRs tab and branch picker,
    /// created when first needed.
    repos: HashMap<usize, Entity<Repo>>,
    branch_pickers: HashMap<usize, Entity<BranchPicker>>,
    /// Layout saved last session, used to restore panels as they are created.
    saved: ProjectUiState,
    chat: Entity<ChatPanel>,
    /// Watches each folder; created with the first panel that needs them.
    watchers: Vec<Entity<ProjectWatcher>>,
    files: Option<Entity<FilesPanel>>,
    /// The Files tool's tree over `files`.
    file_tree: Option<Entity<FileTree>>,
    changes: HashMap<usize, Entity<ChangesPanel>>,
    board: Option<Entity<BoardPanel>>,
    terminal: Option<Entity<TerminalPanel>>,
    browser: Option<Entity<BrowserPanel>>,
    devices: Option<Entity<DevicesPanel>>,
    runs: Option<Entity<RunsPanel>>,
    pulls: HashMap<usize, Entity<PullsPanel>>,
    tests: Option<Entity<ph_uitest::TestsPanel>>,
    /// This is the project on screen (live panes stream only then).
    visible: bool,
    /// The window shows its sidebar (with the chat lists); the chat offers
    /// its own history menu only when not.
    sidebar_shown: bool,
    /// Ports and CI, for the status bar.
    signals: crate::signals::Signals,
    _signals: gpui_kit::Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WorkspaceScreenEvent> for WorkspaceScreen {}

impl WorkspaceScreen {
    pub fn new(project: Project, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = AppState::store(cx);
        let saved = store.project_ui_state(project.id).unwrap_or_default();
        let sessions = store.sessions(project.id).unwrap_or_default();
        let active_session = saved
            .session_id
            .filter(|id| sessions.iter().any(|session| session.id == *id))
            .or_else(|| sessions.first().map(|session| session.id));

        let store = store.clone();
        let chat = cx.new(|cx| {
            let mut chat = ChatPanel::new(store.clone(), project.clone(), window, cx);
            let session = active_session
                .and_then(|id| sessions.iter().find(|session| session.id == id).cloned());
            chat.set_session(session, cx);
            chat
        });
        let chat_subscription = cx.subscribe_in(
            &chat,
            window,
            |this, _, event: &ChatPanelEvent, window, cx| match event {
                ChatPanelEvent::SessionsChanged => this.reload_sessions(cx),
                ChatPanelEvent::SelectSession(Some(id)) => this.select_session(*id, cx),
                ChatPanelEvent::SelectSession(None) => this.new_session(window, cx),
                ChatPanelEvent::TurnEnded => {
                    // A UI test being recorded ends with the agent's turn.
                    if ph_uitest::record::current(cx).is_some()
                        && let Some(tests) = this.tests.clone()
                    {
                        tests.update(cx, |tests, cx| tests.finish_recording(cx));
                    }
                }
                ChatPanelEvent::WantPulls => this.feed_pulls(window, cx),
                // A workspace's chat has no starters of its own.
                ChatPanelEvent::Starter(_) => {}
                ChatPanelEvent::RunParallel { prompt } => {
                    let root = this.project.path.clone();
                    crate::parallel::open_start(
                        cx.entity().downgrade(),
                        root,
                        prompt.clone(),
                        window,
                        cx,
                    );
                }
                ChatPanelEvent::OpenFile { path, line } => {
                    let line = line.map(|line| line.saturating_sub(1) as u32);
                    this.open_file(path, line, window, cx);
                }
                ChatPanelEvent::ReviewAgain { review_id } => {
                    if this.is_own_review(*review_id, cx) {
                        this.review_changes(window, cx)
                    } else {
                        cx.emit(WorkspaceScreenEvent::ReviewAgain(*review_id))
                    }
                }
                ChatPanelEvent::OpenFindings { review_id } => {
                    if this.is_own_review(*review_id, cx) {
                        this.show_review_notes(*review_id, window, cx)
                    } else {
                        cx.emit(WorkspaceScreenEvent::OpenFindings(*review_id))
                    }
                }
            },
        );
        // Chats a schedule starts appear in the list.
        let chats = chat.read(cx).chats().clone();
        // When this screen goes (its window closed), what finishes from now
        // on counts as unseen.
        let (off_screen, project_id) = (chats.clone(), project.id);
        cx.on_release(move |_, cx| {
            off_screen.update(cx, |chats, cx| {
                chats.set_on_screen(false, cx);
            });
            if let Some(runs) = ph_uitest::runner::TestRunHub::existing(project_id, cx) {
                runs.update(cx, |runs, cx| {
                    runs.set_on_screen(false, cx);
                });
            }
        })
        .detach();
        let chats_subscription =
            cx.subscribe(&chats, |this, _, event: &WorkspaceChatsEvent, cx| {
                if let WorkspaceChatsEvent::SessionsChanged = event {
                    this.reload_sessions(cx)
                }
            });
        // `@screenshot` and `@log` read from this workspace's live panes,
        // `@test-run` from its UI test reports.
        let (screenshot_space, logs_space) = (cx.weak_entity(), cx.weak_entity());
        let (tests_project, tests_root) = (project.id, project.path.clone());
        chat.update(cx, |chat, _| {
            chat.set_live_sources(LiveSources {
                screenshot: Box::new(move |cx| {
                    screenshot_space.upgrade()?.read(cx).live_screenshot(cx)
                }),
                logs: Box::new(move |cx| logs_space.upgrade()?.read(cx).live_logs(cx)),
                tests: Box::new(move |_| to_chat::test_results(tests_project, &tests_root)),
            })
        });

        let (area, skin) = gpui_kit::component::dock::DockSkin::dock_area(
            SharedString::from(format!("space-{}", project.id)),
            Some(dock::LAYOUT_VERSION),
            window,
            cx,
        );
        // Always a tab bar, even for one pane: its tab is what drags.
        skin.set_panel_style(gpui_kit::component::dock::PanelStyle::TabBar, cx);
        skin.set_close_button_visible(true, cx);
        let panes = Panes::new(area.clone(), cx.weak_entity(), cx);
        let dock_subscription = cx.subscribe(
            &area,
            |this, _, event: &gpui_kit::component::dock::DockEvent, cx| {
                if let gpui_kit::component::dock::DockEvent::LayoutChanged = event {
                    this.save_dock_soon(cx);
                    cx.notify();
                }
            },
        );
        let mut space = Self {
            context_tab: saved
                .context_tab
                .as_deref()
                .and_then(ContextTab::from_key)
                .unwrap_or_default(),
            shown: HashSet::new(),
            panes,
            reveal_pending: false,
            save_dock_task: None,
            away: None,
            traits: None,
            git_folder: 0,
            repos: HashMap::new(),
            branch_pickers: HashMap::new(),
            saved,
            project,
            sessions,
            active_session,
            chat,
            watchers: Vec::new(),
            files: None,
            file_tree: None,
            changes: HashMap::new(),
            board: None,
            terminal: None,
            browser: None,
            devices: None,
            runs: None,
            pulls: HashMap::new(),
            tests: None,
            visible: true,
            sidebar_shown: true,
            signals: Default::default(),
            _signals: crate::signals::watch(store.clone(), cx),
            _subscriptions: vec![chat_subscription, chats_subscription, dock_subscription],
        };
        space.git_folder = space.first_repo_folder();
        let saved_dock = store.setting::<Option<gpui_kit::component::dock::DockAreaState>>(
            &dock_key(space.project.id),
        );
        if !space.panes.load(saved_dock, window, cx) {
            // A workspace without folders starts with the chat alone.
            let open = space
                .saved
                .context_panel_open
                .unwrap_or(!space.project.empty);
            space.panes.set_default(space.context_tab, open, window, cx);
        }
        // Files (the editor and the tree) from the start: open files come
        // back with the screen.
        if !space.project.empty {
            space.files(window, cx);
        }
        space.repo(window, cx);
        space.inspect_folder(cx);
        space
    }

    /// Looks at what the workspace's folders hold, off the UI thread.
    fn inspect_folder(&mut self, cx: &mut Context<Self>) {
        let folders = self.project.folders();
        let tests_root = self.project.path.clone();
        let inspect = cx.background_spawn(async move {
            let mut traits = Traits {
                has_tests: !ph_uitest::model::list(&tests_root).is_empty(),
                network: folders
                    .first()
                    .and_then(|folder| ph_remote::drives::network_drive_of(folder)),
                ..Traits::default()
            };
            for folder in &folders {
                for stack in detect(folder) {
                    if !traits.stacks.contains(&stack) {
                        traits.stacks.push(stack);
                    }
                }
                if ph_git::is_repository(folder) {
                    traits.git = true;
                    traits.remote |= ph_git::remote::hosted_repo(folder).is_some();
                }
            }
            traits
        });
        cx.spawn(async move |this, cx| {
            let traits = inspect.await;
            _ = this.update(cx, |this, cx| {
                this.traits = Some(traits);
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether `tab` is worth a place on the icon bar for this workspace:
    /// Git with a repository, PRs with a hosted remote, Browser for web
    /// code, Devices for Android, Run with something to run, Tests with
    /// tests or an app to test. Until the folders are looked at, every tool
    /// shows; the tool in view always does.
    fn is_relevant(&self, tab: ContextTab) -> bool {
        // In the Chats space there is no folder to show or add.
        if tab == ContextTab::Files {
            return self.project.is_workspace();
        }
        let Some(traits) = &self.traits else {
            return true;
        };
        if self.shown.contains(&Pane::Tool(tab)) {
            return true;
        }
        let empty = self.project.empty;
        let has = |stack| traits.stacks.contains(&stack);
        let web = has(Stack::Web) || has(Stack::Node);
        match tab {
            ContextTab::Files => true,
            ContextTab::Changes => traits.git,
            ContextTab::Pulls => traits.remote,
            ContextTab::Board | ContextTab::Terminal => true,
            ContextTab::Browser => empty || web,
            ContextTab::Devices => empty || has(Stack::Android),
            ContextTab::Runs => !traits.stacks.is_empty(),
            ContextTab::Tests => empty || traits.has_tests || web || has(Stack::Android),
        }
    }

    /// The network drive the primary folder is on (once looked at).
    pub fn network_drive(&self) -> Option<&ph_remote::drives::NetworkDrive> {
        self.traits.as_ref()?.network.as_ref()
    }

    /// Where the workspace's Git, shells and runs work when its primary
    /// folder is on a mapped network drive.
    pub fn server(&self) -> Option<ph_remote::Remote> {
        ph_remote::remote_of(self.project.folders().first()?)
    }

    /// Runs Git again everywhere (a network drive mapping changed, so Git
    /// may now run elsewhere).
    pub fn refresh_git(&mut self, cx: &mut Context<Self>) {
        for repo in self.repos.values() {
            repo.update(cx, |repo, cx| repo.refresh(cx));
        }
        self.inspect_folder(cx);
        cx.notify();
    }

    /// The repository model, created with the space. Git operations report
    /// their result as a notification.
    pub fn repo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Repo> {
        self.repo_for(self.git_folder, window, cx)
    }

    /// The repository model of folder `ix` (each folder's changes are
    /// marked in the tree).
    fn repo_for(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> Entity<Repo> {
        if let Some(repo) = self.repos.get(&ix) {
            return repo.clone();
        }
        let watcher = self.watchers(cx)[ix.min(self.watchers.len() - 1)].clone();
        let repo = cx.new(|cx| Repo::new(&watcher, cx));
        self._subscriptions.push(cx.observe(&repo, |this, _, cx| {
            this.mark_changes(cx);
            cx.notify()
        }));
        self._subscriptions.push(cx.subscribe_in(
            &repo,
            window,
            |_, _, event: &RepoEvent, window, cx| match event {
                RepoEvent::Done(message) => {
                    window.push_notification(Notification::success(message.clone()), cx)
                }
                RepoEvent::Failed(message) => {
                    window.push_notification(Notification::error(message.clone()).title("Git"), cx)
                }
            },
        ));
        self.repos.insert(ix, repo.clone());
        repo
    }

    /// Passes every folder's Git changes to the tree.
    fn mark_changes(&mut self, cx: &mut Context<Self>) {
        let Some(files) = self.files.clone() else {
            return;
        };
        let roots = self.roots();
        let mut changes = std::collections::HashMap::new();
        let mut staged = std::collections::HashSet::new();
        for repo in self.repos.values() {
            let repo = repo.read(cx);
            let Some(status) = repo.status() else {
                continue;
            };
            for (path, change) in &status.changes {
                if let Some(id) = roots.path_of(&repo.root().join(path)) {
                    changes.insert(id, *change);
                }
            }
            staged.extend(
                status
                    .staged
                    .iter()
                    .filter_map(|path| roots.path_of(&repo.root().join(path))),
            );
        }
        files.update(cx, |files, cx| files.set_changes(changes, staged, cx));
    }

    pub fn repo_model(&self) -> Option<&Entity<Repo>> {
        self.repos.get(&self.git_folder)
    }

    /// The status bar's branch picker, created when first opened.
    pub fn branch_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<BranchPicker> {
        if let Some(picker) = self.branch_pickers.get(&self.git_folder) {
            return picker.clone();
        }
        let repo = self.repo(window, cx);
        let picker = cx.new(|cx| BranchPicker::new(repo, window, cx));
        self.branch_pickers.insert(self.git_folder, picker.clone());
        picker
    }

    pub fn stacks(&self) -> &[Stack] {
        self.traits
            .as_ref()
            .map_or(&[], |traits| traits.stacks.as_slice())
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn signals(&self) -> &crate::signals::Signals {
        &self.signals
    }

    pub(crate) fn signals_mut(&mut self) -> &mut crate::signals::Signals {
        &mut self.signals
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn active_session(&self) -> Option<SessionId> {
        self.active_session
    }

    pub fn context_tab(&self) -> ContextTab {
        self.context_tab
    }

    /// Some tool is on screen.
    pub fn is_context_open(&self) -> bool {
        self.shown.iter().any(|pane| matches!(pane, Pane::Tool(_)))
    }

    /// Shows `tab` where it is docked (on the right if it is not).
    pub fn show_tab(&mut self, tab: ContextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !AppState::enabled_tabs(cx).contains(&tab) {
            return;
        }
        self.ensure_panel(tab, window, cx);
        self.context_tab = tab;
        self.panes.show(Pane::Tool(tab), window, cx);
        self.save_layout(cx);
        cx.notify();
    }

    /// Back to the starting layout: the chat and the editor as tabs, the
    /// tool in view docked on the right.
    pub fn reset_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panes.set_default(self.context_tab, true, window, cx);
        cx.notify();
    }

    /// Moves `pane` where its menu said.
    fn move_pane(
        &mut self,
        pane: Pane,
        to: dock::MoveTo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panes.move_to(pane, to, window, cx);
        cx.notify();
    }

    /// A pane came into view or left it (its tab, its dock, its group).
    fn pane_shown(&mut self, pane: Pane, shown: bool, cx: &mut Context<Self>) {
        let changed = if shown {
            self.shown.insert(pane)
        } else {
            self.shown.remove(&pane)
        };
        if !changed {
            return;
        }
        if shown && let Pane::Tool(tab) = pane {
            self.context_tab = tab;
        }
        self.update_live_panes(cx);
        self.save_layout(cx);
        cx.notify();
    }

    /// Saves the layout in a moment: a split made by a drop has its sizes
    /// only once it has been laid out, and a drag makes many changes.
    fn save_dock_soon(&mut self, cx: &mut Context<Self>) {
        self.save_dock_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(600))
                .await;
            _ = this.update(cx, |this, cx| this.save_dock(cx));
        }));
    }

    /// Keeps the layout for next time.
    fn save_dock(&self, cx: &mut Context<Self>) {
        let state = self.panes.area.read(cx).dump(cx);
        if let Err(err) = AppState::store(cx).set_setting(&dock_key(self.project.id), &Some(state))
        {
            tracing::warn!("saving the layout failed: {err:#}");
        }
    }

    /// A file is open in the editor.
    pub(crate) fn has_open_files(&self, cx: &App) -> bool {
        self.files
            .as_ref()
            .is_some_and(|files| files.read(cx).has_open_files())
    }

    /// Called by the workspace when this project is shown or hidden.
    /// Coming back shows what finished while it was away.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        self.update_live_panes(cx);
        let chats = self.chat.read(cx).chats().clone();
        let finished = chats.update(cx, |chats, cx| chats.set_on_screen(visible, cx));
        let runs = ph_uitest::runner::TestRunHub::runs(self.project.id, cx);
        let tests = runs.update(cx, |runs, cx| runs.set_on_screen(visible, cx));
        if visible && (!finished.is_empty() || tests) {
            self.away = Some(Away {
                chats: finished,
                tests,
            });
            cx.notify();
        }
    }

    /// How many open files have unsaved changes.
    pub fn unsaved_files(&self, cx: &App) -> usize {
        self.files
            .as_ref()
            .map_or(0, |files| files.read(cx).unsaved_count(cx))
    }

    /// Nothing here would be lost if this screen were closed: no unsaved
    /// file, no message being written.
    pub fn can_unload(&self, cx: &App) -> bool {
        self.unsaved_files(cx) == 0 && !self.chat.read(cx).has_draft(cx)
    }

    /// Gives the keyboard to what is in front: the chat's composer, or the
    /// open file when the editor is on screen and the chat is not (they
    /// may share a tab group). The layout stays as it is.
    pub fn focus_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shown.contains(&Pane::Chat)
            && self.shown.contains(&Pane::Editor)
            && let Some(files) = self.files.clone()
        {
            files.update(cx, |files, cx| files.focus_active(window, cx));
            return;
        }
        self.chat
            .update(cx, |chat, cx| chat.focus_composer(window, cx));
    }

    /// Brings the chat forward and puts the keyboard in its composer.
    pub fn show_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_chat(cx);
        self.chat
            .update(cx, |chat, cx| chat.focus_composer(window, cx));
    }

    /// The window's sidebar was shown or hidden.
    pub fn set_sidebar_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.sidebar_shown = shown;
        self.chat
            .update(cx, |chat, cx| chat.set_history_menu(!shown, cx));
        cx.notify();
    }

    /// The workspace's colour changed.
    pub fn set_color(&mut self, color: Option<u8>, cx: &mut Context<Self>) {
        self.project.color = color;
        cx.notify();
    }

    /// Brings the chat into view, for something sent to it (on the next
    /// frame, where the dock can be changed).
    fn reveal_chat(&mut self, cx: &mut Context<Self>) {
        self.reveal_pending = true;
        cx.notify();
    }

    /// Streams frames only into a pane the user can see.
    fn update_live_panes(&mut self, cx: &mut Context<Self>) {
        let shown = |tab| self.visible && self.shown.contains(&Pane::Tool(tab));
        let (browser_shown, devices_shown) =
            (shown(ContextTab::Browser), shown(ContextTab::Devices));
        if let Some(browser) = &self.browser {
            browser.update(cx, |browser, cx| browser.set_active(browser_shown, cx));
        }
        if let Some(devices) = &self.devices {
            devices.update(cx, |devices, cx| devices.set_active(devices_shown, cx));
        }
        let runs_shown = shown(ContextTab::Runs);
        if let Some(runs) = &self.runs {
            runs.update(cx, |runs, cx| runs.set_active(runs_shown, cx));
        }
    }

    /// ⌘J: opens or closes the dock holding the tool shown last.
    pub fn toggle_context_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.context_tab;
        self.ensure_panel(tab, window, cx);
        let pane = Pane::Tool(tab);
        if self.panes.placement(pane, cx).is_none() {
            return self.show_tab(tab, window, cx);
        }
        self.panes.toggle_dock_of(pane, window, cx);
        self.save_layout(cx);
        cx.notify();
    }

    /// ⌘⇧J: the tool shown last fills the screen, or gives it back.
    pub fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = Pane::Tool(self.context_tab);
        if !self.panes.is_docked_open(pane, cx) {
            self.show_tab(self.context_tab, window, cx);
        }
        let group = self.panes.pane(pane).read(cx).group();
        if let Some(group) = group {
            group.update(cx, |group, cx| group.toggle_zoom(window, cx));
        }
        cx.notify();
    }

    /// The files panel, created on first use.
    pub fn files(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<FilesPanel> {
        if let Some(files) = &self.files {
            return files.clone();
        }
        let watchers = self.watchers(cx);
        let roots = self.roots();
        let files = cx.new(|cx| {
            let mut panel = FilesPanel::new(roots, &watchers, window, cx);
            panel.restore(
                &self.saved.open_files,
                self.saved.active_file.as_deref(),
                window,
                cx,
            );
            panel
        });
        self._subscriptions.push(cx.subscribe_in(
            &files,
            window,
            |this, _, event: &FilesPanelEvent, window, cx| match event {
                FilesPanelEvent::OpenFilesChanged => this.save_layout(cx),
                FilesPanelEvent::Opened => this.panes.show(Pane::Editor, window, cx),
                FilesPanelEvent::Ask(ask) => this.ask_about_code(ask.clone(), window, cx),
                FilesPanelEvent::MakePrimary(path) => {
                    cx.emit(WorkspaceScreenEvent::MakePrimary(path.clone()))
                }
                FilesPanelEvent::RemoveFolder(path) => {
                    cx.emit(WorkspaceScreenEvent::RemoveFolder(path.clone()))
                }
                event => this.on_file_menu(event, window, cx),
            },
        ));
        // The editor column comes and goes with the open files.
        self._subscriptions
            .push(cx.observe(&files, |_, _, cx| cx.notify()));
        self.file_tree = Some(cx.new(|cx| FileTree::new(files.clone(), cx)));
        self.files = Some(files.clone());
        // Each Git folder's changes are marked in the tree (a folder that
        // is not a repository simply has no status; that is found out off
        // the UI thread).
        if !self.project.empty {
            for ix in 0..self.project.roots().len() {
                self.repo_for(ix, window, cx);
            }
        }
        self.mark_changes(cx);
        files
    }

    /// The Files tool's tree (none for a workspace without folders).
    pub fn file_tree(&self) -> Option<&Entity<FileTree>> {
        self.file_tree.as_ref()
    }

    pub fn files_panel(&self) -> Option<&Entity<FilesPanel>> {
        self.files.as_ref()
    }

    /// One watcher per folder, in the order of the workspace's roots. Their
    /// changes are passed on as [`WorkspaceScreenEvent::FilesChanged`].
    fn watchers(&mut self, cx: &mut Context<Self>) -> Vec<Entity<ProjectWatcher>> {
        if self.watchers.is_empty() {
            self.watchers = self
                .project
                .roots()
                .into_iter()
                .map(|root| ph_files::WatcherHub::watcher(root, cx))
                .collect();
        }
        self.watchers.clone()
    }

    /// The workspace's folders with the names Files gives them, the
    /// primary first.
    fn named_folders(&self) -> Vec<(String, std::path::PathBuf)> {
        self.roots()
            .list()
            .iter()
            .map(|root| (root.name.clone(), root.path.clone()))
            .collect()
    }

    /// The workspace's folders as Files shows them; a workspace without
    /// folders shows its scratch folder.
    pub fn roots(&self) -> ph_files::roots::Roots {
        let folders: Vec<_> = self
            .project
            .roots()
            .into_iter()
            .enumerate()
            .map(|(ix, root)| {
                (
                    root,
                    (ix == 0 && self.project.empty).then(|| "Scratch".to_string()),
                )
            })
            .collect();
        ph_files::roots::Roots::new(&folders)
    }

    /// The first folder with a Git repository (the primary one when none
    /// has).
    fn first_repo_folder(&self) -> usize {
        self.project
            .roots()
            .iter()
            .enumerate()
            .skip(usize::from(self.project.empty))
            .find(|(_, root)| ph_git::is_repository(root))
            .map_or(0, |(ix, _)| ix)
    }

    /// Which folder the Git and PRs tabs show.
    pub fn git_folder(&self) -> usize {
        self.git_folder
    }

    pub fn set_git_folder(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.git_folder || ix >= self.project.roots().len() {
            return;
        }
        self.git_folder = ix;
        self.repo(window, cx);
        self.ensure_panel(self.context_tab, window, cx);
        cx.notify();
    }

    /// The workspace's folders changed (added or removed): the tools that
    /// show folders start again; the chat keeps going and gets the new
    /// folders with its next agent session.
    pub fn set_project(&mut self, project: Project, window: &mut Window, cx: &mut Context<Self>) {
        let open_files = self
            .files
            .as_ref()
            .map(|files| files.read(cx).open_file_paths(cx))
            .unwrap_or_default();
        self.project = project.clone();
        self.watchers.clear();
        self.files = None;
        self.file_tree = None;
        self.repos.clear();
        self.branch_pickers.clear();
        self.changes.clear();
        self.pulls.clear();
        self.tests = None;
        self.runs = None;
        self.terminal = None;
        self.git_folder = self.first_repo_folder();
        self.saved.open_files = open_files;
        self.chat
            .update(cx, |chat, cx| chat.set_project(project, cx));
        self.repo(window, cx);
        self.inspect_folder(cx);
        let shown: Vec<ContextTab> = self
            .shown
            .iter()
            .filter_map(|pane| match pane {
                Pane::Tool(tab) => Some(*tab),
                _ => None,
            })
            .collect();
        for tab in shown {
            self.ensure_panel(tab, window, cx);
        }
        if !self.project.empty {
            self.files(window, cx);
        }
        cx.notify();
    }

    /// ⌘P from anywhere: the file finder.
    pub fn quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.files(window, cx);
        files.update(cx, |files, cx| files.quick_open(window, cx));
    }

    /// ⌘⇧F from anywhere: the Files tool's tree turns into workspace search.
    pub fn find_in_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.project.is_workspace() {
            return;
        }
        self.show_tab(ContextTab::Files, window, cx);
        let files = self.files(window, cx);
        files.update(cx, |files, cx| files.find_in_project(window, cx));
    }

    /// Opens `relative` in the editor beside the chat, optionally at a
    /// 0-based line.
    pub fn open_file(
        &mut self,
        relative: &str,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files = self.files(window, cx);
        // The panel says it opened the file, and the editor comes into view.
        files.update(cx, |files, cx| files.open_file(relative, line, window, cx));
    }

    /// Shows chat `session` scrolled to `message` (a search result).
    pub fn show_message(&mut self, session: SessionId, message: i64, cx: &mut Context<Self>) {
        self.select_session(session, cx);
        self.reveal_chat(cx);
        let position = AppState::store(cx).message_position(session, message);
        if let Ok(position) = position {
            self.chat
                .update(cx, |chat, cx| chat.reveal_entry(position, cx));
        }
    }

    pub fn select_session(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if self.active_session == Some(id) {
            return;
        }
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned();
        self.active_session = Some(id);
        self.chat
            .update(cx, |chat, cx| chat.set_session(session, cx));
        self.save_layout(cx);
        cx.emit(WorkspaceScreenEvent::SessionsChanged);
        cx.notify();
    }

    /// Clears the chat so the next message starts a new session (with the
    /// agent picked in the composer). Nothing is stored until it is sent.
    pub fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_session = None;
        self.chat.update(cx, |chat, cx| {
            chat.set_session(None, cx);
            chat.focus_composer(window, cx);
        });
        self.save_layout(cx);
        cx.emit(WorkspaceScreenEvent::SessionsChanged);
        cx.notify();
    }

    /// Whether review `review_id` is of this workspace's own changes.
    fn is_own_review(&self, review_id: i64, cx: &App) -> bool {
        AppState::store(cx)
            .pr_review(review_id)
            .ok()
            .flatten()
            .is_some_and(|review| review.kind == "self" && review.project_id == self.project.id)
    }

    /// Has an agent review the changes in the Git tab's folder before they
    /// are pushed, in a chat; its findings come back as notes on the lines.
    pub(crate) fn review_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.project.folders().get(self.git_folder).cloned() else {
            return;
        };
        let store = AppState::store(cx).clone();
        let (project_id, agent) = (self.project.id, ph_agent::chats::default_agent(&store));
        let template = ph_review::templates::default_review(&store);
        self.reveal_chat(cx);
        let work = cx.background_spawn(async move {
            crate::review::prepare_self(&store, project_id, &root, &template, agent.name)
        });
        let chat = self.chat.clone();
        cx.spawn_in(window, async move |_, cx| {
            let result = work.await;
            _ = chat.update_in(cx, |chat, window, cx| match result {
                Ok(prepared) => chat.start_review_turn(
                    Some(prepared.review_id),
                    Some(ph_agent::ReviewTurn::Round),
                    Some(agent),
                    prepared.worktree,
                    prepared.title,
                    prepared.display,
                    prepared.prompt,
                    window,
                    cx,
                ),
                Err(err) => chat.set_failed(format!("Could not start the review: {err:#}"), cx),
            });
        })
        .detach();
    }

    /// Puts review `review_id`'s open findings on the changed lines in the
    /// Git tab, as notes to fix with the agent.
    fn show_review_notes(&mut self, review_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let store = AppState::store(cx).clone();
        let findings = store.pr_findings(review_id).unwrap_or_default();
        let notes = findings
            .iter()
            .filter(|f| f.status == "open" && !f.path.is_empty())
            .map(|f| ph_changes::notes::Note {
                path: f.path.clone(),
                line: f.line.unwrap_or(1) as u32,
                code: f.evidence.lines().next().unwrap_or_default().to_string(),
                text: format!(
                    "{} [{}] {}{}",
                    f.label,
                    f.severity,
                    f.title.trim(),
                    if f.suggestion.trim().is_empty() {
                        String::new()
                    } else {
                        format!(". Fix: {}", f.suggestion.trim())
                    }
                ),
                from_review: true,
            })
            .collect();
        let changes = self.changes(window, cx);
        changes.update(cx, |changes, cx| changes.set_review_notes(notes, cx));
        self.show_tab(ContextTab::Changes, window, cx);
    }

    /// Starts a new chat named `title` that shows `display` and asks
    /// `prompt` (a fix in a task).
    pub fn ask_chat(
        &mut self,
        title: String,
        display: String,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chat.update(cx, |chat, cx| {
            chat.start_prompt(title, display, prompt, window, cx)
        });
        cx.notify();
    }

    /// Steps aside from the chat in front into a side chat about it.
    pub fn side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| chat.side_chat(window, cx));
        cx.notify();
    }

    /// Re-reads sessions after the chat created or renamed one.
    fn reload_sessions(&mut self, cx: &mut Context<Self>) {
        self.sessions = AppState::store(cx)
            .sessions(self.project.id)
            .unwrap_or_default();
        self.active_session = self.chat.read(cx).active_session_id(cx);
        self.save_layout(cx);
        cx.emit(WorkspaceScreenEvent::SessionsChanged);
        cx.notify();
    }

    fn save_layout(&mut self, cx: &mut Context<Self>) {
        let (open_files, active_file) = match &self.files {
            Some(files) => {
                let files = files.read(cx);
                (files.open_file_paths(cx), files.active_file_path(cx))
            }
            // Not opened this session: keep what was saved.
            None => (
                self.saved.open_files.clone(),
                self.saved.active_file.clone(),
            ),
        };
        self.saved = ProjectUiState {
            context_tab: Some(self.context_tab.key().to_string()),
            context_panel_open: Some(self.is_context_open()),
            session_id: self.active_session,
            open_files,
            active_file,
        };
        if let Err(err) = AppState::store(cx).save_project_ui_state(self.project.id, &self.saved) {
            tracing::warn!(
                "saving layout for project {} failed: {err:#}",
                self.project.id
            );
        }
    }
}

/// The settings key of workspace `id`'s dock layout.
fn dock_key(id: ph_store::projects::ProjectId) -> String {
    format!("workspace.dock.{id}")
}
