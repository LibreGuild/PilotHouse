//! What *Workspaces* shows while the user has none (a first start, or the
//! last one removed): what a workspace is, and the two ways to make one.
//! It used to ask for a folder at once, which says nothing of what the
//! folder is for.

use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _,
    Window, div,
};
use ph_ui::{EmptyState, Icons};

use crate::actions::{AddProject, NewWorkspace};

pub(crate) fn render(window: &mut Window, cx: &mut App) -> AnyElement {
    let screen = EmptyState::new("No workspaces yet")
        .icon(Icons::Folders)
        .description(
            "A workspace is a folder, or several, with the work that belongs to it: its chats \
             and tasks, files, changes, terminal and previews. Each keeps going while you look \
             at another, and the Bridge shows them all.",
        )
        .action(
            h_flex()
                .gap_2()
                .flex_wrap()
                .justify_center()
                .child(
                    Button::new("no-workspaces-open")
                        .primary()
                        .small()
                        .icon(Icons::FolderOpen)
                        .label("Open a folder…")
                        .tooltip(ph_ui::tip(format!(
                            "Open a folder as a workspace ({})",
                            ph_ui::platform::shortcut("secondary-o")
                        )))
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(AddProject), cx)),
                )
                .child(
                    Button::new("no-workspaces-new")
                        .outline()
                        .small()
                        .label("New workspace without folders")
                        .tooltip(ph_ui::tip(
                            "Make an empty workspace; folders can be added to it later",
                        ))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(NewWorkspace), cx)
                        }),
                ),
        );
    div()
        .id("no-workspaces")
        .flex_1()
        .min_w_0()
        .h_full()
        .pr_2()
        .pb(crate::frame::gap())
        .child(crate::frame::island(window, cx).child(screen))
        .into_any_element()
}
