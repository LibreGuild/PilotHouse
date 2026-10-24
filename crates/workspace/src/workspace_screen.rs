//! Everything shown for one open workspace: the editor column (while a file
//! is open), the chat, the tool panel and the icon bar that picks the tool,
//! plus what the window's sidebar shows for it (the file tree, the chats).
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
//! sends what they produce to the chat, `render` draws the screen.

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

mod panels;
mod render;
mod to_chat;

/// Emitted when something the sidebar or title bar shows has changed.
#[derive(Debug, Clone)]
pub enum WorkspaceScreenEvent {
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
                tests: Box::new(move |_| to_chat::test_results(tests_project, &tests_root)),
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

    /// Puts the keyboard in the chat's composer.
    pub fn focus_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    cx.emit(WorkspaceScreenEvent::MakePrimary(path.clone()))
                }
                FilesPanelEvent::RemoveFolder(path) => {
                    cx.emit(WorkspaceScreenEvent::RemoveFolder(path.clone()))
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
    pub fn files_height(&self) -> Option<f32> {
        self.saved.files_height
    }

    /// The Files/Chats divider was dragged: Files is now `height` tall.
    pub fn set_files_height(&mut self, height: f32, cx: &mut Context<Self>) {
        if self.saved.files_height != Some(height) {
            self.saved.files_height = Some(height);
            self.save_layout(cx);
        }
    }

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
        if self.context_open {
            self.ensure_panel(self.context_tab, window, cx);
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
        cx.emit(WorkspaceScreenEvent::RevealFiles);
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
}
