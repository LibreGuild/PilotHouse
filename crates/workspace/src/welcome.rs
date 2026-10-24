//! Shown when no workspace is open yet: what PilotHouse is, and three steps
//! to start.

use gpui_kit::component::{
    ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
    px,
};
use ph_ui::{EmptyState, Icons, Surface};

use crate::actions::{
    AddProject, NewWorkspace, ShowShortcuts, ToggleCommandPalette, WORKSPACE_CONTEXT,
};
use crate::workspace::Workspace;

pub(crate) fn render(window: &mut Window, cx: &mut Context<Workspace>) -> impl IntoElement {
    let shortcut = Kbd::binding_for_action(&AddProject, Some(WORKSPACE_CONTEXT), window);
    let key = |action: &dyn gpui_kit::Action, window: &mut Window| {
        Kbd::binding_for_action(action, Some(WORKSPACE_CONTEXT), window)
            .map(IntoElement::into_any_element)
    };
    let steps = [
        (
            "Open a folder",
            "or start an empty workspace. Add more folders to it any time.",
            None,
        ),
        (
            "Pick an agent",
            "under the message box: Claude Code, Codex, Gemini CLI or Antigravity. PilotHouse says what to install and sign in to.",
            None,
        ),
        (
            "Ask, then review",
            "Edits show in Git, plans on the board, and you keep or undo them.",
            None,
        ),
    ];
    let palette = key(&ToggleCommandPalette, window);
    let shortcuts = key(&ShowShortcuts, window);
    let muted = cx.theme().muted_foreground;
    let list = v_flex()
        .mt_6()
        .w(px(460.))
        .gap_3()
        .children(
            steps
                .into_iter()
                .enumerate()
                .map(|(ix, (title, text, keys))| step(ix + 1, title, text, keys, cx)),
        )
        .child(
            h_flex()
                .gap_1()
                .text_xs()
                .text_color(muted)
                .child("Every command:")
                .when_some(palette, |this, keys| this.child(keys))
                .child(div().w_2())
                .child("All shortcuts:")
                .when_some(shortcuts, |this, keys| this.child(keys)),
        );

    div().flex_1().h_full().pr_2().pb_2().child(Surface::new().child(
        v_flex().size_full().items_center().justify_center().child(
            EmptyState::new("Welcome to PilotHouse")
                .icon(Icons::Zap)
                .description(
                    "Work with your coding agents on chat, code, Git and the board in one place. A workspace holds as many folders as you add to it, or none, just to chat or use the browser.",
                )
                .action(
                    v_flex()
                        .items_center()
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    Button::new("welcome-add-project")
                                        .primary()
                                        .icon(Icons::FolderPlus)
                                        .label("Open a folder")
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(AddProject), cx)
                                        }),
                                )
                                .when_some(shortcut, |this, shortcut| this.child(shortcut))
                                .child(
                                    Button::new("welcome-new-workspace")
                                        .ghost()
                                        .icon(Icons::MessageSquare)
                                        .label("New workspace")
                                        .tooltip("An empty workspace; add folders to it any time")
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(NewWorkspace), cx)
                                        }),
                                ),
                        )
                        .child(list),
                ),
        ),
    ))
}

/// One numbered step.
fn step(
    number: usize,
    title: &'static str,
    text: &'static str,
    keys: Option<AnyElement>,
    cx: &Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .flex_none()
                .size(px(22.))
                .rounded_full()
                .bg(theme.secondary)
                .text_xs()
                .flex()
                .items_center()
                .justify_center()
                .child(SharedString::from(number.to_string())),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .text_sm()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child(title))
                        .when_some(keys, |this, keys| this.child(keys)),
                )
                .child(div().text_color(theme.muted_foreground).child(text)),
        )
}
