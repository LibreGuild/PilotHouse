//! The model picker: the agents down the left (each with its mark, and what
//! it is to this chat), the models of the one in view on the right, each
//! with a line about it. It stays open while you choose, so you can look
//! around and pick; click outside or press Esc to close it. Effort and mode
//! are their own buttons beside it in the composer.
//!
//! Models come from each agent's own ACP config options. The current chat's
//! agent is live; other agents show what was cached the last time they ran
//! (an agent never started offers to load its models). Picking another
//! agent's model switches a fresh chat to it, or opens a new chat if this one
//! already started (an agent cannot change mid-conversation).

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, button::Button, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, div, px,
};
use ph_ui::Icons;

use crate::agents::{AgentSpec, Availability};
use crate::chat_panel::ChatPanel;
use crate::options::{AgentOption, OptionCategory, OptionKind};

/// Everything the picker shows, gathered by the chat panel.
pub struct PickerData {
    pub active: &'static AgentSpec,
    pub started: bool,
    /// Per agent: its options (live for the active one, cached otherwise).
    pub agents: Vec<(&'static AgentSpec, Vec<AgentOption>)>,
    /// The agent whose models are in view (its tab is selected).
    pub viewing: &'static AgentSpec,
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
    /// Show another agent's models (nothing changes yet).
    View(&'static AgentSpec),
}

pub fn render(data: &PickerData, panel: WeakEntity<ChatPanel>, cx: &App) -> AnyElement {
    let viewing = data.viewing;
    let options = data
        .agents
        .iter()
        .find(|(agent, _)| agent.id == viewing.id)
        .map(|(_, options)| options.as_slice())
        .unwrap_or_default();
    let pick = move |pick: Pick| {
        let panel = panel.clone();
        move |_: &gpui_kit::ClickEvent, window: &mut gpui_kit::Window, cx: &mut App| {
            let pick = pick.clone();
            _ = panel.update(cx, |panel, cx| panel.pick(pick, window, cx));
        }
    };

    let agents = v_flex()
        .flex_none()
        .w(px(158.))
        .p_1()
        .gap_0p5()
        .border_r_1()
        .border_color(cx.theme().border)
        .children(data.agents.iter().map(|(agent, _)| {
            let selected = agent.id == viewing.id;
            let ready = matches!(agent.availability(), Availability::Ready { .. });
            let (note, note_color) = if agent.id == data.active.id {
                ("This chat", agent.tint())
            } else if !ready {
                ("Not set up", cx.theme().muted_foreground)
            } else {
                (agent.provider, cx.theme().muted_foreground)
            };
            h_flex()
                .id(SharedString::from(format!("picker-agent-{}", agent.id)))
                .role(gpui_kit::accesskit::Role::Tab)
                .aria_label(agent.name)
                .relative()
                .h_10()
                .px_2()
                .gap_2()
                .rounded(cx.theme().radius)
                .cursor_pointer()
                .when(selected, |this| this.bg(cx.theme().secondary))
                .when(!selected, |this| {
                    this.hover(|style| style.bg(cx.theme().list_hover))
                })
                .when(!ready, |this| this.opacity(0.7))
                .on_click(pick(Pick::View(agent)))
                // The agent in view has a bar in its colour.
                .when(selected, |this| {
                    this.child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(10.))
                            .bottom(px(10.))
                            .w(px(3.))
                            .rounded_full()
                            .bg(agent.tint()),
                    )
                })
                .child(agent.mark(px(20.)))
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(agent.short_name()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(note_color)
                                .child(note),
                        ),
                )
                .test_support()
        }));

    let model = options
        .iter()
        .find(|option| option.category == OptionCategory::Model);
    let choices = match model.map(|option| &option.kind) {
        Some(OptionKind::Select(choices)) => choices.as_slice(),
        _ => &[],
    };
    let models: AnyElement = if choices.is_empty() {
        let ready = matches!(viewing.availability(), Availability::Ready { .. });
        let reason = match viewing.availability() {
            Availability::Ready { .. } => {
                format!("{} lists its models once it starts.", viewing.name)
            }
            // The real reason (CLI missing, no Node.js); the button opens
            // the setup card.
            Availability::Missing(reason) => reason,
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_4()
            .child(viewing.mark(px(32.)))
            .child(
                div()
                    .text_xs()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(reason),
            )
            .child(
                Button::new(SharedString::from(format!("picker-load-{}", viewing.id)))
                    .outline()
                    .small()
                    .label(if ready { "Load models" } else { "Set up" })
                    .on_click(pick(Pick::Agent(viewing))),
            )
            .into_any_element()
    } else {
        let option_id = model.map(|option| option.id.clone()).unwrap_or_default();
        let current = model
            .map(|option| option.current.clone())
            .unwrap_or_default();
        v_flex()
            .id("picker-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_1()
            .gap_0p5()
            .children(choices.iter().enumerate().map(|(ix, choice)| {
                let selected = choice.value == current;
                h_flex()
                    .id((SharedString::from(format!("picker-{}", viewing.id)), ix))
                    .role(gpui_kit::accesskit::Role::MenuItem)
                    .aria_label(SharedString::from(choice.name.clone()))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(viewing.tint().opacity(0.12))
                            .border_color(viewing.tint().opacity(0.4))
                    })
                    .when(!selected, |this| {
                        this.border_color(gpui_kit::transparent_black())
                            .hover(|style| style.bg(cx.theme().list_hover))
                    })
                    .on_click(pick(Pick::Model {
                        agent: viewing,
                        option: option_id.clone(),
                        value: choice.value.clone(),
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .child(choice.name.clone()),
                            )
                            .children(choice.description.clone().map(|description| {
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(description)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .rounded_full()
                            .when(selected, |this| {
                                this.bg(viewing.tint()).child(
                                    Icon::new(Icons::Check)
                                        .xsmall()
                                        .text_color(gpui_kit::white()),
                                )
                            })
                            .when(!selected, |this| {
                                this.border_1().border_color(cx.theme().border)
                            }),
                    )
                    .test_support()
            }))
            .into_any_element()
    };

    let header = h_flex()
        .flex_none()
        .h_9()
        .px_3()
        .gap_1p5()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(viewing.name),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("· {}", viewing.provider)),
        )
        .child(div().flex_1())
        .when(data.started && viewing.id != data.active.id, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("opens a new chat"),
            )
        });

    h_flex()
        .w(px(500.))
        .h(px(320.))
        .items_stretch()
        .child(agents)
        .child(v_flex().flex_1().min_w_0().child(header).child(models))
        .into_any_element()
}
