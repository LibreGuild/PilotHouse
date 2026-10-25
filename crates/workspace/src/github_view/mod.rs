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
    panels: HashMap<RepoId, (Entity<PullsPanel>, Subscription)>,
    /// Each repository space's chat.
    chats: HashMap<RepoId, (Entity<ChatPanel>, Subscription)>,
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
        self.chat(&id, window, cx);
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

    /// The repository's pull requests panel, made the first time.
    fn panel(
        &mut self,
        id: &RepoId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PullsPanel> {
        if let Some((panel, _)) = self.panels.get(id) {
            return panel.clone();
        }
        let store = self.store.clone();
        let panel = cx.new(|cx| PullsPanel::for_repo(store, id.clone(), cx));
        let subscription = cx.subscribe_in(
            &panel,
            window,
            |this, _, event: &PullsPanelEvent, window, cx| match event {
                PullsPanelEvent::OpenSettings => {
                    window.dispatch_action(Box::new(crate::actions::OpenSettings), cx)
                }
                PullsPanelEvent::ReviewWithAgent { hosting, pull } => {
                    this.review((**hosting).clone(), (**pull).clone(), window, cx)
                }
                PullsPanelEvent::OpenSession(session) => this.show_session_id(*session, window, cx),
            },
        );
        self.panels
            .insert(id.clone(), (panel.clone(), subscription));
        panel
    }

    /// The panel of repository `id`, if made.
    pub(crate) fn panel_of(&self, id: &RepoId) -> Option<&Entity<PullsPanel>> {
        self.panels.get(id).map(|(panel, _)| panel)
    }

    /// The chat of repository `id`'s space, made once the space exists (its
    /// first review makes it).
    fn chat(
        &mut self,
        id: &RepoId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ChatPanel>> {
        if let Some((chat, _)) = self.chats.get(id) {
            return Some(chat.clone());
        }
        let project = self.store.github_space_at(&id.clone_dir()).ok().flatten()?;
        Some(self.chat_for(id, project, window, cx))
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
            let latest = store
                .sessions(project.id)
                .ok()
                .and_then(|mut sessions| (!sessions.is_empty()).then(|| sessions.remove(0)));
            chat.set_session(latest, cx);
            chat
        });
        let repo = id.clone();
        let subscription = cx.subscribe_in(
            &chat,
            window,
            move |this, chat, event: &ChatPanelEvent, window, cx| match event {
                ChatPanelEvent::SessionsChanged | ChatPanelEvent::TurnEnded => cx.notify(),
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
                ChatPanelEvent::RunParallel { .. } => window.push_notification(
                    Notification::info("Run with several agents from a workspace."),
                    cx,
                ),
            },
        );
        self.chats.insert(id.clone(), (chat.clone(), subscription));
        chat
    }

    /// The chat of the repository in view, if it has one.
    pub(crate) fn current_chat(&self) -> Option<&Entity<ChatPanel>> {
        match &self.page {
            Page::Repo(id) => self.chats.get(id).map(|(chat, _)| chat),
            Page::Inbox(_) => None,
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
        if !ph_pulls::clones::is_managed(&ph_store::paths::canonical(&full).unwrap_or(full.clone()))
        {
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

    /// ⌘N: a new chat in the repository in view.
    pub fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(chat) = self.current_chat().cloned() {
            chat.update(cx, |chat, cx| {
                chat.set_session(None, cx);
                chat.focus_composer(window, cx);
            });
        }
    }

    /// Starts an agent review of `pull`: checks it out beside the
    /// repository's clone (made the first time), then opens a review chat
    /// there that starts by itself.
    pub fn review(
        &mut self,
        hosting: Hosting,
        pull: PullRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = RepoId::of(&hosting);
        let number = pull.number;
        self.show_pull(id.clone(), number, window, cx);
        let first = !id.clone_dir().join(".git").exists();
        window.push_notification(
            Notification::info(if first {
                format!("Cloning {} for the review of #{number}…", id.full_name())
            } else {
                format!("Getting #{number} ready for the review…")
            }),
            cx,
        );
        let store = self.store.clone();
        let work = cx.background_spawn(async move {
            let clone = ph_pulls::clones::ensure(&hosting)?;
            let project = store.github_space(&RepoId::of(&hosting).full_name(), &clone)?;
            let prepared = crate::review::prepare(&store, project.id, &hosting, &pull)?;
            anyhow::Ok((project, prepared))
        });
        self._work.push(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok((project, prepared)) => {
                    let chat = this.chat_for(&id, project, window, cx);
                    chat.update(cx, |chat, cx| {
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
                    cx.notify();
                }
                Err(err) => window.push_notification(
                    Notification::error(format!("{err:#}")).title("Could not start the review"),
                    cx,
                ),
            });
        }));
    }

    /// A new run of review `review_id`, on its pull request's latest head.
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
                Ok((hosting, pull)) => this.review(hosting, pull, window, cx),
                Err(err) => window.push_notification(Notification::error(format!("{err:#}")), cx),
            });
        }));
    }

    /// Shows chat `session` of repository space `project`.
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
        let chat = self.chat_for(&id, project, window, cx);
        let session = self.store.session(session).ok().flatten();
        chat.update(cx, |chat, cx| chat.set_session(session, cx));
        self.show_repo(id, window, cx);
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
