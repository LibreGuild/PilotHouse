//! The bar at the bottom of the window: the git branch (click for the
//! branch picker), the project folder, and the detected stack.

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    spinner::Spinner,
    status_bar::StatusBar,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div,
};
use ph_ui::Icons;

use crate::workspace::Workspace;

pub(crate) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let space_entity = workspace.active_space().cloned();
    let space = space_entity.as_ref().map(|space| space.read(cx));
    // Where the workspace is: its folder's name and how many more it has
    // (the full paths in the tooltip), or none.
    let folders = space
        .map(|space| space.project().folders())
        .unwrap_or_default();
    let path = space.map(|_| {
        let name = |folder: &std::path::Path| {
            folder
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| ph_ui::platform::display_path(folder))
        };
        match folders.len() {
            0 => "No folders".to_string(),
            1 => name(&folders[0]),
            n => format!(
                "{} + {} more folder{}",
                name(&folders[0]),
                n - 1,
                if n == 2 { "" } else { "s" }
            ),
        }
    });
    let full_paths: String = folders
        .iter()
        .map(|folder| ph_ui::platform::display_path(folder))
        .collect::<Vec<_>>()
        .join("\n");
    let stacks: Vec<&'static str> = space
        .map(|space| space.stacks().iter().map(|stack| stack.label()).collect())
        .unwrap_or_default();
    let branch = space_entity.and_then(|space| branch_item(&space, cx));

    StatusBar::new()
        .left(
            h_flex()
                .gap_3()
                .min_w_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .children(branch)
                .child(
                    h_flex()
                        .id("status-folders")
                        .gap_1()
                        .min_w_0()
                        .when(!full_paths.is_empty(), |this| {
                            this.tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(full_paths.clone())
                                    .build(window, cx)
                            })
                        })
                        .child(Icon::new(Icons::Folder).xsmall())
                        .child(
                            div()
                                .truncate()
                                .child(path.unwrap_or_else(|| "No workspace open".into())),
                        ),
                ),
        )
        .right(
            h_flex()
                .gap_3()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .when(!stacks.is_empty(), |this| this.child(stacks.join(" · ")))
                .child(concat!("PilotHouse ", env!("CARGO_PKG_VERSION"))),
        )
}

/// Branch, ahead/behind and a running operation; opens the branch picker.
fn branch_item(
    space: &gpui_kit::Entity<crate::workspace_screen::WorkspaceScreen>,
    cx: &Context<Workspace>,
) -> Option<AnyElement> {
    let space_ref = space.read(cx);
    // With several folders, the branch names its folder.
    let folder = (space_ref.project().roots().len() > 1)
        .then(|| {
            space_ref
                .roots()
                .list()
                .get(space_ref.git_folder())
                .map(|root| root.name.clone())
        })
        .flatten();
    let repo = space_ref.repo_model()?.read(cx);
    let status = repo.status()?.clone();
    let busy = repo.busy().cloned();
    let operation = repo.operation();
    let name = status
        .branch
        .clone()
        .or_else(|| status.head.clone().map(|head| format!("({head})")))
        .unwrap_or_else(|| "no commits".into());
    let name = match folder {
        Some(folder) => format!("{folder} · {name}"),
        None => name,
    };
    let label = h_flex()
        .gap_1()
        .text_xs()
        .child(Icon::new(Icons::GitBranch).xsmall())
        .child(name)
        .when(status.ahead > 0, |this| {
            this.child(format!("↑{}", status.ahead))
        })
        .when(status.behind > 0, |this| {
            this.child(format!("↓{}", status.behind))
        })
        .when_some(operation, |this, operation| {
            this.child(
                div()
                    .text_color(cx.theme().warning)
                    .child(format!("{operation} in progress")),
            )
        })
        .when_some(busy, |this, busy| {
            this.child(Spinner::new().xsmall()).child(busy)
        });
    let trigger = Button::new("status-branch")
        .ghost()
        .xsmall()
        .child(label)
        .tooltip("Branches, fetch, pull and push")
        .accessibility_label("Branch");
    let space = space.clone();
    Some(
        Popover::new("branch-picker")
            .offset(ph_ui::POPUP_GAP)
            .anchor(gpui_kit::Anchor::BottomLeft)
            .trigger(trigger)
            .content(move |_, window, cx| {
                let picker = space.update(cx, |space, cx| space.branch_picker(window, cx));
                picker.update(cx, |picker, cx| picker.focus_when_opened(window, cx));
                picker
            })
            .into_any_element(),
    )
}
