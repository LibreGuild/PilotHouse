//! The Agents view: every chat that is working, waiting for the user or
//! has news, across all workspaces, as a board. It answers "what needs me?"
//! without visiting each workspace; a card opens its chat.
//!
//! It reads the chats each workspace keeps loaded ([`ph_agent::ChatHub`]),
//! so it shows what is live in this run of PilotHouse, not old history.

use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, spinner::Spinner, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use ph_agent::{ChatState, ChatSummary};
use ph_store::projects::ProjectId;
use ph_store::sessions::SessionId;
use ph_ui::{EmptyState, Icons};

use crate::workspace::Workspace;

/// Most cards in the quiet *Recent* column.
const RECENT: usize = 8;

/// What a card opens.
#[derive(Clone, PartialEq)]
pub(crate) enum Target {
    Chat(SessionId),
    /// An agent CLI in a terminal (its pane id).
    Terminal(String),
}

/// One chat, or agent in a terminal, on the board.
pub(crate) struct Card {
    pub project: ProjectId,
    pub target: Target,
    pub title: String,
    pub place: String,
    pub color: Option<Hsla>,
    pub updated_at: i64,
    pub summary: ChatSummary,
}

/// The board's columns, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    NeedsYou,
    Working,
    Finished,
    Recent,
}

impl Column {
    const ALL: [Column; 4] = [
        Column::NeedsYou,
        Column::Working,
        Column::Finished,
        Column::Recent,
    ];

    fn of(state: ChatState) -> Self {
        match state {
            ChatState::NeedsYou => Column::NeedsYou,
            ChatState::Working => Column::Working,
            ChatState::Done | ChatState::Failed => Column::Finished,
            ChatState::Idle => Column::Recent,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Column::NeedsYou => "Needs you",
            Column::Working => "Working",
            Column::Finished => "Finished",
            Column::Recent => "Recent",
        }
    }
}

/// Every loaded chat of every workspace (and the Chats space), newest
/// first.
pub(crate) fn cards(workspace: &Workspace, cx: &App) -> Vec<Card> {
    let dark = cx.theme().is_dark();
    let mut places: Vec<(ProjectId, String, Option<Hsla>)> = workspace
        .projects()
        .iter()
        .map(|project| {
            let color = ph_ui::workspace_color(project.shade(ph_ui::WORKSPACE_COLORS.len()), dark);
            (project.id, project.name.clone(), Some(color))
        })
        .collect();
    if let Some(chats) = workspace.chats_space() {
        places.push((chats.id, "Chats".into(), None));
    }
    let mut cards = Vec::new();
    for (project, place, color) in places.iter().cloned() {
        let Some(chats) = ph_agent::ChatHub::existing(project, cx) else {
            continue;
        };
        let sessions = workspace.sessions_of(project, cx);
        for (session, summary) in chats.read(cx).live(cx) {
            let Some(stored) = sessions.iter().find(|stored| stored.id == session) else {
                continue;
            };
            // Chats marked done leave the board until they work again.
            if workspace.is_settled(session)
                && !matches!(summary.state, ChatState::Working | ChatState::NeedsYou)
            {
                continue;
            }
            cards.push(Card {
                project,
                target: Target::Chat(session),
                title: stored.title.clone(),
                place: place.clone(),
                color,
                updated_at: stored.updated_at,
                summary,
            });
        }
    }
    // Agents in terminals, of the same places.
    for agent in crate::terminal_agents::TerminalAgents::all(cx) {
        let Some((_, place, color)) = places.iter().find(|(id, ..)| *id == agent.project) else {
            continue;
        };
        let label = agent
            .terminal
            .upgrade()
            .and_then(|terminal| terminal.read(cx).label().cloned())
            .unwrap_or_else(|| agent.agent.name.into());
        cards.push(Card {
            project: agent.project,
            target: Target::Terminal(agent.pane.clone()),
            title: agent
                .title
                .clone()
                .unwrap_or_else(|| format!("{label} (terminal)")),
            place: format!("{place} · terminal"),
            color: *color,
            updated_at: agent.updated_at,
            summary: ChatSummary {
                state: agent.state,
                title: label,
                agent: agent.agent,
                line: agent.line.clone(),
            },
        });
    }
    cards.sort_by_key(|card| std::cmp::Reverse(card.updated_at));
    cards
}

/// How many chats need the user and how many are working, for the view's
/// tab in the title bar.
pub(crate) fn counts(workspace: &Workspace, cx: &App) -> (usize, usize) {
    let mut counts = (0, 0);
    for project in workspace
        .projects()
        .iter()
        .map(|project| project.id)
        .chain(workspace.chats_space().map(|chats| chats.id))
    {
        if let Some(chats) = ph_agent::ChatHub::existing(project, cx) {
            let activity = chats.read(cx).activity(cx);
            counts.0 += activity.waiting;
            counts.1 += activity.working;
        }
    }
    for agent in crate::terminal_agents::TerminalAgents::all(cx) {
        match agent.state {
            ChatState::NeedsYou => counts.0 += 1,
            ChatState::Working => counts.1 += 1,
            _ => {}
        }
    }
    counts
}

pub(crate) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let cards = cards(workspace, cx);
    let busy = cards
        .iter()
        .any(|card| card.summary.state != ChatState::Idle);
    if cards.is_empty() || !busy && cards.len() < 2 {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background)
            .child(
                EmptyState::new("No agents at work")
                    .icon(Icons::Bot)
                    .description(
                        "Chats that are working, waiting for you or have finished show here, from every workspace.",
                    ),
            )
            .into_any_element();
    }
    let entity = cx.entity().downgrade();
    let columns = Column::ALL.into_iter().map(|column| {
        let mut items: Vec<&Card> = cards
            .iter()
            .filter(|card| Column::of(card.summary.state) == column)
            .collect();
        if column == Column::Recent {
            items.truncate(RECENT);
        }
        let count = items.len();
        v_flex()
            .flex_1()
            .flex_basis(px(0.))
            .min_w_0()
            .h_full()
            .gap_2()
            .child(
                h_flex()
                    .h_7()
                    .gap_2()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child(column.title())
                    .child(div().child(count.to_string())),
            )
            .child(
                v_flex()
                    .id(SharedString::from(format!(
                        "agents-column-{}",
                        column.title()
                    )))
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .overflow_y_scroll()
                    .when(items.is_empty(), |this| {
                        this.child(
                            div()
                                .h_16()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_dashed()
                                .border_color(cx.theme().border),
                        )
                    })
                    .children(items.into_iter().map(|card| {
                        let (project, target) = (card.project, card.target.clone());
                        let entity = entity.clone();
                        card_view(card, cx).on_click(move |_, window, cx| {
                            _ = entity.update(cx, |workspace, cx| match &target {
                                Target::Chat(session) => {
                                    workspace.open_chat(project, *session, window, cx)
                                }
                                Target::Terminal(pane) => {
                                    workspace.open_terminal_agent(project, pane, window, cx)
                                }
                            });
                        })
                    })),
            )
    });
    v_flex()
        .id("agents-board")
        .size_full()
        .bg(cx.theme().background)
        .px_6()
        .pt_5()
        .pb_4()
        .gap_4()
        .child(
            div()
                .text_base()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child("Agents"),
        )
        .child(
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .gap_4()
                .children(columns),
        )
        .into_any_element()
}

/// A chat's card: its state, title and age; where it lives and its agent;
/// what it is doing or said last.
fn card_view(card: &Card, cx: &App) -> gpui_kit::Stateful<gpui_kit::Div> {
    let theme = cx.theme();
    let state = card.summary.state;
    let (glyph, edge): (AnyElement, Hsla) = match state {
        ChatState::NeedsYou => (question_mark(cx), theme.warning.opacity(0.45)),
        ChatState::Working => (
            Spinner::new().xsmall().color(theme.info).into_any_element(),
            theme.border,
        ),
        ChatState::Failed => (dot(theme.danger), theme.border),
        ChatState::Done => (dot(theme.success), theme.border),
        ChatState::Idle => (
            Icon::new(Icons::MessageSquare)
                .size(px(14.))
                .text_color(theme.muted_foreground)
                .into_any_element(),
            theme.border,
        ),
    };
    let line_color = match state {
        ChatState::NeedsYou => theme.warning,
        ChatState::Failed => theme.danger,
        _ => theme.muted_foreground,
    };
    v_flex()
        .id(SharedString::from(match &card.target {
            Target::Chat(session) => format!("agents-card-{session}"),
            Target::Terminal(pane) => format!("agents-card-{pane}"),
        }))
        .p_3()
        .gap_1p5()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(edge)
        .bg(theme.popover)
        .cursor_pointer()
        .hover(|style| style.border_color(theme.muted_foreground.opacity(0.5)))
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .flex_none()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(glyph),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .child(card.title.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(crate::sidebar::age(card.updated_at)),
                ),
        )
        .child(
            h_flex()
                .gap_1p5()
                .text_xs()
                .text_color(theme.muted_foreground)
                .children(
                    card.color
                        .map(|color| div().flex_none().size(px(7.)).rounded_full().bg(color)),
                )
                .child(div().truncate().child(card.place.clone()))
                .child("·")
                .child(card.summary.agent.mark(px(12.)))
                .child(div().truncate().child(card.summary.agent.name)),
        )
        .when_some(
            card.summary.line.clone().filter(|line| !line.is_empty()),
            |this, line| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(line_color)
                        .line_clamp(2)
                        .child(line),
                )
            },
        )
}

/// Amber, with a question mark: the agent waits for an answer.
pub(crate) fn question_mark(cx: &App) -> AnyElement {
    div()
        .size(px(12.))
        .rounded_full()
        .bg(cx.theme().warning)
        .text_color(cx.theme().warning_foreground)
        .text_size(px(9.))
        .font_weight(gpui_kit::FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .child("?")
        .into_any_element()
}

fn dot(color: Hsla) -> AnyElement {
    div()
        .size(px(8.))
        .rounded_full()
        .bg(color)
        .into_any_element()
}
