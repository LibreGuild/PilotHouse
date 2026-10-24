//! Usage across chats: tokens and cost by agent or by project, over the
//! last week, month, or all time. Numbers are what the agents reported;
//! agents that report nothing are not counted.

use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonGroup},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};
use ph_agent::usage_meter::{compact, money};
use ph_store::usage::{UsageBy, UsageTotal};

use crate::AppState;

const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Range {
    Week,
    Month,
    All,
}

impl Range {
    const ALL: [Range; 3] = [Range::Week, Range::Month, Range::All];

    fn label(self) -> &'static str {
        match self {
            Range::Week => "7 days",
            Range::Month => "30 days",
            Range::All => "All time",
        }
    }

    /// The earliest activity counted, in milliseconds.
    fn since(self, now: i64) -> i64 {
        match self {
            Range::Week => now - 7 * DAY_MS,
            Range::Month => now - 30 * DAY_MS,
            Range::All => 0,
        }
    }
}

pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(UsageView::new);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Usage").w(px(620.)).child(view.clone())
    });
}

struct UsageView {
    by: UsageBy,
    range: Range,
    totals: Vec<UsageTotal>,
}

impl UsageView {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            by: UsageBy::Agent,
            range: Range::Month,
            totals: Vec::new(),
        };
        view.reload(cx);
        view
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or_default();
        self.totals = AppState::store(cx)
            .usage_totals(self.by, self.range.since(now))
            .unwrap_or_else(|err| {
                tracing::warn!("reading usage failed: {err:#}");
                Vec::new()
            });
        cx.notify();
    }

    /// An agent id as its name; a project name as it is.
    fn name(&self, key: &str) -> SharedString {
        match self.by {
            UsageBy::Agent => ph_agent::agents::find(key)
                .map(|agent| agent.name.to_string())
                .unwrap_or_else(|| key.to_string())
                .into(),
            UsageBy::Project => key.to_string().into(),
        }
    }
}

impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let header = |text: &'static str| div().w(px(90.)).text_right().child(text);
        let total_cost: f64 = self.totals.iter().map(|total| total.cost).sum();
        let currency = self.totals.iter().find_map(|total| total.currency.clone());
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        ButtonGroup::new("usage-by")
                            .small()
                            .child(
                                Button::new("usage-by-agent")
                                    .label("By agent")
                                    .selected(self.by == UsageBy::Agent),
                            )
                            .child(
                                Button::new("usage-by-project")
                                    .label("By workspace")
                                    .selected(self.by == UsageBy::Project),
                            )
                            .on_click(cx.listener(|this, picked: &Vec<usize>, _, cx| {
                                this.by = if picked.contains(&1) {
                                    UsageBy::Project
                                } else {
                                    UsageBy::Agent
                                };
                                this.reload(cx);
                            })),
                    )
                    .child(
                        ButtonGroup::new("usage-range")
                            .small()
                            .children(Range::ALL.map(|range| {
                                Button::new(range.label())
                                    .label(range.label())
                                    .selected(self.range == range)
                            }))
                            .on_click(cx.listener(|this, picked: &Vec<usize>, _, cx| {
                                if let Some(range) =
                                    picked.first().and_then(|ix| Range::ALL.get(*ix))
                                {
                                    this.range = *range;
                                    this.reload(cx);
                                }
                            })),
                    ),
            )
            .child(
                h_flex()
                    .px_2()
                    .text_xs()
                    .text_color(muted)
                    .child(div().flex_1().child(if self.by == UsageBy::Agent {
                        "Agent"
                    } else {
                        "Workspace"
                    }))
                    .child(header("Chats"))
                    .child(header("Tokens in"))
                    .child(header("Tokens out"))
                    .child(header("Cost")),
            )
            .children(self.totals.iter().map(|total| {
                h_flex()
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.name(&total.key)),
                    )
                    .child(div().w(px(90.)).text_right().child(total.chats.to_string()))
                    .child(
                        div()
                            .w(px(90.))
                            .text_right()
                            .child(compact(total.input_tokens)),
                    )
                    .child(
                        div()
                            .w(px(90.))
                            .text_right()
                            .child(compact(total.output_tokens)),
                    )
                    .child(div().w(px(90.)).text_right().child(if total.cost > 0.0 {
                        money(total.cost, total.currency.as_deref())
                    } else {
                        "–".into()
                    }))
            }))
            .when(self.totals.is_empty(), |this| {
                this.child(
                    div()
                        .py_6()
                        .text_center()
                        .text_sm()
                        .text_color(muted)
                        .child("No usage reported in this time. Not every agent reports it."),
                )
            })
            .when(total_cost > 0.0, |this| {
                this.child(div().text_xs().text_color(muted).child(format!(
                    "Total {}: the agents' own estimates, which may differ from your bill or plan.",
                    money(total_cost, currency.as_deref())
                )))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_reach_back_from_now() {
        let now = 100 * DAY_MS;
        assert_eq!(Range::Week.since(now), 93 * DAY_MS);
        assert_eq!(Range::Month.since(now), 70 * DAY_MS);
        assert_eq!(Range::All.since(now), 0);
    }
}
