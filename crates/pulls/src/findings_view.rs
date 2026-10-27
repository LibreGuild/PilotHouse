//! A pull request's Findings tab: what agent reviews found, kept across
//! rounds, and what became of each (fixed, not a problem, won't fix: by
//! whom, and why).
//!
//! For the reviewer: post them to GitHub (on the code and in one summary
//! comment), read back the author's ticks and replies, ask an agent whether
//! the author is right, check the fixes, get a second opinion or a
//! double-check, fix them in a task, open issues, and remember why a
//! finding was not a problem. For the author, whose PilotHouse has no
//! review of its own: the findings from the summary comment, answered with
//! a reply each, or fixed in a task.

use std::collections::{HashMap, HashSet};

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Textarea, TextareaState},
    notification::Notification,
    spinner::Spinner,
    text::{TextView, TextViewState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div,
};
use ph_github::models::PullRequest;
use ph_review::comment::{SharedReview, status_label};
use ph_review::github;
use ph_review::post::PostMode;
use ph_review::stats::Stats;
use ph_store::Store;
use ph_store::reviews::{Finding, PrReview, PullRef};
use ph_ui::Icons;

use crate::AgentRequest;
use crate::hosting::Hosting;

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
    expanded: HashSet<String>,
    bodies: HashMap<String, Entity<TextViewState>>,
    ask: Option<Ask>,
    input: Entity<TextareaState>,
    busy: Option<&'static str>,
    rules: Vec<String>,
    show_rules: bool,
    stats: Option<Stats>,
    /// A security finding about to become a public issue: its label.
    confirm_public: Option<String>,
    /// The pull request's summary comment, once a round was posted.
    summary_comment: Option<u64>,
    _task: Option<Task<()>>,
    _shared: Option<Task<()>>,
    _check: Option<Task<()>>,
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
        let input = cx.new(|cx| TextareaState::new(window, cx).rows(2));
        let mut view = Self {
            store,
            hosting,
            number,
            pull: None,
            review: None,
            findings: Vec::new(),
            shared: None,
            filter: Filter::Open,
            expanded: HashSet::new(),
            bodies: HashMap::new(),
            ask: None,
            input,
            busy: None,
            rules: Vec::new(),
            show_rules: false,
            stats: None,
            confirm_public: None,
            summary_comment: None,
            _task: None,
            _shared: None,
            _check: None,
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
        let texts: Vec<(String, String)> = self
            .findings()
            .iter()
            .map(|f| (f.label.clone(), finding_text(f)))
            .collect();
        self.bodies = texts
            .into_iter()
            .map(|(label, text)| (label, cx.new(|cx| TextViewState::markdown(&text, cx))))
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

    fn publish(&mut self, mode: PostMode, window: &mut Window, cx: &mut Context<Self>) {
        self.run(
            "Posting",
            move |store, pull| github::publish(store, pull, mode),
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
        }
    }

    /// Opens an issue for finding `label`: a security finding in a public
    /// repository asks first (an issue would disclose it).
    fn open_issue(&mut self, label: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(finding) = self.finding(&label) else {
            return;
        };
        let Some(review) = self.review.clone() else {
            return;
        };
        let security = finding.impact == "security";
        if security && self.confirm_public.as_deref() != Some(label.as_str()) {
            let (store, pull) = (self.store.clone(), self.pull_ref());
            let check = cx.background_spawn(async move { github::is_public(&store, &pull) });
            let label = label.clone();
            self._check = Some(cx.spawn_in(window, async move |this, cx| {
                let public = check.await.unwrap_or(true);
                _ = this.update_in(cx, |this, window, cx| {
                    if public {
                        this.confirm_public = Some(label);
                        cx.notify();
                    } else {
                        this.confirm_public = Some(label.clone());
                        this.open_issue(label, window, cx);
                    }
                });
            }));
            return;
        }
        self.confirm_public = None;
        let (title, body) = github::issue_draft(&finding, &review);
        let id = finding.id;
        self.run(
            "Opening an issue",
            move |store, _| {
                let number = github::create_issue(store, id, &title, &body)?;
                Ok(format!("Opened issue #{number}"))
            },
            window,
            cx,
        );
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

    fn render_toolbar(&self, findings: &[Finding], cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let open = findings.iter().filter(|f| f.status == "open").count();
        let local = self.review.is_some();
        let posted = self.summary_comment.is_some();
        let open_findings: Vec<Finding> = findings
            .iter()
            .filter(|f| f.status == "open")
            .cloned()
            .collect();
        h_flex()
            .gap_1p5()
            .flex_wrap()
            .when(local, |this| {
                this.child(
                    Button::new("findings-post")
                        .small()
                        .primary()
                        .icon(IconName::ArrowUp)
                        .label(if posted { "Update on GitHub" } else { "Post to GitHub" })
                        .tooltip("Open findings on their lines, and one summary comment the author can tick")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.publish(PostMode::Inline, window, cx)
                        })),
                )
                .child(
                    Button::new("findings-post-summary")
                        .small()
                        .ghost()
                        .label("Summary only")
                        .tooltip("Only the summary comment, nothing on the lines")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.publish(PostMode::Conversation, window, cx)
                        })),
                )
            })
            .child(
                Button::new("findings-sync")
                    .small()
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .label(if local { "Read replies" } else { "Refresh" })
                    .tooltip("Read ticks in the summary comment and replies on the findings")
                    .disabled(busy || (local && !posted))
                    .on_click(cx.listener(|this, _, window, cx| this.sync(window, cx))),
            )
            .when(local, |this| {
                this.child(
                    Button::new("findings-check-fixes")
                        .small()
                        .outline()
                        .label("Check the fixes")
                        .tooltip("A new round on the latest commit: each open finding, and what changed")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agent(|pull| AgentRequest::Review { pull, kind: "followup" }, cx)
                        })),
                )
                .child(
                    Button::new("findings-second")
                        .small()
                        .ghost()
                        .label("Second opinion")
                        .tooltip("Another agent reviews it; findings both make are marked")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agent(|pull| AgentRequest::Review { pull, kind: "second" }, cx)
                        })),
                )
                .when(open > 0, |this| {
                    this.child(
                        Button::new("findings-double-check")
                            .small()
                            .ghost()
                            .label("Double-check")
                            .tooltip("An agent tries to disprove each open finding")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.agent(
                                    |pull| AgentRequest::Review { pull, kind: "double_check" },
                                    cx,
                                )
                            })),
                    )
                })
            })
            .when(open > 0, |this| {
                this.child(
                    Button::new("findings-fix-all")
                        .small()
                        .ghost()
                        .icon(Icons::GitBranch)
                        .label(format!("Fix {open} in a task"))
                        .tooltip("A task on the pull request's branch, its chat given the open findings")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let findings = open_findings.clone();
                            this.agent(move |pull| AgentRequest::Fix { pull, findings }, cx)
                        })),
                )
            })
            .child(
                Button::new("findings-copy")
                    .small()
                    .ghost()
                    .icon(IconName::Copy)
                    .accessibility_label("Copy as Markdown")
                    .tooltip("Copy as Markdown")
                    .on_click(cx.listener(|this, _, window, cx| this.copy_markdown(window, cx))),
            )
            .child(
                Button::new("findings-stats")
                    .small()
                    .ghost()
                    .label("Stats")
                    .tooltip("How reviews of this repository went")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_stats(cx))),
            )
            .when_some(self.busy, |this, busy| {
                this.child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .child(Spinner::new().xsmall())
                        .child(busy),
                )
            })
            .into_any_element()
    }

    fn render_summary(&self, findings: &[Finding], cx: &mut Context<Self>) -> AnyElement {
        let (round, kind, sha, verdict, summary, who) = match (&self.review, &self.shared) {
            (Some(review), _) => (
                review.round,
                review.kind.as_str(),
                review.head_sha.clone(),
                review.verdict.clone(),
                review.summary.clone(),
                String::new(),
            ),
            (None, Some(shared)) => (
                shared.round,
                "full",
                shared.sha.clone(),
                shared.verdict.clone(),
                shared.summary.clone(),
                shared.reviewer.clone(),
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
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tag(verdict_text, verdict_color))
                    .child(format!(
                        "Round {round} · {kind} · at {}{}",
                        &sha[..sha.len().min(8)],
                        if who.is_empty() {
                            String::new()
                        } else {
                            format!(" · by {who} (from the summary comment)")
                        }
                    ))
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
                    })
                    .when(count("not_problem") > 0, |this| {
                        this.child(tag_owned(
                            format!("{} not a problem", count("not_problem")),
                            cx.theme().muted_foreground,
                        ))
                    })
                    .when(count("wont_fix") > 0, |this| {
                        this.child(tag_owned(
                            format!("{} won't fix", count("wont_fix")),
                            cx.theme().muted_foreground,
                        ))
                    }),
            )
            .when(!summary.trim().is_empty(), |this| {
                this.child(div().text_sm().child(summary))
            })
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

    fn render_finding(&self, finding: &Finding, cx: &mut Context<Self>) -> AnyElement {
        let label = finding.label.clone();
        let expanded = self.expanded.contains(&label);
        let open = finding.status == "open";
        let local = self.review.is_some();
        let round = self
            .review
            .as_ref()
            .map(|r| r.round)
            .or_else(|| self.shared.as_ref().map(|s| s.round))
            .unwrap_or(1);
        let color = severity_color(&finding.severity, cx);
        let (path, line) = (finding.path.clone(), finding.line);
        let reasoning = ph_review::severity::reasoning(finding);
        let both = finding.agents.contains(", ");
        let status_line = (!open).then(|| {
            let who = match finding.status_source.as_str() {
                "me" => "you".to_string(),
                "author" if finding.status_by.is_empty() => "the author".to_string(),
                "author" => finding.status_by.clone(),
                "double_check" => "the double-check".to_string(),
                _ => "the agent's check".to_string(),
            };
            let mut text = format!("{} · by {who}", status_label(&finding.status));
            if !finding.reason.trim().is_empty() {
                text.push_str(&format!(": {}", finding.reason.trim()));
            }
            text
        });
        v_flex()
            .id(SharedString::from(format!("finding-{label}")))
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(if open {
                color.opacity(0.4)
            } else {
                cx.theme().border
            })
            .bg(color.opacity(if open { 0.04 } else { 0.0 }))
            .child(
                h_flex()
                    .id(SharedString::from(format!("finding-head-{label}")))
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(SharedString::from(format!(
                        "{} {}: {}",
                        finding.label, finding.severity, finding.title
                    )))
                    .gap_2()
                    .min_w_0()
                    .cursor_pointer()
                    .on_click(cx.listener({
                        let label = label.clone();
                        move |this, _, _, cx| {
                            if !this.expanded.remove(&label) {
                                this.expanded.insert(label.clone());
                            }
                            cx.notify();
                        }
                    }))
                    .child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(format!("{} {}", finding.label, finding.severity)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(!open, |this| this.text_color(cx.theme().muted_foreground))
                            .child(finding.title.clone()),
                    )
                    .test_support()
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
                    .when(!open, |this| {
                        this.child(tag(
                            status_label(&finding.status),
                            cx.theme().muted_foreground,
                        ))
                    })
                    .when(finding.status_source == "author" && open, |this| {
                        this.child(tag("author replied", cx.theme().info))
                    })
                    .when_some(finding.issue_number, |this, issue| {
                        this.child(tag_owned(format!("issue #{issue}"), cx.theme().info))
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .pl_5()
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
                                })),
                        )
                    })
                    .when(!reasoning.is_empty(), |this| this.child(reasoning))
                    .when_some(status_line, |this, status| this.child(status)),
            )
            .when(expanded, |this| {
                this.child(self.render_details(finding, local, cx))
            })
            .into_any_element()
    }

    fn render_details(&self, finding: &Finding, local: bool, cx: &mut Context<Self>) -> AnyElement {
        let label = finding.label.clone();
        let open = finding.status == "open";
        let busy = self.busy.is_some();
        let asking = self.ask.as_ref().filter(|ask| match ask {
            Ask::Status { label: l, .. } | Ask::Reply { label: l } | Ask::Rule { label: l } => {
                *l == label
            }
        });
        let answered = local && finding.status_source == "author" && finding.claim.is_empty();
        v_flex()
            .gap_1p5()
            .pl_5()
            .children(self.bodies.get(&label).map(|body| TextView::new(body).text_sm()))
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
                        .child(div().text_xs().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(
                            if agree {
                                "The agent thinks the author is right. Drafted reply:"
                            } else {
                                "The agent thinks the finding still stands. Drafted reply:"
                            },
                        ))
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
                                                this.ask = Some(Ask::Reply { label: label.clone() });
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
                                                    Ask::Reply { label: label.clone() },
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
            .when(self.confirm_public.as_deref() == Some(label.as_str()), |this| {
                let advisory = format!(
                    "https://{}/{}/{}/security/advisories/new",
                    self.hosting.account.host, self.hosting.repo.owner, self.hosting.repo.name
                );
                this.child(
                    v_flex()
                        .gap_1()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().warning.opacity(0.1))
                        .text_sm()
                        .child("This is a security problem and the repository is public: an issue would tell everyone. A private security advisory is safer.")
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!("advisory-{label}")))
                                        .xsmall()
                                        .primary()
                                        .label("Open an advisory")
                                        .on_click(move |_, _, cx| cx.open_url(&advisory)),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("public-issue-{label}")))
                                        .xsmall()
                                        .danger()
                                        .label("Open a public issue anyway")
                                        .on_click(cx.listener({
                                            let label = label.clone();
                                            move |this, _, window, cx| {
                                                this.open_issue(label.clone(), window, cx)
                                            }
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("issue-cancel-{label}")))
                                        .xsmall()
                                        .ghost()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_public = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

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
        let status_button = |what: &'static str, text: &'static str, cx: &mut Context<Self>| {
            let label = label.clone();
            Button::new(id(what))
                .xsmall()
                .outline()
                .label(text)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if what == "fixed" {
                        this.set_status(&label, "fixed", String::new(), window, cx)
                    } else {
                        this.open_ask(
                            Ask::Status {
                                label: label.clone(),
                                status: what,
                            },
                            String::new(),
                            window,
                            cx,
                        )
                    }
                }))
        };
        let single = finding.clone();
        h_flex()
            .gap_1()
            .flex_wrap()
            .when(open, |this| {
                this.child(status_button("fixed", "Fixed", cx))
                    .child(status_button("not_problem", "Not a problem…", cx))
                    .child(status_button("wont_fix", "Won't fix…", cx))
            })
            .when(!open && local, |this| {
                this.child(
                    Button::new(id("reopen"))
                        .xsmall()
                        .ghost()
                        .label("Reopen")
                        .on_click(cx.listener({
                            let label = label.clone();
                            move |this, _, window, cx| {
                                this.set_status(&label, "open", String::new(), window, cx)
                            }
                        })),
                )
            })
            .when(answered, |this| {
                let finding_id = finding.id;
                this.child(
                    Button::new(id("claim"))
                        .xsmall()
                        .ghost()
                        .icon(Icons::Sparkles)
                        .label("Is the author right?")
                        .tooltip("An agent reads the code again and drafts a reply for you to post")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(FindingsEvent::Agent(AgentRequest::Claim { finding_id }))
                        })),
                )
            })
            .when(open, |this| {
                this.child(
                    Button::new(id("fix"))
                        .xsmall()
                        .ghost()
                        .icon(Icons::GitBranch)
                        .label("Fix in a task")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let findings = vec![single.clone()];
                            this.agent(move |pull| AgentRequest::Fix { pull, findings }, cx)
                        })),
                )
            })
            .when(finding.comment_id.is_some() || local, |this| {
                this.child(
                    Button::new(id("reply"))
                        .xsmall()
                        .ghost()
                        .label("Reply…")
                        .on_click(cx.listener({
                            let label = label.clone();
                            move |this, _, window, cx| {
                                this.open_ask(Ask::Reply { label: label.clone() }, String::new(), window, cx)
                            }
                        })),
                )
            })
            .when(local && finding.issue_number.is_none() && finding.status != "fixed", |this| {
                this.child(
                    Button::new(id("issue"))
                        .xsmall()
                        .ghost()
                        .label("Open issue")
                        .tooltip("Track it in an issue (labelled pilothouse) with its place and the fix")
                        .disabled(busy)
                        .on_click(cx.listener({
                            let label = label.clone();
                            move |this, _, window, cx| this.open_issue(label.clone(), window, cx)
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
                        .label("Remember why…")
                        .tooltip("Later reviews of this repository are told, so they don't raise it again")
                        .on_click(cx.listener({
                            let label = label.clone();
                            move |this, _, window, cx| {
                                this.open_ask(Ask::Rule { label: label.clone() }, rule.clone(), window, cx)
                            }
                        })),
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
        };
        let propose = matches!(ask, Ask::Rule { .. });
        v_flex()
            .gap_1()
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(hint))
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
                                .tooltip("Opens a pull request adding it to .pilothouse/review.md, for everyone's reviews")
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
        let count = self.rules.len();
        v_flex()
            .gap_1()
            .child(
                div()
                    .id("findings-rules")
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_rules = !this.show_rules;
                        cx.notify();
                    }))
                    .child(format!(
                        "{count} rule{} remembered for this repository {}",
                        if count == 1 { "" } else { "s" },
                        if self.show_rules { "▾" } else { "▸" }
                    )),
            )
            .when(self.show_rules, |this| {
                this.children(self.rules.iter().enumerate().map(|(ix, rule)| {
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
            })
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
                .child("No agent review of this pull request yet.")
                .child("Review with agent (above) finds problems you can post, track across rounds and fix; if someone reviewed it with PilotHouse, its findings show here from their summary comment.")
                .into_any_element();
        }
        let shown: Vec<&Finding> = findings
            .iter()
            .filter(|f| match self.filter {
                Filter::Open => f.status == "open",
                Filter::Done => f.status != "open",
                Filter::All => true,
            })
            .collect();
        v_flex()
            .p_3()
            .gap_2()
            .child(self.render_summary(&findings, cx))
            .child(self.render_toolbar(&findings, cx))
            .when_some(self.stats.clone(), |this, stats| {
                this.child(self.render_stats(&stats, cx))
            })
            .when(!self.rules.is_empty(), |this| {
                this.child(self.render_rules(cx))
            })
            .child(self.render_filters(&findings, cx))
            .when(shown.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(match self.filter {
                            Filter::Open => "Nothing open.",
                            _ => "Nothing here.",
                        }),
                )
            })
            .children(
                shown
                    .into_iter()
                    .map(|finding| self.render_finding(finding, cx)),
            )
            .into_any_element()
    }
}

/// A finding's text in full, as Markdown.
fn finding_text(finding: &Finding) -> String {
    let mut text = finding.body.trim().to_string();
    if !finding.evidence.trim().is_empty() {
        text.push_str(&format!("\n\n```\n{}\n```", finding.evidence.trim()));
    }
    if !finding.suggestion.trim().is_empty() {
        text.push_str(&format!("\n\n**Fix:** {}", finding.suggestion.trim()));
    }
    if !finding.suggested_code.trim().is_empty() {
        text.push_str(&format!(
            "\n\n```\n{}\n```",
            finding.suggested_code.trim_end()
        ));
    }
    text
}

/// The colour of a severity.
fn severity_color(severity: &str, cx: &App) -> Hsla {
    match severity {
        "critical" => cx.theme().danger,
        "major" => cx.theme().warning,
        "minor" => cx.theme().info,
        _ => cx.theme().muted_foreground,
    }
}

fn tag(text: &'static str, color: Hsla) -> AnyElement {
    tag_owned(text.to_string(), color)
}

fn tag_owned(text: String, color: Hsla) -> AnyElement {
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
