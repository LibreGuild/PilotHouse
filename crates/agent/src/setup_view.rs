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
    AnyElement, App, ElementId, IntoElement, ParentElement as _, SharedString, Styled as _, div,
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

/// The first-run card, shown while no agent is installed: every agent
/// PilotHouse works with, whether it was found, and how to install it.
/// `checks` are each agent's [`crate::setup::checks`].
pub fn welcome(
    checks: &[(&'static AgentSpec, Vec<Check>)],
    on_recheck: Recheck,
    cx: &App,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_4()
        .child(
            v_flex()
                .items_center()
                .gap_2()
                .child(ph_ui::logo_mark(gpui_kit::px(36.)))
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .child("Welcome to PilotHouse"),
                )
                .child(
                    div()
                        .max_w(gpui_kit::px(460.))
                        .text_sm()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "PilotHouse works with the coding agents you install yourself, \
                             with your own sign-ins. Install one of these to start: it shows \
                             up here on its own.",
                        ),
                ),
        )
        .child(
            v_flex()
                .w_full()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().secondary)
                .children(
                    checks
                        .iter()
                        .enumerate()
                        .map(|(ix, (spec, checks))| agent_entry(spec, checks, ix > 0, cx)),
                ),
        )
        .child(
            h_flex()
                .justify_center()
                .gap_3()
                .child(
                    Button::new("welcome-recheck")
                        .small()
                        .outline()
                        .icon(IconName::Redo)
                        .label("Check again")
                        .on_click(move |_, window, cx| on_recheck(window, cx)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("PilotHouse also looks every few seconds."),
                ),
        )
        .into_any_element()
}

/// One agent in the first-run card: its name and whether it was found,
/// then what is missing with how to fix it.
fn agent_entry(spec: &AgentSpec, checks: &[Check], divided: bool, cx: &App) -> AnyElement {
    let ready = crate::setup::ready(checks);
    let installed = checks
        .first()
        .is_some_and(|check| check.state == CheckState::Ok);
    let (status, color) = match (ready, installed, checks.is_empty()) {
        (_, _, true) => ("Looking…", cx.theme().muted_foreground),
        (true, _, _) => ("Installed", cx.theme().success),
        (false, true, _) => ("Needs setup", cx.theme().warning),
        (false, false, _) => ("Not installed", cx.theme().muted_foreground),
    };
    let page = spec.home_page();
    v_flex()
        .px_3()
        .py_2p5()
        .gap_1()
        .when(divided, |this| {
            this.border_t_1().border_color(cx.theme().border)
        })
        .child(
            h_flex()
                .gap_2()
                .child(spec.mark(gpui_kit::px(18.)))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(spec.name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(spec.provider),
                )
                .child(div().flex_1())
                .child(div().text_xs().text_color(color).child(status))
                .child(
                    Button::new(SharedString::from(format!("welcome-page-{}", spec.id)))
                        .xsmall()
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .tooltip(format!("{}'s page", spec.name))
                        .accessibility_label(SharedString::from(format!(
                            "Open {}'s page",
                            spec.name
                        )))
                        .on_click(move |_, _, cx| cx.open_url(page)),
                ),
        )
        // Only what stands in the way, as what to do rather than as an
        // error: the install command, Node.js where its adapter needs it.
        .children(
            checks
                .iter()
                .enumerate()
                .filter(|(_, check)| check.state == CheckState::Missing)
                .filter_map(|(ix, check)| {
                    let fix = check.action.as_ref()?;
                    let label = match fix {
                        Fix::Command(_) => "Install".to_string(),
                        Fix::Link { .. } => check.detail.clone(),
                    };
                    Some(
                        h_flex()
                            .pl(gpui_kit::px(26.))
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(fix_element(spec, ix, fix, cx)),
                            ),
                    )
                }),
        )
        .into_any_element()
}

fn row(spec: &AgentSpec, ix: usize, check: &Check, cx: &App) -> AnyElement {
    let (icon, color) = match check.state {
        CheckState::Ok => (IconName::CircleCheck, cx.theme().success),
        CheckState::Missing => (IconName::CircleX, cx.theme().danger),
        CheckState::Note => (IconName::Info, cx.theme().muted_foreground),
    };
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
                .children(
                    check
                        .action
                        .as_ref()
                        .map(|fix| div().mt_1().child(fix_element(spec, ix, fix, cx))),
                ),
        )
        .into_any_element()
}

/// How to fix a check: a command with Copy, or a link to open.
fn fix_element(spec: &AgentSpec, ix: usize, fix: &Fix, cx: &App) -> AnyElement {
    let id =
        |name: &str| ElementId::from(SharedString::from(format!("setup-{}-{name}-{ix}", spec.id)));
    match fix {
        Fix::Command(command) => h_flex()
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
                ph_ui::CopyButton::new(id("copy"), command.clone())
                    .small()
                    .tooltip("Copy command"),
            )
            .into_any_element(),
        Fix::Link { label, url } => {
            let url = *url;
            Button::new(id("link"))
                .small()
                .outline()
                .icon(IconName::ExternalLink)
                .label(*label)
                .on_click(move |_, _, cx| cx.open_url(url))
                .into_any_element()
        }
    }
}
