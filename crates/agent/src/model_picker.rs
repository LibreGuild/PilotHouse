//! The model picker: one compact list, an agent's name over its models,
//! the agents one after another (this chat's first), with a search on top.
//! It stays open while you choose; click outside or press Esc to close it.
//! Typing filters by model or agent, ↑/↓ and Enter pick one, Tab jumps to
//! the next agent. Effort and mode are their own buttons beside it in the
//! composer.
//!
//! Models come from each agent's own ACP config options. The current chat's
//! agent is live; other agents show what was cached the last time they ran
//! (an agent never started offers to load its models). Picking another
//! agent's model switches a fresh chat to it, or opens a new chat if this one
//! already started (an agent cannot change mid-conversation).

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, h_flex,
    input::{Enter, IndentInline, Input, InputState, MoveDown, MoveUp},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, px,
};
use ph_ui::Icons;

use crate::agents::{AgentSpec, Availability};
use crate::chat_panel::ChatPanel;
use crate::options::{AgentOption, OptionCategory, OptionChoice, OptionKind};

/// Everything the picker shows, gathered by the chat panel.
pub struct PickerData {
    pub active: &'static AgentSpec,
    pub started: bool,
    /// Per agent: its options (live for the active one, cached otherwise),
    /// this chat's agent first.
    pub agents: Vec<(&'static AgentSpec, Vec<AgentOption>)>,
    /// What the filter says, and the highlighted row (keyboard).
    pub query: String,
    pub highlight: usize,
}

/// What a picked row does.
#[derive(Clone)]
pub enum Pick {
    Model {
        agent: &'static AgentSpec,
        option: String,
        value: String,
    },
    /// Switch to an agent whose models are not known yet.
    Agent(&'static AgentSpec),
}

/// A key the picker acts on.
#[derive(Clone, Copy)]
pub enum PickerKey {
    Up,
    Down,
    Pick,
    NextAgent,
}

/// One agent as listed: its model option (when known) and the models that
/// match the filter. An agent with nothing matching is left out.
struct Section<'a> {
    agent: &'static AgentSpec,
    model: Option<&'a AgentOption>,
    choices: Vec<&'a OptionChoice>,
}

fn sections(data: &PickerData) -> Vec<Section<'_>> {
    let query = data.query.trim().to_lowercase();
    data.agents
        .iter()
        .filter_map(|(agent, options)| {
            let model = options
                .iter()
                .find(|option| option.category == OptionCategory::Model);
            // An agent's name matching shows all its models.
            let named = query.is_empty()
                || agent.name.to_lowercase().contains(&query)
                || agent.provider.to_lowercase().contains(&query);
            let matches = |choice: &&OptionChoice| {
                named
                    || choice.name.to_lowercase().contains(&query)
                    || choice
                        .description
                        .as_deref()
                        .is_some_and(|text| text.to_lowercase().contains(&query))
            };
            let choices: Vec<&OptionChoice> = match model.map(|option| &option.kind) {
                Some(OptionKind::Select(choices)) => choices.iter().filter(matches).collect(),
                _ => Vec::new(),
            };
            let shown = !choices.is_empty() || (model.is_none() && named);
            shown.then_some(Section {
                agent,
                model,
                choices,
            })
        })
        .collect()
}

/// The rows Enter can pick, in order: each model, or an agent's *Load
/// models* when its models are not known.
pub fn listed_models(data: &PickerData) -> Vec<Pick> {
    sections(data)
        .into_iter()
        .flat_map(|section| match section.model {
            None => vec![Pick::Agent(section.agent)],
            Some(model) => section
                .choices
                .into_iter()
                .map(|choice| Pick::Model {
                    agent: section.agent,
                    option: model.id.clone(),
                    value: choice.value.clone(),
                })
                .collect(),
        })
        .collect()
}

/// Where each agent's first row is in [`listed_models`], for Tab.
pub fn section_starts(data: &PickerData) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut at = 0;
    for section in sections(data) {
        starts.push(at);
        at += section
            .choices
            .len()
            .max(usize::from(section.model.is_none()));
    }
    starts
}

pub fn render(
    data: &PickerData,
    search: &Entity<InputState>,
    panel: WeakEntity<ChatPanel>,
    cx: &App,
) -> AnyElement {
    let keys = panel.clone();
    let key = move |key: PickerKey| {
        let panel = keys.clone();
        move |window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            _ = panel.update(cx, |panel, cx| panel.picker_key(key, window, cx));
        }
    };
    let pick = move |pick: Pick| {
        let panel = panel.clone();
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            let pick = pick.clone();
            _ = panel.update(cx, |panel, cx| panel.pick(pick, window, cx));
        }
    };

    let sections = sections(data);
    let total: usize = sections
        .iter()
        .map(|section| {
            section
                .choices
                .len()
                .max(usize::from(section.model.is_none()))
        })
        .sum();
    let highlight = data.highlight.min(total.saturating_sub(1));
    let mut row = 0usize;
    let mut rows: Vec<AnyElement> = Vec::new();
    for section in &sections {
        let agent = section.agent;
        let ready = matches!(agent.availability(), Availability::Ready { .. });
        let note = if agent.id == data.active.id {
            Some(("This chat", agent.tint()))
        } else if !ready {
            Some(("Not set up", cx.theme().muted_foreground))
        } else if data.started {
            Some(("New chat", cx.theme().muted_foreground))
        } else {
            None
        };
        // The agent's name, over its models.
        rows.push(
            h_flex()
                .flex_none()
                .h_7()
                .px_2()
                .mt_1()
                .gap_1p5()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(agent.mark(px(14.)))
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child(agent.short_name()),
                )
                .child(agent.provider)
                .child(div().flex_1())
                .children(note.map(|(text, color)| div().text_color(color).child(text)))
                .into_any_element(),
        );
        let Some(model) = section.model else {
            let reason = match agent.availability() {
                Availability::Ready { .. } => "Load its models".to_string(),
                Availability::Missing(reason) => format!("Set up: {reason}"),
            };
            rows.push(
                model_row(
                    SharedString::from(format!("picker-load-{}", agent.id)),
                    agent,
                    reason,
                    None,
                    false,
                    row == highlight,
                    cx,
                )
                .on_click(pick(Pick::Agent(agent)))
                .test_support()
                .into_any_element(),
            );
            row += 1;
            continue;
        };
        for choice in &section.choices {
            let selected = agent.id == data.active.id && choice.value == model.current;
            rows.push(
                model_row(
                    SharedString::from(format!("picker-{}-{}", agent.id, choice.value)),
                    agent,
                    choice.name.clone(),
                    choice.description.clone(),
                    selected,
                    row == highlight,
                    cx,
                )
                .on_click(pick(Pick::Model {
                    agent,
                    option: model.id.clone(),
                    value: choice.value.clone(),
                }))
                .test_support()
                .into_any_element(),
            );
            row += 1;
        }
    }
    if rows.is_empty() {
        rows.push(
            div()
                .p_3()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No models match.")
                .into_any_element(),
        );
    }

    let (up, down, enter, next) = (
        key(PickerKey::Up),
        key(PickerKey::Down),
        key(PickerKey::Pick),
        key(PickerKey::NextAgent),
    );
    v_flex()
        .id("model-picker-card")
        .w(px(320.))
        .max_h(px(300.))
        // The filter has the keyboard; these keys steer the list instead.
        .capture_action(move |_: &MoveUp, window, cx| up(window, cx))
        .capture_action(move |_: &MoveDown, window, cx| down(window, cx))
        .capture_action(move |_: &Enter, window, cx| enter(window, cx))
        .capture_action(move |_: &IndentInline, window, cx| next(window, cx))
        .child(
            div().flex_none().p_1p5().child(
                Input::new(search)
                    .small()
                    .prefix(Icon::new(Icons::Search).xsmall()),
            ),
        )
        .child(
            v_flex()
                .id("picker-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_1()
                .pb_1()
                .gap_0p5()
                .children(rows),
        )
        .into_any_element()
}

/// A model (or *Load models*) row: the agent's mark, the name with its
/// note muted beside it, and a check on the one in use.
fn model_row(
    id: SharedString,
    agent: &'static AgentSpec,
    name: String,
    description: Option<String>,
    selected: bool,
    highlighted: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    h_flex()
        .id(id)
        .role(gpui_kit::accesskit::Role::MenuItem)
        .aria_label(SharedString::from(name.clone()))
        .h_7()
        .px_2()
        .gap_2()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .text_sm()
        .when(selected, |this| this.bg(agent.tint().opacity(0.12)))
        .when(highlighted && !selected, |this| {
            this.bg(cx.theme().list_hover)
        })
        .when(!selected, |this| {
            this.hover(|style| style.bg(cx.theme().list_hover))
        })
        .child(agent.mark(px(14.)))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1p5()
                .child(div().flex_none().child(name))
                .children(description.map(|text| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(text)
                })),
        )
        .when(selected, |this| {
            this.child(Icon::new(Icons::Check).xsmall().text_color(agent.tint()))
        })
}
