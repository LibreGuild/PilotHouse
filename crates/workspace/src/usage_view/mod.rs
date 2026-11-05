//! *Usage*: what the coding agents used, in a compact dialog.
//!
//! One page, read top to bottom:
//!
//! - **The period** (a menu: the last 7, 30 or 90 days, or 12 months) and
//!   **what to list by**: agents, workspaces (where the work happened), or
//!   models.
//! - **The headline**: tokens processed and sessions, with the cost agents
//!   reported, if any; a strip of bars below, a day, week or month each,
//!   coloured by agent, that reads out what you point at.
//! - **A warning** when an agent is close to one of its limits.
//! - **The list**, busiest first, each line with its share. An agent's line
//!   also shows its limit windows and opens to its details: when each
//!   window resets, its models, its busiest day, and where its numbers
//!   come from.
//! - **How this is counted**, folded away.
//!
//! Numbers come from the agents' own session files on this computer and
//! from what agents reported in PilotHouse chats ([`data`]); limits from
//! what the agents report while they work ([`ph_agent::rate_limits`]).
//! *At API prices* is an estimate from the price list shipped with
//! PilotHouse ([`prices`]), updated only when the user asks. It is all read
//! when the dialog opens, or on *Refresh*, off the UI thread.

mod data;
mod prices;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    Window, div, px, relative,
};
use jiff::ToSpan as _;
use ph_agent::agents::{AGENTS, AgentSpec};
use ph_agent::rate_limits::{Limit, LimitState, RateLimits};
use ph_agent::usage_logs::Progress;
use ph_agent::usage_meter::{compact, money};
use ph_ui::{Icons, MenuButton as _};

use crate::AppState;
use data::{Dashboard, Group, Key, Line, Place, Range, Source, Summary};

/// Models shown in an agent's details.
const MODELS_SHOWN: usize = 5;
/// Lines listed before *Show all*.
const LINES_SHOWN: usize = 8;
/// A limit window this full is called out.
const CLOSE_TO_LIMIT: f32 = 0.7;
/// The list's number columns: tokens, sessions and the estimate.
const TOKENS_W: f32 = 64.;
const SESSIONS_W: f32 = 64.;
const COST_W: f32 = 80.;
/// A workspace's or model's share: its bar and percentage.
const SHARE_W: f32 = 90.;
const PERCENT_W: f32 = 36.;

pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(UsageView::new);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Usage").w(px(600.)).child(view.clone())
    });
}

struct UsageView {
    range: Range,
    group: Group,
    dashboard: Option<Arc<Dashboard>>,
    /// While reading: how far it got.
    reading: Option<Arc<Progress>>,
    /// When the numbers were read.
    read_at: Option<jiff::Zoned>,
    /// The bar under the pointer.
    hovered_bar: Option<usize>,
    /// The agent whose details are open.
    open_agent: Option<&'static str>,
    show_all: bool,
    explained: bool,
    /// While *Update prices* downloads the list.
    updating_prices: bool,
    /// What the last *Update prices* came to, when it failed.
    prices_failed: bool,
    _load: Option<Task<()>>,
    _tick: Option<Task<()>>,
    _prices: Option<Task<()>>,
}

impl UsageView {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            range: Range::Month,
            group: Group::Agent,
            dashboard: None,
            reading: None,
            read_at: None,
            hovered_bar: None,
            open_agent: None,
            show_all: false,
            explained: false,
            updating_prices: false,
            prices_failed: false,
            _load: None,
            _tick: None,
            _prices: None,
        };
        view.reload(cx);
        view
    }

    /// Reads everything again: the agents' files (only what changed is
    /// parsed again), PilotHouse's records, the workspaces and which CLIs
    /// are installed.
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.reading.is_some() {
            return;
        }
        let progress = Arc::new(Progress::default());
        self.reading = Some(progress.clone());
        cx.notify();
        let store = AppState::store(cx).clone();
        let work = cx.background_spawn(async move {
            let tz = jiff::tz::TimeZone::system();
            let now = jiff::Zoned::now().with_time_zone(tz.clone());
            let today = now.date();
            let since = today.checked_sub(data::DAYS_READ.days()).unwrap_or(today);
            let since_ms = since
                .to_zoned(tz.clone())
                .map(|start| start.timestamp().as_millisecond())
                .unwrap_or(0);
            let scan = ph_agent::usage_logs::scan(since_ms, &progress);
            let recorded = store.usage_days(&since.to_string()).unwrap_or_else(|err| {
                tracing::warn!("reading usage failed: {err:#}");
                Vec::new()
            });
            let projects = store.projects().unwrap_or_default();
            let prices = prices::Prices::load();
            let dashboard =
                Dashboard::build(today, &tz, scan, recorded, &projects, &prices, |spec| {
                    ph_env::which(spec.cli).is_some()
                });
            (dashboard, now)
        });
        // Redraws the progress while reading.
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        }));
        self._load = Some(cx.spawn(async move |this, cx| {
            let (dashboard, now) = work.await;
            _ = this.update(cx, |this, cx| {
                this.dashboard = Some(Arc::new(dashboard));
                this.read_at = Some(now);
                this.reading = None;
                this._tick = None;
                cx.notify();
            });
        }));
    }

    /// Downloads the latest price list (only on the user's click), then
    /// reads the numbers again with it.
    fn update_prices(&mut self, cx: &mut Context<Self>) {
        if self.updating_prices {
            return;
        }
        self.updating_prices = true;
        self.prices_failed = false;
        cx.notify();
        let work = cx.background_spawn(async move {
            prices::Prices::update(jiff::Timestamp::now().as_millisecond()).is_some()
        });
        self._prices = Some(cx.spawn(async move |this, cx| {
            let updated = work.await;
            _ = this.update(cx, |this, cx| {
                this.updating_prices = false;
                this.prices_failed = !updated;
                if updated {
                    this.reload(cx);
                }
                cx.notify();
            });
        }));
    }
}

impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match self.dashboard.clone() {
            None => self.reading_note(cx).into_any_element(),
            Some(dashboard) => {
                let summary = dashboard.summary(self.range, self.group);
                let limits = RateLimits::all(cx);
                v_flex()
                    .gap_5()
                    .child(self.headline(&dashboard, &summary, cx))
                    .child(self.bars(&summary, cx))
                    .children(limit_warning(&limits, cx))
                    .child(self.list(&dashboard, &summary, &limits, cx))
                    .into_any_element()
            }
        };
        v_flex()
            .gap_5()
            .pb_1()
            .child(self.controls(cx))
            .child(body)
            .child(self.footer(cx))
    }
}

impl UsageView {
    /// The period's menu, what to list by, and *Refresh*.
    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        let range = self.range;
        h_flex()
            .gap_2()
            .child(
                Button::new("usage-range")
                    .outline()
                    .small()
                    .label(range.label())
                    .icon(Icons::Calendar)
                    .dropdown_caret(true)
                    .accessibility_label(format!("Period: {}", range.label()))
                    .menu(gpui_kit::Anchor::TopLeft, move |mut menu, _, _| {
                        for option in Range::ALL {
                            let entity = entity.clone();
                            menu = menu.item(
                                PopupMenuItem::new(option.label())
                                    .checked(option == range)
                                    .on_click(move |_, _, cx| {
                                        _ = entity.update(cx, |this, cx| {
                                            this.range = option;
                                            this.hovered_bar = None;
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(div().flex_1())
            .child(
                ButtonGroup::new("usage-group")
                    .small()
                    .outline()
                    .child(
                        Button::new("usage-by-agent")
                            .label("Agents")
                            .selected(self.group == Group::Agent),
                    )
                    .child(
                        Button::new("usage-by-workspace")
                            .label("Workspaces")
                            .selected(self.group == Group::Workspace),
                    )
                    .child(
                        Button::new("usage-by-model")
                            .label("Models")
                            .selected(self.group == Group::Model),
                    )
                    .on_click(cx.listener(|this, picked: &Vec<usize>, _, cx| {
                        this.group = match picked.first() {
                            Some(1) => Group::Workspace,
                            Some(2) => Group::Model,
                            _ => Group::Agent,
                        };
                        this.show_all = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("usage-refresh")
                    .ghost()
                    .small()
                    .icon(Icons::RotateCw)
                    .loading(self.reading.is_some() && self.dashboard.is_some())
                    .tooltip("Read again")
                    .accessibility_label("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
    }

    /// While the first read runs: what it reads, and how far it got.
    fn reading_note(&self, cx: &App) -> impl IntoElement {
        let (done, files) = self
            .reading
            .as_ref()
            .map(|progress| {
                (
                    progress.done.load(Ordering::Relaxed),
                    progress.files.load(Ordering::Relaxed),
                )
            })
            .unwrap_or_default();
        v_flex()
            .py_8()
            .gap_3()
            .items_center()
            .child(div().text_sm().child("Reading the agents' session files…"))
            .child(div().w(px(240.)).child(bar_track(
                if files > 0 {
                    done as f32 / files as f32
                } else {
                    0.
                },
                cx.theme().info,
                cx,
            )))
            .child(muted(
                if files > 0 {
                    format!(
                        "{} of {} files · the first time takes longest",
                        grouped(done as u64),
                        grouped(files as u64)
                    )
                } else {
                    "Looking for them".to_string()
                },
                cx,
            ))
    }

    /// Tokens and what they would cost at API prices, then sessions and
    /// the rest in a line.
    fn headline(&self, dashboard: &Dashboard, summary: &Summary, cx: &App) -> impl IntoElement {
        let total = &summary.total;
        let mut facts = vec![format!(
            "{} session{} {}",
            grouped(summary.sessions as u64),
            if summary.sessions == 1 { "" } else { "s" },
            self.range.phrase()
        )];
        if let Some(share) = total.cache_share() {
            facts.push(format!("{:.0}% of input from cache", share * 100.));
        }
        if total.output > 0 {
            facts.push(format!("{} output", compact(total.output)));
        }
        if total.cost > 0.0 {
            facts.push(format!(
                "{} reported by agents",
                money(total.cost, total.currency.as_deref())
            ));
        }
        let priced = dashboard.prices_at > 0;
        let mut note = String::from(
            "What these tokens would cost at each model's API list price, from LiteLLM's public price list",
        );
        if let Ok(at) = jiff::Timestamp::from_millisecond(dashboard.prices_at)
            && priced
        {
            note.push_str(&format!(
                " (as of {})",
                at.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%-d %b %Y")
            ));
        }
        note.push_str(". With a subscription you pay its fee instead.");
        if total.unpriced > 0 {
            note.push_str(&format!(
                " {} tokens of models without a listed price are not included.",
                compact(total.unpriced)
            ));
        }
        if !priced {
            note = "No price list could be read, so there is no estimate.".into();
        }
        let block = |value: String, caption: AnyElement, end: bool| {
            v_flex()
                .gap_0p5()
                .when(end, |this| this.items_end())
                .child(
                    div()
                        .text_3xl()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .child(value),
                )
                .child(caption)
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(block(
                        compact(total.tokens()),
                        muted("tokens processed", cx).into_any_element(),
                        false,
                    ))
                    .child(block(
                        if priced {
                            format!("≈ {}", dollars(total.estimate))
                        } else {
                            "–".to_string()
                        },
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(muted("at API prices", cx))
                            .child(info("usage-cost-info", note, cx))
                            .into_any_element(),
                        true,
                    )),
            )
            .child(muted(facts.join(" · "), cx))
    }

    /// A bar per day, week or month, its parts coloured by agent; pointing
    /// at one reads it out below.
    fn bars(&self, summary: &Summary, cx: &mut Context<Self>) -> impl IntoElement {
        const HEIGHT: f32 = 72.;
        let theme = cx.theme();
        let most = summary
            .bars
            .iter()
            .map(data::Bar::tokens)
            .max()
            .unwrap_or(0);
        let hovered = self.hovered_bar.filter(|ix| *ix < summary.bars.len());
        let bars = summary.bars.iter().enumerate().map(|(ix, bar)| {
            let tokens = bar.tokens();
            let height = if most == 0 {
                0.
            } else {
                (tokens as f32 / most as f32 * HEIGHT).max(if tokens > 0 { 2. } else { 0. })
            };
            let dim = hovered.is_some_and(|hovered| hovered != ix);
            // Parts stacked from the bottom, in agent order.
            let parts = AGENTS.iter().enumerate().rev().filter_map(|(agent, spec)| {
                let part = bar.by_agent[agent];
                (part > 0).then(|| {
                    div()
                        .w_full()
                        .h(relative(part as f32 / tokens.max(1) as f32))
                        .bg(spec.tint())
                })
            });
            v_flex()
                .id(("usage-bar", ix))
                .flex_1()
                .h(px(HEIGHT))
                .justify_end()
                .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                    if *hovering {
                        this.hovered_bar = Some(ix);
                    } else if this.hovered_bar == Some(ix) {
                        this.hovered_bar = None;
                    }
                    cx.notify();
                }))
                .child(
                    v_flex()
                        .w_full()
                        .h(px(height))
                        .rounded_t(px(2.))
                        .overflow_hidden()
                        .when(dim, |this| this.opacity(0.45))
                        .children(parts),
                )
                .child(
                    // The baseline, so empty bars still mark their place.
                    div().w_full().h(px(1.)).bg(theme.border),
                )
        });
        let readout: AnyElement = match hovered.and_then(|ix| summary.bars.get(ix)) {
            Some(bar) => {
                let parts: Vec<String> = AGENTS
                    .iter()
                    .zip(&bar.by_agent)
                    .filter(|(_, tokens)| **tokens > 0)
                    .map(|(spec, tokens)| format!("{} {}", spec.short_name(), compact(*tokens)))
                    .collect();
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(bar_name(self.range, bar.start)),
                    )
                    .child(format!("{} tokens", compact(bar.tokens())))
                    .when(!parts.is_empty(), |this| {
                        this.child(muted(parts.join(" · "), cx))
                    })
                    .into_any_element()
            }
            None => muted(
                match (summary.bars.first(), summary.bars.last()) {
                    (Some(first), Some(last)) => format!(
                        "{} – {}, a bar per {}. Point at one to read it.",
                        first.start.strftime("%-d %b"),
                        last.start.strftime("%-d %b %Y"),
                        self.range.unit()
                    ),
                    _ => String::new(),
                },
                cx,
            )
            .into_any_element(),
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_end()
                    .gap(px(if summary.bars.len() > 40 { 1. } else { 3. }))
                    .children(bars),
            )
            .child(div().h(px(20.)).child(readout))
    }

    /// The list, busiest first.
    fn list(
        &self,
        dashboard: &Dashboard,
        summary: &Summary,
        limits: &[(&'static str, Vec<Limit>)],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let most = summary
            .lines
            .iter()
            .map(|line| line.tally.tokens())
            .max()
            .unwrap_or(0);
        let total = summary.total.tokens();
        let shown = if self.show_all {
            summary.lines.len()
        } else {
            LINES_SHOWN
        };
        let hidden = summary.lines.len().saturating_sub(shown);
        let header = |text: &'static str, width: f32| {
            div().w(px(width)).flex_none().text_right().child(text)
        };
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .px_2()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_1().child(match self.group {
                        Group::Agent => "Agent",
                        Group::Workspace => "Where",
                        Group::Model => "Model",
                    }))
                    .when(self.group != Group::Agent, |this| {
                        this.child(header("Share", SHARE_W + PERCENT_W + 8.))
                    })
                    .child(header("Tokens", TOKENS_W))
                    .child(header("Sessions", SESSIONS_W))
                    .child(header("At API prices", COST_W))
                    .child(div().w(px(14.)).flex_none()),
            )
            .children(
                summary
                    .lines
                    .iter()
                    .take(shown)
                    .map(|line| self.line(dashboard, line, most, total, limits, cx)),
            )
            .when(summary.lines.is_empty(), |this| {
                this.child(muted(format!("Nothing used {}.", self.range.phrase()), cx).py_4())
            })
            .when(hidden > 0, |this| {
                this.child(
                    Button::new("usage-show-all")
                        .ghost()
                        .xsmall()
                        .label(format!("Show {hidden} more"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_all = true;
                            cx.notify();
                        })),
                )
            })
    }

    fn line(
        &self,
        dashboard: &Dashboard,
        line: &Line,
        most: u64,
        total: u64,
        limits: &[(&'static str, Vec<Limit>)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let tokens = line.tally.tokens();
        let (mark, name, tip): (AnyElement, String, Option<String>) = match &line.key {
            Key::Agent(id) => {
                let spec = ph_agent::agents::find(id).unwrap_or(&AGENTS[0]);
                (spec.mark(px(18.)), spec.name.to_string(), None)
            }
            Key::Model(agent, model) => {
                let spec = ph_agent::agents::find(agent).unwrap_or(&AGENTS[0]);
                (
                    spec.mark(px(14.)),
                    if model.is_empty() {
                        format!("{} (model not recorded)", spec.short_name())
                    } else {
                        model.to_string()
                    },
                    Some(spec.name.to_string()),
                )
            }
            Key::Place(place) => match place {
                Place::Workspace(id) => {
                    let info = dashboard.workspaces.get(id);
                    (
                        swatch(ph_ui::workspace_color(
                            info.map_or(0, |info| info.shade),
                            theme.is_dark(),
                        ))
                        .into_any_element(),
                        info.map_or_else(|| "A removed workspace".to_string(), |info| info.name.clone()),
                        None,
                    )
                }
                Place::Folder(folder) => (
                    Icon::new(Icons::Folder)
                        .small()
                        .text_color(theme.muted_foreground)
                        .into_any_element(),
                    std::path::Path::new(&**folder)
                        .file_name()
                        .map_or_else(|| folder.to_string(), |name| name.to_string_lossy().into_owned()),
                    Some(tilde(std::path::Path::new(&**folder))),
                ),
                Place::Temporary => (
                    Icon::new(Icons::Folder)
                        .small()
                        .text_color(theme.muted_foreground)
                        .into_any_element(),
                    "Temporary folders".to_string(),
                    Some("Scratch work in the system's temporary folders, together.".to_string()),
                ),
                Place::Unknown => (
                    Icon::new(Icons::Folder)
                        .small()
                        .text_color(theme.muted_foreground)
                        .into_any_element(),
                    "Folder not recorded".to_string(),
                    Some("Gemini CLI's files and some older sessions don't say which folder they worked in.".to_string()),
                ),
            },
        };
        let agent = match line.key {
            Key::Agent(id) => dashboard.agent(id),
            _ => None,
        };
        let open = agent.is_some_and(|agent| self.open_agent == Some(agent.spec.id));
        let numbers = if tokens > 0 {
            format!(
                "{} tokens, {} session{}",
                compact(tokens),
                line.sessions,
                if line.sessions == 1 { "" } else { "s" }
            )
        } else {
            "not used".to_string()
        };
        let column = |width: f32, text: String, strong: bool| {
            div()
                .w(px(width))
                .flex_none()
                .text_right()
                .text_sm()
                .text_color(if strong {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .child(text)
        };
        let columns = [
            column(
                TOKENS_W,
                if tokens > 0 {
                    compact(tokens)
                } else {
                    "–".into()
                },
                tokens > 0,
            ),
            column(
                SESSIONS_W,
                if line.sessions > 0 {
                    grouped(line.sessions as u64)
                } else {
                    "–".into()
                },
                false,
            ),
            column(
                COST_W,
                if line.tally.estimate > 0.0 {
                    dollars(line.tally.estimate)
                } else {
                    "–".into()
                },
                line.tally.estimate > 0.0,
            ),
        ];
        // Under the name: an agent's limits, else its share.
        let status: AnyElement = match agent {
            Some(agent) => {
                let windows = limits
                    .iter()
                    .find(|(name, _)| *name == agent.spec.name)
                    .map(|(_, windows)| windows.as_slice())
                    .unwrap_or_default();
                if windows.iter().any(|limit| limit.used.is_some()) {
                    h_flex()
                        .gap_3()
                        .children(windows.iter().filter(|limit| limit.used.is_some()).map(
                            |limit| {
                                let used = limit.used.unwrap_or(0.);
                                h_flex()
                                    .gap_1p5()
                                    .text_xs()
                                    .child(muted(window_name(&limit.window), cx).text_xs())
                                    .child(div().w(px(36.)).child(bar_track(
                                        used,
                                        state_color(limit.state, cx),
                                        cx,
                                    )))
                                    .child(
                                        div()
                                            .text_color(state_color(limit.state, cx))
                                            .child(format!("{:.0}%", used * 100.)),
                                    )
                            },
                        ))
                        .into_any_element()
                } else {
                    muted(
                        if !agent.installed {
                            "Not installed"
                        } else {
                            "No limits reported"
                        },
                        cx,
                    )
                    .text_xs()
                    .into_any_element()
                }
            }
            None => div().into_any_element(),
        };
        let key = line.key.clone();
        let agent_id = agent.map(|agent| agent.spec.id);
        let row = v_flex()
            .id(SharedString::from(format!("usage-line-{key:?}")))
            .gap_1p5()
            .px_2()
            .py(px(if agent_id.is_some() { 8. } else { 6. }))
            .rounded(theme.radius)
            .when(open, |this| this.bg(theme.secondary))
            .when(agent_id.is_some(), |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(theme.list_hover))
                    .tab_index(0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_agent = if this.open_agent == agent_id {
                            None
                        } else {
                            agent_id
                        };
                        cx.notify();
                    }))
            })
            .aria_label(format!("{name}: {numbers}"))
            .when_some(tip, |this, tip| {
                this.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(18.)).flex().justify_center().child(mark))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(name),
                    )
                    // A workspace or model: its share on the same line.
                    .when(agent_id.is_none(), |this| {
                        this.child(
                            div()
                                .w(px(SHARE_W))
                                .flex_none()
                                .child(share_bar(line, most, cx)),
                        )
                        .child(
                            div()
                                .w(px(PERCENT_W))
                                .flex_none()
                                .text_right()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(share(tokens, total)),
                        )
                    })
                    .children(columns)
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .when(agent_id.is_some(), |this| {
                                this.child(
                                    Icon::new(if open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                                )
                            }),
                    ),
            )
            // An agent: its share, and its limits.
            .when(agent_id.is_some(), |this| {
                this.child(
                    h_flex()
                        .pl(px(26.))
                        .gap_3()
                        .child(
                            div()
                                .w(px(120.))
                                .flex_none()
                                .child(share_bar(line, most, cx)),
                        )
                        .child(div().flex_1().min_w_0().child(status)),
                )
            });
        match agent.filter(|_| open) {
            Some(agent) => v_flex()
                .child(row)
                .child(self.details(dashboard, agent.spec, &line.tally, limits, cx))
                .into_any_element(),
            None => row.into_any_element(),
        }
    }

    /// An agent's details: its limit windows and when they reset, its
    /// models, its busiest day, and where its numbers come from.
    fn details(
        &self,
        dashboard: &Dashboard,
        spec: &'static AgentSpec,
        tally: &data::Tally,
        limits: &[(&'static str, Vec<Limit>)],
        cx: &App,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let detail = dashboard.detail(spec.id, self.range);
        let total: u64 = detail.models.iter().map(|(_, tally)| tally.tokens()).sum();
        let windows: Vec<Limit> = limits
            .iter()
            .find(|(name, _)| *name == spec.name)
            .map(|(_, windows)| windows.clone())
            .unwrap_or_default();
        let source = match dashboard.agent(spec.id).map(|agent| &agent.source) {
            Some(Source::Files(dir)) => format!(
                "Tokens from {}'s own session files ({}), wherever it ran.",
                spec.name,
                tilde(dir)
            ),
            _ => format!(
                "What {} reported in PilotHouse chats; sessions elsewhere are not counted.",
                spec.name
            ),
        };
        let account = match spec.id {
            "claude" => Some("https://claude.ai/settings/usage"),
            "codex" => Some("https://chatgpt.com/codex/settings/usage"),
            _ => None,
        };
        let no_limits = match spec.id {
            "claude" => {
                "Claude Code reports its limits while it works in a PilotHouse chat; none since PilotHouse started."
            }
            "codex" => "Codex keeps its limits in its session files; none found.",
            _ => "Not reported to PilotHouse.",
        };
        v_flex()
            .ml(px(26.))
            .mt_1()
            .mb_2()
            .pl_3()
            .gap_4()
            .border_l_2()
            .border_color(spec.tint().opacity(0.6))
            .child(
                v_flex()
                    .gap_2()
                    .child(label("Limits", cx))
                    .when(windows.is_empty(), |this| this.child(muted(no_limits, cx)))
                    .children(windows.iter().map(|limit| {
                        let used = limit.used.unwrap_or(0.);
                        h_flex()
                            .gap_3()
                            .text_sm()
                            .child(div().w(px(130.)).child(window_name(&limit.window)))
                            .child(div().w(px(90.)).child(bar_track(
                                used,
                                state_color(limit.state, cx),
                                cx,
                            )))
                            .child(
                                div()
                                    .w(px(44.))
                                    .text_color(state_color(limit.state, cx))
                                    .child(match limit.used {
                                        Some(used) => format!("{:.0}%", used * 100.),
                                        None => "–".to_string(),
                                    }),
                            )
                            .child(muted(resets(limit.resets_at), cx))
                    })),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(label("Models", cx))
                    .when(detail.models.is_empty(), |this| {
                        this.child(muted(format!("None used {}.", self.range.phrase()), cx))
                    })
                    .children(
                        detail
                            .models
                            .iter()
                            .take(MODELS_SHOWN)
                            .map(|(model, tally)| {
                                h_flex()
                                    .gap_3()
                                    .text_sm()
                                    .child(div().flex_1().min_w_0().truncate().child(
                                        if model.is_empty() {
                                            "Model not recorded".to_string()
                                        } else {
                                            model.to_string()
                                        },
                                    ))
                                    .child(muted(share(tally.tokens(), total), cx))
                                    .child(
                                        div()
                                            .w(px(64.))
                                            .text_right()
                                            .child(compact(tally.tokens())),
                                    )
                            }),
                    ),
            )
            .when(tally.estimate > 0.0 || tally.cost > 0.0, |this| {
                let mut parts = Vec::new();
                if tally.estimate > 0.0 {
                    parts.push(format!("≈ {} at API prices", dollars(tally.estimate)));
                }
                if tally.unpriced > 0 {
                    parts.push(format!(
                        "{} tokens without a listed price",
                        compact(tally.unpriced)
                    ));
                }
                if tally.cost > 0.0 {
                    parts.push(format!(
                        "{} reported in PilotHouse chats",
                        money(tally.cost, tally.currency.as_deref())
                    ));
                }
                this.child(muted(parts.join(" · "), cx))
            })
            .when_some(detail.busiest, |this, (day, tokens)| {
                this.child(muted(
                    format!(
                        "Busiest day {}: {} tokens · active on {} day{}",
                        day.strftime("%a %-d %b"),
                        compact(tokens),
                        detail.active_days,
                        if detail.active_days == 1 { "" } else { "s" }
                    ),
                    cx,
                ))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(muted(source, cx).text_xs().flex_1().min_w_0())
                    .when_some(account, |this, url| {
                        this.child(
                            Button::new(SharedString::from(format!("usage-account-{}", spec.id)))
                                .ghost()
                                .xsmall()
                                .label("Plan and limits")
                                .icon(Icons::ExternalLink)
                                .tooltip(url)
                                .on_click(move |_, _, cx| cx.open_url(url)),
                        )
                    }),
            )
            .child(div().h(px(1.)).bg(theme.border.opacity(0.0)))
    }

    /// How this is counted (folded away), and when it was read.
    fn footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let sources: Vec<String> = self
            .dashboard
            .as_ref()
            .map(|dashboard| {
                dashboard
                    .agents
                    .iter()
                    .map(|agent| match &agent.source {
                        Source::Files(dir) => {
                            format!("{}: its session files in {}.", agent.spec.name, tilde(dir))
                        }
                        Source::PilotHouse => {
                            format!("{}: what it reported in PilotHouse chats.", agent.spec.name)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let prices_date = self
            .dashboard
            .as_ref()
            .filter(|dashboard| dashboard.prices_at > 0)
            .and_then(|dashboard| jiff::Timestamp::from_millisecond(dashboard.prices_at).ok())
            .map(|at| {
                at.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%-d %b %Y")
                    .to_string()
            });
        v_flex()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(theme.border.opacity(0.6))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Button::new("usage-explain")
                            .ghost()
                            .xsmall()
                            .icon(if self.explained {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .label("How this is counted")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.explained = !this.explained;
                                cx.notify();
                            })),
                    )
                    .child(muted(
                        match &self.read_at {
                            Some(at) => format!("Read at {}", at.strftime("%-I:%M %p")),
                            None => "Reading…".to_string(),
                        },
                        cx,
                    ).text_xs()),
            )
            .when(self.explained, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .px_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Tokens are input (read from a cache or not) and output, as each agent records them. Where an agent keeps its own session files on this computer, they are read, so sessions it ran in a terminal or an editor count too:")
                        .children(sources.into_iter().map(|source| div().pl_2().child(format!("• {source}"))))
                        .child("Files are only read, never sent anywhere; what they held is kept in PilotHouse's data folder (usage-files.json) so the next read is quick.")
                        .child(format!(
                            "\"At API prices\" is an estimate: the tokens at each model's list price, from LiteLLM's public price list{}. PilotHouse ships a copy and never downloads it by itself; Update prices fetches the latest from GitHub when you click it. Costs agents reported in PilotHouse chats are shown as reported. Limits are what the agents report while they work.",
                            prices_date
                                .as_deref()
                                .map(|date| format!(" as of {date}"))
                                .unwrap_or_default()
                        ))
                        .child("Work belongs to the workspace whose folder it ran in.")
                        .child(
                            h_flex()
                                .gap_2()
                                .pt_1()
                                .child(
                                    Button::new("usage-update-prices")
                                        .outline()
                                        .xsmall()
                                        .label(if self.updating_prices {
                                            "Updating prices…"
                                        } else {
                                            "Update prices"
                                        })
                                        .loading(self.updating_prices)
                                        .tooltip("Download the latest price list from GitHub (LiteLLM)")
                                        .on_click(cx.listener(|this, _, _, cx| this.update_prices(cx))),
                                )
                                .when(self.prices_failed, |this| {
                                    this.child("Could not download the price list; the one you had is kept.")
                                }),
                        ),
                )
            })
    }
}

/// The window closest to its limit, when any is close.
fn limit_warning(limits: &[(&'static str, Vec<Limit>)], cx: &App) -> Option<AnyElement> {
    let (agent, limit) = limits
        .iter()
        .flat_map(|(agent, windows)| windows.iter().map(move |limit| (*agent, limit)))
        .filter(|(_, limit)| {
            limit.state != LimitState::Fine || limit.used.is_some_and(|used| used >= CLOSE_TO_LIMIT)
        })
        .max_by(|a, b| a.1.used.unwrap_or(1.).total_cmp(&b.1.used.unwrap_or(1.)))?;
    let color = if limit.state == LimitState::Reached {
        cx.theme().danger
    } else {
        cx.theme().warning
    };
    let used = match limit.used {
        Some(used) if used >= 1. => "has reached".to_string(),
        Some(used) => format!("has used {:.0}% of", used * 100.),
        None => "is close to".to_string(),
    };
    Some(
        h_flex()
            .gap_2()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .bg(color.opacity(0.12))
            .text_sm()
            .child(Icon::new(IconName::TriangleAlert).small().text_color(color))
            .child(format!(
                "{agent} {used} its {} · {}",
                window_name(&limit.window).to_lowercase(),
                resets(limit.resets_at).to_lowercase()
            ))
            .into_any_element(),
    )
}

/// A line's share of the busiest line, split by agent.
fn share_bar(line: &Line, most: u64, cx: &App) -> impl IntoElement {
    let tokens = line.tally.tokens();
    let width = if most == 0 {
        0.
    } else {
        tokens as f32 / most as f32
    };
    h_flex()
        .w_full()
        .h(px(5.))
        .rounded_full()
        .bg(cx.theme().muted)
        .child(
            h_flex()
                .h_full()
                .w(relative(width))
                .rounded_full()
                .overflow_hidden()
                .children(
                    AGENTS
                        .iter()
                        .zip(&line.by_agent)
                        .filter(|(_, part)| **part > 0)
                        .map(|(spec, part)| {
                            div()
                                .h_full()
                                .w(relative(*part as f32 / tokens.max(1) as f32))
                                .bg(spec.tint())
                        }),
                ),
        )
}

fn bar_track(used: f32, color: Hsla, cx: &App) -> impl IntoElement {
    div()
        .w_full()
        .h(px(5.))
        .rounded_full()
        .bg(cx.theme().muted)
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(relative(used.clamp(0., 1.)))
                .bg(color),
        )
}

fn swatch(color: Hsla) -> impl IntoElement {
    div().size(px(10.)).rounded(px(3.)).bg(color)
}

fn state_color(state: LimitState, cx: &App) -> Hsla {
    match state {
        LimitState::Reached => cx.theme().danger,
        LimitState::Warning => cx.theme().warning,
        LimitState::Fine => cx.theme().info,
    }
}

/// What a bar stands for: "Thu 5 Nov", "Week of 1 Nov", "October 2026".
fn bar_name(range: Range, start: jiff::civil::Date) -> String {
    match range {
        Range::Week | Range::Month => start.strftime("%a %-d %b").to_string(),
        Range::Quarter => format!("Week of {}", start.strftime("%-d %b")),
        Range::Year => start.strftime("%B %Y").to_string(),
    }
}

/// `5h` → "5-hour window", `week (Opus)` → "Weekly (Opus)".
fn window_name(window: &str) -> String {
    let (base, rest) = match window.split_once(' ') {
        Some((base, rest)) => (base, format!(" {rest}")),
        None => (window, String::new()),
    };
    let name = match base {
        "week" => "Weekly".to_string(),
        "month" => "Monthly".to_string(),
        base => match base.strip_suffix('h') {
            Some(hours) => format!("{hours}-hour window"),
            None => match base.strip_suffix('d') {
                Some(days) => format!("{days}-day window"),
                None => base.to_string(),
            },
        },
    };
    name + &rest
}

/// "Resets in 4h 19m", or "Resets Sat 7 Nov" more than a day ahead.
fn resets(at: Option<i64>) -> String {
    let Some(at) = at.and_then(|at| jiff::Timestamp::from_second(at).ok()) else {
        return "Reset time not reported".to_string();
    };
    let left = at.as_second() - jiff::Timestamp::now().as_second();
    match left {
        ..=0 => "Resets now".to_string(),
        1..3_600 => format!("Resets in {}m", (left + 59) / 60),
        3_600..86_400 => format!("Resets in {}h {}m", left / 3_600, (left % 3_600) / 60),
        _ => format!(
            "Resets {}",
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%a %-d %b")
        ),
    }
}

/// `$0.42`, `$86.10`, `$7,868`: cents below a hundred dollars.
fn dollars(amount: f64) -> String {
    if amount < 100. {
        format!("${amount:.2}")
    } else {
        format!("${}", grouped(amount.round() as u64))
    }
}

/// `12%`, `<1%` for a sliver.
fn share(part: u64, total: u64) -> String {
    if total == 0 || part == 0 {
        return "–".to_string();
    }
    let percent = part as f64 * 100. / total as f64;
    if percent < 1. {
        "<1%".to_string()
    } else {
        format!("{percent:.0}%")
    }
}

/// `37,246,052`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `~/.claude/projects` for a folder in the home folder.
fn tilde(dir: &std::path::Path) -> String {
    match dirs::home_dir()
        .and_then(|home| dir.strip_prefix(home).ok().map(|rest| rest.to_path_buf()))
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
}

fn label(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text.to_uppercase())
}

fn muted(text: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn info(id: &'static str, text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let text: SharedString = text.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .child(
            Icon::new(IconName::Info)
                .xsmall()
                .text_color(cx.theme().muted_foreground),
        )
        .tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_numbers_read_well() {
        assert_eq!(window_name("5h"), "5-hour window");
        assert_eq!(window_name("week"), "Weekly");
        assert_eq!(window_name("week (Opus)"), "Weekly (Opus)");
        assert_eq!(window_name("7d"), "7-day window");
        assert_eq!(share(1, 1_000), "<1%");
        assert_eq!(share(250, 1_000), "25%");
        assert_eq!(share(0, 1_000), "–");
        assert_eq!(grouped(37_246_052), "37,246,052");
        assert_eq!(grouped(999), "999");
        assert_eq!(resets(None), "Reset time not reported");
        let day = jiff::civil::date(2026, 11, 5);
        assert_eq!(bar_name(Range::Month, day), "Thu 5 Nov");
        assert_eq!(bar_name(Range::Year, day), "November 2026");
        assert_eq!(dollars(0.4249), "$0.42");
        assert_eq!(dollars(7_868.47), "$7,868");
    }
}
