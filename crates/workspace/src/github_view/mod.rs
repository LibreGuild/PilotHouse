//! The GitHub view: what you would see on GitHub's own pages, apart from
//! any workspace. Its sidebar lists the pull requests waiting for you
//! across every repository (review requested, your own, assigned to you)
//! and the repositories you follow: those your workspaces' folders come
//! from, and any you add. The middle shows a list or a repository's pull
//! requests and one PR in full; the chat on the right is the repository's.
//!
//! Agent reviews run here, in the view's own clone of the repository
//! ([`ph_pulls::clones`]); the user's checkouts are never touched. Each
//! repository has a space of its own ([`ph_store::projects::ProjectKind::GitHub`])
//! that holds its chats, so they have what workspace chats have.
//!
//! `render` draws it.

use std::collections::{BTreeSet, HashMap};

use gpui_kit::component::{
    WindowExt as _,
    input::{InputEvent, InputState},
    notification::Notification,
};
use gpui_kit::{AppContext as _, Context, Entity, Subscription, Task, WeakEntity, Window};
use ph_agent::{ChatPanel, ChatPanelEvent};
use ph_github::Account;
use ph_github::models::PullRequest;
use ph_github::search::{FoundPull, Inbox};
use ph_pulls::clones::RepoId;
use ph_pulls::{Hosting, PullsPanel, PullsPanelEvent};
use ph_review::templates::Template;
use ph_store::Store;
use ph_store::projects::Project;
use ph_store::sessions::SessionId;

use crate::app_state::AppState;
use crate::workspace::Workspace;

mod render;

/// The repositories the user added (`host/owner/name` each).
const ADDED_KEY: &str = "github.repositories";

/// The inbox is read again when shown after this long.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(120);

/// What the middle shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Page {
    Inbox(Inbox),
    Repo(RepoId),
}

/// One inbox list as last read.
#[derive(Default)]
pub(crate) struct List {
    /// Each pull request with the host it is on.
    pub(crate) items: Vec<(String, FoundPull)>,
    pub(crate) error: Option<String>,
}

/// A repository in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoRow {
    pub(crate) id: RepoId,
    /// Added by the user (they can take it out again).
    pub(crate) added: bool,
    /// The workspaces with a folder from it.
    pub(crate) workspaces: Vec<String>,
}

pub struct GithubView {
    store: Store,
    workspace: WeakEntity<Workspace>,
    pub(crate) accounts: Vec<Account>,
    pub(crate) inbox: HashMap<Inbox, List>,
    pub(crate) loading: bool,
    loaded_at: Option<std::time::Instant>,
    /// The workspaces' names and folders, as the window last gave them.
    folders: Vec<(String, Vec<std::path::PathBuf>)>,
    /// Repositories of the workspaces' folders, as last looked up.
    from_workspaces: Vec<(RepoId, String)>,
    pub(crate) page: Page,
    /// Each repository's pull requests (and the PR open in it).
    panels: HashMap<RepoId, (Entity<PullsPanel>, Subscription, Subscription)>,
    /// Each repository space's chat, and the pull request it is about (the
    /// one open in that repository's panel).
    chats: HashMap<RepoId, (Entity<ChatPanel>, Subscription)>,
    attached: HashMap<RepoId, u64>,
    /// The "add a repository" box, while open.
    pub(crate) adding: Option<(Entity<InputState>, Subscription)>,
    pub(crate) sidebar_shown: bool,
    _load: Option<Task<()>>,
    _work: Vec<Task<()>>,
}

impl GithubView {
    /// `folders`: each workspace's name and folders (their remotes are the
    /// sidebar's repositories).
    pub fn new(
        workspace: WeakEntity<Workspace>,
        folders: Vec<(String, Vec<std::path::PathBuf>)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = AppState::store(cx).clone();
        let mut view = Self {
            accounts: ph_github::accounts::accounts(&store),
            store,
            workspace,
            inbox: HashMap::new(),
            loading: false,
            loaded_at: None,
            folders,
            from_workspaces: Vec::new(),
            page: Page::Inbox(Inbox::ReviewRequested),
            panels: HashMap::new(),
            chats: HashMap::new(),
            attached: HashMap::new(),
            adding: None,
            sidebar_shown: true,
            _load: None,
            _work: Vec::new(),
        };
        view.refresh(cx);
        view
    }

    /// The view came into sight, with the workspaces' `folders` as they are
    /// now: changed accounts or folders, or an old inbox, are read again.
    pub fn shown(
        &mut self,
        folders: Vec<(String, Vec<std::path::PathBuf>)>,
        cx: &mut Context<Self>,
    ) {
        let accounts = ph_github::accounts::accounts(&self.store);
        let stale = self.loaded_at.is_none_or(|at| at.elapsed() >= STALE_AFTER);
        let moved = folders != self.folders;
        self.folders = folders;
        if accounts != self.accounts || stale || moved {
            self.accounts = accounts;
            self.refresh(cx);
        }
    }

    pub fn set_sidebar_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.sidebar_shown = shown;
        for (chat, _) in self.chats.values() {
            chat.update(cx, |chat, cx| chat.set_history_menu(true, cx));
        }
        cx.notify();
    }

    /// How many pull requests wait for the user's review.
    pub fn waiting_for_review(&self) -> usize {
        self.inbox
            .get(&Inbox::ReviewRequested)
            .map_or(0, |list| list.items.len())
    }

    /// Reads the inbox lists from every account and finds the workspaces'
    /// repositories again, off the UI thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.accounts = ph_github::accounts::accounts(&self.store);
        let accounts = self.accounts.clone();
        let folders = self.folders.clone();
        self.loading = true;
        cx.notify();
        let load = cx.background_spawn(async move {
            let mut inbox: HashMap<Inbox, List> = HashMap::new();
            for account in &accounts {
                let client = match account.client() {
                    Ok(client) => client,
                    Err(err) => {
                        for kind in Inbox::ALL {
                            inbox.entry(kind).or_default().error = Some(format!("{err:#}"));
                        }
                        continue;
                    }
                };
                for kind in Inbox::ALL {
                    let list = inbox.entry(kind).or_default();
                    match ph_github::search::pulls(&client, kind) {
                        Ok(found) => list
                            .items
                            .extend(found.into_iter().map(|pull| (account.host.clone(), pull))),
                        Err(err) => list.error = Some(format!("{}: {err:#}", account.host)),
                    }
                }
            }
            for list in inbox.values_mut() {
                list.items
                    .sort_by(|a, b| b.1.updated_at.cmp(&a.1.updated_at));
            }
            // Only hosts with an account can be shown.
            let mut repos = Vec::new();
            for (workspace, folders) in folders {
                for folder in folders {
                    let Some(hosted) = ph_git::remote::hosted_repo(&folder) else {
                        continue;
                    };
                    let id = RepoId {
                        host: hosted.host.to_ascii_lowercase(),
                        owner: hosted.owner,
                        name: hosted.name,
                    };
                    let known = accounts.iter().any(|account| account.host == id.host);
                    if known && RepoId::parse(&id.key()).is_some() {
                        repos.push((id, workspace.clone()));
                    }
                }
            }
            (inbox, repos)
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let (inbox, repos) = load.await;
            _ = this.update(cx, |this, cx| {
                this.inbox = inbox;
                this.from_workspaces = repos;
                this.loading = false;
                this.loaded_at = Some(std::time::Instant::now());
                cx.notify();
            });
        }));
    }

    /// The sidebar's repositories: the workspaces' and the added ones, by
    /// name.
    pub(crate) fn repositories(&self) -> Vec<RepoRow> {
        let added: Vec<String> = self.store.setting(ADDED_KEY);
        let mut ids: BTreeSet<RepoId> = added.iter().filter_map(|key| RepoId::parse(key)).collect();
        ids.extend(self.from_workspaces.iter().map(|(id, _)| id.clone()));
        ids.into_iter()
            .map(|id| RepoRow {
                added: added.contains(&id.key()),
                workspaces: self
                    .from_workspaces
                    .iter()
                    .filter(|(known, _)| *known == id)
                    .map(|(_, name)| name.clone())
                    .collect(),
                id,
            })
            .collect()
    }

    pub(crate) fn show_inbox(&mut self, kind: Inbox, cx: &mut Context<Self>) {
        self.page = Page::Inbox(kind);
        cx.notify();
    }

    /// Shows repository `id`'s pull requests (and its chat).
    pub(crate) fn show_repo(&mut self, id: RepoId, window: &mut Window, cx: &mut Context<Self>) {
        self.panel(&id, window, cx);
        self.page = Page::Repo(id);
        cx.notify();
    }

    /// Shows pull request `number` of repository `id`.
    pub(crate) fn show_pull(
        &mut self,
        id: RepoId,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.panel(&id, window, cx);
        panel.update(cx, |panel, cx| panel.open_pull(number, window, cx));
        self.show_repo(id, window, cx);
    }

    /// The repository's pull requests panel, made the first time. When it
    /// opens a pull request, the chat beside it becomes that PR's.
    fn panel(
        &mut self,
        id: &RepoId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PullsPanel> {
        if let Some((panel, _, _)) = self.panels.get(id) {
            return panel.clone();
        }
        let store = self.store.clone();
        let panel = cx.new(|cx| PullsPanel::for_repo(store, id.clone(), cx));
        let events = cx.subscribe_in(
            &panel,
            window,
            |this, _, event: &PullsPanelEvent, window, cx| match event {
                PullsPanelEvent::OpenSettings => {
                    window.dispatch_action(Box::new(crate::actions::OpenSettings), cx)
                }
                PullsPanelEvent::ReviewWithAgent { hosting, pull } => {
                    let template = ph_review::templates::default_review(&this.store);
                    this.review((**hosting).clone(), (**pull).clone(), template, window, cx)
                }
                PullsPanelEvent::OpenSession(session) => this.show_session_id(*session, window, cx),
            },
        );
        let repo = id.clone();
        let opened = cx.observe_in(&panel, window, move |this, panel, window, cx| {
            let showing = panel.read(cx).showing(cx);
            if this.attached.get(&repo).copied() != showing {
                match showing {
                    Some(number) => this.attach_pull(&repo, number, window, cx),
                    None => {
                        this.attached.remove(&repo);
                    }
                }
                cx.notify();
            }
        });
        self.panels
            .insert(id.clone(), (panel.clone(), events, opened));
        panel
    }

    /// The panel of repository `id`, if made.
    pub(crate) fn panel_of(&self, id: &RepoId) -> Option<&Entity<PullsPanel>> {
        self.panels.get(id).map(|(panel, _, _)| panel)
    }

    /// The repository's space: its chats live there. Made (with the folder
    /// the clone will go to) the first time.
    fn space(&self, id: &RepoId) -> anyhow::Result<Project> {
        let clone = id.clone_dir();
        if !clone.exists() {
            ph_store::paths::ensure_private_dir(&clone)?;
        }
        self.store.github_space(&id.full_name(), &clone)
    }

    /// Makes the chat beside the PR view the chat about pull request
    /// `number`: its latest chat, or a new one, ready to ask; templates as
    /// starters. The repository is cloned and the PR checked out when the
    /// first message is sent.
    fn attach_pull(
        &mut self,
        id: &RepoId,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = match self.space(id) {
            Ok(project) => project,
            Err(err) => {
                window.push_notification(Notification::error(format!("{err:#}")), cx);
                return;
            }
        };
        self.attached.insert(id.clone(), number);
        let chat = self.chat_for(id, project.clone(), window, cx);
        let cwd = id.pull_dir(number);
        let title = self.pull_title(id, number, cx);
        let cloned = id.clone_dir().join(".git").exists();
        let (store, repo) = (self.store.clone(), id.clone());
        let prepare = ph_agent::Prepare {
            label: if cloned {
                format!("Checking out #{number}…")
            } else {
                format!("Cloning {} and checking out #{number}…", id.full_name())
            },
            run: std::rc::Rc::new(move |cx: &mut gpui_kit::App| {
                let (store, repo) = (store.clone(), repo.clone());
                cx.background_spawn(async move {
                    let hosting = ph_pulls::hosting::for_repo(&store, &repo)
                        .map_err(|missing| anyhow::anyhow!("{}", missing.explain()))?;
                    ph_pulls::clones::check_out_pull(&hosting, number).map(|_| ())
                })
            }),
        };
        let templates = self.store.clone();
        let starters = std::rc::Rc::new(move || {
            ph_review::templates::all(&templates)
                .into_iter()
                .map(|template| ph_agent::Starter {
                    id: template.id,
                    title: template.name,
                    hint: template.description,
                })
                .collect()
        });
        let context = ph_agent::ChatContext {
            title: match &title {
                Some(title) => format!("#{number} {title}"),
                None => format!("Pull request #{number}"),
            },
            subtitle: "Ask about the change, or start with a template. Chats here only read: nothing is edited or pushed.".into(),
            starters,
            placeholder: format!("Ask about #{number}…"),
            cwd: cwd.clone(),
            prepare: Some(prepare),
        };
        let cwd_text = cwd.to_string_lossy().into_owned();
        let latest = self
            .store
            .sessions(project.id)
            .unwrap_or_default()
            .into_iter()
            .find(|session| session.cwd.as_deref() == Some(cwd_text.as_str()));
        // This PR's latest chat, else a new one: never another PR's.
        chat.update(cx, |chat, cx| {
            chat.set_context(Some(context), window, cx);
            chat.set_session(latest, cx);
        });
    }

    /// The title of pull request `number`, as a list read it.
    fn pull_title(&self, id: &RepoId, number: u64, cx: &gpui_kit::App) -> Option<String> {
        let from_panel = self.panel_of(id).and_then(|panel| {
            panel
                .read(cx)
                .list()
                .iter()
                .find(|pull| pull.number == number)
                .map(|pull| pull.title.clone())
        });
        from_panel.or_else(|| {
            self.inbox
                .values()
                .flat_map(|list| &list.items)
                .find_map(|(host, pull)| {
                    let same = *host == id.host
                        && pull.number == number
                        && pull.repo() == Some((id.owner.clone(), id.name.clone()));
                    same.then(|| pull.title.clone())
                })
        })
    }

    fn chat_for(
        &mut self,
        id: &RepoId,
        project: Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ChatPanel> {
        if let Some((chat, _)) = self.chats.get(id) {
            return chat.clone();
        }
        let store = self.store.clone();
        let chat = cx.new(|cx| {
            let mut chat = ChatPanel::new(store.clone(), project.clone(), window, cx);
            // Its chats are listed in the chat's own header.
            chat.set_history_menu(true, cx);
            chat
        });
        let repo = id.clone();
        let subscription = cx.subscribe_in(
            &chat,
            window,
            move |this, chat, event: &ChatPanelEvent, window, cx| match event {
                ChatPanelEvent::SessionsChanged => cx.notify(),
                // A review's findings are in: the PR's header shows them.
                ChatPanelEvent::TurnEnded => {
                    if let Some(panel) = this.panel_of(&repo).cloned() {
                        panel.update(cx, |panel, cx| panel.review_changed(cx));
                    }
                    cx.notify();
                }
                ChatPanelEvent::SelectSession(id) => {
                    let session = id.and_then(|id| this.store.session(id).ok().flatten());
                    chat.update(cx, |chat, cx| chat.set_session(session, cx));
                }
                ChatPanelEvent::ReviewAgain { review_id } => {
                    this.review_again(*review_id, window, cx)
                }
                ChatPanelEvent::OpenFile { path, .. } => this.open_file(chat, path, cx),
                ChatPanelEvent::WantPulls => {
                    if let Some(panel) = this.panel_of(&repo).cloned() {
                        let briefs = crate::workspace_screen::briefs(panel.read(cx).list());
                        chat.update(cx, |chat, cx| chat.set_pulls(briefs, cx));
                    }
                }
                ChatPanelEvent::Starter(template) => {
                    if let Some(number) = this.attached.get(&repo).copied() {
                        this.run_template(&repo, number, template, window, cx)
                    }
                }
                ChatPanelEvent::RunParallel { .. } => window.push_notification(
                    Notification::info("Run with several agents from a workspace."),
                    cx,
                ),
            },
        );
        self.chats.insert(id.clone(), (chat.clone(), subscription));
        chat
    }

    /// The chat beside the pull request in view, if one is open.
    pub(crate) fn current_chat(&self) -> Option<&Entity<ChatPanel>> {
        match &self.page {
            Page::Repo(id) if self.attached.contains_key(id) => {
                self.chats.get(id).map(|(chat, _)| chat)
            }
            _ => None,
        }
    }

    /// A file a chat names, opened in a window of its own (the view has no
    /// editor). Paths are the chat's: in a review, the PR's copy.
    fn open_file(&self, chat: &Entity<ChatPanel>, path: &str, cx: &mut Context<Self>) {
        let cwd = chat
            .read(cx)
            .active_session_id(cx)
            .and_then(|id| self.store.session(id).ok().flatten())
            .and_then(|session| session.cwd);
        let path = std::path::Path::new(path);
        let full = match cwd {
            Some(cwd) if path.is_relative() => std::path::PathBuf::from(cwd).join(path),
            _ => path.to_path_buf(),
        };
        // Only files of the view's own clones open from here.
        if !ph_pulls::clones::is_managed(&full) {
            return;
        }
        if let Err(err) = crate::file_window::open(full, cx) {
            tracing::warn!("opening a file from a GitHub chat failed: {err:#}");
        }
    }

    /// Puts the keyboard in the chat in view.
    pub fn focus_chat(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(chat) = self.current_chat().cloned() else {
            return false;
        };
        chat.update(cx, |chat, cx| chat.focus_composer(window, cx));
        true
    }

    /// ⌘N: a new chat about the pull request in view.
    pub fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(chat) = self.current_chat().cloned() {
            chat.update(cx, |chat, cx| {
                chat.set_session(None, cx);
                chat.focus_composer(window, cx);
            });
        }
    }

    /// Runs template `template_id` on pull request `number`: a review
    /// template starts a review, a prompt template asks its request in a
    /// new chat.
    fn run_template(
        &mut self,
        id: &RepoId,
        number: u64,
        template_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(template) = ph_review::templates::get(&self.store, template_id) else {
            return;
        };
        let Some(chat) = self.chats.get(id).map(|(chat, _)| chat.clone()) else {
            return;
        };
        let cloned = id.clone_dir().join(".git").exists();
        chat.update(cx, |chat, cx| {
            chat.set_busy(
                Some(if cloned {
                    format!("Getting #{number} ready…")
                } else {
                    format!("Cloning {} and checking out #{number}…", id.full_name())
                }),
                cx,
            )
        });
        let (store, repo) = (self.store.clone(), id.clone());
        let find = cx.background_spawn(async move {
            let hosting = ph_pulls::hosting::for_repo(&store, &repo)
                .map_err(|missing| anyhow::anyhow!("{}", missing.explain()))?;
            let pull = ph_github::pulls::get(&hosting.client, &hosting.repo, number)?;
            anyhow::Ok((hosting, pull))
        });
        self._work.push(cx.spawn_in(window, async move |this, cx| {
            let result = find.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok((hosting, pull)) => match template.kind {
                    ph_review::templates::TemplateKind::Review => {
                        this.review(hosting, pull, template, window, cx)
                    }
                    ph_review::templates::TemplateKind::Prompt => {
                        this.ask(hosting, pull, template, window, cx)
                    }
                },
                Err(err) => chat.update(cx, |chat, cx| chat.set_failed(format!("{err:#}"), cx)),
            });
        }));
    }

    /// Asks prompt `template`'s request about `pull` in a new chat, once
    /// the PR is checked out.
    fn ask(
        &mut self,
        hosting: Hosting,
        pull: PullRequest,
        template: Template,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = RepoId::of(&hosting);
        let Some(chat) = self.chats.get(&id).map(|(chat, _)| chat.clone()) else {
            return;
        };
        let work = cx.background_spawn(async move {
            let (_, sha) = ph_pulls::clones::check_out_pull(&hosting, pull.number)?;
            let info = ph_review::prompt::PullInfo {
                number: pull.number,
                title: &pull.title,
                body: pull.body.as_deref().unwrap_or_default(),
                author: &pull.user.login,
                owner: &hosting.repo.owner,
                repo: &hosting.repo.name,
                base: &pull.base.name,
                head: &pull.head.name,
                head_sha: &sha,
            };
            let prompt = ph_review::prompt::request_prompt(&info, &template.instructions);
            anyhow::Ok((
                format!("{} #{}: {}", template.name, pull.number, pull.title),
                format!("{} (#{} “{}”).", template.name, pull.number, pull.title),
                prompt,
            ))
        });
        self._work.push(cx.spawn_in(window, async move |_, cx| {
            let result = work.await;
            _ = chat.update_in(cx, |chat, window, cx| match result {
                Ok((title, display, prompt)) => {
                    chat.start_prompt(title, display, prompt, window, cx)
                }
                Err(err) => chat.set_failed(format!("{err:#}"), cx),
            });
        }));
    }

    /// Starts an agent review of `pull` with review `template`: checks it
    /// out beside the repository's clone (made the first time), then opens
    /// a review chat there that starts by itself.
    pub fn review(
        &mut self,
        hosting: Hosting,
        pull: PullRequest,
        template: Template,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = RepoId::of(&hosting);
        let number = pull.number;
        self.show_pull(id.clone(), number, window, cx);
        let project = match self.space(&id) {
            Ok(project) => project,
            Err(err) => {
                window.push_notification(Notification::error(format!("{err:#}")), cx);
                return;
            }
        };
        let chat = self.chat_for(&id, project.clone(), window, cx);
        let first = !id.clone_dir().join(".git").exists();
        chat.update(cx, |chat, cx| {
            chat.set_busy(
                Some(if first {
                    format!("Cloning {} for the review of #{number}…", id.full_name())
                } else {
                    format!("Getting #{number} ready for the review…")
                }),
                cx,
            )
        });
        let store = self.store.clone();
        let work = cx.background_spawn(async move {
            crate::review::prepare(&store, project.id, &hosting, &pull, &template)
        });
        self._work.push(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |_, window, cx| {
                chat.update(cx, |chat, cx| match result {
                    Ok(prepared) => chat.start_review(
                        prepared.review_id,
                        prepared.worktree,
                        prepared.title,
                        prepared.display,
                        prepared.prompt,
                        window,
                        cx,
                    ),
                    Err(err) => chat.set_failed(format!("Could not start the review: {err:#}"), cx),
                });
                cx.notify();
            });
        }));
    }

    /// A new run of review `review_id`, on its pull request's latest head,
    /// with the default review template.
    pub fn review_again(&mut self, review_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let find = cx.background_spawn(async move {
            let review = store
                .pr_review(review_id)?
                .ok_or_else(|| anyhow::anyhow!("review not found"))?;
            let id = RepoId {
                host: review.pull.host.clone(),
                owner: review.pull.owner.clone(),
                name: review.pull.repo.clone(),
            };
            let hosting = ph_pulls::hosting::for_repo(&store, &id)
                .map_err(|missing| anyhow::anyhow!("{}", missing.explain()))?;
            let pull = ph_github::pulls::get(&hosting.client, &hosting.repo, review.pull.number)?;
            anyhow::Ok((hosting, pull))
        });
        self._work.push(cx.spawn_in(window, async move |this, cx| {
            let result = find.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok((hosting, pull)) => {
                    let template = ph_review::templates::default_review(&this.store);
                    this.review(hosting, pull, template, window, cx)
                }
                Err(err) => window.push_notification(Notification::error(format!("{err:#}")), cx),
            });
        }));
    }

    /// Shows chat `session` of repository space `project`, beside its pull
    /// request when it is about one.
    pub fn show_session(
        &mut self,
        project: Project,
        session: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = RepoId::from_clone_dir(&project.path) else {
            return;
        };
        let Some(found) = self.store.session(session).ok().flatten() else {
            return;
        };
        let number = found.cwd.as_deref().and_then(|cwd| {
            std::path::Path::new(cwd)
                .file_name()?
                .to_str()?
                .strip_prefix("pr-")?
                .parse::<u64>()
                .ok()
        });
        match number {
            Some(number) => {
                self.show_pull(id.clone(), number, window, cx);
                self.attach_pull(&id, number, window, cx);
            }
            None => self.show_repo(id.clone(), window, cx),
        }
        let chat = self.chat_for(&id, project, window, cx);
        chat.update(cx, |chat, cx| chat.set_session(Some(found), cx));
    }

    /// Shows chat `session`, which may be one of a workspace's (an earlier
    /// review): the window shows it where it lives.
    fn show_session_id(&mut self, session: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            _ = workspace.update(cx, |workspace, cx| {
                workspace.show_session(session, window, cx)
            });
        });
    }

    /// Opens the "add a repository" box.
    pub(crate) fn start_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/name or its URL"));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let text = input.read(cx).value().to_string();
                    this.add_repository(&text, window, cx);
                }
                InputEvent::Blur if input.read(cx).value().trim().is_empty() => {
                    this.adding = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.adding = Some((input, subscription));
        cx.notify();
    }

    /// Adds the repository `text` names (`owner/name` on the account's
    /// host, or a URL), once GitHub confirms the token can see it.
    fn add_repository(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = parse_repository(text, &self.accounts) else {
            window.push_notification(
                Notification::warning("Type owner/name, or the repository's URL."),
                cx,
            );
            return;
        };
        let store = self.store.clone();
        let check = cx.background_spawn(async move {
            let hosting = ph_pulls::hosting::for_repo(&store, &id)
                .map_err(|missing| anyhow::anyhow!("{}", missing.explain()))?;
            ph_github::pulls::repository(&hosting.client, &hosting.repo)?;
            anyhow::Ok(id)
        });
        self._work.push(cx.spawn_in(window, async move |this, cx| {
            let result = check.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok(id) => {
                    let mut added: Vec<String> = this.store.setting(ADDED_KEY);
                    if !added.contains(&id.key()) {
                        added.push(id.key());
                        if let Err(err) = this.store.set_setting(ADDED_KEY, &added) {
                            tracing::warn!("saving the repositories failed: {err:#}");
                        }
                    }
                    this.adding = None;
                    this.show_repo(id, window, cx);
                }
                Err(err) => window.push_notification(
                    Notification::error(format!("{err:#}")).title("Could not add the repository"),
                    cx,
                ),
            });
        }));
    }

    /// Takes an added repository out of the list (its clone stays until
    /// PilotHouse tidies it).
    pub(crate) fn remove_repository(&mut self, id: &RepoId, cx: &mut Context<Self>) {
        let mut added: Vec<String> = self.store.setting(ADDED_KEY);
        added.retain(|key| *key != id.key());
        if let Err(err) = self.store.set_setting(ADDED_KEY, &added) {
            tracing::warn!("saving the repositories failed: {err:#}");
        }
        if self.page == Page::Repo(id.clone())
            && !self.from_workspaces.iter().any(|(known, _)| known == id)
        {
            self.page = Page::Inbox(Inbox::ReviewRequested);
        }
        cx.notify();
    }
}

/// The repository `text` names: a URL (`https://host/owner/name`,
/// `git@host:owner/name.git`), or `owner/name` on github.com when there is
/// an account for it, else on the only account's host.
fn parse_repository(text: &str, accounts: &[Account]) -> Option<RepoId> {
    let text = text.trim();
    let (host, owner, name) = match ph_git::remote::parse_url(text) {
        Some(parts) => parts,
        None => {
            let (owner, name) = text.trim_end_matches(".git").split_once('/')?;
            let host = accounts
                .iter()
                .find(|account| account.host == "github.com")
                .or(accounts.first())?
                .host
                .clone();
            (host, owner.to_string(), name.to_string())
        }
    };
    RepoId::parse(&format!("{}/{owner}/{name}", host.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(host: &str) -> Account {
        Account {
            id: format!("{host}-me"),
            host: host.into(),
            login: "me".into(),
            api_url: None,
        }
    }

    #[test]
    fn repositories_by_name_or_url() {
        let both = [account("ghe.example.net"), account("github.com")];
        let id = |host: &str, owner: &str, name: &str| RepoId {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
        };
        assert_eq!(
            parse_repository("acme/app", &both),
            Some(id("github.com", "acme", "app"))
        );
        assert_eq!(
            parse_repository("acme/app", &[account("ghe.example.net")]),
            Some(id("ghe.example.net", "acme", "app"))
        );
        assert_eq!(
            parse_repository("https://ghe.example.net/team/svc", &both),
            Some(id("ghe.example.net", "team", "svc"))
        );
        assert_eq!(
            parse_repository("git@github.com:acme/app.git", &both),
            Some(id("github.com", "acme", "app"))
        );
        assert_eq!(parse_repository("acme", &both), None);
        assert_eq!(parse_repository("acme/app", &[]), None);
        assert_eq!(parse_repository("../x/y", &both), None);
    }
}
