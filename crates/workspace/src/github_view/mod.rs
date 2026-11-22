//! The GitHub view: what you would see on GitHub's own pages, apart from
//! any workspace. Its sidebar lists the pull requests by whose turn it is
//! across every repository (those needing your review, those you reviewed
//! that wait for their author, your own) and the repositories you follow:
//! those your workspaces' folders come from, and any you add. The middle shows a list or a repository's pull
//! requests and one PR in full; the chat on the right is the repository's.
//! An inbox list is a queue ([`queue`]): grouped by what needs doing, with
//! reviews of several at once.
//!
//! The lists are read again every few minutes while the app runs, so the
//! count in the column of icons is current and a new request for review is
//! announced; an agent can be started on each by itself (off unless asked
//! for, Settings → GitHub).
//!
//! Agent reviews run here, in the view's own clone of the repository
//! ([`ph_pulls::clones`]); the user's checkouts are never touched. Each
//! repository has a space of its own ([`ph_store::projects::ProjectKind::GitHub`])
//! that holds its chats, so they have what workspace chats have.
//!
//! `render` draws it.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

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

mod agents;
mod queue;
mod render;

/// The repositories the user added (`host/owner/name` each).
const ADDED_KEY: &str = "github.repositories";

/// The inbox is read again when shown after this long.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(120);
/// Watched pull requests are looked at this often.
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(180);
/// The lists are read again this often while the app runs (three searches
/// per account), so the count is current and new requests are announced.
const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(300);
/// Reviews started from the queue that run at the same time; the rest wait
/// their turn.
pub(crate) const REVIEWS_AT_ONCE: usize = 3;
/// Settings key: the inbox sorted for review (not by date).
const REVIEW_FIRST_KEY: &str = "github.inbox.review_first";
/// Settings key: the pull request's chat is folded away.
const CHAT_HIDDEN_KEY: &str = "github.chat.hidden";
/// Settings key: what the list of pull requests to review leaves out.
const SHOWN_KEY: &str = "github.inbox.shown";

/// What the list of pull requests needing a review shows beyond those
/// asked of the user by name: all off unless turned on, so the count is of
/// what really waits for them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Shown {
    pub drafts: bool,
    /// Pull requests opened by an app (Dependabot, Renovate…).
    pub bots: bool,
}

impl Shown {
    /// Whether `pull` is left out of the list.
    pub(crate) fn hides(self, pull: &FoundPull) -> bool {
        (pull.draft && !self.drafts) || (pull.user.is_bot() && !self.bots)
    }
}

/// A pull request in the inbox: host, owner, name, number.
pub(crate) type PullKey = (String, String, String, u64);

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
    /// The window, for what starts without a click (a review of a new
    /// request, the next review in line).
    window: gpui_kit::AnyWindowHandle,
    /// What the list to review leaves out.
    pub(crate) shown: Shown,
    /// The box that narrows a list, made when the view is first drawn.
    pub(crate) search: Option<(Entity<InputState>, Subscription)>,
    /// The pull requests asking for a review as last read (`None` before
    /// the first read: nothing is new then).
    known_requests: Option<HashSet<PullKey>>,
    /// Reviews asked for from the queue that wait their turn.
    pub(crate) in_line: VecDeque<PullKey>,
    /// The agent and template the queue's reviews run with, when chosen
    /// there (else the defaults).
    pub(crate) batch_agent: Option<&'static ph_agent::agents::AgentSpec>,
    pub(crate) batch_template: Option<String>,
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
    /// Inbox pull requests assessed (size, risk, CI, your review), and when
    /// each was last updated (assessed again only once it changes).
    pub(crate) triage: HashMap<PullKey, (String, ph_review::triage::Triage)>,
    /// The inbox sorted for review, not by date.
    pub(crate) review_first: bool,
    /// What agents found on the inbox's pull requests, as last read.
    reviews: HashMap<PullKey, queue::Reviewed>,
    /// Reviews being readied, and those running: each one's space and chat.
    starting: HashMap<PullKey, queue::Starting>,
    running: HashMap<PullKey, (ph_store::projects::ProjectId, SessionId)>,
    /// The pull requests ticked for a review.
    selected: BTreeSet<PullKey>,
    /// The agent and template a review from the queue runs with.
    reviewer: queue::Reviewer,
    /// The spaces whose chats the queue follows.
    watched_spaces: HashSet<ph_store::projects::ProjectId>,
    _watches: Vec<Subscription>,
    _seconds: Option<Task<()>>,
    _reviews: Option<Task<()>>,
    /// Each repository space with a chat open, and its name (for the
    /// Bridge: reviews show there).
    spaces: Vec<(ph_store::projects::ProjectId, String)>,
    /// The "add a repository" box, while open.
    pub(crate) adding: Option<(Entity<InputState>, Subscription)>,
    pub(crate) sidebar_shown: bool,
    /// The pull request's chat is shown beside it (it can be folded away
    /// to give the page the room).
    pub(crate) chat_shown: bool,
    /// What GitHub clones and their copies take on disk, as last measured.
    pub(crate) clones_size: Option<u64>,
    /// When the clones were measured (it walks every file: not often).
    clones_measured: Option<std::time::Instant>,
    _measure: Option<Task<()>>,
    _load: Option<Task<()>>,
    _work: Vec<Task<()>>,
    _rounds: Subscription,
}

impl GithubView {
    /// `folders`: each workspace's name and folders (their remotes are the
    /// sidebar's repositories).
    pub fn new(
        workspace: WeakEntity<Workspace>,
        folders: Vec<(String, Vec<std::path::PathBuf>)>,
        window: gpui_kit::AnyWindowHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = AppState::store(cx).clone();
        let rounds = ph_agent::review_rounds::ReviewRounds::global(cx);
        let mut view = Self {
            accounts: ph_github::accounts::accounts(&store),
            shown: store.setting(SHOWN_KEY),
            store,
            workspace,
            window,
            search: None,
            known_requests: None,
            in_line: VecDeque::new(),
            batch_agent: None,
            batch_template: None,
            inbox: HashMap::new(),
            loading: false,
            loaded_at: None,
            folders,
            from_workspaces: Vec::new(),
            page: Page::Inbox(Inbox::NeedsReview),
            panels: HashMap::new(),
            chats: HashMap::new(),
            attached: HashMap::new(),
            spaces: Vec::new(),
            triage: HashMap::new(),
            review_first: store_flag(AppState::store(cx), REVIEW_FIRST_KEY),
            reviews: HashMap::new(),
            starting: HashMap::new(),
            running: HashMap::new(),
            selected: BTreeSet::new(),
            reviewer: queue::Reviewer::default(),
            watched_spaces: HashSet::new(),
            _watches: Vec::new(),
            _seconds: None,
            _reviews: None,
            adding: None,
            sidebar_shown: true,
            chat_shown: !store_flag_off(AppState::store(cx), CHAT_HIDDEN_KEY),
            clones_size: None,
            clones_measured: None,
            _measure: None,
            _load: None,
            _work: Vec::new(),
            // A review's round may end in any chat: every open pull
            // request shows its findings and walkthrough at once.
            _rounds: cx.subscribe(
                &rounds,
                |this, _, _: &ph_agent::review_rounds::RoundRecorded, cx| {
                    for (panel, ..) in this.panels.values() {
                        panel.update(cx, |panel, cx| panel.review_changed(cx));
                    }
                    // And the queue moves it to the findings to read.
                    this.read_reviews(cx);
                },
            ),
        };
        view.refresh(cx);
        view.watch(cx);
        view.poll(cx);
        view
    }

    /// Reads the lists again every few minutes, shown or not.
    fn poll(&mut self, cx: &mut Context<Self>) {
        push_work(
            &mut self._work,
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL_EVERY).await;
                    let alive = this.update(cx, |this, cx| {
                        if !this.loading && !ph_github::accounts::accounts(&this.store).is_empty() {
                            this.refresh(cx);
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            }),
        );
    }

    /// What the list to review shows (drafts, bots), kept.
    pub(crate) fn set_shown(&mut self, shown: Shown, cx: &mut Context<Self>) {
        self.shown = shown;
        let _ = self.store.set_setting(SHOWN_KEY, &shown);
        cx.notify();
    }

    /// The pull requests of `kind` as the list shows them: for those to
    /// review, without what is left out.
    pub(crate) fn listed(&self, kind: Inbox) -> Vec<(String, FoundPull)> {
        self.inbox
            .get(&kind)
            .map(|list| {
                list.items
                    .iter()
                    .filter(|(_, pull)| kind != Inbox::NeedsReview || !self.shown.hides(pull))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The requests for review that came since the lists were last read:
    /// announced, and reviewed by an agent when that is asked for.
    fn greet_new_requests(&mut self, cx: &mut Context<Self>) {
        let now: HashSet<PullKey> = self
            .listed(Inbox::NeedsReview)
            .iter()
            .filter_map(|(host, pull)| queue::key_of(host, pull))
            .collect();
        let Some(known) = self.known_requests.replace(now.clone()) else {
            return;
        };
        let new: Vec<(String, FoundPull)> = self
            .listed(Inbox::NeedsReview)
            .into_iter()
            .filter(|(host, pull)| {
                queue::key_of(host, pull).is_some_and(|key| !known.contains(&key))
            })
            .collect();
        if new.is_empty() {
            return;
        }
        let settings = ph_pulls::ReviewSettings::read(&self.store);
        if settings.announce {
            let body = match new.as_slice() {
                [(_, pull)] => format!("{} by {}", pull.title, pull.user.login),
                many => many
                    .iter()
                    .map(|(_, pull)| pull.title.as_str())
                    .collect::<Vec<_>>()
                    .join(" · "),
            };
            cx.show_system_notification(gpui_kit::SystemNotification {
                tag: "review-requests".into(),
                title: match new.len() {
                    1 => {
                        let (_, pull) = &new[0];
                        format!(
                            "Review requested: {}/{} #{}",
                            pull.owner, pull.name, pull.number
                        )
                    }
                    n => format!("{n} pull requests ask for your review"),
                }
                .into(),
                body: body.into(),
                actions: Vec::new(),
            });
        }
        if settings.auto_review {
            let keys: Vec<PullKey> = new
                .iter()
                .filter_map(|(host, pull)| queue::key_of(host, pull))
                .filter(|key| !self.reviews.contains_key(key))
                .collect();
            let window = self.window;
            cx.spawn(async move |this, cx| {
                _ = cx.update_window(window, |_, window, cx| {
                    _ = this.update(cx, |this, cx| {
                        for key in keys {
                            this.line_up(key, window, cx);
                        }
                    });
                });
            })
            .detach();
        }
    }

    /// Looks at watched pull requests every few minutes: new commits and
    /// answers to findings are announced (and the answers applied).
    fn watch(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        push_work(
            &mut self._work,
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(WATCH_EVERY).await;
                    let store = store.clone();
                    let news = cx
                        .background_spawn(async move {
                            ph_review::github::watched(&store)
                                .into_iter()
                                .filter_map(|pull| {
                                    let news = ph_review::github::check_watched(&store, &pull)
                                        .map_err(|err| {
                                            tracing::info!("watching #{}: {err:#}", pull.number)
                                        })
                                        .ok()?;
                                    Some((pull, news))
                                })
                                .collect::<Vec<_>>()
                        })
                        .await;
                    let alive = this.update(cx, |this, cx| this.announce(news, cx)).is_ok();
                    if !alive {
                        break;
                    }
                }
            }),
        );
    }

    /// Tells the user what changed on watched pull requests.
    fn announce(
        &mut self,
        news: Vec<(ph_store::reviews::PullRef, ph_review::github::News)>,
        cx: &mut Context<Self>,
    ) {
        for (pull, news) in news {
            let name = format!("{}/{} #{}", pull.owner, pull.repo, pull.number);
            let mut lines = Vec::new();
            if news.pushed.is_some() {
                lines.push("New commits: check the fixes".to_string());
            }
            for (label, status, who) in &news.answers {
                let who = if who.is_empty() {
                    "the author"
                } else {
                    who.as_str()
                };
                lines.push(format!(
                    "{label}: {} ({who})",
                    ph_review::comment::status_label(status)
                ));
            }
            for (label, who) in &news.talk {
                lines.push(format!("{who} replied on {label}"));
            }
            if news.closed {
                lines.push("Merged or closed: no longer watched".into());
                let _ = ph_review::github::set_watched(&self.store, &pull, false);
            }
            if lines.is_empty() {
                continue;
            }
            cx.show_system_notification(gpui_kit::SystemNotification {
                tag: format!("watch-{name}").into(),
                title: name.into(),
                body: lines.join(" · ").into(),
                actions: Vec::new(),
            });
            let id = RepoId {
                host: pull.host.clone(),
                owner: pull.owner.clone(),
                name: pull.repo.clone(),
            };
            if let Some(panel) = self.panel_of(&id).cloned() {
                panel.update(cx, |panel, cx| panel.review_changed(cx));
            }
        }
    }

    /// Sorts the inbox for review, or by date.
    pub(crate) fn set_review_first(&mut self, on: bool, cx: &mut Context<Self>) {
        self.review_first = on;
        let _ = self.store.set_setting(REVIEW_FIRST_KEY, &Some(on));
        cx.notify();
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

    /// Folds the pull request's chat away, or brings it back; kept.
    pub fn toggle_chat(&mut self, cx: &mut Context<Self>) {
        self.chat_shown = !self.chat_shown;
        let _ = self
            .store
            .set_setting(CHAT_HIDDEN_KEY, &Some(!self.chat_shown));
        cx.notify();
    }

    /// Brings the chat back: something was asked that is answered there.
    pub(crate) fn reveal_chat(&mut self, cx: &mut Context<Self>) {
        if !self.chat_shown {
            self.toggle_chat(cx);
        }
    }

    pub fn set_sidebar_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.sidebar_shown = shown;
        for (chat, _) in self.chats.values() {
            chat.update(cx, |chat, cx| chat.set_history_menu(true, cx));
        }
        cx.notify();
    }

    /// How many pull requests of list `kind` need the user now: those to
    /// review; those reviewed whose author pushed since; one's own with
    /// changes requested, checks failing or a conflict.
    pub(crate) fn needs_you(&self, kind: Inbox) -> usize {
        use ph_review::triage::Mine;
        let listed = self.listed(kind);
        match kind {
            Inbox::NeedsReview => listed.len(),
            Inbox::Waiting => listed
                .iter()
                .filter(|(host, pull)| {
                    queue::key_of(host, pull)
                        .and_then(|key| self.triage.get(&key))
                        .is_some_and(|(_, triage)| triage.behind)
                })
                .count(),
            Inbox::Mine => listed
                .iter()
                .filter(|(_, pull)| {
                    matches!(
                        Mine::of(pull),
                        Mine::ChangesRequested | Mine::ChecksFailing | Mine::Conflicts
                    )
                })
                .count(),
        }
    }

    /// How many pull requests wait for the user's review (drafts and
    /// bots' aside, unless they are shown).
    pub fn waiting_for_review(&self) -> usize {
        self.listed(Inbox::NeedsReview).len()
    }

    /// Reads the inbox lists from every account and finds the workspaces'
    /// repositories again, off the UI thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let stale = self
            .clones_measured
            .is_none_or(|at| at.elapsed() > std::time::Duration::from_secs(10 * 60));
        if stale {
            self.clones_measured = Some(std::time::Instant::now());
            let measure = cx.background_spawn(async { crate::storage::clones_size() });
            self._measure = Some(cx.spawn(async move |this, cx| {
                let size = measure.await;
                _ = this.update(cx, |this, cx| {
                    this.clones_size = Some(size);
                    cx.notify();
                });
            }));
        }
        self.accounts = ph_github::accounts::accounts(&self.store);
        let accounts = self.accounts.clone();
        let folders = self.folders.clone();
        let store = self.store.clone();
        self.loading = true;
        cx.notify();
        let load = cx.background_spawn(async move {
            let mut inbox: HashMap<Inbox, List> = HashMap::new();
            let mut triage = HashMap::new();
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
                // The searches at once: each brings what its rows show.
                let searches: Vec<_> = std::thread::scope(|scope| {
                    let (client, me) = (&client, account.login.as_str());
                    Inbox::ALL
                        .map(|kind| {
                            (
                                kind,
                                scope.spawn(move || ph_github::search::pulls(client, kind, me)),
                            )
                        })
                        .into_iter()
                        .map(|(kind, handle)| {
                            let found = handle.join().unwrap_or_else(|_| {
                                Err(anyhow::anyhow!("searching GitHub failed"))
                            });
                            (kind, found)
                        })
                        .collect()
                });
                for (kind, found) in searches {
                    let list = inbox.entry(kind).or_default();
                    match found {
                        Ok(found) => {
                            for pull in &found {
                                let Some((owner, name)) = pull.repo() else {
                                    continue;
                                };
                                let key = (account.host.clone(), owner, name, pull.number);
                                let reviewed = reviewed_at(&store, &key);
                                if let Some(assessed) =
                                    ph_review::triage::assess(pull, reviewed.as_deref())
                                {
                                    triage.insert(key, (pull.updated_at.clone(), assessed));
                                }
                            }
                            list.items
                                .extend(found.into_iter().map(|pull| (account.host.clone(), pull)))
                        }
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
            (inbox, repos, triage)
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let (inbox, repos, triage) = load.await;
            _ = this.update(cx, |this, cx| {
                this.inbox = inbox;
                this.triage = triage;
                this.from_workspaces = repos;
                this.loading = false;
                this.loaded_at = Some(std::time::Instant::now());
                this.read_reviews(cx);
                this.greet_new_requests(cx);
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
                    window.dispatch_action(Box::new(crate::actions::OpenGitHubSettings), cx)
                }
                PullsPanelEvent::Agent { hosting, request } => {
                    this.agent((**hosting).clone(), request.clone(), window, cx)
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
                // The page has these two already: its Review with agent
                // button, and its Walkthrough tab.
                .filter(|template| {
                    template.id != ph_review::templates::REVIEW
                        && template.id != ph_review::templates::EXPLAIN
                })
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
            instructions: None,
            app_tools: false,
            own_files: None,
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
        self.spaces.push((project.id, id.full_name()));
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
                ChatPanelEvent::OpenFindings { review_id } => {
                    this.open_findings(*review_id, window, cx)
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
                // Terminal agents' chats belong to workspaces, not here; and
                // this panel gives Past chats no row actions.
                ChatPanelEvent::ShowAs(_)
                | ChatPanelEvent::PickOutsideSession(_)
                | ChatPanelEvent::TypeInCli(_)
                | ChatPanelEvent::StartCli { .. }
                | ChatPanelEvent::ChatAction(..) => {}
            },
        );
        self.chats.insert(id.clone(), (chat.clone(), subscription));
        chat
    }

    /// The repository spaces with a chat open: (space, `owner/name`).
    pub(crate) fn spaces(&self) -> &[(ph_store::projects::ProjectId, String)] {
        &self.spaces
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
        push_work(
            &mut self._work,
            cx.spawn_in(window, async move |this, cx| {
                let result = find.await;
                _ = this.update_in(cx, |this, window, cx| match result {
                    Ok((hosting, pull)) => match template.kind {
                        ph_review::templates::TemplateKind::Review => {
                            this.review(hosting, pull, template, "full", window, cx)
                        }
                        ph_review::templates::TemplateKind::Prompt => {
                            this.ask(hosting, pull, template, window, cx)
                        }
                    },
                    Err(err) => chat.update(cx, |chat, cx| chat.set_failed(format!("{err:#}"), cx)),
                });
            }),
        );
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
            let (_, sha, _) = ph_pulls::clones::check_out_pull(&hosting, pull.number)?;
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
        push_work(
            &mut self._work,
            cx.spawn_in(window, async move |_, cx| {
                let result = work.await;
                _ = chat.update_in(cx, |chat, window, cx| match result {
                    Ok((title, display, prompt)) => {
                        chat.start_prompt(title, display, prompt, window, cx)
                    }
                    Err(err) => chat.set_failed(format!("{err:#}"), cx),
                });
            }),
        );
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
        self.reveal_chat(cx);
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
        push_work(
            &mut self._work,
            cx.spawn_in(window, async move |this, cx| {
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
                        Notification::error(format!("{err:#}"))
                            .title("Could not add the repository"),
                        cx,
                    ),
                });
            }),
        );
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
            self.page = Page::Inbox(Inbox::NeedsReview);
        }
        cx.notify();
    }
}

/// The repository `text` names: a URL (`https://host/owner/name`,
/// `git@host:owner/name.git`), or `owner/name` on github.com when there is
/// an account for it, else on the only account's host.
/// A flag kept in the settings (true when never set).
fn store_flag(store: &Store, key: &str) -> bool {
    store.setting::<Option<bool>>(key).unwrap_or(true)
}

/// A setting that is on only once set.
fn store_flag_off(store: &Store, key: &str) -> bool {
    store.setting::<Option<bool>>(key).unwrap_or(false)
}

/// The commit your newest review of inbox pull request `key` read.
fn reviewed_at(store: &Store, key: &PullKey) -> Option<String> {
    let (host, owner, name, number) = key;
    store
        .latest_pr_review(
            &ph_store::reviews::PullRef {
                host: host.clone(),
                owner: owner.clone(),
                repo: name.clone(),
                number: *number,
            },
            None,
        )
        .ok()
        .flatten()
        .map(|round| round.head_sha)
}

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

/// Keeps `task` running with the view's other work, letting go of what
/// already finished (the list would grow with every action otherwise).
pub(super) fn push_work(work: &mut Vec<Task<()>>, task: Task<()>) {
    work.retain(|task| !task.is_ready());
    work.push(task);
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
