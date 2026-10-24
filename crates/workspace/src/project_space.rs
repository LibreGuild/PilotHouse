//! Everything shown for one open workspace: the editor column (while a file
//! is open), the chat, the tool panel and the icon bar that picks the tool,
//! plus what the window's sidebar shows for it (the file tree, the chats).
//!
//! A `ProjectSpace` is the screen of a workspace. Its running work (chats,
//! shells, test runs) belongs to the workspace and outlives the screen (see
//! [`ph_agent::chats`], [`ph_terminal::TerminalHub`],
//! [`ph_uitest::runner`]), so a space can be rebuilt without stopping any.
//!
//! Tool panels (board, terminal, ...) are created the first time their tool
//! is shown. A workspace that never opens the terminal never starts a shell.

use gpui_kit::TestSupportExt as _;
use std::collections::HashMap;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    resizable::{h_resizable, resizable_panel},
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
use ph_theme::layout;
use ph_ui::MenuButton as _;
use ph_ui::{Icons, Surface};

use crate::app_state::AppState;
use crate::context_panel::ContextTab;
use crate::detect::{Stack, detect};

/// Emitted when something the sidebar or title bar shows has changed.
#[derive(Debug, Clone)]
pub enum ProjectSpaceEvent {
    SessionsChanged,
    /// Show the sidebar's file tree (it may be hidden).
    RevealFiles,
    /// Make this folder the workspace's primary one.
    MakePrimary(std::path::PathBuf),
    /// Take this folder out of the workspace.
    RemoveFolder(std::path::PathBuf),
}

/// What a narrow window shows when a file and a tool are both open: the
/// one opened last, the other folded to the side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Front {
    Editor,
    Tools,
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
}

/// What finished while the workspace was away, shown above the chat until
/// dismissed.
#[derive(Debug, Clone, Default)]
struct Away {
    chats: Vec<SessionId>,
    tests: bool,
}

pub struct ProjectSpace {
    project: Project,
    sessions: Vec<Session>,
    active_session: Option<SessionId>,
    context_tab: ContextTab,
    context_open: bool,
    /// The tool panel fills the space and the chat is hidden.
    zoomed: bool,
    front: Front,
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
    /// The sidebar's view of `files`.
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
    /// The window shows its sidebar (with the chat list); the chat offers
    /// its own history menu only when not.
    sidebar_shown: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProjectSpaceEvent> for ProjectSpace {}

impl ProjectSpace {
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
                    this.review_again(*review_id, window, cx)
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
                tests: Box::new(move |_| test_results(tests_project, &tests_root)),
            })
        });

        let mut space = Self {
            context_tab: saved
                .context_tab
                .as_deref()
                .and_then(ContextTab::from_key)
                .unwrap_or_default(),
            // A workspace without folders starts with the chat alone.
            context_open: saved.context_panel_open.unwrap_or(!project.empty),
            zoomed: false,
            front: Front::Editor,
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
            _subscriptions: vec![chat_subscription, chats_subscription],
        };
        space.git_folder = space.first_repo_folder();
        if space.context_open {
            space.ensure_panel(space.context_tab, window, cx);
        }
        // The tree is in the sidebar from the start.
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
        let Some(traits) = &self.traits else {
            return true;
        };
        if tab == self.context_tab && self.context_open {
            return true;
        }
        let empty = self.project.empty;
        let has = |stack| traits.stacks.contains(&stack);
        let web = has(Stack::Web) || has(Stack::Node);
        match tab {
            ContextTab::Changes => traits.git,
            ContextTab::Pulls => traits.remote,
            ContextTab::Board | ContextTab::Terminal => true,
            ContextTab::Browser => empty || web,
            ContextTab::Devices => empty || has(Stack::Android),
            ContextTab::Runs => !traits.stacks.is_empty(),
            ContextTab::Tests => empty || traits.has_tests || web || has(Stack::Android),
        }
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
        }
        files.update(cx, |files, cx| files.set_changes(changes, cx));
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

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn active_session(&self) -> Option<SessionId> {
        self.active_session
    }

    pub fn context_tab(&self) -> ContextTab {
        self.context_tab
    }

    pub fn is_context_open(&self) -> bool {
        self.context_open
    }

    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    /// Shows `tab`, opening the panel if it was hidden.
    pub fn show_tab(&mut self, tab: ContextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !AppState::enabled_tabs(cx).contains(&tab) {
            return;
        }
        self.ensure_panel(tab, window, cx);
        self.context_tab = tab;
        self.context_open = true;
        self.front = Front::Tools;
        self.update_live_panes(cx);
        self.save_layout(cx);
        cx.notify();
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

    /// Brings the chat into view, for something sent to it.
    fn reveal_chat(&mut self, cx: &mut Context<Self>) {
        self.zoomed = false;
        cx.notify();
    }

    /// Streams frames only into a pane the user can see.
    fn update_live_panes(&mut self, cx: &mut Context<Self>) {
        let tools = self.context_open;
        let shown = |tab| self.visible && tools && self.context_tab == tab;
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

    pub fn toggle_context_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_open = !self.context_open;
        if self.context_open {
            self.ensure_panel(self.context_tab, window, cx);
        } else {
            self.zoomed = false;
        }
        self.update_live_panes(cx);
        self.save_layout(cx);
        cx.notify();
    }

    /// Lets the context panel take the whole width (for reading code or the board).
    pub fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.zoomed = !self.zoomed;
        if self.zoomed && !self.context_open {
            self.toggle_context_panel(window, cx);
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
                FilesPanelEvent::Ask(ask) => this.ask_about_code(ask.clone(), window, cx),
                FilesPanelEvent::MakePrimary(path) => {
                    cx.emit(ProjectSpaceEvent::MakePrimary(path.clone()))
                }
                FilesPanelEvent::RemoveFolder(path) => {
                    cx.emit(ProjectSpaceEvent::RemoveFolder(path.clone()))
                }
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

    /// The sidebar's file tree (none for a workspace without folders).
    pub fn file_tree(&self) -> Option<&Entity<FileTree>> {
        self.file_tree.as_ref()
    }

    pub fn files_panel(&self) -> Option<&Entity<FilesPanel>> {
        self.files.as_ref()
    }

    pub fn files_folded(&self) -> bool {
        self.saved.files_folded
    }

    pub fn chats_folded(&self) -> bool {
        self.saved.chats_folded
    }

    pub fn runs_open(&self) -> bool {
        self.saved.runs_open
    }

    /// Folds or unfolds the sidebar's Files or Chats section, or lists the
    /// chats PilotHouse started.
    pub fn set_sidebar_folds(
        &mut self,
        files_folded: bool,
        chats_folded: bool,
        runs_open: bool,
        cx: &mut Context<Self>,
    ) {
        self.saved.files_folded = files_folded;
        self.saved.chats_folded = chats_folded;
        self.saved.runs_open = runs_open;
        self.save_layout(cx);
        cx.notify();
    }

    /// One watcher per folder, in the order of the workspace's roots. Their
    /// changes are passed on as [`ProjectSpaceEvent::FilesChanged`].
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
        if self.context_open {
            self.ensure_panel(self.context_tab, window, cx);
        }
        if !self.project.empty {
            self.files(window, cx);
        }
        cx.notify();
    }

    /// The changes panel, created on first use.
    pub fn changes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ChangesPanel> {
        if let Some(changes) = self.changes.get(&self.git_folder) {
            return changes.clone();
        }
        let watcher = self.watchers(cx)[self.git_folder.min(self.watchers.len() - 1)].clone();
        let repo = self.repo(window, cx);
        let writer = ph_agent::oneshot::writer(AppState::store(cx).clone());
        let changes = cx.new(|cx| {
            let mut changes = ChangesPanel::new(&watcher, repo, window, cx);
            changes.set_writer(writer, cx);
            changes
        });
        // The tool rail shows the change count.
        self._subscriptions
            .push(cx.observe(&changes, |_, _, cx| cx.notify()));
        self._subscriptions.push(cx.subscribe_in(
            &changes,
            window,
            |this, _, event: &ChangesPanelEvent, window, cx| match event {
                ChangesPanelEvent::OpenFile { path, line } => {
                    // The path is in the Git tab's folder; Files names it
                    // with that folder in a multi-folder workspace.
                    let roots = this.roots();
                    let path = roots
                        .list()
                        .get(this.git_folder)
                        .and_then(|root| roots.path_of(&root.path.join(path)))
                        .unwrap_or_else(|| path.clone());
                    this.open_file(&path, Some(*line), window, cx);
                }
            },
        ));
        self.changes.insert(self.git_folder, changes.clone());
        changes
    }

    /// The board panel, created on first use.
    pub fn board(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<BoardPanel> {
        if let Some(board) = &self.board {
            return board.clone();
        }
        let store = AppState::store(cx).clone();
        let project_id = self.project.id;
        let board = cx.new(|cx| BoardPanel::new(store, project_id, cx));
        self._subscriptions.push(cx.subscribe_in(
            &board,
            window,
            |this, _, event: &BoardPanelEvent, window, cx| match event {
                BoardPanelEvent::SendToAgent { prompt, title } => {
                    let (prompt, title) = (prompt.clone(), title.clone());
                    this.chat.update(cx, |chat, cx| {
                        chat.send_text(prompt, Some(title), window, cx)
                    });
                }
            },
        ));
        self.board = Some(board.clone());
        board
    }

    /// The browser pane, created on first use (Chrome starts when it is shown).
    pub fn browser(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<BrowserPanel> {
        if let Some(browser) = &self.browser {
            return browser.clone();
        }
        let project_id = self.project.id;
        let browser = cx.new(|cx| BrowserPanel::new(project_id, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &browser,
            window,
            |this, _, event: &ToChat, window, cx| this.receive_to_chat(event, window, cx),
        ));
        self.browser = Some(browser.clone());
        browser
    }

    /// The devices pane, created on first use.
    pub fn devices(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<DevicesPanel> {
        if let Some(devices) = &self.devices {
            return devices.clone();
        }
        let devices = cx.new(|cx| DevicesPanel::new(window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &devices,
            window,
            |this, _, event: &ToChat, window, cx| this.receive_to_chat(event, window, cx),
        ));
        self.devices = Some(devices.clone());
        devices
    }

    /// The Run tab, created on first use.
    /// The pull requests panel, created on first use.
    /// The Tests tab, created on first use.
    pub fn tests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ph_uitest::TestsPanel> {
        if let Some(tests) = &self.tests {
            return tests.clone();
        }
        let (project_id, root) = (self.project.id, self.project.path.clone());
        let tests = cx.new(|cx| ph_uitest::TestsPanel::new(project_id, root, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &tests,
            window,
            |this, _, event: &ph_uitest::TestsPanelEvent, window, cx| match event {
                ph_uitest::TestsPanelEvent::Agent { title, prompt } => {
                    this.reveal_chat(cx);
                    let (title, prompt) = (title.clone(), prompt.clone());
                    this.chat.update(cx, |chat, cx| {
                        chat.start_tool_task(title, prompt, window, cx)
                    });
                }
                ph_uitest::TestsPanelEvent::Card {
                    title,
                    description,
                    test,
                } => {
                    let card = ph_store::board::NewCard {
                        title: title.clone(),
                        description: description.clone(),
                        acceptance: "The UI test passes again.".into(),
                    };
                    let store = AppState::store(cx).clone();
                    // The card can only be Done once the test passes again.
                    let added = store.add_cards(this.project.id, &[card]).and_then(|cards| {
                        cards
                            .first()
                            .map_or(Ok(()), |card| store.link_card_test(card.id, test, true))
                    });
                    let note = match added {
                        Ok(()) => {
                            ph_board::BoardHub::notify(this.project.id, cx);
                            Notification::success(
                                "Added a card to the board; it is Done once the test passes.",
                            )
                        }
                        Err(err) => Notification::error(format!("Could not add the card: {err:#}")),
                    };
                    window.push_notification(note, cx);
                }
                ph_uitest::TestsPanelEvent::OpenFile(path) => {
                    if let Ok(relative) = path.strip_prefix(&this.project.path) {
                        let relative = relative.to_string_lossy().replace('\\', "/");
                        this.open_file(&relative, None, window, cx);
                    }
                }
            },
        ));
        self.tests = Some(tests.clone());
        tests
    }

    pub fn pulls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PullsPanel> {
        if let Some(pulls) = self.pulls.get(&self.git_folder) {
            return pulls.clone();
        }
        let repo = self.repo(window, cx);
        let store = AppState::store(cx).clone();
        let writer = ph_agent::oneshot::writer(store.clone());
        let pulls = cx.new(|cx| {
            let mut pulls = PullsPanel::new(store, repo, cx);
            pulls.set_writer(writer);
            pulls
        });
        self._subscriptions.push(cx.subscribe_in(
            &pulls,
            window,
            |this, _, event: &PullsPanelEvent, window, cx| match event {
                PullsPanelEvent::OpenSettings => {
                    window.dispatch_action(Box::new(crate::actions::OpenSettings), cx)
                }
                PullsPanelEvent::ReviewWithAgent { hosting, pull } => {
                    this.review_with_agent((**hosting).clone(), (**pull).clone(), window, cx)
                }
                PullsPanelEvent::OpenSession(id) => {
                    let session = AppState::store(cx).session(*id).ok().flatten();
                    if session.is_some() {
                        if this.zoomed {
                            this.toggle_zoom(window, cx);
                        }
                        this.chat
                            .update(cx, |chat, cx| chat.set_session(session, cx));
                    }
                }
            },
        ));
        self.pulls.insert(self.git_folder, pulls.clone());
        pulls
    }

    /// Loads the pull requests of the Git folder in view and keeps the chat
    /// told, for `@pr:` mentions.
    fn feed_pulls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.folders().is_empty() {
            return;
        }
        let pulls = self.pulls(window, cx);
        let chat = self.chat.downgrade();
        self._subscriptions
            .push(cx.observe(&pulls, move |_, pulls, cx| {
                let briefs = briefs(pulls.read(cx).list());
                _ = chat.update(cx, |chat, cx| chat.set_pulls(briefs, cx));
            }));
        let briefs = briefs(pulls.read(cx).list());
        if briefs.is_empty() {
            pulls.update(cx, |pulls, cx| pulls.reload(cx));
        }
        self.chat.update(cx, |chat, cx| chat.set_pulls(briefs, cx));
    }

    /// ⌘L in a terminal: its selection (or screen) goes to the chat.
    fn watch_terminal(
        &mut self,
        terminal: &Entity<TerminalPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions.push(cx.subscribe_in(
            terminal,
            window,
            |this, _, event: &ph_terminal::TerminalPanelEvent, window, cx| {
                let ph_terminal::TerminalPanelEvent::Ask { terminal, text } = event;
                this.reveal_chat(cx);
                let attachment = Attachment::Text {
                    label: terminal.clone(),
                    text: format!("{terminal}:\n```\n{text}\n```"),
                };
                this.chat
                    .update(cx, |chat, cx| chat.ask_about(attachment, None, window, cx));
                cx.notify();
            },
        ));
    }

    /// Selected code goes to the chat: attached, with a prompt for the
    /// chosen intent (or left for the user to finish typing).
    fn ask_about_code(&mut self, ask: ph_files::Ask, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_chat(cx);
        let attachment = Attachment::Text {
            label: ask.label(),
            text: ask.attachment_text(),
        };
        let prompt = ask.prompt().map(|prompt| (ask.title(), prompt));
        self.chat.update(cx, |chat, cx| {
            chat.ask_about(attachment, prompt, window, cx)
        });
        cx.notify();
    }

    /// Starts an agent review of a pull request: checks it out in its own
    /// worktree, then opens a review chat there that starts by itself.
    fn review_with_agent(
        &mut self,
        hosting: ph_pulls::Hosting,
        pull: ph_github::models::PullRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.push_notification(
            Notification::info(format!("Checking out #{} for the review…", pull.number)),
            cx,
        );
        let store = AppState::store(cx).clone();
        let (project_id, root) = (self.project.id, self.project.path.clone());
        let work = cx.background_spawn(async move {
            crate::review::prepare(&store, project_id, &root, &hosting, &pull)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok(prepared) => {
                    this.reveal_chat(cx);
                    this.chat.update(cx, |chat, cx| {
                        chat.start_review(
                            prepared.review_id,
                            prepared.worktree,
                            prepared.title,
                            prepared.display,
                            prepared.prompt,
                            window,
                            cx,
                        )
                    });
                }
                Err(err) => window.push_notification(
                    Notification::error(format!("{err:#}")).title("Could not start the review"),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// A new run of review `review_id`'s pull request, on its latest head.
    fn review_again(&mut self, review_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let store = AppState::store(cx).clone();
        let root = self.project.path.clone();
        let find = cx.background_spawn(async move {
            let review = store
                .pr_review(review_id)?
                .ok_or_else(|| anyhow::anyhow!("review not found"))?;
            let hosting = ph_pulls::hosting::resolve(&root, &store)
                .map_err(|missing| anyhow::anyhow!("{missing:?}"))?;
            let pull = ph_github::pulls::get(&hosting.client, &hosting.repo, review.pull.number)?;
            anyhow::Ok((hosting, pull))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = find.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok((hosting, pull)) => this.review_with_agent(hosting, pull, window, cx),
                Err(err) => window.push_notification(Notification::error(format!("{err:#}")), cx),
            });
        })
        .detach();
    }

    pub fn runs(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<RunsPanel> {
        if let Some(runs) = &self.runs {
            return runs.clone();
        }
        let (store, id, folders) = (
            AppState::store(cx).clone(),
            self.project.id,
            self.named_folders(),
        );
        let runs = cx.new(|cx| RunsPanel::new(store, id, folders, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &runs,
            window,
            |this, _, event: &RunsPanelEvent, window, cx| match event {
                RunsPanelEvent::Run { name, command, cwd } => {
                    let terminal = this.terminal_for_run(window, cx);
                    terminal.update(cx, |terminal, cx| {
                        terminal.run_in(name, command, cwd.clone(), window, cx)
                    });
                }
                RunsPanelEvent::OpenUrl(url) => {
                    this.show_tab(ContextTab::Browser, window, cx);
                    let browser = ph_browser::BrowserHub::browser(this.project.id, cx);
                    browser.update(cx, |browser, cx| browser.open(url, cx));
                }
            },
        ));
        self.runs = Some(runs.clone());
        runs
    }

    /// Captures, picked elements and log lines from a live pane.
    fn receive_to_chat(&mut self, item: &ToChat, window: &mut Window, cx: &mut Context<Self>) {
        let attachments = match item {
            ToChat::Captured { images, note } => images
                .iter()
                .map(|image| Attachment::Image {
                    label: image.label.clone(),
                    path: image.path.clone(),
                })
                .chain(note.iter().map(|note| Attachment::Text {
                    label: format!(
                            "About the {}",
                            images
                                .first()
                                .map_or("capture", |image| image.label.as_str())
                        ),
                    text: note.clone(),
                }))
                .collect(),
            ToChat::Text { label, text } => vec![Attachment::Text {
                label: label.clone(),
                text: text.clone(),
            }],
        };
        self.attach_to_chat(attachments, window, cx);
    }

    /// Adds attachments to the composer, bringing the chat into view.
    fn attach_to_chat(
        &mut self,
        attachments: Vec<Attachment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_chat(cx);
        self.chat.update(cx, |chat, cx| {
            for attachment in attachments {
                chat.attach(attachment, window, cx);
            }
        });
        cx.notify();
    }

    /// ⌘⇧S: send the visible browser or device (with its marks) to chat.
    pub fn capture_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tools = match self.context_tab {
            _ if !self.context_open => None,
            ContextTab::Browser => self
                .browser
                .as_ref()
                .map(|browser| browser.read(cx).tools().clone()),
            ContextTab::Devices => self
                .devices
                .as_ref()
                .map(|devices| devices.read(cx).tools().clone()),
            _ => None,
        };
        match tools {
            Some(tools) => tools.update(cx, |tools, cx| tools.capture(cx)),
            None => {
                if let Some(attachment) = self.live_screenshot(cx) {
                    self.attach_to_chat(vec![attachment], window, cx);
                }
            }
        }
    }

    /// For `@screenshot`: the visible live pane first, else whichever of the
    /// browser and device has a picture.
    fn live_screenshot(&self, cx: &gpui_kit::App) -> Option<Attachment> {
        let browser = || Some(("browser", self.browser.as_ref()?.read(cx).composite(cx)?));
        let device = || Some(("device", self.devices.as_ref()?.read(cx).composite(cx)?));
        let (source, frame) = if self.context_tab == ContextTab::Devices {
            device().or_else(browser)
        } else {
            browser().or_else(device)
        }?;
        let path = ph_media::capture::save_png(&frame, source).ok()?;
        Some(Attachment::Image {
            label: format!("{source} screenshot"),
            path,
        })
    }

    /// For `@log`: browser console errors and failed requests, and logcat errors.
    fn live_logs(&self, cx: &gpui_kit::App) -> Option<Attachment> {
        let browser = self
            .browser
            .as_ref()
            .and_then(|browser| browser.read(cx).recent_logs(cx));
        let device = self
            .devices
            .as_ref()
            .and_then(|devices| devices.read(cx).recent_logs(cx));
        let text: Vec<String> = browser.into_iter().chain(device).collect();
        (!text.is_empty()).then(|| Attachment::Text {
            label: "logs".into(),
            text: text.join("\n"),
        })
    }

    /// ⌘P from anywhere: the file finder.
    pub fn quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.files(window, cx);
        files.update(cx, |files, cx| files.quick_open(window, cx));
    }

    /// ⌘⇧F from anywhere: the sidebar's tree turns into workspace search.
    pub fn find_in_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_files(cx);
        let files = self.files(window, cx);
        files.update(cx, |files, cx| files.find_in_project(window, cx));
    }

    /// Shows the sidebar's file tree, unfolded.
    pub fn reveal_files(&mut self, cx: &mut Context<Self>) {
        if self.saved.files_folded {
            let (chats, runs) = (self.saved.chats_folded, self.saved.runs_open);
            self.set_sidebar_folds(false, chats, runs, cx);
        }
        cx.emit(ProjectSpaceEvent::RevealFiles);
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
        self.zoomed = false;
        self.front = Front::Editor;
        let files = self.files(window, cx);
        files.update(cx, |files, cx| files.open_file(relative, line, window, cx));
    }

    /// Hands the architecture map to the chat, asking for an explanation.
    pub fn explain_architecture(
        &mut self,
        map: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_chat(cx);
        let attachment = Attachment::Text {
            label: "Architecture map".into(),
            text: map,
        };
        let prompt = "Using the attached map of this project's parts and the code itself, explain its architecture: \
                      what each part is for, how they work together (the main flows of data and control), and where a \
                      new contributor should start. Be concise; use a short list per part."
            .to_string();
        self.chat.update(cx, |chat, cx| {
            chat.ask_about(
                attachment,
                Some(("Explain the architecture".into(), prompt)),
                window,
                cx,
            )
        });
    }

    /// Starts the chats of a parallel run whose worktrees are ready.
    pub fn run_parallel(
        &mut self,
        prompt: &str,
        agents: Vec<&'static ph_agent::agents::AgentSpec>,
        lanes: Vec<(i64, std::path::PathBuf)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_chat(cx);
        let title: String = prompt
            .lines()
            .next()
            .unwrap_or(prompt)
            .chars()
            .take(50)
            .collect();
        let starts = agents
            .iter()
            .copied()
            .zip(lanes.iter().map(|(_, path)| path.clone()))
            .collect();
        let sessions = self.chat.update(cx, |chat, cx| {
            chat.start_parallel(starts, &title, prompt, window, cx)
        });
        let store = AppState::store(cx);
        for ((lane, _), session) in lanes.iter().zip(sessions) {
            if let Some(session) = session {
                let _ = store.set_lane_session(*lane, session);
            }
        }
        self.reload_sessions(cx);
        let names: Vec<_> = agents.iter().map(|agent| agent.name).collect();
        window.push_notification(
            Notification::info(format!(
                "{} are working side by side. Compare and keep one in Parallel runs (⌘K).",
                names.join(" and ")
            )),
            cx,
        );
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
        cx.emit(ProjectSpaceEvent::SessionsChanged);
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
        cx.emit(ProjectSpaceEvent::SessionsChanged);
        cx.notify();
    }

    /// Re-reads sessions after the chat created or renamed one.
    fn reload_sessions(&mut self, cx: &mut Context<Self>) {
        self.sessions = AppState::store(cx)
            .sessions(self.project.id)
            .unwrap_or_default();
        self.active_session = self.chat.read(cx).active_session_id(cx);
        self.save_layout(cx);
        cx.emit(ProjectSpaceEvent::SessionsChanged);
        cx.notify();
    }

    fn ensure_panel(&mut self, tab: ContextTab, window: &mut Window, cx: &mut Context<Self>) {
        match tab {
            ContextTab::Board => {
                self.board(window, cx);
            }
            ContextTab::Terminal => {
                if self.terminal.is_none() {
                    // The first time a workspace's Terminal tab opens, a
                    // shell is ready in it.
                    let first = ph_terminal::TerminalHub::existing(self.project.id, cx).is_none();
                    let terminal = self.terminal_panel(window, cx);
                    if first {
                        terminal.update(cx, |terminal, cx| terminal.new_terminal(window, cx));
                    }
                }
            }
            ContextTab::Changes => {
                self.changes(window, cx);
            }
            ContextTab::Browser => {
                self.browser(window, cx);
            }
            ContextTab::Devices => {
                self.devices(window, cx);
            }
            ContextTab::Runs => {
                self.runs(window, cx);
            }
            ContextTab::Pulls => {
                self.pulls(window, cx);
            }
            ContextTab::Tests => {
                self.tests(window, cx);
            }
        }
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
            context_panel_open: Some(self.context_open),
            session_id: self.active_session,
            open_files,
            active_file,
            ..self.saved.clone()
        };
        if let Err(err) = AppState::store(cx).save_project_ui_state(self.project.id, &self.saved) {
            tracing::warn!(
                "saving layout for project {} failed: {err:#}",
                self.project.id
            );
        }
    }

    /// The chat, under what finished while the workspace was away.
    fn render_chat(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(away) = &self.away else {
            return self.chat.clone().into_any_element();
        };
        v_flex()
            .size_full()
            .child(self.render_away(away, cx))
            .child(div().flex_1().min_h_0().child(self.chat.clone()))
            .into_any_element()
    }

    /// "While you were away": each chat that finished and the test run, one
    /// click to see it.
    fn render_away(&self, away: &Away, cx: &mut Context<Self>) -> impl IntoElement {
        let chats = away.chats.iter().filter_map(|id| {
            let session = self.sessions.iter().find(|session| session.id == *id)?;
            let id = *id;
            Some(
                Button::new(("away-chat", id as u64))
                    .outline()
                    .xsmall()
                    .icon(Icons::MessageSquare)
                    .label(short(&session.title, 32))
                    .tooltip("Show this chat")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_session(id, cx);
                        this.dismiss_away(cx);
                    })),
            )
        });
        let tests = away.tests.then(|| {
            let failed = ph_uitest::runner::TestRunHub::existing(self.project.id, cx)
                .is_some_and(|runs| runs.read(cx).failed());
            Button::new("away-tests")
                .outline()
                .xsmall()
                .icon(Icons::FlaskConical)
                .label(if failed {
                    "UI test failed"
                } else {
                    "UI test run done"
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_tab(ContextTab::Tests, window, cx);
                    this.dismiss_away(cx);
                }))
        });
        h_flex()
            .id("while-away")
            .flex_none()
            .flex_wrap()
            .gap_1p5()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.5))
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child("While you were away"),
            )
            .children(chats)
            .children(tests)
            .child(div().flex_1())
            .child(
                Button::new("away-dismiss")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Dismiss")
                    .accessibility_label("Dismiss")
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_away(cx))),
            )
    }

    fn dismiss_away(&mut self, cx: &mut Context<Self>) {
        self.away = None;
        cx.notify();
    }

    /// The open files, beside the chat.
    fn render_editor(&self) -> Option<AnyElement> {
        Some(Surface::new().child(self.files.clone()?).into_any_element())
    }

    /// The editor folded to a strip (a narrow window showing a tool): one
    /// click brings the files back.
    fn render_editor_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self
            .files
            .as_ref()
            .map(|files| files.read(cx).open_file_paths(cx).len())
            .unwrap_or_default();
        v_flex()
            .id("editor-strip")
            .flex_none()
            .w(px(36.))
            .h_full()
            .mr_2()
            .py_2()
            .gap_1()
            .items_center()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().secondary_hover))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(format!("Show open files ({count})"))
                    .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.front = Front::Editor;
                cx.notify();
            }))
            .child(Icon::new(Icons::FileCode).small())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count.to_string()),
            )
            .into_any_element()
    }

    fn render_context_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.context_tab;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .pl_3()
                    .pr_1p5()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(Icon::new(tab.icon()).small().text_color(cx.theme().info))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(tab.label()),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("zoom-context-panel")
                            .ghost()
                            .small()
                            .icon(if self.zoomed {
                                IconName::Minimize
                            } else {
                                IconName::Maximize
                            })
                            .tooltip(if self.zoomed {
                                "Restore chat (⌘⇧J)"
                            } else {
                                "Maximize (⌘⇧J)"
                            })
                            .accessibility_label("Maximize panel")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_zoom(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("close-context-panel")
                            .ghost()
                            .small()
                            .icon(Icons::PanelRightClose)
                            .tooltip("Hide (⌘J)")
                            .accessibility_label("Hide panel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_context_panel(window, cx)
                            })),
                    ),
            )
            // Absolutely filled, so each panel gets an exact height (a
            // percentage height inside a flex item resolves too tall).
            .child(
                div().relative().flex_1().min_h_0().child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(self.render_tab_content(cx)),
                ),
            )
            .into_any_element()
    }

    /// The tools down the window's right edge: icons, with names in their
    /// tooltips (or under them, in Settings).
    fn render_tool_rail(&self, shown: bool, cx: &mut Context<Self>) -> AnyElement {
        let space = cx.entity();
        let labels = ph_theme::preferences::preferences(cx).tool_labels;
        let changes = self
            .changes
            .get(&self.git_folder)
            .map(|changes| changes.read(cx).change_count())
            .unwrap_or_default();
        let (shown_tabs, more): (Vec<ContextTab>, Vec<ContextTab>) = AppState::enabled_tabs(cx)
            .into_iter()
            .partition(|tab| self.is_relevant(*tab));
        let more_menu = (!more.is_empty()).then(|| {
            let space = space.clone();
            Button::new("more-tools")
                .ghost()
                .small()
                .icon(IconName::Ellipsis)
                .tooltip("More tools")
                .accessibility_label("More tools")
                .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    more.iter().fold(menu, |menu, tab| {
                        let (space, tab) = (space.clone(), *tab);
                        menu.item(
                            gpui_kit::component::menu::PopupMenuItem::new(tab.label())
                                .icon(Icon::new(tab.icon()))
                                .on_click(move |_, window, cx| {
                                    space.update(cx, |space, cx| space.show_tab(tab, window, cx))
                                }),
                        )
                    })
                })
        });
        v_flex()
            .id("context-tabs")
            .flex_none()
            .w(tool_rail_width(labels))
            .h_full()
            .py_1()
            .gap_0p5()
            .items_center()
            .children(shown_tabs.into_iter().map(|tab| {
                let selected = shown && tab == self.context_tab;
                let space = space.clone();
                let badge = (tab == ContextTab::Changes && changes > 0).then(|| {
                    div()
                        .absolute()
                        .top(px(3.))
                        .right(px(3.))
                        .min_w(px(14.))
                        .h(px(14.))
                        .px(px(3.))
                        .rounded_full()
                        .bg(cx.theme().info)
                        .text_color(cx.theme().info_foreground)
                        .text_size(px(9.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if changes > 99 {
                            "99+".to_string()
                        } else {
                            changes.to_string()
                        })
                });
                let hint = tab_shortcut(tab);
                v_flex()
                    .id(SharedString::from(format!("context-tab-{}", tab.key())))
                    .role(gpui_kit::accesskit::Role::Tab)
                    .aria_label(tab.label())
                    .relative()
                    .w(tool_rail_width(labels) - px(6.))
                    .when(labels, |this| this.py_1p5().gap_1())
                    .when(!labels, |this| this.h(px(34.)))
                    .items_center()
                    .justify_center()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .text_color(if selected {
                        cx.theme().info
                    } else {
                        cx.theme().muted_foreground
                    })
                    .when(selected, |this| this.bg(cx.theme().info.opacity(0.14)))
                    .when(!selected, |this| {
                        this.hover(|style| {
                            style
                                .bg(cx.theme().secondary_hover)
                                .text_color(cx.theme().foreground)
                        })
                    })
                    .when(!labels, |this| {
                        let label = format!("{} ({hint})", tab.label());
                        this.tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(label.clone())
                                .build(window, cx)
                        })
                    })
                    .on_click(move |_, window, cx| {
                        space.update(cx, |space, cx| space.show_tab(tab, window, cx))
                    })
                    .child(Icon::new(tab.icon()).size(px(18.)))
                    .when(labels, |this| {
                        this.child(
                            div()
                                .text_size(px(10.))
                                .when(selected, |this| {
                                    this.font_weight(gpui_kit::FontWeight::MEDIUM)
                                })
                                .child(tab.label()),
                        )
                    })
                    .children(badge)
                    // Findable by label in UI tests and automation.
                    .test_support()
            }))
            .children(more_menu)
            .into_any_element()
    }

    /// The open tab's panel. Panels are created by `ensure_panel` before
    /// their tab is shown, so `None` only flashes for a frame at most.
    /// The Terminal tab, shown (and turned on, since runs need it), for a
    /// command that opens its own terminal; a first one opens no spare shell.
    fn terminal_for_run(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        if !AppState::enabled_tabs(cx).contains(&ContextTab::Terminal) {
            AppState::set_tab_enabled(ContextTab::Terminal, true, cx);
        }
        let terminal = self.terminal_panel(window, cx);
        self.show_tab(ContextTab::Terminal, window, cx);
        terminal
    }

    /// The Terminal tab's panel, over the workspace's shells (which outlive
    /// it), made when first needed.
    fn terminal_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        if let Some(terminal) = self.terminal.clone() {
            return terminal;
        }
        let terminals =
            ph_terminal::TerminalHub::terminals(self.project.id, self.project.path.clone(), cx);
        let (cwd, folders) = (self.project.path.clone(), self.named_folders());
        terminals.update(cx, |terminals, _| terminals.set_folders(cwd, folders));
        let terminal = cx.new(|cx| TerminalPanel::new(terminals, cx));
        self.watch_terminal(&terminal, window, cx);
        self.terminal = Some(terminal.clone());
        terminal
    }

    /// Starts an agent CLI that has no chat in its own terminal.
    pub fn start_cli(
        &mut self,
        agent: &ph_terminal::cli_agents::CliAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal = self.terminal_for_run(window, cx);
        terminal.update(cx, |terminal, cx| terminal.start_cli(agent, window, cx));
    }

    fn render_tab_content(&self, cx: &mut Context<Self>) -> AnyElement {
        // Git and PRs need a folder; say so instead of an empty tool.
        if self.project.folders().is_empty()
            && matches!(self.context_tab, ContextTab::Changes | ContextTab::Pulls)
        {
            return ph_ui::EmptyState::new("No folders in this workspace")
                .icon(Icons::FolderPlus)
                .description(
                    "Add a folder to see its Git changes, branches and pull requests. Chat, the board, the browser and devices work without one.",
                )
                .action(
                    Button::new("empty-add-folder")
                        .primary()
                        .icon(Icons::FolderPlus)
                        .label("Add a folder")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::AddFolderToWorkspace), cx)
                        }),
                )
                .into_any_element();
        }
        let panel = match self.context_tab {
            ContextTab::Changes => self
                .changes
                .get(&self.git_folder)
                .cloned()
                .map(|panel| self.with_folder_picker(panel.into_any_element(), cx)),
            ContextTab::Board => self.board.clone().map(IntoElement::into_any_element),
            ContextTab::Terminal => self.terminal.clone().map(IntoElement::into_any_element),
            ContextTab::Browser => self.browser.clone().map(IntoElement::into_any_element),
            ContextTab::Devices => self.devices.clone().map(IntoElement::into_any_element),
            ContextTab::Runs => self.runs.clone().map(IntoElement::into_any_element),
            ContextTab::Pulls => self
                .pulls
                .get(&self.git_folder)
                .cloned()
                .map(|panel| self.with_folder_picker(panel.into_any_element(), cx)),
            ContextTab::Tests => self.tests.clone().map(IntoElement::into_any_element),
        };
        panel.unwrap_or_else(|| div().into_any_element())
    }

    /// Git and PRs work on one folder at a time: with several folders, a
    /// row on top says which and switches it.
    fn with_folder_picker(&self, panel: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let roots = self.roots();
        let folders: Vec<(usize, String, bool)> = roots
            .list()
            .iter()
            .enumerate()
            .skip(usize::from(self.project.empty))
            .map(|(ix, root)| (ix, root.name.clone(), ph_git::is_repository(&root.path)))
            .collect();
        if folders.len() < 2 {
            return panel;
        }
        let current = folders
            .iter()
            .find(|(ix, ..)| *ix == self.git_folder)
            .map(|(_, name, _)| name.clone())
            .unwrap_or_default();
        let space = cx.entity().downgrade();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Folder")
                    .child(
                        Button::new("git-folder")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Folder)
                            .label(current)
                            .dropdown_caret(true)
                            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                                folders.iter().fold(menu, |menu, (ix, name, repo)| {
                                    let (space, ix) = (space.clone(), *ix);
                                    let label = if *repo {
                                        name.clone()
                                    } else {
                                        format!("{name} (no Git)")
                                    };
                                    menu.item(
                                        gpui_kit::component::menu::PopupMenuItem::new(label)
                                            .on_click(move |_, window, cx| {
                                                _ = space.update(cx, |space, cx| {
                                                    space.set_git_folder(ix, window, cx)
                                                });
                                            }),
                                    )
                                })
                            }),
                    ),
            )
            .child(div().flex_1().min_h_0().child(panel))
            .into_any_element()
    }
}

impl Render for ProjectSpace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let labels = ph_theme::preferences::preferences(cx).tool_labels;
        let sidebar = if self.sidebar_shown {
            layout::SIDEBAR_WIDTH
        } else {
            px(0.)
        };
        // Room for the columns: the window less the sidebar, the tool icons
        // and the margins.
        let available = window.viewport_size().width - sidebar - tool_rail_width(labels) - px(16.);
        let editor_open = self
            .files
            .as_ref()
            .is_some_and(|files| files.read(cx).has_open_files());
        let tools_open = self.context_open;
        let narrow = available < layout::EDITOR_MIN + layout::CHAT_MIN + layout::CONTEXT_PANEL_MIN;
        // A narrow window shows the file or the tool opened last; the
        // other folds to the side.
        let (show_editor, show_tools, editor_strip) = match (editor_open, tools_open) {
            (true, true) if narrow => match self.front {
                Front::Editor => (true, false, false),
                Front::Tools => (false, true, true),
            },
            (editor, tools) => (editor, tools, false),
        };
        let rail = self.render_tool_rail(show_tools, cx);
        let shell = h_flex().size_full().pr_1().pb_2().items_stretch();

        if self.zoomed && tools_open {
            let tools = Surface::new().child(self.render_context_panel(cx));
            return shell
                .child(div().flex_1().min_w_0().h_full().pr_1().child(tools))
                .child(rail)
                .into_any_element();
        }

        let chat = Surface::new().child(self.render_chat(cx));
        let split_id = SharedString::from(format!(
            "space-{}-{}{}",
            self.project.id,
            u8::from(show_editor),
            u8::from(show_tools)
        ));
        let editor_width =
            (available * if show_tools { 0.38 } else { 0.5 }).max(layout::EDITOR_MIN);
        let tools_width =
            (available - layout::CHAT_MIN - if show_editor { editor_width } else { px(0.) })
                .min(layout::CONTEXT_PANEL_DEFAULT)
                .max(layout::CONTEXT_PANEL_MIN);
        let mut split = h_resizable(split_id);
        if show_editor && let Some(editor) = self.render_editor() {
            split = split.child(
                resizable_panel()
                    .size(editor_width)
                    .size_range(layout::EDITOR_MIN..gpui_kit::Pixels::MAX)
                    .child(div().size_full().pr_1().child(editor)),
            );
        }
        split = split.child(
            resizable_panel()
                .size_range(layout::CHAT_MIN..gpui_kit::Pixels::MAX)
                .child(div().size_full().px_1().child(chat)),
        );
        if show_tools {
            let tools = Surface::new().child(self.render_context_panel(cx));
            split = split.child(
                resizable_panel()
                    .size(tools_width)
                    .size_range(layout::CONTEXT_PANEL_MIN..layout::CONTEXT_PANEL_MAX)
                    .child(div().size_full().pl_1().child(tools)),
            );
        }
        shell
            .when(editor_strip, |this| {
                this.child(self.render_editor_strip(cx))
            })
            .child(div().flex_1().min_w_0().h_full().child(split))
            .child(rail)
            .into_any_element()
    }
}

/// The tool icons' column: icons alone, or wider with their names.
fn tool_rail_width(labels: bool) -> gpui_kit::Pixels {
    if labels { px(64.) } else { px(42.) }
}

/// The shortcut that shows `tab` (see `actions::bind_keys`).
fn tab_shortcut(tab: ContextTab) -> String {
    let key = match tab {
        ContextTab::Changes => 2,
        ContextTab::Board => 3,
        ContextTab::Terminal => 4,
        ContextTab::Browser => 5,
        ContextTab::Devices => 6,
        ContextTab::Runs => 7,
        ContextTab::Pulls => 8,
        ContextTab::Tests => 9,
    };
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// `text` cut to `max` characters, with an ellipsis.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// `@test-run`: how each UI test of the workspace last went, and the screen
/// of the first failing step.
fn test_results(
    project_id: ph_store::projects::ProjectId,
    root: &std::path::Path,
) -> Vec<ph_agent::Attachment> {
    let tests = ph_uitest::model::list(root);
    if tests.is_empty() {
        return vec![ph_agent::Attachment::Text {
            label: "UI tests".into(),
            text: "This workspace has no UI tests yet.".into(),
        }];
    }
    let mut lines = vec!["UI tests in this workspace, by their newest run:".to_string()];
    let mut failing_screen = None;
    for (_, test) in &tests {
        let slug = ph_uitest::model::slug(&test.name);
        let reports = ph_uitest::run::last_reports(project_id, &slug);
        if reports.is_empty() {
            lines.push(format!("- {}: not run yet", test.name));
            continue;
        }
        for (dir, report) in &reports {
            let on = report
                .variant
                .as_ref()
                .map(|variant| format!(" on {}", variant.label()))
                .unwrap_or_default();
            match report.failure() {
                Some((ix, step)) => {
                    lines.push(format!(
                        "- {}{on}: failed at step {} ({}): {} (report: {})",
                        test.name,
                        ix + 1,
                        step.step.describe(),
                        step.error.clone().unwrap_or_default(),
                        dir.display()
                    ));
                    if failing_screen.is_none()
                        && let Some(shot) = &step.screenshot
                    {
                        failing_screen = Some(ph_agent::Attachment::Image {
                            label: format!("{} step {}", test.name, ix + 1),
                            path: dir.join(shot),
                        });
                    }
                }
                None if report.looks_different() > 0 => lines.push(format!(
                    "- {}{on}: every step worked, but {} look different from the baseline",
                    test.name,
                    report.looks_different()
                )),
                None => lines.push(format!("- {}{on}: passed", test.name)),
            }
        }
    }
    let mut attachments = vec![ph_agent::Attachment::Text {
        label: "UI test results".into(),
        text: lines.join("\n"),
    }];
    attachments.extend(failing_screen);
    attachments
}

/// Open pull requests as `@pr:` mentions attach them.
fn briefs(pulls: &[ph_github::models::PullRequest]) -> Vec<ph_agent::PullBrief> {
    pulls
        .iter()
        .filter(|pull| pull.state == "open")
        .map(|pull| ph_agent::PullBrief {
            number: pull.number,
            title: pull.title.clone(),
            author: pull.user.login.clone(),
            branches: format!("{} → {}", pull.head.name, pull.base.name),
            url: pull.html_url.clone(),
            body: pull.body.clone().unwrap_or_default(),
        })
        .collect()
}
