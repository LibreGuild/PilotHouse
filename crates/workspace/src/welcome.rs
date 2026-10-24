//! Shown when no workspace is open yet.

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Context, IntoElement, ParentElement as _, Styled as _, Window, div};
use ph_ui::{EmptyState, Icons, Surface};

use crate::actions::{AddProject, NewWorkspace, WORKSPACE_CONTEXT};
use crate::workspace::Workspace;

pub(crate) fn render(window: &mut Window, _: &mut Context<Workspace>) -> impl IntoElement {
    let shortcut = Kbd::binding_for_action(&AddProject, Some(WORKSPACE_CONTEXT), window);

    div().flex_1().h_full().pr_2().pb_2().child(Surface::new().child(
        EmptyState::new("Welcome to PilotHouse")
            .icon(Icons::Zap)
            .description(
                "Work with your coding agents on chat, code, Git and the board in one place. A workspace holds as many folders as you add to it, or none, just to chat or use the browser.",
            )
            .action(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("welcome-add-project")
                            .primary()
                            .icon(Icons::FolderPlus)
                            .label("Open a folder")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(AddProject), cx)),
                    )
                    .when_some(shortcut, |this, shortcut| this.child(shortcut))
                    .child(
                        Button::new("welcome-new-workspace")
                            .ghost()
                            .icon(Icons::MessageSquare)
                            .label("New workspace")
                            .tooltip("An empty workspace; add folders to it any time")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(NewWorkspace), cx)),
                    ),
            ),
    ))
}
