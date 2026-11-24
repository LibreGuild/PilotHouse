//! The queue: an inbox list grouped by what needs doing. Pull requests to
//! review, by what an agent has made of each (findings to read, agents at
//! work, not started, reviewed with nothing found), with reviews of
//! several at once; those the user reviewed, by whose turn it is; the
//! user's own, by what stands before their merge.
//!
//! Someone who reviews for a team does not open pull requests one by one:
//! they tick the ones nobody looked at, start an agent on each, and read
//! the findings as they come in. Each review runs in the view's own copy of
//! the repository and only reads; nothing is posted until the reviewer
//! says so on the pull request's page.

use std::collections::HashMap;
use std::time::Instant;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::Input,
    menu::PopupMenuItem,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use ph_agent::ChatState;
use ph_github::search::{FoundPull, Inbox};
use ph_pulls::clones::RepoId;
use ph_store::Store;
use ph_store::projects::ProjectId;
use ph_store::reviews::PullRef;
use ph_store::sessions::SessionId;
use ph_ui::{Icons, MenuButton as _};

use super::{GithubView, PullKey, Shown};

/// One queued review got ready at a time (see `review_behind`).
static PREPARING: smol::lock::Mutex<()> = smol::lock::Mutex::new(());

/// What the newest finished round found on a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reviewed {
    pub review_id: i64,
    /// The findings still open.
    pub open: usize,
    /// The most serious severity among them, and how many have it.
    pub worst: String,
    pub worst_count: usize,
}

/// A review being readied: the repository cloned, the pull request checked
/// out.
pub(crate) struct Starting {
    pub label: SharedString,
    pub since: Instant,
    /// What it is, for the Bridge: `Review #12 Title`.
    pub title: SharedString,
    /// The repository's space, when it was started from the queue: behind
    /// the chat in view, which says nothing of it.
    pub behind: Option<ProjectId>,
}

/// Who would run a review started from the queue, and by which template.
#[derive(Default)]
pub(crate) struct Reviewer {
    pub agent: Option<&'static ph_agent::agents::AgentSpec>,
    pub template: SharedString,
}

/// Where a pull request stands in the queue, in the order shown: what
/// waits for the reviewer comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Group {
    Findings,
    AtWork,
    NotStarted,
    Clean,
}

impl Group {
    const ALL: [Group; 4] = [
        Group::Findings,
        Group::AtWork,
        Group::NotStarted,
        Group::Clean,
    ];

    /// A review under way counts before what an earlier one found: its
    /// findings are about to change.
    pub(crate) fn of(at_work: bool, reviewed: Option<&Reviewed>) -> Self {
        match reviewed {
            _ if at_work => Group::AtWork,
            Some(reviewed) if reviewed.open > 0 => Group::Findings,
            Some(_) => Group::Clean,
            None => Group::NotStarted,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Group::Findings => "Findings to read",
            Group::AtWork => "Agents at work",
            Group::NotStarted => "Not started",
            Group::Clean => "Reviewed, nothing found",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Group::Findings => "findings",
            Group::AtWork => "working",
            Group::NotStarted => "new",
            Group::Clean => "clean",
        }
    }
}

/// An agent on a pull request now: what it is doing, and its chat once it
/// has one.
struct AtWork {
    line: String,
    session: Option<SessionId>,
    needs_you: bool,
}

/// The inbox's key of `pull` on `host`.
pub(super) fn key_of(host: &str, pull: &FoundPull) -> Option<PullKey> {
    pull.repo()
        .map(|(owner, name)| (host.to_string(), owner, name, pull.number))
}

fn repo_of(key: &PullKey) -> RepoId {
    RepoId {
        host: key.0.clone(),
        owner: key.1.clone(),
        name: key.2.clone(),
    }
}

/// The key as part of an element's id.
fn id_of(key: &PullKey) -> String {
    format!("{}/{}/{}#{}", key.0, key.1, key.2, key.3)
}

/// What the store has on each of `keys`: blocking.
fn read(store: &Store, keys: Vec<PullKey>) -> HashMap<PullKey, Reviewed> {
    keys.into_iter()
        .filter_map(|key| {
            let pull = PullRef {
                host: key.0.clone(),
                owner: key.1.clone(),
                repo: key.2.clone(),
                number: key.3,
            };
            let (round, findings) = ph_review::github::current(store, &pull).ok().flatten()?;
            let open: Vec<_> = findings
                .iter()
                .filter(|finding| finding.status == "open")
                .collect();
            let worst = open
                .iter()
                .min_by_key(|finding| ph_review::severity_rank(&finding.severity))
                .map(|finding| finding.severity.clone())
                .unwrap_or_default();
            Some((
                key,
                Reviewed {
                    review_id: round.id,
                    open: open.len(),
                    worst_count: open
                        .iter()
                        .filter(|finding| finding.severity == worst)
                        .count(),
                    worst,
                },
            ))
        })
        .collect()
}

impl GithubView {
    /// Reads what agents found on the inbox's pull requests, off the UI
    /// thread, and drops ticks on those no longer listed.
    pub(super) fn read_reviews(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<PullKey> = self
            .inbox
            .values()
            .flat_map(|list| &list.items)
            .filter_map(|(host, pull)| key_of(host, pull))
            .collect();
        self.selected.retain(|key| keys.contains(key));
        let store = self.store.clone();
        let load = cx.background_spawn(async move {
            let reviewer = Reviewer {
                agent: Some(ph_agent::chats::default_agent(&store)),
                template: ph_review::templates::default_review(&store).name.into(),
            };
            (read(&store, keys), reviewer)
        });
        self._reviews = Some(cx.spawn(async move |this, cx| {
            let (reviews, reviewer) = load.await;
            _ = this.update(cx, |this, cx| {
                this.reviews = reviews;
                this.reviewer = reviewer;
                cx.notify();
            });
        }));
    }

    /// The agent on pull request `key` now, if one is.
    fn at_work(&self, key: &PullKey, cx: &App) -> Option<AtWork> {
        if self.in_line.contains(key) {
            return Some(AtWork {
                line: "Waits its turn".into(),
                session: None,
                needs_you: false,
            });
        }
        if let Some(starting) = self.starting.get(key) {
            return Some(AtWork {
                line: format!("{} {}s", starting.label, starting.since.elapsed().as_secs()),
                session: None,
                needs_you: false,
            });
        }
        let (space, session) = self.running.get(key).copied()?;
        let summary = ph_agent::ChatHub::existing(space, cx)?
            .read(cx)
            .summary(session, cx)?;
        let needs_you = summary.state == ChatState::NeedsYou;
        (needs_you || summary.state == ChatState::Working).then(|| AtWork {
            line: match summary.line {
                _ if needs_you => "Asks you something".into(),
                Some(line) if !line.is_empty() => line,
                _ => "Reading the change".into(),
            },
            session: Some(session),
            needs_you,
        })
    }

    /// A review of pull request `key` is being readied or runs.
    pub(super) fn is_under_review(&self, key: &PullKey, cx: &App) -> bool {
        !self.in_line.contains(key)
            && (self.starting.contains_key(key) || self.at_work(key, cx).is_some())
    }

    /// The chat of the review of `key` started in this run, when its last
    /// turn failed: the row says so instead of looking untouched.
    fn stopped(&self, key: &PullKey, cx: &App) -> Option<SessionId> {
        let (space, session) = self.running.get(key).copied()?;
        let summary = ph_agent::ChatHub::existing(space, cx)?
            .read(cx)
            .summary(session, cx)?;
        (summary.state == ChatState::Failed).then_some(session)
    }

    /// The reviews of `space`'s pull requests started from the queue and
    /// still being readied: each one's number.
    pub(crate) fn starting_behind(
        &self,
        space: ProjectId,
    ) -> impl Iterator<Item = (u64, &Starting)> {
        self.starting
            .iter()
            .filter(move |(_, starting)| starting.behind == Some(space))
            .map(|(key, starting)| (key.3, starting))
    }

    /// The agent a review started here runs with, as last read (`None`
    /// until the lists were read once).
    pub(crate) fn review_agent(&self) -> Option<&'static ph_agent::agents::AgentSpec> {
        self.reviewer.agent
    }

    /// How many reviews started from the queue are still being readied.
    pub(crate) fn starting_behind_count(&self) -> usize {
        self.starting
            .values()
            .filter(|starting| starting.behind.is_some())
            .count()
    }

    /// Notes that a review of `key` is being readied (`None`: it is not any
    /// more).
    pub(super) fn set_starting(
        &mut self,
        key: &PullKey,
        starting: Option<Starting>,
        cx: &mut Context<Self>,
    ) {
        match starting {
            Some(starting) => {
                self.starting.insert(key.clone(), starting);
                // Under way however it was started: no longer ticked.
                self.selected.remove(key);
                self.count_seconds(cx);
            }
            None => {
                self.starting.remove(key);
            }
        }
        cx.notify();
    }

    /// Notes that the review of `key` runs as chat `session` of `space`.
    pub(super) fn set_running(
        &mut self,
        key: PullKey,
        space: ProjectId,
        session: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Its state is read from the space's chats: the queue follows them,
        // and starts the next review in line when one ends.
        if let Some(chats) = ph_agent::ChatHub::existing(space, cx)
            && self.watched_spaces.insert(space)
        {
            self._watches
                .push(cx.observe_in(&chats, window, |this, _, window, cx| {
                    cx.notify();
                    this.pump(window, cx);
                }));
        }
        self.running.insert(key, (space, session));
        cx.notify();
    }

    /// How many reviews are being readied or run now.
    fn busy_reviews(&self, cx: &App) -> usize {
        let running = self
            .running
            .keys()
            .filter(|key| !self.starting.contains_key(*key) && self.at_work(key, cx).is_some())
            .count();
        self.starting.len() + running
    }

    /// Asks for a review of `key` from the queue: started now when fewer
    /// than [`super::REVIEWS_AT_ONCE`] run, else when one of them ends.
    pub(super) fn line_up(&mut self, key: PullKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_under_review(&key, cx) || self.in_line.contains(&key) {
            return;
        }
        self.selected.remove(&key);
        if self.busy_reviews(cx) < super::REVIEWS_AT_ONCE {
            self.review_behind(key, window, cx);
        } else {
            self.in_line.push_back(key);
            cx.notify();
        }
    }

    /// Starts the reviews in line there is room for.
    pub(super) fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while self.busy_reviews(cx) < super::REVIEWS_AT_ONCE {
            let Some(key) = self.in_line.pop_front() else {
                break;
            };
            self.review_behind(key, window, cx);
        }
    }

    /// The agent and template a review from the queue runs with: those
    /// chosen in its bar, else the defaults.
    fn batch_reviewer(&self) -> (Option<&'static ph_agent::agents::AgentSpec>, SharedString) {
        let template = self
            .batch_template
            .as_deref()
            .and_then(|id| ph_review::templates::get(&self.store, id))
            .map(|template| SharedString::from(template.name))
            .unwrap_or_else(|| self.reviewer.template.clone());
        (self.batch_agent.or(self.reviewer.agent), template)
    }

    /// Draws again each second while a review is being readied: the time it
    /// has taken shows on its row.
    fn count_seconds(&mut self, cx: &mut Context<Self>) {
        if self._seconds.is_some() {
            return;
        }
        self._seconds = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let counting = this.update(cx, |this, cx| {
                    cx.notify();
                    !this.starting.is_empty()
                });
                if !counting.unwrap_or(false) {
                    break;
                }
            }
            _ = this.update(cx, |this, _| this._seconds = None);
        }));
    }

    fn toggle_selected(&mut self, key: PullKey, on: bool, cx: &mut Context<Self>) {
        if on {
            self.selected.insert(key);
        } else {
            self.selected.remove(&key);
        }
        cx.notify();
    }

    /// Starts a review of every ticked pull request.
    fn review_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = std::mem::take(&mut self.selected);
        for key in picked {
            // Only what nobody reviewed: a tick may outlive that (the pull
            // request was reviewed from its own page meanwhile).
            if !self.reviews.contains_key(&key) {
                self.line_up(key, window, cx);
            }
        }
    }

    /// Starts a full review of pull request `key` without leaving the
    /// queue: its chat runs behind the one in view, and its row says how
    /// far it is.
    fn review_behind(&mut self, key: PullKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_under_review(&key, cx) {
            return;
        }
        let id = repo_of(&key);
        let number = key.3;
        let project = match self.space(&id) {
            Ok(project) => project,
            Err(err) => {
                window.push_notification(Notification::error(format!("{err:#}")), cx);
                return;
            }
        };
        let chat = self.chat_for(&id, project.clone(), window, cx);
        let cloned = id.clone_dir().join(".git").exists();
        let space = project.id;
        let title = self.pull_title(&id, number, cx).unwrap_or_default();
        self.set_starting(
            &key,
            Some(Starting {
                label: if cloned {
                    format!("Checking out #{number}")
                } else {
                    format!("Cloning {}", id.full_name())
                }
                .into(),
                since: Instant::now(),
                title: format!("Review #{number} {title}")
                    .trim()
                    .to_string()
                    .into(),
                behind: Some(space),
            }),
            cx,
        );
        let store = self.store.clone();
        let (chosen_agent, chosen_template) = (self.batch_agent, self.batch_template.clone());
        let work = cx.background_spawn(async move {
            // Clones and fetches take turns behind a lock that blocks its
            // thread; waiting here instead keeps N ticked reviews from
            // holding N of the few background threads the app has.
            let _turn = PREPARING.lock().await;
            let hosting = ph_pulls::hosting::for_repo(&store, &id)
                .map_err(|missing| anyhow::anyhow!("{}", missing.explain()))?;
            let pull = ph_github::pulls::get(&hosting.client, &hosting.repo, number)?;
            let agent = chosen_agent.unwrap_or_else(|| ph_agent::chats::default_agent(&store));
            let template = chosen_template
                .and_then(|id| ph_review::templates::get(&store, &id))
                .filter(|template| template.kind == ph_review::templates::TemplateKind::Review)
                .unwrap_or_else(|| ph_review::templates::default_review(&store));
            let prepared = crate::review::prepare(
                &store, space, &hosting, &pull, &template, "full", agent.name,
            )?;
            anyhow::Ok((prepared, agent))
        });
        super::push_work(
            &mut self._work,
            cx.spawn_in(window, async move |this, cx| {
                let result = work.await;
                _ = this.update_in(cx, |this, window, cx| {
                    this.set_starting(&key, None, cx);
                    match result {
                        Ok((prepared, agent)) => {
                            let session = chat.update(cx, |chat, cx| {
                                chat.start_review_behind(
                                    prepared.review_id,
                                    agent,
                                    prepared.worktree,
                                    prepared.title,
                                    prepared.display,
                                    prepared.prompt,
                                    cx,
                                )
                            });
                            if let Some(session) = session {
                                this.set_running(key, space, session, window, cx);
                            }
                        }
                        Err(err) => window.push_notification(
                            Notification::error(format!(
                                "Could not start the review of #{number}: {err:#}"
                            ))
                            .title("Reviews"),
                            cx,
                        ),
                    }
                    // One that could not start leaves room for the next.
                    this.pump(window, cx);
                });
            }),
        );
    }

    /// Shows pull request `number` of `repo`, on its findings when it has
    /// some to read.
    fn open_from_queue(
        &mut self,
        repo: RepoId,
        number: u64,
        findings: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_pull(repo.clone(), number, window, cx);
        if findings && let Some(panel) = self.panel_of(&repo).cloned() {
            panel.update(cx, |panel, cx| panel.show_findings(cx));
        }
    }

    /// Where `pull` of list `kind` stands: the group it is shown under (its
    /// place among the groups, its title and a word for ids).
    fn section_of(
        &self,
        kind: Inbox,
        pull: &FoundPull,
        key: Option<&PullKey>,
        at_work: bool,
    ) -> (usize, &'static str, &'static str) {
        use ph_review::triage::{Mine, Turn};
        let reviewed = key.and_then(|key| self.reviews.get(key));
        match kind {
            Inbox::NeedsReview => {
                let group = Group::of(at_work, reviewed);
                let at = Group::ALL.iter().position(|g| *g == group).unwrap_or(0);
                (at, group.title(), group.key())
            }
            Inbox::Waiting => {
                // An agent's round here read an older commit, or the user's
                // own review on GitHub did.
                let seen = key
                    .and_then(|key| self.triage.get(key))
                    .filter(|(_, triage)| triage.behind)
                    .map(|_| "");
                let turn = match seen {
                    Some(_) => Turn::Yours,
                    None => Turn::of(pull, None),
                };
                let at = Turn::ALL.iter().position(|t| *t == turn).unwrap_or(0);
                (
                    at,
                    turn.title(),
                    match turn {
                        Turn::Yours => "yours",
                        Turn::Authors => "authors",
                        Turn::Approved => "approved",
                    },
                )
            }
            Inbox::Mine => {
                let mine = Mine::of(pull);
                let at = Mine::ALL.iter().position(|m| *m == mine).unwrap_or(0);
                (
                    at,
                    mine.title(),
                    match mine {
                        Mine::ChangesRequested => "changes",
                        Mine::ChecksFailing => "checks",
                        Mine::Conflicts => "conflicts",
                        Mine::Ready => "ready",
                        Mine::Waiting => "waiting",
                        Mine::Draft => "drafts",
                    },
                )
            }
        }
    }

    /// One inbox list as a queue: each pull request with its repository,
    /// under what needs doing about it.
    pub(super) fn render_inbox(&self, kind: Inbox, cx: &mut Context<Self>) -> AnyElement {
        let list = self.inbox.get(&kind);
        let error = list.and_then(|list| list.error.clone());
        let all = list.map_or(0, |list| list.items.len());
        let mut items = self.listed(kind);
        // What the list leaves out, to say so under it.
        let (drafts, bots) = list.map_or((0, 0), |list| {
            let hidden = |by: fn(&FoundPull) -> bool| {
                list.items
                    .iter()
                    .filter(|(_, pull)| self.shown.hides(pull) && by(pull))
                    .count()
            };
            (hidden(|pull| pull.draft), hidden(|pull| !pull.draft))
        });
        let hidden = if kind == Inbox::NeedsReview {
            all - items.len()
        } else {
            0
        };
        let wanted = self
            .search
            .as_ref()
            .map(|(search, _)| search.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        if !wanted.is_empty() {
            items.retain(|(_, pull)| {
                pull.title.to_lowercase().contains(&wanted)
                    || format!("{}/{}", pull.owner, pull.name)
                        .to_lowercase()
                        .contains(&wanted)
                    || pull.user.login.to_lowercase().contains(&wanted)
                    || format!("#{}", pull.number).contains(&wanted)
            });
        }
        if self.review_first && kind == Inbox::NeedsReview {
            // Assessed ones by rank; the rest after, by date as read.
            items.sort_by_key(|(host, pull)| {
                key_of(host, pull)
                    .and_then(|key| self.triage.get(&key))
                    .map(|(_, triage)| (0, triage.rank()))
                    .unwrap_or((1, (9, 9, 9, ph_review::triage::Size::Large)))
            });
        }
        let shown = items.len();
        let empty = items.is_empty() && !self.loading;
        let mut sections: Vec<(usize, &'static str, &'static str, Vec<AnyElement>)> = Vec::new();
        let mut not_started = Vec::new();
        for (host, pull) in items {
            let key = key_of(&host, &pull);
            let at_work = key.as_ref().and_then(|key| self.at_work(key, cx));
            let (at, title, word) = self.section_of(kind, &pull, key.as_ref(), at_work.is_some());
            let tickable = kind == Inbox::NeedsReview && word == Group::NotStarted.key();
            if tickable && let Some(key) = &key {
                not_started.push(key.clone());
            }
            let row = self.queue_row(kind, &host, &pull, key, tickable, at_work, cx);
            match sections.iter_mut().find(|(known, ..)| *known == at) {
                Some((.., rows)) => rows.push(row),
                None => sections.push((at, title, word, vec![row])),
            }
        }
        sections.sort_by_key(|(at, ..)| *at);
        let sections = sections
            .into_iter()
            .map(|(_, title, word, rows)| {
                v_flex()
                    .id(SharedString::from(format!("queue-group-{word}")))
                    .flex_none()
                    .gap_1()
                    .child(
                        h_flex()
                            .h(ph_ui::rems_from_px(24.))
                            .px_1()
                            .gap_2()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(title)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::NORMAL)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(rows.len().to_string()),
                            ),
                    )
                    .children(rows)
                    .test_support()
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let batch = (!not_started.is_empty()).then(|| self.batch_bar(not_started, cx));
        let view = cx.weak_entity();
        let now = self.shown;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Icon::new(super::render::inbox_icon(kind))
                            .small()
                            .text_color(cx.theme().link),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(kind.label()),
                    )
                    .child(div().flex_1())
                    .when(self.loading, |this| this.child(ph_ui::Loader::new().xsmall()))
                    // A box to narrow the list once it is longer than a
                    // glance takes in.
                    .when(all > 6, |this| {
                        this.children(self.search.as_ref().map(|(search, _)| {
                            div().w(ph_ui::rems_from_px(180.)).child(
                                Input::new(search)
                                    .small()
                                    .prefix(Icon::new(IconName::Search).xsmall()),
                            )
                        }))
                    })
                    .when(kind == Inbox::NeedsReview, |this| {
                        this.child(
                            Button::new("inbox-shown")
                                .xsmall()
                                .ghost()
                                .icon(Icons::SlidersHorizontal)
                                .accessibility_label("What the list shows")
                                .tooltip("Drafts and pull requests from bots")
                                .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                                    let toggle = |shown: Shown| {
                                        let view = view.clone();
                                        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                                            _ = view.update(cx, |this, cx| this.set_shown(shown, cx));
                                        }
                                    };
                                    menu.item(
                                        PopupMenuItem::new("Show drafts")
                                            .checked(now.drafts)
                                            .on_click(toggle(Shown {
                                                drafts: !now.drafts,
                                                ..now
                                            })),
                                    )
                                    .item(
                                        PopupMenuItem::new("Show pull requests from bots")
                                            .checked(now.bots)
                                            .on_click(toggle(Shown {
                                                bots: !now.bots,
                                                ..now
                                            })),
                                    )
                                }),
                        )
                        .child(
                            Button::new("inbox-sort")
                                .xsmall()
                                .ghost()
                                .label(if self.review_first { "Review first" } else { "Newest first" })
                                .tooltip(ph_ui::tip(if self.review_first {
                                    "Sorted for review: pushed since your review, CI not failing, risky, small. Click for newest first."
                                } else {
                                    "Sorted by date. Click to sort for review."
                                }))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let on = !this.review_first;
                                    this.set_review_first(on, cx)
                                })),
                        )
                    }),
            )
            .children(batch)
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                v_flex()
                    .id("github-inbox-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_2()
                    .gap_3()
                    .when(empty, |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(match (kind, wanted.is_empty()) {
                                    (_, false) => "Nothing matches.",
                                    (Inbox::NeedsReview, _) => "Nothing waits for your review.",
                                    (Inbox::Waiting, _) => {
                                        "No pull request you reviewed waits for its author."
                                    }
                                    (Inbox::Mine, _) => "You have no open pull requests.",
                                }),
                        )
                    })
                    .children(sections)
                    // What is left out, and the way to it.
                    .when(hidden > 0 && wanted.is_empty(), |this| {
                        let what = match (drafts, bots) {
                            (drafts, 0) => format!("{drafts} draft{}", plural(drafts)),
                            (0, bots) => format!("{bots} from bots"),
                            (drafts, bots) => {
                                format!("{drafts} draft{} and {bots} from bots", plural(drafts))
                            }
                        };
                        this.child(
                            h_flex()
                                .px_1()
                                .gap_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{what} not shown{}.",
                                    if shown == 0 { "" } else { " here" }
                                ))
                                .child(
                                    Button::new("inbox-show-hidden")
                                        .xsmall()
                                        .ghost()
                                        .label("Show")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_shown(
                                                Shown {
                                                    drafts: true,
                                                    bots: true,
                                                },
                                                cx,
                                            )
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Reviews of several at once, in one line: a tick for all, the button
    /// that starts them, and who reviews (a menu to choose).
    fn batch_bar(&self, not_started: Vec<PullKey>, cx: &mut Context<Self>) -> AnyElement {
        // Ticks on what is still listed as not started.
        let picked = not_started
            .iter()
            .filter(|key| self.selected.contains(*key))
            .count();
        let all = not_started.len();
        let every_one = picked > 0 && picked == all;
        let (agent, template) = self.batch_reviewer();
        let who = agent.map_or("an agent", |agent| agent.name);
        let view = cx.weak_entity();
        let store = self.store.clone();
        let (chosen_agent, chosen_template) =
            (agent.map(|agent| agent.name), self.batch_template.clone());
        h_flex()
            .id("queue-batch")
            .flex_none()
            .px_3()
            .py_1p5()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Checkbox::new("queue-select-all")
                    .checked(every_one)
                    .label(match all {
                        1 => "1 not reviewed yet".to_string(),
                        all => format!("{all} not reviewed yet"),
                    })
                    .tooltip("Tick them all")
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if *checked {
                            this.selected.extend(not_started.iter().cloned());
                        } else {
                            this.selected.clear();
                        }
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            // Who reviews, and by which template: said, and changed here.
            .child(
                Button::new("queue-reviewer")
                    .xsmall()
                    .ghost()
                    .label(format!("{who} · {template}"))
                    .dropdown_caret(true)
                    .tooltip("The agent and the template these reviews use")
                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                        let mut menu = menu.label("Agent");
                        for agent in ph_agent::agents::AGENTS.iter().filter(|agent| {
                            matches!(
                                agent.availability(),
                                ph_agent::agents::Availability::Ready { .. }
                            )
                        }) {
                            let view = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(agent.name)
                                    .checked(chosen_agent == Some(agent.name))
                                    .on_click(move |_, _, cx| {
                                        _ = view.update(cx, |this, cx| {
                                            this.batch_agent = Some(agent);
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu = menu.separator().label("Template");
                        let default = ph_review::templates::default_review(&store).id;
                        for template in ph_review::templates::all(&store)
                            .into_iter()
                            .filter(|t| t.kind == ph_review::templates::TemplateKind::Review)
                        {
                            let view = view.clone();
                            let id = template.id.clone();
                            let chosen = chosen_template.as_deref().unwrap_or(&default) == id;
                            menu = menu.item(
                                PopupMenuItem::new(template.name)
                                    .checked(chosen)
                                    .on_click(move |_, _, cx| {
                                        _ = view.update(cx, |this, cx| {
                                            this.batch_template = Some(id.clone());
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                Button::new("queue-review-selected")
                    .primary()
                    .small()
                    .icon(Icons::Bot)
                    .label(match picked {
                        0 => "Review".to_string(),
                        many => format!("Review {many}"),
                    })
                    .tooltip(ph_ui::tip(format!(
                        "Each runs on its own copy of the repository and only reads; nothing is sent until you finish the review. {} run at a time, the rest wait their turn.",
                        super::REVIEWS_AT_ONCE
                    )))
                    .disabled(picked == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.review_selected(window, cx))),
            )
            .test_support()
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_row(
        &self,
        kind: Inbox,
        host: &str,
        pull: &FoundPull,
        key: Option<PullKey>,
        tickable: bool,
        at_work: Option<AtWork>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let triage = key
            .as_ref()
            .and_then(|key| self.triage.get(key).map(|(_, triage)| triage.clone()));
        let reviewed = key.as_ref().and_then(|key| self.reviews.get(key)).cloned();
        let repo = key.as_ref().map(repo_of);
        let number = pull.number;
        let place = repo
            .as_ref()
            .map(|repo| repo.full_name())
            .unwrap_or_default();
        let id = key
            .as_ref()
            .map(id_of)
            .unwrap_or_else(|| format!("{host}/#{number}"));
        // Only a pull request nobody reviewed is ticked for a first review;
        // in that list the others keep the column so the titles line up.
        let tick = (kind == Inbox::NeedsReview).then(|| {
            div().flex_none().w(ph_ui::rems_from_px(20.)).when_some(
                key.clone().filter(|_| tickable),
                |this, key| {
                    this.child(
                        Checkbox::new(SharedString::from(format!("queue-pick-{id}")))
                            .checked(self.selected.contains(&key))
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.toggle_selected(key.clone(), *checked, cx)
                            })),
                    )
                },
            )
        });
        let findings_first = reviewed.as_ref().is_some_and(|reviewed| reviewed.open > 0);
        let body = v_flex()
            .id(SharedString::from(format!("queue-open-{id}")))
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .cursor_pointer()
            .when_some(repo.clone(), |this, repo| {
                this.on_click(cx.listener(move |this, _, window, cx| {
                    this.open_from_queue(repo.clone(), number, findings_first, window, cx)
                }))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(Icons::GitPullRequest)
                            .small()
                            .text_color(if pull.draft {
                                cx.theme().muted_foreground
                            } else {
                                cx.theme().success
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(pull.title.clone()),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    // One's own need no author.
                    .child(if kind == Inbox::Mine {
                        format!(
                            "{place} #{number} · {}",
                            ph_ui::time::ago_of(&pull.updated_at)
                        )
                    } else {
                        format!(
                            "{place} #{number} · {} · {}",
                            pull.user.login,
                            ph_ui::time::ago_of(&pull.updated_at)
                        )
                    })
                    .children(super::render::pull_chips(kind, pull, triage.as_ref(), cx)),
            )
            .test_support();
        let pushed_since = triage.as_ref().is_some_and(|triage| triage.behind);
        let stopped = key.as_ref().and_then(|key| self.stopped(key, cx));
        // A first review is offered where one is asked of the user, and
        // where the author answered one of theirs.
        let can_review = match kind {
            Inbox::NeedsReview => true,
            Inbox::Waiting => pushed_since,
            Inbox::Mine => false,
        };
        let state = self.queue_state(
            &id,
            key,
            can_review,
            at_work,
            reviewed,
            pushed_since,
            stopped,
            cx,
        );
        h_flex()
            .id(SharedString::from(format!("github-found-{id}")))
            .aria_label(SharedString::from(format!(
                "{place} #{number} {}",
                pull.title
            )))
            .flex_none()
            .items_center()
            .gap_2()
            .px_2p5()
            .py_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .hover(|style| style.bg(cx.theme().list_hover))
            .children(tick)
            .child(body)
            .child(state)
            .test_support()
            .into_any_element()
    }

    /// The end of a row: what an agent made of the pull request, and what
    /// to do about it.
    #[allow(clippy::too_many_arguments)]
    fn queue_state(
        &self,
        id: &str,
        key: Option<PullKey>,
        can_review: bool,
        at_work: Option<AtWork>,
        reviewed: Option<Reviewed>,
        pushed_since: bool,
        stopped: Option<SessionId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let note = |text: String, color: Hsla| {
            div()
                .min_w_0()
                .max_w(ph_ui::rems_from_px(260.))
                .truncate()
                .text_xs()
                .text_color(color)
                .child(text)
        };
        let cell = h_flex().flex_none().max_w_1_2().gap_2().justify_end();
        match (at_work, reviewed) {
            (Some(work), _) => {
                let waits = work.session.is_none()
                    && key.as_ref().is_some_and(|key| self.in_line.contains(key));
                cell.when(!work.needs_you && !waits, |this| {
                    this.child(ph_ui::Loader::new().xsmall())
                })
                .child(note(
                    work.line,
                    if work.needs_you {
                        cx.theme().warning
                    } else {
                        muted
                    },
                ))
                .when_some(work.session, |this, session| {
                    this.child(
                        Button::new(SharedString::from(format!("queue-watch-{id}")))
                            .xsmall()
                            .when(work.needs_you, |this| this.primary())
                            .when(!work.needs_you, |this| this.ghost())
                            .label(if work.needs_you { "Answer" } else { "Watch" })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.show_session_id(session, window, cx)
                            })),
                    )
                })
                // One in line can be taken out again.
                .when_some(key.filter(|_| waits), |this, key| {
                    this.child(
                        Button::new(SharedString::from(format!("queue-unline-{id}")))
                            .xsmall()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.in_line.retain(|waiting| *waiting != key);
                                cx.notify();
                            })),
                    )
                })
                .into_any_element()
            }
            (None, Some(reviewed)) => {
                let review_id = reviewed.review_id;
                let (text, color) = match reviewed.open {
                    0 => ("No findings".to_string(), cx.theme().success),
                    open => (
                        format!(
                            "{open} finding{}, {} {}",
                            if open == 1 { "" } else { "s" },
                            reviewed.worst_count,
                            reviewed.worst
                        ),
                        match ph_review::severity_rank(&reviewed.worst) {
                            0 | 1 => cx.theme().danger,
                            2 => cx.theme().warning,
                            _ => muted,
                        },
                    ),
                };
                cell.child(note(text, color))
                    // The author pushed since: the findings are about an
                    // older commit, and a round on the new one says which
                    // still stand.
                    .when(pushed_since, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("queue-again-{id}")))
                                .xsmall()
                                .outline()
                                .label("Check the fixes")
                                .tooltip("The author pushed since this review")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.review_again(review_id, window, cx)
                                })),
                        )
                    })
                    .when(reviewed.open > 0 && !pushed_since, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("queue-read-{id}")))
                                .xsmall()
                                .primary()
                                .label("Read")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_findings(review_id, window, cx)
                                })),
                        )
                    })
                    .into_any_element()
            }
            (None, None) => cell
                // A review started here ended on an error: said, with the
                // chat that has the reason, instead of looking untouched.
                .when_some(stopped, |this, session| {
                    this.child(note("The review stopped".into(), cx.theme().danger))
                        .child(
                            Button::new(SharedString::from(format!("queue-why-{id}")))
                                .xsmall()
                                .ghost()
                                .label("See why")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.show_session_id(session, window, cx)
                                })),
                        )
                })
                .when_some(key.filter(|_| can_review), |this, key| {
                    this.child(
                        Button::new(SharedString::from(format!("queue-review-{id}")))
                            .xsmall()
                            .ghost()
                            .icon(Icons::Bot)
                            .label("Review")
                            .tooltip("Start an agent on this one")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.line_up(key.clone(), window, cx)
                            })),
                    )
                })
                .into_any_element(),
        }
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reviewed(open: usize) -> Reviewed {
        Reviewed {
            review_id: 1,
            open,
            worst: if open > 0 {
                "major".into()
            } else {
                String::new()
            },
            worst_count: open.min(1),
        }
    }

    #[test]
    fn a_pull_request_is_grouped_by_what_an_agent_made_of_it() {
        assert_eq!(Group::of(false, None), Group::NotStarted);
        assert_eq!(Group::of(false, Some(&reviewed(2))), Group::Findings);
        assert_eq!(Group::of(false, Some(&reviewed(0))), Group::Clean);
        // A round under way comes before what the last one found.
        assert_eq!(Group::of(true, Some(&reviewed(2))), Group::AtWork);
        assert_eq!(Group::of(true, None), Group::AtWork);
    }
}
