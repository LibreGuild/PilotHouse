//! A pull request's Findings tab: what agent reviews found, kept across
//! rounds, and what became of each (fixed, not a problem, won't fix: by
//! whom, and why).
//!
//! Each finding is a card that reads top to bottom for someone who has not
//! read the code: what goes wrong (its title), what happens, why it
//! matters, an example, the code and the change that fixes it
//! ([`ph_review::writing`]). What a reviewer does most is in sight (it is
//! not a problem, fix it in a task); the rest is behind one menu.
//!
//! Sending the findings is not done here: the pull request's page finishes
//! the review ([`crate::finish_view`]), with the findings picked, the
//! reviewer's own comments and how it ends, in one post.
//!
//! For the author, whose copy has no review of its own: the findings from
//! the summary comment, answered with a reply each, or fixed in a task.

use std::collections::{HashMap, HashSet};

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    menu::PopupMenuItem,
    notification::Notification,
    text::{TextView, TextViewState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
};
use ph_github::models::PullRequest;
use ph_review::comment::{SharedReview, status_label};
use ph_review::github;
use ph_review::post::PostMode;
use ph_review::stats::Stats;
use ph_store::Store;
use ph_store::reviews::{Finding, PrReview, PullRef};
use ph_ui::{Icons, MenuButton as _};

use crate::AgentRequest;
use crate::hosting::Hosting;
use crate::issue_form::{IssueForm, IssueFormEvent};

#[derive(Debug, Clone)]
pub enum FindingsEvent {
    Agent(AgentRequest),
    /// Show a finding's place in the Files tab.
    ShowLine {
        path: String,
        line: Option<u64>,
    },
    /// The findings were read again (their counts may have changed).
    Changed,
    /// Show the chat a review ran in.
    OpenSession(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    Open,
    Done,
    All,
}

/// The little form open under a finding.
#[derive(Debug, Clone, PartialEq)]
enum Ask {
    /// Why it is not a problem / won't be fixed.
    Status { label: String, status: &'static str },
    /// A reply on GitHub.
    Reply { label: String },
    /// A rule for later reviews of the repository.
    Rule { label: String },
    /// The finding's own wording, before it is posted.
    Edit { label: String },
}

/// A finding's text as it is drawn: its parts, and the fix in words.
struct Texts {
    sections: Vec<(Option<&'static str>, Entity<TextViewState>)>,
    fix: Option<Entity<TextViewState>>,
}

pub struct FindingsView {
    store: Store,
    hosting: Hosting,
    number: u64,
    pull: Option<PullRequest>,
    /// The newest round on this computer, and its findings.
    review: Option<PrReview>,
    findings: Vec<Finding>,
    /// The review the summary comment carries, when this computer has none
    /// (the author's PilotHouse).
    shared: Option<SharedReview>,
    filter: Filter,
    /// Open findings read in full unless folded; done ones the other way.
    folded: HashSet<String>,
    unfolded: HashSet<String>,
    texts: HashMap<String, Texts>,
    ask: Option<Ask>,
    input: Entity<TextareaState>,
    /// The title, while a finding's wording is edited.
    title_input: Entity<InputState>,
    /// The tab scrolls itself, so a finding can be brought into sight.
    scroll: gpui_kit::ScrollHandle,
    /// The finding the keyboard is on (j and k move it), by label.
    cursor: Option<String>,
    /// How many things the tab draws above its first finding.
    above: usize,
    /// The issue being written for a finding (in a dialog), by its label.
    issue_form: Option<(Entity<IssueForm>, Subscription)>,
    busy: Option<&'static str>,
    rules: Vec<String>,
    show_rules: bool,
    stats: Option<Stats>,
    /// The pull request's summary comment, once a round was posted.
    summary_comment: Option<u64>,
    _task: Option<Task<()>>,
    _shared: Option<Task<()>>,
}

impl EventEmitter<FindingsEvent> for FindingsView {}

impl FindingsView {
    pub fn new(
        store: Store,
        hosting: Hosting,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 14));
        let title_input = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let mut view = Self {
            store,
            hosting,
            number,
            pull: None,
            review: None,
            findings: Vec::new(),
            shared: None,
            filter: Filter::Open,
            folded: HashSet::new(),
            unfolded: HashSet::new(),
            texts: HashMap::new(),
            ask: None,
            input,
            title_input,
            scroll: gpui_kit::ScrollHandle::default(),
            cursor: None,
            above: 0,
            issue_form: None,
            busy: None,
            rules: Vec::new(),
            show_rules: false,
            stats: None,
            summary_comment: None,
            _task: None,
            _shared: None,
        };
        view.reload(cx);
        view
    }

    fn pull_ref(&self) -> PullRef {
        PullRef {
            host: self.hosting.account.host.clone(),
            owner: self.hosting.repo.owner.clone(),
            repo: self.hosting.repo.name.clone(),
            number: self.number,
        }
    }

    /// The pull request as last loaded (for agent work about it).
    pub fn set_pull(&mut self, pull: PullRequest, cx: &mut Context<Self>) {
        let first = self.pull.is_none();
        self.pull = Some(pull);
        // The author's view: the review from the summary comment.
        if first && self.review.is_none() {
            self.read_shared(cx);
        }
    }

    /// How many findings are open (for the tab's label).
    pub fn open_count(&self) -> usize {
        self.findings()
            .iter()
            .filter(|f| f.status == "open")
            .count()
    }

    /// Whether any review of this pull request is known.
    pub fn has_review(&self) -> bool {
        self.review.is_some() || self.shared.is_some()
    }

    /// The findings that can be sent now: open, their code found, not on
    /// GitHub yet (this computer's review only).
    pub fn sendable(&self) -> Vec<Finding> {
        if self.review.is_none() {
            return Vec::new();
        }
        self.findings
            .iter()
            .filter(|f| f.status == "open" && f.verified && f.comment_kind.is_none())
            .cloned()
            .collect()
    }

    /// Whether an agent reviewed the pull request on this computer (what
    /// Finish review can post; a review read from someone's summary
    /// comment is theirs).
    pub fn has_own_review(&self) -> bool {
        self.review.is_some()
    }

    /// How many findings of this computer's review are still open, those
    /// that cannot be sent among them; `None` without such a review.
    pub fn still_open(&self) -> Option<usize> {
        self.review.as_ref()?;
        Some(self.findings.iter().filter(|f| f.status == "open").count())
    }

    /// Whether this computer's review has something on GitHub already.
    pub fn posted(&self) -> bool {
        self.summary_comment.is_some() || self.findings.iter().any(|f| f.comment_kind.is_some())
    }

    /// The commit this computer's newest round read.
    pub fn reviewed_at(&self) -> Option<String> {
        self.review.as_ref().map(|review| review.head_sha.clone())
    }

    /// The open findings on `path`, for the Files tab: each with the last
    /// line it is about.
    pub fn on_file(&self, path: &str) -> Vec<Finding> {
        self.findings()
            .into_iter()
            .filter(|f| f.status == "open" && f.path == path && f.line.is_some())
            .collect()
    }

    /// Unfolds finding `label` and brings it into sight (the Files tab
    /// leads here).
    pub fn show(&mut self, label: &str, cx: &mut Context<Self>) {
        self.filter = Filter::All;
        self.go_to(label, cx);
    }

    /// The findings the filter shows, in order.
    fn listed(&self) -> Vec<Finding> {
        self.findings()
            .into_iter()
            .filter(|f| match self.filter {
                Filter::Open => f.status == "open",
                Filter::Done => f.status != "open",
                Filter::All => true,
            })
            .collect()
    }

    fn go_to(&mut self, label: &str, cx: &mut Context<Self>) {
        self.folded.remove(label);
        self.unfolded.insert(label.to_string());
        self.cursor = Some(label.to_string());
        if let Some(ix) = self.listed().iter().position(|f| f.label == label) {
            self.scroll.scroll_to_item(self.above + ix);
        }
        cx.notify();
    }

    /// Moves to the next finding (or the one before), unfolded and in sight.
    pub fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let listed = self.listed();
        if listed.is_empty() {
            return;
        }
        let at = self
            .cursor
            .as_ref()
            .and_then(|cursor| listed.iter().position(|f| f.label == *cursor));
        let next = match (at, forward) {
            (None, _) => 0,
            (Some(at), true) => (at + 1).min(listed.len() - 1),
            (Some(at), false) => at.saturating_sub(1),
        };
        let label = listed[next].label.clone();
        self.go_to(&label, cx);
    }

    /// Whether the keyboard is in one of the tab's boxes.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        use gpui_kit::Focusable as _;
        self.input.read(cx).focus_handle(cx).is_focused(window)
            || self
                .title_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
    }

    /// The findings shown: this computer's, else the summary comment's.
    fn findings(&self) -> Vec<Finding> {
        match (&self.review, &self.shared) {
            (Some(_), _) => self.findings.clone(),
            (None, Some(shared)) => shared.findings.iter().map(|f| f.to_finding()).collect(),
            (None, None) => Vec::new(),
        }
    }

    /// Re-reads this computer's findings.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let pull = self.pull_ref();
        match github::current(&self.store, &pull) {
            Ok(Some((review, findings))) => {
                self.review = Some(review);
                self.findings = findings;
            }
            _ => {
                self.review = None;
                self.findings.clear();
            }
        }
        self.rules = github::learned_rules(&self.store, &pull);
        self.summary_comment = self.store.summary_comment_of(&pull).ok().flatten();
        let markdown = |text: &str, cx: &mut Context<Self>| {
            // (Text nested too deeply to parse is shown plain.)
            let text = ph_ui::markdown::guarded(text);
            cx.new(|cx| TextViewState::markdown(&text, cx))
        };
        self.texts = self
            .findings()
            .iter()
            .map(|finding| {
                let sections = ph_review::writing::sections(&finding.body)
                    .into_iter()
                    .map(|section| (section.label, markdown(&section.text, cx)))
                    .collect();
                let fix = Some(finding.suggestion.trim())
                    .filter(|fix| !fix.is_empty())
                    .map(|fix| markdown(fix, cx));
                (finding.label.clone(), Texts { sections, fix })
            })
            .collect();
        cx.emit(FindingsEvent::Changed);
        cx.notify();
    }

    /// Reads the review the summary comment carries (the author's view).
    fn read_shared(&mut self, cx: &mut Context<Self>) {
        let (store, pull) = (self.store.clone(), self.pull_ref());
        let work = cx.background_spawn(async move { github::shared(&store, &pull) });
        self._shared = Some(cx.spawn(async move |this, cx| {
            let found = work.await;
            _ = this.update(cx, |this, cx| {
                if let Ok(Some((_, shared))) = found {
                    this.shared = Some(shared);
                    this.reload(cx);
                }
            });
        }));
    }

    /// Runs GitHub work in the background, then reloads and says how it went.
    fn run(
        &mut self,
        label: &'static str,
        work: impl FnOnce(&Store, &PullRef) -> anyhow::Result<String> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label);
        cx.notify();
        let (store, pull) = (self.store.clone(), self.pull_ref());
        let task = cx.background_spawn(async move { work(&store, &pull) });
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(said) if !said.is_empty() => {
                        window.push_notification(Notification::success(said), cx)
                    }
                    Ok(_) => {}
                    Err(err) => window.push_notification(
                        Notification::error(format!("{err:#}")).title("GitHub"),
                        cx,
                    ),
                }
                if this.review.is_none() && this.shared.is_some() {
                    this.read_shared(cx);
                }
                this.reload(cx);
            });
        }));
    }

    /// Reads the author's ticks and replies on GitHub.
    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_none() || self.summary_comment.is_none() {
            if self.review.is_none() {
                self.read_shared(cx);
            }
            return;
        }
        self.run(
            "Reading replies",
            |store, pull| {
                let synced = github::sync(store, pull)?;
                let mut said = Vec::new();
                for (label, status, who, _) in &synced.changed {
                    let who = if who.is_empty() {
                        "the author"
                    } else {
                        who.as_str()
                    };
                    said.push(format!("{label}: {} ({who})", status_label(status)));
                }
                for (label, who) in &synced.talked {
                    said.push(format!("{label}: {who} replied"));
                }
                Ok(said.join(" · "))
            },
            window,
            cx,
        );
    }

    /// Brings GitHub up to date with what became of the findings there.
    fn update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(
            "Updating",
            |store, pull| github::publish(store, pull, PostMode::Inline),
            window,
            cx,
        );
    }

    fn agent(
        &self,
        request: impl FnOnce(Box<PullRequest>) -> AgentRequest,
        cx: &mut Context<Self>,
    ) {
        if let Some(pull) = &self.pull {
            cx.emit(FindingsEvent::Agent(request(Box::new(pull.clone()))));
        }
    }

    fn finding(&self, label: &str) -> Option<Finding> {
        self.findings().into_iter().find(|f| f.label == label)
    }

    /// Opens form `ask` under its finding, the box filled with `text`.
    fn open_ask(&mut self, ask: Ask, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.focus(window, cx);
        });
        self.ask = Some(ask);
        cx.notify();
    }

    /// Sets a finding's status: the reviewer's word here; the author's a
    /// reply on GitHub (their PilotHouse has no review to change).
    fn set_status(
        &mut self,
        label: &str,
        status: &'static str,
        reason: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self.finding(label) else {
            return;
        };
        self.ask = None;
        if self.review.is_some() {
            if let Err(err) = github::set_status(&self.store, finding.id, status, &reason) {
                window.push_notification(Notification::error(format!("{err:#}")), cx);
            }
            self.reload(cx);
            return;
        }
        let text = match (status, reason.trim()) {
            ("open", _) => return,
            (status, "") => status_label(status).to_string(),
            (status, why) => format!("{}: {why}", status_label(status)),
        };
        self.run(
            "Replying",
            move |store, pull| {
                github::reply(store, pull, &finding, &text)?;
                Ok("Replied on GitHub".into())
            },
            window,
            cx,
        );
    }

    fn submit_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = self.ask.clone() else {
            return;
        };
        let text = self.input.read(cx).value().trim().to_string();
        match ask {
            Ask::Status { label, status } => self.set_status(&label, status, text, window, cx),
            Ask::Reply { label } => {
                if text.is_empty() {
                    return;
                }
                let Some(finding) = self.finding(&label) else {
                    return;
                };
                self.ask = None;
                self.run(
                    "Replying",
                    move |store, pull| {
                        github::reply(store, pull, &finding, &text)?;
                        Ok("Reply posted".into())
                    },
                    window,
                    cx,
                );
            }
            Ask::Rule { .. } => {
                if text.is_empty() {
                    return;
                }
                self.ask = None;
                if let Err(err) = github::learn(&self.store, &self.pull_ref(), &text) {
                    window.push_notification(Notification::error(format!("{err:#}")), cx);
                } else {
                    window.push_notification(
                        Notification::success(
                            "Remembered: later reviews of this repository are told.",
                        ),
                        cx,
                    );
                }
                self.reload(cx);
            }
            Ask::Edit { label } => {
                let title = self.title_input.read(cx).value().trim().to_string();
                let Some(finding) = self.finding(&label) else {
                    return;
                };
                if title.is_empty() || finding.id == 0 {
                    return;
                }
                self.ask = None;
                if let Err(err) = self.store.set_finding_text(finding.id, &title, &text) {
                    window.push_notification(Notification::error(format!("{err:#}")), cx);
                }
                self.reload(cx);
            }
        }
    }

    /// Writes an issue for finding `label` in a dialog: its title and text
    /// (drafted from the finding) and who it is assigned to. A security
    /// finding in a public repository is warned about there.
    fn open_issue(&mut self, label: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(finding), Some(review)) = (self.finding(&label), self.review.clone()) else {
            return;
        };
        let (title, body) = github::issue_draft(&finding, &review);
        let advisory = (finding.impact == "security").then(|| {
            format!(
                "https://{}/{}/{}/security/advisories/new",
                self.hosting.account.host, self.hosting.repo.owner, self.hosting.repo.name
            )
        });
        let hosting = self.hosting.clone();
        let form = cx.new(|cx| IssueForm::new(&hosting, &title, &body, advisory, window, cx));
        let id = finding.id;
        let submitted = cx.subscribe_in(
            &form,
            window,
            move |this, _, event: &IssueFormEvent, window, cx| {
                let IssueFormEvent::Submit {
                    title,
                    body,
                    assignees,
                } = event.clone();
                window.close_dialog(cx);
                this.issue_form = None;
                this.run(
                    "Opening an issue",
                    move |store, _| {
                        let number = github::create_issue(store, id, &title, &body, &assignees)?;
                        Ok(match assignees.as_slice() {
                            [] => format!("Opened issue #{number}"),
                            people => {
                                format!("Opened issue #{number}, assigned to {}", people.join(", "))
                            }
                        })
                    },
                    window,
                    cx,
                );
            },
        );
        IssueForm::open(&form, "Open an issue", window, cx);
        self.issue_form = Some((form, submitted));
    }

    fn copy_markdown(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = match &self.pull {
            Some(pull) => format!("Review of #{} {}", pull.number, pull.title),
            None => format!("Review of #{}", self.number),
        };
        let summary = self
            .review
            .as_ref()
            .map(|r| r.summary.clone())
            .or_else(|| self.shared.as_ref().map(|s| s.summary.clone()))
            .unwrap_or_default();
        let findings = self.findings();
        cx.write_to_clipboard(ClipboardItem::new_string(ph_review::comment::markdown(
            &title, &summary, &findings,
        )));
        window.push_notification(Notification::success("Copied the findings as Markdown"), cx);
    }

    fn toggle_stats(&mut self, cx: &mut Context<Self>) {
        self.stats = match self.stats {
            Some(_) => None,
            None => github::repo_stats(&self.store, &self.pull_ref()).ok(),
        };
        cx.notify();
    }

    /// What the reviewer does next, in one row: the fixes checked when the
    /// author pushed, the open findings fixed in a task; the rest in a menu.
    fn render_toolbar(&self, findings: &[Finding], cx: &mut Context<Self>) -> AnyElement {
        let open = findings.iter().filter(|f| f.status == "open").count();
        let local = self.review.is_some();
        let posted = self.posted();
        let open_findings: Vec<Finding> = findings
            .iter()
            .filter(|f| f.status == "open")
            .cloned()
            .collect();
        // The author pushed since the newest round read the change.
        let behind = match (&self.review, &self.pull) {
            (Some(review), Some(pull)) => review.head_sha != pull.head.sha,
            _ => false,
        };
        let session = self.review.as_ref().and_then(|review| review.session_id);
        let (view, rules) = (cx.weak_entity(), self.rules.len());
        h_flex()
            .gap_1p5()
            .flex_wrap()
            .when(local && behind, |this| {
                this.child(
                    Button::new("findings-check-fixes")
                        .small()
                        .outline()
                        .icon(IconName::RefreshCw)
                        .label("Check the fixes")
                        .tooltip(ph_ui::tip("The author pushed since this review. A new round on the latest commit says which findings still stand, and looks at what changed."))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agent(|pull| AgentRequest::Review { pull, kind: "followup" }, cx)
                        })),
                )
            })
            .when(open > 0, |this| {
                this.child(
                    Button::new("findings-fix-all")
                        .small()
                        .outline()
                        .icon(Icons::GitBranch)
                        .label(match open {
                            1 => "Fix it in a task".to_string(),
                            open => format!("Fix {open} in a task"),
                        })
                        .tooltip(ph_ui::tip("A task on the pull request's branch, its chat given the open findings"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let findings = open_findings.clone();
                            this.agent(move |pull| AgentRequest::Fix { pull, findings }, cx)
                        })),
                )
            })
            .when_some(self.busy, |this, busy| {
                this.child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .child(ph_ui::Loader::new().xsmall())
                        .child(busy),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("findings-more")
                    .small()
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .accessibility_label("More for these findings")
                    .tooltip("Second opinion, double-check, replies, copy, stats")
                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                        let mut menu = menu;
                        let agent = |kind: &'static str| {
                            let view = view.clone();
                            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                                _ = view.update(cx, |this, cx| {
                                    this.agent(|pull| AgentRequest::Review { pull, kind }, cx)
                                });
                            }
                        };
                        if local {
                            if !behind {
                                menu = menu.item(
                                    PopupMenuItem::new("Check the fixes")
                                        .icon(IconName::RefreshCw)
                                        .on_click(agent("followup")),
                                );
                            }
                            menu = menu.item(
                                PopupMenuItem::new("Second opinion from another agent")
                                    .icon(Icons::Bot)
                                    .on_click(agent("second")),
                            );
                            if open > 0 {
                                menu = menu.item(
                                    PopupMenuItem::new("Double-check: try to disprove each finding")
                                        .icon(Icons::ShieldCheck)
                                        .on_click(agent("double_check")),
                                );
                            }
                            menu = menu.separator();
                        }
                        if posted || !local {
                            let sync = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(if local {
                                    "Read the author's replies"
                                } else {
                                    "Read again"
                                })
                                .icon(IconName::RefreshCw)
                                .on_click(move |_, window, cx| {
                                    _ = sync.update(cx, |this, cx| this.sync(window, cx));
                                }),
                            );
                        }
                        if posted && local {
                            let update = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new("Update on GitHub")
                                    .icon(IconName::ArrowUp)
                                    .on_click(move |_, window, cx| {
                                        _ = update.update(cx, |this, cx| this.update(window, cx));
                                    }),
                            );
                        }
                        let copy = view.clone();
                        menu = menu.item(
                            PopupMenuItem::new("Copy as Markdown")
                                .icon(IconName::Copy)
                                .on_click(move |_, window, cx| {
                                    _ = copy.update(cx, |this, cx| this.copy_markdown(window, cx));
                                }),
                        );
                        if let Some(session) = session {
                            let chat = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new("Open the review's chat")
                                    .icon(Icons::MessagesSquare)
                                    .on_click(move |_, _, cx| {
                                        _ = chat.update(cx, |_, cx| {
                                            cx.emit(FindingsEvent::OpenSession(session))
                                        });
                                    }),
                            );
                        }
                        menu = menu.separator();
                        let stats = view.clone();
                        menu = menu.item(
                            PopupMenuItem::new("How reviews of this repository went")
                                .icon(Icons::ChartColumn)
                                .on_click(move |_, _, cx| {
                                    _ = stats.update(cx, |this, cx| this.toggle_stats(cx));
                                }),
                        );
                        if rules > 0 {
                            let show = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(match rules {
                                    1 => "1 rule remembered for this repository".to_string(),
                                    rules => format!("{rules} rules remembered for this repository"),
                                })
                                .icon(Icons::BookOpen)
                                .on_click(move |_, _, cx| {
                                    _ = show.update(cx, |this, cx| {
                                        this.show_rules = !this.show_rules;
                                        cx.notify();
                                    });
                                }),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }

    /// The verdict, how much is open, and the review in two or three
    /// sentences; which round it was, small, under it.
    fn render_summary(&self, findings: &[Finding], cx: &mut Context<Self>) -> AnyElement {
        let (round, kind, sha, verdict, summary, who, agent) = match (&self.review, &self.shared) {
            (Some(review), _) => (
                review.round,
                review.kind.as_str(),
                review.head_sha.clone(),
                review.verdict.clone(),
                review.summary.clone(),
                String::new(),
                review.agent.clone(),
            ),
            (None, Some(shared)) => (
                shared.round,
                "full",
                shared.sha.clone(),
                shared.verdict.clone(),
                shared.summary.clone(),
                shared.reviewer.clone(),
                String::new(),
            ),
            _ => return div().into_any_element(),
        };
        let kind = match kind {
            "followup" => "fixes checked",
            "second" => "second opinion",
            "double_check" => "double-checked",
            _ => "review",
        };
        let count = |status: &str| findings.iter().filter(|f| f.status == status).count();
        let (verdict_text, verdict_color) = match verdict.as_str() {
            "approve" => ("Looks good", cx.theme().success),
            "request_changes" => ("Changes needed", cx.theme().danger),
            _ => ("Comments", cx.theme().info),
        };
        let mut about = format!("Round {round} · {kind}");
        if !agent.is_empty() {
            about.push_str(&format!(" by {agent}"));
        }
        about.push_str(&format!(" · at {}", &sha[..sha.len().min(8)]));
        if !who.is_empty() {
            about.push_str(&format!(" · posted by {who}"));
        }
        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .gap_1p5()
                    .flex_wrap()
                    .child(tag(verdict_text, verdict_color))
                    .children(ph_review::SEVERITIES.iter().filter_map(|severity| {
                        let n = findings
                            .iter()
                            .filter(|f| f.status == "open" && f.severity == *severity)
                            .count();
                        (n > 0).then(|| {
                            tag_owned(format!("{n} {severity}"), severity_color(severity, cx))
                        })
                    }))
                    .when(count("fixed") > 0, |this| {
                        this.child(tag_owned(
                            format!("{} fixed", count("fixed")),
                            cx.theme().success,
                        ))
                    }),
            )
            .when(!summary.trim().is_empty(), |this| {
                this.child(div().text_sm().child(summary))
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(about),
            )
            .into_any_element()
    }

    fn render_filters(&self, findings: &[Finding], cx: &mut Context<Self>) -> AnyElement {
        let open = findings.iter().filter(|f| f.status == "open").count();
        let filters = [
            (Filter::Open, format!("Open {open}")),
            (Filter::Done, format!("Done {}", findings.len() - open)),
            (Filter::All, format!("All {}", findings.len())),
        ];
        h_flex()
            .gap_1()
            .children(filters.into_iter().map(|(filter, label)| {
                let selected = self.filter == filter;
                div()
                    .id(SharedString::from(format!(
                        "findings-filter-{}",
                        label.split(' ').next().unwrap_or_default().to_lowercase()
                    )))
                    .role(gpui_kit::accesskit::Role::Tab)
                    .aria_label(SharedString::from(label.clone()))
                    .px_2()
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if selected {
                        cx.theme().foreground
                    } else {
                        cx.theme().muted_foreground
                    })
                    .when(selected, |this| this.bg(cx.theme().secondary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        cx.notify();
                    }))
                    .child(label)
                    .test_support()
            }))
            .into_any_element()
    }

    /// Whether finding `finding` is read in full: an open one unless
    /// folded, a done one only when asked for.
    fn unfolded(&self, finding: &Finding) -> bool {
        if finding.status == "open" {
            !self.folded.contains(&finding.label)
        } else {
            self.unfolded.contains(&finding.label)
        }
    }

    fn toggle_fold(&mut self, label: &str, open: bool, cx: &mut Context<Self>) {
        let set = if open {
            &mut self.folded
        } else {
            &mut self.unfolded
        };
        if !set.remove(label) {
            set.insert(label.to_string());
        }
        cx.notify();
    }

    fn render_finding(&self, finding: &Finding, cx: &mut Context<Self>) -> AnyElement {
        let label = finding.label.clone();
        let unfolded = self.unfolded(finding);
        let open = finding.status == "open";
        let local = self.review.is_some();
        let round = self
            .review
            .as_ref()
            .map(|r| r.round)
            .or_else(|| self.shared.as_ref().map(|s| s.round))
            .unwrap_or(1);
        let color = if open {
            severity_color(&finding.severity, cx)
        } else {
            cx.theme().muted_foreground
        };
        let (path, line) = (finding.path.clone(), finding.line);
        let reasoning = ph_review::severity::reasoning(finding);
        let both = finding.agents.contains(", ");
        // Sent already, while this one waits on this computer.
        let held = local && open && finding.comment_kind.is_none() && self.posted();
        let status_line = (!open).then(|| {
            let who = match finding.status_source.as_str() {
                "me" => "you".to_string(),
                "author" if finding.status_by.is_empty() => "the author".to_string(),
                "author" => finding.status_by.clone(),
                "double_check" => "the double-check".to_string(),
                _ => "the agent's check".to_string(),
            };
            let mut text = format!("{} by {who}", status_label(&finding.status));
            if !finding.reason.trim().is_empty() {
                text.push_str(&format!(": {}", finding.reason.trim()));
            }
            text
        });
        v_flex()
            .id(SharedString::from(format!("finding-{label}")))
            .gap_1p5()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(if self.cursor.as_deref() == Some(label.as_str()) {
                cx.theme().primary.opacity(0.6)
            } else {
                cx.theme().border
            })
            .child(
                v_flex()
                    .id(SharedString::from(format!("finding-head-{label}")))
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(SharedString::from(format!(
                        "{} {}: {}",
                        finding.label, finding.severity, finding.title
                    )))
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener({
                        let label = label.clone();
                        move |this, _, _, cx| this.toggle_fold(&label, open, cx)
                    }))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .flex_wrap()
                            .child(tag_owned(capitalized(&finding.severity), color))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(finding.label.clone()),
                            )
                            .when(finding.first_round == round && round > 1 && open, |this| {
                                this.child(tag("new", cx.theme().info))
                            })
                            .when(finding.found_late, |this| {
                                this.child(tag("missed earlier", cx.theme().muted_foreground))
                            })
                            .when(both, |this| {
                                this.child(tag("both agents", cx.theme().success))
                            })
                            .when(!finding.verified, |this| {
                                this.child(tag("code not found", cx.theme().warning))
                            })
                            .when(held, |this| {
                                this.child(tag("not sent", cx.theme().muted_foreground))
                            })
                            .when(finding.status_source == "author" && open, |this| {
                                this.child(tag("author replied", cx.theme().info))
                            })
                            .when_some(finding.issue_number, |this, issue| {
                                this.child(tag_owned(format!("issue #{issue}"), cx.theme().info))
                            })
                            .child(div().flex_1())
                            .child(
                                Icon::new(if unfolded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                            ),
                    )
                    // The whole title: it is the finding in one sentence.
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .when(!open, |this| this.text_color(cx.theme().muted_foreground))
                            .child(finding.title.clone()),
                    )
                    .test_support(),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .when(!path.is_empty(), |this| {
                        this.child(
                            div()
                                .id(SharedString::from(format!("finding-place-{label}")))
                                .role(gpui_kit::accesskit::Role::Link)
                                .aria_label(SharedString::from(format!(
                                    "Show {label} in the files"
                                )))
                                .text_color(cx.theme().link)
                                .cursor_pointer()
                                .child(match line {
                                    Some(line) => format!("{path}:{line}"),
                                    None => path.clone(),
                                })
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(FindingsEvent::ShowLine {
                                        path: path.clone(),
                                        line,
                                    })
                                }))
                                .test_support(),
                        )
                    })
                    .when(!reasoning.is_empty(), |this| this.child(reasoning))
                    .when_some(status_line, |this, status| this.child(status)),
            )
            .when(unfolded, |this| {
                this.child(self.render_details(finding, local, cx))
            })
            .into_any_element()
    }

    /// One part of a finding's text under its small label.
    fn part(label: &'static str, body: AnyElement, cx: &App) -> AnyElement {
        v_flex()
            .gap_0p5()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(body)
            .into_any_element()
    }

    /// The code a finding is about and the change that fixes it, as a diff
    /// (the quoted lines alone when no replacement was given).
    fn render_code(&self, finding: &Finding, cx: &App) -> Option<AnyElement> {
        let before = finding.evidence.trim_end();
        if before.trim().is_empty() {
            return None;
        }
        let after = ph_review::comment::unfenced(&finding.suggested_code).trim_end();
        let lines: Vec<String> = if after.trim().is_empty() {
            before.lines().map(|line| format!(" {line}")).collect()
        } else {
            before
                .lines()
                .map(|line| format!("-{line}"))
                .chain(after.lines().map(|line| format!("+{line}")))
                .collect()
        };
        let rows = ph_preview::diff::unnumbered_lines(&lines)
            .iter()
            .map(|line| ph_preview::diff::line_row(line, 0, cx).into_any_element())
            .collect();
        Some(
            div()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .overflow_hidden()
                .text_xs()
                .py_1()
                .child(ph_preview::diff::scroller(
                    SharedString::from(format!("finding-code-{}", finding.label)),
                    rows,
                ))
                .into_any_element(),
        )
    }

    fn render_details(&self, finding: &Finding, local: bool, cx: &mut Context<Self>) -> AnyElement {
        let label = finding.label.clone();
        let open = finding.status == "open";
        let busy = self.busy.is_some();
        let asking = self.ask.as_ref().filter(|ask| match ask {
            Ask::Status { label: l, .. }
            | Ask::Reply { label: l }
            | Ask::Rule { label: l }
            | Ask::Edit { label: l } => *l == label,
        });
        let answered = local && finding.status_source == "author" && finding.claim.is_empty();
        let texts = self.texts.get(&label);
        let has_change = !finding.suggested_code.trim().is_empty();
        v_flex()
            .gap_2p5()
            .pt_1()
            .children(texts.into_iter().flat_map(|texts| {
                texts.sections.iter().map(|(part, view)| {
                    let body = TextView::new(view)
                        .text_sm()
                        .image_source(ph_ui::images_safe::web_only)
                        .into_any_element();
                    match part {
                        Some(part) => Self::part(part, body, cx),
                        None => body,
                    }
                })
            }))
            .children(self.render_code(finding, cx).map(|code| {
                Self::part(
                    if has_change {
                        "The change that fixes it"
                    } else {
                        "The code"
                    },
                    code,
                    cx,
                )
            }))
            .children(texts.and_then(|texts| texts.fix.as_ref()).map(|fix| {
                Self::part(
                    "Fix",
                    TextView::new(fix)
                        .text_sm()
                        .image_source(ph_ui::images_safe::web_only)
                        .into_any_element(),
                    cx,
                )
            }))
            .when(!finding.note.trim().is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Last check: {}", finding.note.trim())),
                )
            })
            .when(!finding.claim.is_empty(), |this| {
                let agree = finding.claim == "agree";
                this.child(
                    v_flex()
                        .gap_1()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().secondary)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(if agree {
                                    "The agent thinks the author is right. Drafted reply:"
                                } else {
                                    "The agent thinks the finding still stands. Drafted reply:"
                                }),
                        )
                        .child(div().text_sm().child(finding.draft_reply.clone()))
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!("claim-post-{label}")))
                                        .xsmall()
                                        .primary()
                                        .label("Post reply")
                                        .disabled(busy)
                                        .on_click(cx.listener({
                                            let (label, text) =
                                                (label.clone(), finding.draft_reply.clone());
                                            move |this, _, window, cx| {
                                                this.ask = Some(Ask::Reply {
                                                    label: label.clone(),
                                                });
                                                this.input.update(cx, |input, cx| {
                                                    input.set_value(text.clone(), window, cx)
                                                });
                                                this.submit_ask(window, cx)
                                            }
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("claim-edit-{label}")))
                                        .xsmall()
                                        .ghost()
                                        .label("Edit first")
                                        .on_click(cx.listener({
                                            let (label, text) =
                                                (label.clone(), finding.draft_reply.clone());
                                            move |this, _, window, cx| {
                                                this.open_ask(
                                                    Ask::Reply {
                                                        label: label.clone(),
                                                    },
                                                    text.clone(),
                                                    window,
                                                    cx,
                                                )
                                            }
                                        })),
                                ),
                        ),
                )
            })
            .map(|this| match asking {
                Some(ask) => this.child(self.render_ask(ask, cx)),
                None => this.child(self.render_actions(finding, local, open, answered, cx)),
            })
            .into_any_element()
    }

    /// What can be done with a finding: the two things a reviewer does most
    /// in sight, the rest in its menu.
    fn render_actions(
        &self,
        finding: &Finding,
        local: bool,
        open: bool,
        answered: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = finding.label.clone();
        let busy = self.busy.is_some();
        let id = |what: &str| SharedString::from(format!("finding-{what}-{label}"));
        let single = finding.clone();
        let view = cx.weak_entity();
        // On GitHub (its line, or the text): a reply has somewhere to go.
        let on_github = finding.comment_kind.is_some() || !local;
        let editable = local && open && finding.comment_kind.is_none();
        let can_issue = local && finding.issue_number.is_none() && finding.status != "fixed";
        let (title, body) = (finding.title.clone(), finding.body.clone());
        let (more_id, menu_label) = (id("more"), label.clone());
        h_flex()
            .gap_1()
            .flex_wrap()
            .when(open, |this| {
                this.child(
                    Button::new(id("not_problem"))
                        .xsmall()
                        .outline()
                        .label("Not a problem")
                        .tooltip(ph_ui::tip("It is wrong, or does not matter here. Say why, and later reviews can be told."))
                        .disabled(busy)
                        .on_click(cx.listener({
                            let label = label.clone();
                            move |this, _, window, cx| {
                                this.open_ask(
                                    Ask::Status {
                                        label: label.clone(),
                                        status: "not_problem",
                                    },
                                    String::new(),
                                    window,
                                    cx,
                                )
                            }
                        })),
                )
                .child(
                    Button::new(id("fix"))
                        .xsmall()
                        .outline()
                        .icon(Icons::GitBranch)
                        .label("Fix in a task")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let findings = vec![single.clone()];
                            this.agent(move |pull| AgentRequest::Fix { pull, findings }, cx)
                        })),
                )
            })
            .when(answered, |this| {
                let finding_id = finding.id;
                this.child(
                    Button::new(id("claim"))
                        .xsmall()
                        .outline()
                        .icon(Icons::Sparkles)
                        .label("Is the author right?")
                        .tooltip(ph_ui::tip("An agent reads the code again and drafts a reply for you to post"))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(FindingsEvent::Agent(AgentRequest::Claim { finding_id }))
                        })),
                )
            })
            .when(!open && local, |this| {
                this.child(
                    Button::new(id("reopen"))
                        .xsmall()
                        .outline()
                        .label("Reopen")
                        .on_click(cx.listener({
                            let label = label.clone();
                            move |this, _, window, cx| {
                                this.set_status(&label, "open", String::new(), window, cx)
                            }
                        })),
                )
            })
            .when(local && finding.status == "not_problem", |this| {
                let rule = if finding.reason.trim().is_empty() {
                    format!("{}: not a problem here.", finding.title.trim())
                } else {
                    format!("{}: {}", finding.title.trim(), finding.reason.trim())
                };
                this.child(
                    Button::new(id("remember"))
                        .xsmall()
                        .ghost()
                        .label("Remember why")
                        .tooltip(ph_ui::tip("Later reviews of this repository are told, so they don't raise it again"))
                        .on_click(cx.listener({
                            let label = label.clone();
                            move |this, _, window, cx| {
                                this.open_ask(Ask::Rule { label: label.clone() }, rule.clone(), window, cx)
                            }
                        })),
                )
            })
            .when(open || can_issue, |this| {
                this.child(
                    Button::new(more_id.clone())
                        .xsmall()
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .accessibility_label("More for this finding")
                        .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                            let mut menu = menu;
                            let ask = |ask: Ask, text: String| {
                                let view = view.clone();
                                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                                    _ = view.update(cx, |this, cx| {
                                        this.open_ask(ask.clone(), text.clone(), window, cx)
                                    });
                                }
                            };
                            if open {
                                let fixed = view.clone();
                                let fixed_label = menu_label.clone();
                                menu = menu
                                    .item(
                                        PopupMenuItem::new("Mark as fixed")
                                            .icon(IconName::Check)
                                            .on_click(move |_, window, cx| {
                                                _ = fixed.update(cx, |this, cx| {
                                                    this.set_status(
                                                        &fixed_label,
                                                        "fixed",
                                                        String::new(),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }),
                                    )
                                    .item(
                                        PopupMenuItem::new("Won't fix…")
                                            .icon(IconName::Minus)
                                            .on_click(ask(
                                                Ask::Status {
                                                    label: menu_label.clone(),
                                                    status: "wont_fix",
                                                },
                                                String::new(),
                                            )),
                                    );
                            }
                            if on_github {
                                menu = menu.item(
                                    PopupMenuItem::new("Reply on GitHub…")
                                        .icon(Icons::MessagesSquare)
                                        .on_click(ask(Ask::Reply { label: menu_label.clone() }, String::new())),
                                );
                            }
                            if editable {
                                let edit = view.clone();
                                let (label, title, body) =
                                    (menu_label.clone(), title.clone(), body.clone());
                                menu = menu.item(
                                    PopupMenuItem::new("Edit the wording…")
                                        .icon(Icons::Pencil)
                                        .on_click(move |_, window, cx| {
                                            _ = edit.update(cx, |this, cx| {
                                                this.title_input.update(cx, |input, cx| {
                                                    input.set_value(title.clone(), window, cx)
                                                });
                                                this.open_ask(
                                                    Ask::Edit { label: label.clone() },
                                                    body.clone(),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }),
                                );
                            }
                            if can_issue {
                                let issue = view.clone();
                                let label = menu_label.clone();
                                menu = menu.separator().item(
                                    PopupMenuItem::new("Open an issue…")
                                        .icon(Icons::CircleDot)
                                        .on_click(move |_, window, cx| {
                                            _ = issue.update(cx, |this, cx| {
                                                this.open_issue(label.clone(), window, cx)
                                            });
                                        }),
                                );
                            }
                            menu
                        }),
                )
            })
            .into_any_element()
    }

    fn render_ask(&self, ask: &Ask, cx: &mut Context<Self>) -> AnyElement {
        let (hint, action) = match ask {
            Ask::Status { status, .. } => (
                match *status {
                    "not_problem" => {
                        "Why is it not a problem? (optional; the author and later rounds see it)"
                    }
                    _ => "Why won't it be fixed? (optional)",
                },
                status_label(status),
            ),
            Ask::Reply { .. } => ("Your reply, posted on GitHub", "Post reply"),
            Ask::Rule { .. } => (
                "The rule later reviews are told (edit it to be general)",
                "Remember",
            ),
            Ask::Edit { .. } => (
                "The finding as it will be posted under your name (Markdown)",
                "Save",
            ),
        };
        let propose = matches!(ask, Ask::Rule { .. });
        let editing = matches!(ask, Ask::Edit { .. });
        v_flex()
            .gap_1()
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(hint))
            .when(editing, |this| {
                this.child(Input::new(&self.title_input).small())
            })
            .child(Textarea::new(&self.input).small())
            .child(
                h_flex()
                    .gap_1()
                    .justify_end()
                    .child(
                        Button::new("ask-cancel")
                            .xsmall()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ask = None;
                                cx.notify();
                            })),
                    )
                    .when(propose, |this| {
                        this.child(
                            Button::new("ask-propose")
                                .xsmall()
                                .outline()
                                .label("Propose to the repository")
                                .tooltip(ph_ui::tip("Opens a pull request adding it to .pilothouse/review.md, for everyone's reviews"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let rule = this.input.read(cx).value().trim().to_string();
                                    if rule.is_empty() {
                                        return;
                                    }
                                    this.ask = None;
                                    this.run(
                                        "Proposing the rule",
                                        move |store, pull| {
                                            let url = github::propose_rule(store, pull, &rule)?;
                                            Ok(format!("Opened {url}"))
                                        },
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    })
                    .child(
                        Button::new("ask-submit")
                            .xsmall()
                            .primary()
                            .label(action)
                            .on_click(cx.listener(|this, _, window, cx| this.submit_ask(window, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_rules(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Later reviews of this repository are told"),
            )
            .children(self.rules.iter().enumerate().map(|(ix, rule)| {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().flex_1().min_w_0().child(rule.clone()))
                    .child(
                        Button::new(("forget-rule", ix))
                            .xsmall()
                            .ghost()
                            .label("Forget")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let _ = github::forget(&this.store, &this.pull_ref(), ix);
                                this.reload(cx);
                            })),
                    )
            }))
            .into_any_element()
    }

    fn render_stats(&self, stats: &Stats, cx: &mut Context<Self>) -> AnyElement {
        let line = |text: String| div().text_xs().child(text);
        v_flex()
            .gap_0p5()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(format!("Reviews of {}", self.hosting.repo.name)),
            )
            .child(line(format!(
                "{} pull request{} · {} findings · {:.1} rounds each",
                stats.pulls,
                if stats.pulls == 1 { "" } else { "s" },
                stats.findings,
                stats.rounds
            )))
            .child(line(format!(
                "{} fixed · {} not a problem · {} won't fix · {} open · {} missed by an earlier round",
                stats.fixed, stats.not_problem, stats.wont_fix, stats.open, stats.found_late
            )))
            .when_some(stats.false_positive_percent(), |this, percent| {
                this.child(line(format!(
                    "{percent}% of decided findings were not a problem"
                )))
            })
            .children(stats.agents.iter().map(|(agent, (found, wrong))| {
                line(format!("{agent}: {found} findings, {wrong} not a problem"))
            }))
            .into_any_element()
    }
}

impl Render for FindingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let findings = self.findings();
        if !self.has_review() {
            return v_flex()
                .p_3()
                .gap_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No agent has reviewed this pull request yet.")
                .child("Review with agent (above) reads the change against the whole codebase. What it finds is listed here, each with what happens, why it matters, an example and a fix. Nothing is sent to GitHub until you finish the review.")
                .into_any_element();
        }
        let done = findings.iter().filter(|f| f.status != "open").count();
        let shown: Vec<&Finding> = findings
            .iter()
            .filter(|f| match self.filter {
                Filter::Open => f.status == "open",
                Filter::Done => f.status != "open",
                Filter::All => true,
            })
            .collect();
        // What is drawn above the first finding, so one can be scrolled to.
        self.above = 2
            + usize::from(self.stats.is_some())
            + usize::from(self.show_rules && !self.rules.is_empty())
            + usize::from(done > 0)
            + usize::from(shown.is_empty());
        let list = v_flex()
            .id("findings-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p_3()
            .gap_3()
            .child(self.render_summary(&findings, cx))
            .child(self.render_toolbar(&findings, cx))
            .when_some(self.stats.clone(), |this, stats| {
                this.child(self.render_stats(&stats, cx))
            })
            .when(self.show_rules && !self.rules.is_empty(), |this| {
                this.child(self.render_rules(cx))
            })
            // Open and done are told apart only once something is done.
            .when(done > 0, |this| {
                this.child(self.render_filters(&findings, cx))
            })
            .when(shown.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(match self.filter {
                            Filter::Open if findings.is_empty() && self.review.is_some() => {
                                "The agent found nothing worth raising. Finish review (above) posts that on the pull request, so the author knows it was reviewed."
                            }
                            Filter::Open if findings.is_empty() => {
                                "The agent found nothing worth raising."
                            }
                            Filter::Open => "Nothing open: every finding is dealt with.",
                            _ => "Nothing here.",
                        }),
                )
            })
            .children(
                shown
                    .into_iter()
                    .map(|finding| self.render_finding(finding, cx)),
            );
        // (It scrolls itself, to bring a finding into sight; the bar is
        // laid over it.)
        div()
            .relative()
            .size_full()
            .child(list)
            .vertical_scrollbar(&self.scroll)
            .into_any_element()
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The colour of a severity.
pub(crate) fn severity_color(severity: &str, cx: &App) -> Hsla {
    match severity {
        "critical" => cx.theme().danger,
        "major" => cx.theme().warning,
        "minor" => cx.theme().info,
        _ => cx.theme().muted_foreground,
    }
}

pub(crate) fn tag(text: &'static str, color: Hsla) -> AnyElement {
    tag_owned(text.to_string(), color)
}

pub(crate) fn tag_owned(text: String, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .px_1p5()
        .rounded_full()
        .text_xs()
        .bg(color.opacity(0.14))
        .text_color(color)
        .child(text)
        .into_any_element()
}
