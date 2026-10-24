//! The setup card: an agent's [`setup`](crate::setup) checks with a way to
//! fix each one. Shown in an empty chat when the agent cannot start yet, and
//! in Settings → Agents.

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClipboardItem, ElementId, IntoElement, ParentElement as _, SharedString,
    Styled as _, div,
};

use crate::agents::AgentSpec;
use crate::setup::{Check, CheckState, Fix};

/// Runs the checks again (the "Check again" button).
pub type Recheck = Box<dyn Fn(&mut gpui_kit::Window, &mut App) + 'static>;

/// The checks as a list; `on_recheck` adds a "Check again" button.
pub fn card(
    spec: &AgentSpec,
    checks: &[Check],
    on_recheck: Option<Recheck>,
    cx: &App,
) -> AnyElement {
    let ready = crate::setup::ready(checks);
    v_flex()
        .w_full()
        .gap_1()
        .p_3()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().secondary)
        .child(
            h_flex()
                .gap_2()
                .pb_1()
                .child(spec.mark(gpui_kit::px(16.)))
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(if ready {
                            spec.name.to_string()
                        } else {
                            format!("Set up {}", spec.name)
                        }),
                )
                .when(ready, |this| {
                    this.child(
                        div()
                            .px_2()
                            .rounded_full()
                            .bg(cx.theme().success.opacity(0.15))
                            .text_xs()
                            .text_color(cx.theme().success)
                            .child("Ready"),
                    )
                })
                .when_some(on_recheck, |this, recheck| {
                    this.child(
                        Button::new(SharedString::from(format!("recheck-{}", spec.id)))
                            .small()
                            .ghost()
                            .icon(IconName::Redo)
                            .label("Check again")
                            .on_click(move |_, window, cx| recheck(window, cx)),
                    )
                }),
        )
        .children(
            checks
                .iter()
                .enumerate()
                .map(|(ix, check)| row(spec, ix, check, cx)),
        )
        .into_any_element()
}

fn row(spec: &AgentSpec, ix: usize, check: &Check, cx: &App) -> AnyElement {
    let (icon, color) = match check.state {
        CheckState::Ok => (IconName::CircleCheck, cx.theme().success),
        CheckState::Missing => (IconName::CircleX, cx.theme().danger),
        CheckState::Note => (IconName::Info, cx.theme().muted_foreground),
    };
    let id =
        |name: &str| ElementId::from(SharedString::from(format!("setup-{}-{name}-{ix}", spec.id)));
    h_flex()
        .items_start()
        .gap_2()
        .py_1()
        .child(
            div()
                .pt_0p5()
                .child(Icon::new(icon).small().text_color(color)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().child(check.title.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(check.detail.clone()),
                )
                .children(check.action.as_ref().map(|fix| match fix {
                    Fix::Command(command) => {
                        let copy = command.clone();
                        h_flex()
                            .mt_1()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px_2()
                                    .py_1()
                                    .rounded(cx.theme().radius)
                                    .bg(cx.theme().background)
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_xs()
                                    .truncate()
                                    .child(command.clone()),
                            )
                            .child(
                                Button::new(id("copy"))
                                    .small()
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .tooltip("Copy command")
                                    .accessibility_label("Copy command")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            )
                            .into_any_element()
                    }
                    Fix::Link { label, url } => {
                        let url = *url;
                        Button::new(id("link"))
                            .mt_1()
                            .small()
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label(*label)
                            .on_click(move |_, _, cx| cx.open_url(url))
                            .into_any_element()
                    }
                })),
        )
        .into_any_element()
}
