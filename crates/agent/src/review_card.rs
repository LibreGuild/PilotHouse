//! A review round's result in its chat: the verdict, what is new, fixed and
//! still open, and the open findings in a few lines each. The findings live
//! in the pull request's Findings tab, where they are posted, answered and
//! fixed; this card points there.

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, EventEmitter, Global, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use ph_store::Store;
use ph_store::reviews::{Finding, PrReview};

/// What review cards need from the app.
pub struct ReviewService {
    pub store: Store,
}

impl Global for ReviewService {}

#[derive(Debug, Clone)]
pub enum ReviewCardEvent {
    OpenFile {
        path: String,
        line: Option<u64>,
    },
    /// Check the fixes: a new round on the pull request's latest head.
    ReviewAgain {
        review_id: i64,
    },
    /// Show the pull request's Findings tab.
    OpenFindings {
        review_id: i64,
    },
}

pub struct ReviewCard {
    review_id: i64,
    review: Option<PrReview>,
    findings: Vec<Finding>,
    /// Findings the round before had, by label and status.
    before: Vec<(String, String)>,
}

impl EventEmitter<ReviewCardEvent> for ReviewCard {}

/// Most open findings the card lists.
const LISTED: usize = 6;

impl ReviewCard {
    pub fn new(review_id: i64, cx: &mut Context<Self>) -> Self {
        let mut card = Self {
            review_id,
            review: None,
            findings: Vec::new(),
            before: Vec::new(),
        };
        card.reload(true, cx);
        card
    }

    /// Re-reads the round.
    pub fn reload(&mut self, _select: bool, cx: &mut Context<Self>) {
        let Some(store) = cx
            .try_global::<ReviewService>()
            .map(|service| service.store.clone())
        else {
            return;
        };
        self.review = store.pr_review(self.review_id).ok().flatten();
        self.findings = store.pr_findings(self.review_id).unwrap_or_default();
        self.before = self
            .review
            .as_ref()
            .and_then(|review| {
                store
                    .latest_pr_review(&review.pull, Some(review.id))
                    .ok()
                    .flatten()
            })
            .and_then(|earlier| store.pr_findings(earlier.id).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.label, f.status))
            .collect();
        cx.notify();
    }

    fn render_finding(&self, finding: &Finding, cx: &mut Context<Self>) -> AnyElement {
        let (path, line) = (finding.path.clone(), finding.line);
        let round = self.review.as_ref().map_or(1, |r| r.round);
        h_flex()
            .id(SharedString::from(format!("card-finding-{}", finding.id)))
            .gap_2()
            .text_sm()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(severity_color(&finding.severity, cx))
                    .child(format!("{} {}", finding.label, finding.severity)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(finding.title.clone()),
            )
            .when(finding.first_round == round && round > 1, |this| {
                this.child(tag("new", cx.theme().info))
            })
            .when(finding.found_late, |this| {
                this.child(tag("missed earlier", cx.theme().muted_foreground))
            })
            .when(!finding.verified, |this| {
                this.child(tag("code not found", cx.theme().warning))
            })
            .when(!path.is_empty(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().link)
                        .cursor_pointer()
                        .child(match line {
                            Some(line) => format!("{}:{line}", short_path(&path)),
                            None => short_path(&path),
                        })
                        .on_mouse_down(
                            gpui_kit::MouseButton::Left,
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(ReviewCardEvent::OpenFile {
                                    path: path.clone(),
                                    line,
                                })
                            }),
                        ),
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
        let open: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|f| f.status == "open")
            .collect();
        let new = open
            .iter()
            .filter(|f| f.first_round == review.round)
            .count();
        let fixed = self
            .findings
            .iter()
            .filter(|f| {
                f.status == "fixed"
                    && self
                        .before
                        .iter()
                        .any(|(label, status)| *label == f.label && status != "fixed")
            })
            .count();
        let (verdict, verdict_color) = match review.verdict.as_str() {
            "approve" => ("Looks good", cx.theme().success),
            "request_changes" => ("Changes needed", cx.theme().danger),
            _ => ("Comments", cx.theme().info),
        };
        let kind = match review.kind.as_str() {
            "followup" => "Fixes checked",
            "second" => "Second opinion",
            "double_check" => "Double-check",
            "self" => "Your changes",
            _ => "Review",
        };
        let mut counts = Vec::new();
        if review.round > 1 && new > 0 {
            counts.push(format!("{new} new"));
        }
        if fixed > 0 {
            counts.push(format!("{fixed} fixed"));
        }
        counts.push(match open.len() {
            0 => "nothing open".to_string(),
            n => format!("{n} open"),
        });
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
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(if review.pull.number > 0 {
                                format!("{kind} · round {} · #{}", review.round, review.pull.number)
                            } else {
                                format!("{kind} · round {}", review.round)
                            }),
                    )
                    .child(tag(verdict, verdict_color)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} · at {}",
                        counts.join(" · "),
                        &review.head_sha[..review.head_sha.len().min(8)]
                    )),
            )
            .when(!review.summary.is_empty(), |this| {
                this.child(div().text_sm().child(review.summary.clone()))
            })
            .children(
                open.iter()
                    .take(LISTED)
                    .map(|finding| self.render_finding(finding, cx)),
            )
            .when(open.len() > LISTED, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("and {} more", open.len() - LISTED)),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .when(review.kind == "self", |this| {
                        this.when(!open.is_empty(), |this| {
                            this.child(
                                Button::new(("card-notes", review_id as u64))
                                    .small()
                                    .outline()
                                    .label("Show on the lines")
                                    .tooltip("The open findings as notes on the changes in the Git tab, to fix with the agent")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ReviewCardEvent::OpenFindings { review_id })
                                    })),
                            )
                        })
                        .child(
                            Button::new(("card-again", review_id as u64))
                                .small()
                                .ghost()
                                .label("Review again")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(ReviewCardEvent::ReviewAgain { review_id })
                                })),
                        )
                    })
                    .when(review.pull.number > 0, |this| {
                        this.child(
                            Button::new(("card-findings", review_id as u64))
                                .small()
                                .outline()
                                .label("Open findings")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(ReviewCardEvent::OpenFindings { review_id })
                                })),
                        )
                        .child(
                            Button::new(("card-again", review_id as u64))
                                .small()
                                .ghost()
                                .label("Check the fixes")
                                .tooltip("After the author pushes: check each open finding and what changed")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(ReviewCardEvent::ReviewAgain { review_id })
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

fn short_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// The colour of a severity.
pub fn severity_color(severity: &str, cx: &App) -> Hsla {
    match severity {
        "critical" => cx.theme().danger,
        "major" => cx.theme().warning,
        "minor" => cx.theme().info,
        _ => cx.theme().muted_foreground,
    }
}

fn tag(text: &'static str, color: Hsla) -> AnyElement {
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
