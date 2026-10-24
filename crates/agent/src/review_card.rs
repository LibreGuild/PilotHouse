//! The findings of an agent's PR review, shown in its chat: grouped by
//! severity, each with its place in the code, details and suggestion, and
//! whether it is new, still open, fixed or dismissed. The reviewer picks
//! findings and posts them to GitHub as line comments or one comment.
//!
//! Posting is done by the app ([`ReviewService`], set by the workspace),
//! since it needs the GitHub account; this card only asks.

use std::collections::HashSet;
use std::rc::Rc;

use anyhow::Result;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    notification::Notification,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, EventEmitter, Global, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    Window, div,
};
use ph_review::post::PostMode;
use ph_store::Store;
use ph_store::reviews::{Finding, PrReview};

/// Posts findings (by id) of review `review_id`; says what happened.
pub type PostFindings = Rc<dyn Fn(i64, Vec<i64>, PostMode, &mut App) -> Task<Result<String>>>;
/// Tells GitHub which posted findings of `review_id` are fixed.
pub type AnnounceFixed = Rc<dyn Fn(i64, &mut App) -> Task<Result<String>>>;

/// What the app does for review cards.
pub struct ReviewService {
    pub store: Store,
    pub post: PostFindings,
    pub announce_fixed: AnnounceFixed,
}

impl Global for ReviewService {}

#[derive(Debug, Clone)]
pub enum ReviewCardEvent {
    OpenFile { path: String, line: Option<u64> },
    ReviewAgain { review_id: i64 },
}

pub struct ReviewCard {
    review_id: i64,
    review: Option<PrReview>,
    findings: Vec<Finding>,
    selected: HashSet<i64>,
    expanded: HashSet<i64>,
    show_dismissed: bool,
    busy: Option<&'static str>,
    _task: Option<Task<()>>,
}

impl EventEmitter<ReviewCardEvent> for ReviewCard {}

impl ReviewCard {
    pub fn new(review_id: i64, cx: &mut Context<Self>) -> Self {
        let mut card = Self {
            review_id,
            review: None,
            findings: Vec::new(),
            selected: HashSet::new(),
            expanded: HashSet::new(),
            show_dismissed: false,
            busy: None,
            _task: None,
        };
        card.reload(true, cx);
        card
    }

    fn store(cx: &App) -> Option<Store> {
        cx.try_global::<ReviewService>()
            .map(|service| service.store.clone())
    }

    /// Re-reads the review; `select` picks the findings worth posting.
    pub fn reload(&mut self, select: bool, cx: &mut Context<Self>) {
        let Some(store) = Self::store(cx) else { return };
        self.review = store.pr_review(self.review_id).ok().flatten();
        self.findings = store.pr_findings(self.review_id).unwrap_or_default();
        if select {
            self.selected = self
                .findings
                .iter()
                .filter(|f| {
                    matches!(f.status.as_str(), "new" | "open")
                        && f.severity != "nit"
                        && f.comment_id.is_none()
                })
                .map(|f| f.id)
                .collect();
        }
        cx.notify();
    }

    fn post(&mut self, mode: PostMode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = cx.try_global::<ReviewService>() else {
            return;
        };
        let ids: Vec<i64> = self
            .findings
            .iter()
            .filter(|f| self.selected.contains(&f.id))
            .map(|f| f.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let task = (service.post.clone())(self.review_id, ids, mode, cx);
        self.run("Posting…", task, window, cx);
    }

    fn announce(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = cx.try_global::<ReviewService>() else {
            return;
        };
        let task = (service.announce_fixed.clone())(self.review_id, cx);
        self.run("Updating GitHub…", task, window, cx);
    }

    fn run(
        &mut self,
        label: &'static str,
        task: Task<Result<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = Some(label);
        cx.notify();
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(message) => {
                        window.push_notification(Notification::success(message), cx);
                        this.selected.clear();
                    }
                    Err(err) => window.push_notification(
                        Notification::error(format!("{err:#}")).title("GitHub"),
                        cx,
                    ),
                }
                this.reload(false, cx);
            });
        }));
    }

    fn set_status(&mut self, id: i64, status: &str, cx: &mut Context<Self>) {
        if let Some(store) = Self::store(cx) {
            let _ = store.set_finding_status(id, status);
        }
        self.selected.remove(&id);
        self.reload(false, cx);
    }

    fn render_finding(&self, finding: &Finding, cx: &mut Context<Self>) -> AnyElement {
        let id = finding.id;
        let color = severity_color(&finding.severity, cx);
        let expanded = self.expanded.contains(&id);
        let selected = self.selected.contains(&id);
        let fixed = finding.status == "fixed";
        let dismissed = finding.status == "dismissed";
        let (path, line) = (finding.path.clone(), finding.line);
        let place = match finding.line {
            Some(line) => format!("{}:{line}", finding.path),
            None => finding.path.clone(),
        };
        v_flex()
            .flex_none()
            .gap_1()
            .px_2()
            .py_1p5()
            .rounded(cx.theme().radius)
            .border_l_2()
            .border_color(color.opacity(if fixed || dismissed { 0.3 } else { 0.9 }))
            .bg(cx.theme().background.opacity(0.5))
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .when(!fixed && !dismissed, |this| {
                        this.child(
                            Checkbox::new(("finding-select", id as u64))
                                .checked(selected)
                                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                    if *checked {
                                        this.selected.insert(id);
                                    } else {
                                        this.selected.remove(&id);
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        v_flex()
                            .id(("finding", id as u64))
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded.remove(&id) {
                                    this.expanded.insert(id);
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .when(fixed, |this| {
                                        this.line_through().text_color(cx.theme().muted_foreground)
                                    })
                                    .child(finding.title.clone()),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .min_w_0()
                                    .child(status_badge(finding, cx))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .truncate()
                                            .child(place),
                                    ),
                            ),
                    )
                    .when(!path.is_empty(), |this| {
                        this.child(
                            Button::new(("finding-open", id as u64))
                                .flex_none()
                                .xsmall()
                                .ghost()
                                .icon(IconName::ExternalLink)
                                .tooltip("Open in the editor")
                                .accessibility_label("Open in the editor")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(ReviewCardEvent::OpenFile {
                                        path: path.clone(),
                                        line,
                                    })
                                })),
                        )
                    }),
            )
            .when(expanded, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .pl_6()
                        .text_sm()
                        .when(!finding.body.is_empty(), |this| {
                            this.child(finding.body.clone())
                        })
                        .when(!finding.suggestion.is_empty(), |this| {
                            this.child(
                                div()
                                    .p_2()
                                    .rounded(cx.theme().radius)
                                    .bg(cx.theme().secondary)
                                    .text_xs()
                                    .child(format!("Suggestion: {}", finding.suggestion)),
                            )
                        })
                        .child(h_flex().gap_1().child(if dismissed {
                            Button::new(("finding-restore", id as u64))
                                .xsmall()
                                .ghost()
                                .label("Restore")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_status(id, "open", cx)
                                }))
                        } else {
                            Button::new(("finding-dismiss", id as u64))
                                .xsmall()
                                .ghost()
                                .label("Dismiss")
                                .tooltip("Not a problem: re-reviews will not raise it as new")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_status(id, "dismissed", cx)
                                }))
                        })),
                )
            })
            .into_any_element()
    }
}

impl Render for ReviewCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(review) = self.review.clone() else {
            return div().into_any_element();
        };
        let active: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|f| matches!(f.status.as_str(), "new" | "open"))
            .collect();
        let fixed: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|f| f.status == "fixed")
            .collect();
        let dismissed: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|f| f.status == "dismissed")
            .collect();
        let announce = fixed
            .iter()
            .any(|f| f.comment_id.is_some() && !f.fixed_notified);
        let selected = self.selected.len();
        let busy = self.busy;
        let (verdict, verdict_color) = match review.verdict.as_str() {
            "approve" => ("Looks good", cx.theme().success),
            "request_changes" => ("Changes needed", cx.theme().danger),
            _ => ("Comments", cx.theme().info),
        };
        let review_id = self.review_id;
        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.6))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(ph_ui::Icons::GitPullRequest)
                            .small()
                            .text_color(cx.theme().info),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(format!(
                                "Review of #{} {}",
                                review.pull.number, review.title
                            )),
                    )
                    .child(pill(verdict, verdict_color)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(ph_review::SEVERITIES.iter().filter_map(|severity| {
                        let count = active.iter().filter(|f| f.severity == *severity).count();
                        (count > 0).then(|| {
                            pill_owned(format!("{count} {severity}"), severity_color(severity, cx))
                        })
                    }))
                    .when(!fixed.is_empty(), |this| {
                        this.child(pill_owned(
                            format!("{} fixed", fixed.len()),
                            cx.theme().success,
                        ))
                    })
                    .when(active.is_empty() && fixed.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("No findings."),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "at {}",
                                &review.head_sha[..review.head_sha.len().min(8)]
                            )),
                    ),
            )
            .when(!review.summary.is_empty(), |this| {
                this.child(div().text_sm().child(review.summary.clone()))
            })
            .children(
                active
                    .iter()
                    .map(|finding| self.render_finding(finding, cx)),
            )
            .when(!fixed.is_empty(), |this| {
                this.child(
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Fixed since the last review"),
                )
                .children(fixed.iter().map(|finding| self.render_finding(finding, cx)))
            })
            .when(!dismissed.is_empty(), |this| {
                this.child(
                    div()
                        .id(("show-dismissed", review_id as u64))
                        .pt_1()
                        .text_xs()
                        .cursor_pointer()
                        .text_color(cx.theme().muted_foreground)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_dismissed = !this.show_dismissed;
                            cx.notify();
                        }))
                        .child(format!(
                            "{} dismissed {}",
                            dismissed.len(),
                            if self.show_dismissed { "▾" } else { "▸" }
                        )),
                )
                .when(self.show_dismissed, |this| {
                    this.children(
                        dismissed
                            .iter()
                            .map(|finding| self.render_finding(finding, cx)),
                    )
                })
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_24()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(match busy {
                                Some(busy) => SharedString::from(busy),
                                None if selected == 0 => {
                                    "Pick findings to post them to GitHub.".into()
                                }
                                None => format!("{selected} selected").into(),
                            }),
                    )
                    .when(busy.is_some(), |this| this.child(Spinner::new().xsmall()))
                    .child(
                        Button::new(("review-again", review_id as u64))
                            .xsmall()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .label("Review again")
                            .tooltip("A new run on the PR's latest commit, checking these findings")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(ReviewCardEvent::ReviewAgain { review_id })
                            })),
                    )
                    .when(announce, |this| {
                        this.child(
                            Button::new(("announce-fixed", review_id as u64))
                                .xsmall()
                                .outline()
                                .label("Mark fixed on GitHub")
                                .disabled(busy.is_some())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.announce(window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new(("post-conversation", review_id as u64))
                            .xsmall()
                            .outline()
                            .label("Post as one comment")
                            .disabled(busy.is_some() || selected == 0)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.post(PostMode::Conversation, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("post-inline", review_id as u64))
                            .xsmall()
                            .primary()
                            .label("Post as line comments")
                            .disabled(busy.is_some() || selected == 0)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.post(PostMode::Inline, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn severity_color(severity: &str, cx: &App) -> Hsla {
    match severity {
        "critical" => cx.theme().danger,
        "major" => gpui_kit::rgb(0xe0956f).into(),
        "minor" => cx.theme().warning,
        _ => cx.theme().muted_foreground,
    }
}

fn status_badge(finding: &Finding, cx: &App) -> AnyElement {
    let (text, color) = match finding.status.as_str() {
        "new" => ("New", cx.theme().info),
        "open" => ("Still open", cx.theme().warning),
        "fixed" => ("Fixed", cx.theme().success),
        _ => ("Dismissed", cx.theme().muted_foreground),
    };
    h_flex()
        .gap_1()
        .child(pill(text, color))
        .when(finding.comment_id.is_some(), |this| {
            this.child(pill("Posted", cx.theme().muted_foreground))
        })
        .into_any_element()
}

fn pill(text: &'static str, color: Hsla) -> AnyElement {
    pill_owned(text.to_string(), color)
}

fn pill_owned(text: String, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .px_1p5()
        .rounded_full()
        .bg(color.opacity(0.15))
        .text_color(color)
        .text_xs()
        .child(text)
        .into_any_element()
}
