//! Contents of the window title bar: the app's mark and the name of what is
//! in front (*Bridge*, *Reviews*, or a workspace with its colour, whose name
//! opens the list of workspaces to go to another), the command palette
//! trigger in the middle, the next step of the task in front, and
//! window-level buttons (inbox, the side panels).
//!
//! The task in front is not named here: its title and where it stands
//! (in colour) stood after the workspace's name for a while, which crowded
//! the corner and drew the eye from the work. The chat's own header says
//! both. The bar is part of the window's
//! frame (`frame`): it has no fill of its own. What the window shows is
//! picked in the column of icons at its left edge (`rail`), and which
//! workspace, here (`workspace_switcher`).
//!
//! On Windows the whole bar is a caption (drag) area for the system, so a
//! click on it moves the window and a double click maximises it. Every
//! control here is `occlude`d so its clicks reach the app instead.

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
    menu::{ContextMenuExt as _, PopupMenuItem},
    popover::Popover,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::actions::*;
use crate::workspace::{View, Workspace};

pub(crate) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let palette_hint =
        Kbd::binding_for_action(&ToggleCommandPalette, Some(WORKSPACE_CONTEXT), window);
    // The search sits in the middle of the window, not of the space left
    // between the window's buttons: the side with less of the window's own
    // chrome gets that much more room (macOS's buttons are on the left,
    // Windows' and Linux's on the right).
    //
    // The search alone: the two sides grow by the same amount around it,
    // so the ⋯ button beside it is part of the right side (it stood between
    // them, and it was the search and the button together that were
    // centred), and so is the bar's room at its right end.
    let (left_bias, right_bias) = chrome_bias(window);
    h_flex()
        .w_full()
        .h_full()
        .gap_1()
        .child(
            h_flex()
                .flex_1()
                .flex_basis(left_bias)
                .gap_1()
                .min_w_0()
                .child(app_icon(cx))
                .children(front_name(workspace, cx)),
        )
        .child(
            h_flex()
                .id("open-command-palette")
                .role(gpui_kit::accesskit::Role::Button)
                .aria_label("Search or run a command")
                .occlude()
                // A basis, not a width: with less room (a narrow window, a
                // large interface size) the search box gives way, and the
                // buttons on the right stay on screen. A fixed width counts
                // as the least the bar can be, and pushed them off it.
                .flex_basis(gpui_kit::rems(24.))
                .min_w_0()
                .h_7()
                .mx_2()
                .px_2p5()
                .gap_2()
                .rounded(cx.theme().radius)
                .bg(cx.theme().muted)
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().secondary_hover))
                // On press, straight to the workspace (see the view tabs).
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|workspace, _, window, cx| {
                        cx.stop_propagation();
                        workspace.toggle_command_palette(&ToggleCommandPalette, window, cx)
                    }),
                )
                .child(Icon::new(IconName::Search).small())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child("Search or run a command"),
                )
                .when_some(palette_hint, |this, hint| this.child(hint)),
        )
        .child(
            h_flex()
                .flex_1()
                .flex_basis(right_bias)
                .pr_2p5()
                .gap_1()
                .child(div().id("more-button").occlude().child(more_button()))
                .child(div().flex_1())
                .children(next_step(workspace, cx))
                .children(update_item(cx))
                .child(
                    div()
                        .id("inbox-button")
                        .occlude()
                        .child(crate::inbox::button(&workspace.inbox, cx)),
                )
                .children(panel_switches(workspace, cx)),
        )
}

/// The task in the chat of the workspace in front, as the Bridge has it.
fn task_in_front(workspace: &Workspace, cx: &App) -> Option<crate::bridge::OpenChat> {
    if workspace.view() != View::Workspaces {
        return None;
    }
    let space = workspace.active_space()?.read(cx);
    let front = space.active_session()?;
    crate::bridge::open_chats(
        workspace,
        space.project().id,
        space.sessions(),
        Some(front),
        cx,
    )
    .into_iter()
    .find(|chat| chat.session.id == front)
}

/// Where the task in front works: its own folder, else its workspace's
/// first.
fn task_folder(
    workspace: &Workspace,
    task: &crate::bridge::OpenChat,
    cx: &App,
) -> Option<std::path::PathBuf> {
    task.session
        .cwd
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            workspace
                .active_space()?
                .read(cx)
                .project()
                .folders()
                .into_iter()
                .next()
        })
}

/// What the task in front waits for next, one press away: its changes to
/// review (while another tool is in front), or its pull request.
fn next_step(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let task = task_in_front(workspace, cx)?;
    let (project, on_git) = {
        let space = workspace.active_space()?.read(cx);
        (
            space.project().id,
            space.is_context_open() && space.context_tab() == crate::bridge::REVIEW_TOOL,
        )
    };
    let session = task.session.id;
    let changed = task
        .session
        .facts
        .as_ref()
        .is_some_and(|facts| facts.files > 0);
    let waits = matches!(
        task.column,
        crate::bridge::Column::Review | crate::bridge::Column::NeedsYou
    );
    if changed && waits && !on_git {
        return Some(
            div()
                .id("next-step")
                .occlude()
                .child(
                    Button::new("task-review")
                        .outline()
                        .small()
                        .label("Review changes")
                        .tooltip("Read what this task changed, in the Git tool")
                        .on_click(cx.listener(move |workspace, _, window, cx| {
                            workspace.review_task(project, session, window, cx)
                        })),
                )
                .into_any_element(),
        );
    }
    let pull = task_folder(workspace, &task, cx)
        .and_then(|folder| crate::task_facts::of(&folder, cx)?.pull.clone())?;
    let url = pull.url.clone();
    Some(
        div()
            .id("next-step")
            .occlude()
            .child(
                Button::new("task-pull")
                    .outline()
                    .small()
                    .icon(Icons::GitPullRequest)
                    .label(format!("#{}", pull.number))
                    .tooltip("Open the pull request of this branch in the browser")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .into_any_element(),
    )
}

/// More (⋯), right of the search: making things (presentations, with the
/// latest to open again), the tools only the palette otherwise reaches,
/// and help.
fn more_button() -> impl IntoElement {
    Button::new("more-menu")
        .ghost()
        .small()
        .icon(IconName::Ellipsis)
        .tooltip("More: tools, help")
        .accessibility_label("More")
        .menu(gpui_kit::Anchor::TopLeft, |menu, _, _| {
            let item =
                |label: &'static str, icon: Icons, action: fn() -> Box<dyn gpui_kit::Action>| {
                    PopupMenuItem::new(label)
                        .icon(icon)
                        .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
                };
            let mut menu = menu;
            let tools: [MenuEntry; 7] = [
                ("Utilities", Icons::Wrench, || Box::new(ShowUtilities)),
                ("Parallel runs", Icons::Columns2, || {
                    Box::new(ShowParallelRuns)
                }),
                ("Schedules", Icons::CalendarClock, || {
                    Box::new(ShowSchedules)
                }),
                ("Usage", Icons::ChartPie, || Box::new(ShowUsage)),
                ("Network drives", Icons::HardDrive, || {
                    Box::new(ShowNetworkDrives)
                }),
                ("Agent instructions", Icons::BookOpen, || {
                    Box::new(ShowInstructions)
                }),
                ("Search chats", Icons::Search, || Box::new(SearchChats)),
            ];
            menu = menu.label("Tools");
            for (label, icon, action) in tools {
                // Not what this build leaves out (the brand's features).
                if crate::features::allows(action().as_ref()) {
                    menu = menu.item(item(label, icon, action));
                }
            }
            menu.separator()
                .label("Help")
                .when(crate::features::allows(&ShowTour), |menu| {
                    menu.item(item("Welcome tour", Icons::Sparkles, || Box::new(ShowTour)))
                })
                .item(item("Keyboard shortcuts", Icons::Keyboard, || {
                    Box::new(ShowShortcuts)
                }))
                .item(item("Settings", Icons::Settings, || Box::new(OpenSettings)))
        })
}

/// A row of the More menu: label, icon, and the action it dispatches.
type MenuEntry = (&'static str, Icons, fn() -> Box<dyn gpui_kit::Action>);

/// How much more room each side of the bar gets, so its middle is the
/// window's: the width of the window's own buttons on the other side.
fn chrome_bias(window: &Window) -> (gpui_kit::Pixels, gpui_kit::Pixels) {
    // gpui-component's title bar: 80 px for macOS's buttons on the left;
    // elsewhere the bar starts at the window's edge (its 12 px there are
    // taken off, see where the bar is made). Windows and client-decorated
    // Linux draw three 34 px buttons on the right.
    if cfg!(target_os = "macos") {
        return (px(0.), px(80.));
    }
    let controls = cfg!(windows)
        || matches!(
            window.window_decorations(),
            gpui_kit::Decorations::Client { .. }
        );
    if controls {
        (px(102.), px(0.))
    } else {
        (px(0.), px(0.))
    }
}

/// The switches for what the view in front can show or hide: a
/// workspace's chat and its result pane, Reviews' list and its chat. The
/// Bridge has none.
fn panel_switches(workspace: &Workspace, cx: &App) -> Vec<AnyElement> {
    let switch = |id: &'static str,
                  icon: IconName,
                  what: &'static str,
                  keys: &'static str,
                  action: fn() -> Box<dyn gpui_kit::Action>| {
        let label = format!("Show or hide {what}");
        Button::new(id)
            .occlude()
            .ghost()
            .small()
            .icon(icon)
            .tooltip(ph_ui::tip(format!(
                "{label} ({})",
                ph_ui::platform::shortcut(keys)
            )))
            .accessibility_label(label)
            .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
            .into_any_element()
    };
    match workspace.view() {
        View::Workspaces => {
            let folded = workspace
                .active_space()
                .is_some_and(|space| space.read(cx).is_chat_folded());
            vec![
                switch(
                    "toggle-chat",
                    if folded {
                        IconName::PanelLeftOpen
                    } else {
                        IconName::PanelLeftClose
                    },
                    "the chat",
                    "secondary-b",
                    || Box::new(ToggleSidebar),
                ),
                switch(
                    "toggle-context-panel",
                    IconName::PanelRight,
                    "the result pane",
                    "secondary-alt-b",
                    || Box::new(ToggleContextPanel),
                ),
            ]
        }
        View::GitHub => vec![
            switch(
                "toggle-sidebar",
                if workspace.is_sidebar_collapsed() {
                    IconName::PanelLeftOpen
                } else {
                    IconName::PanelLeftClose
                },
                "the list",
                "secondary-b",
                || Box::new(ToggleSidebar),
            ),
            // The pull request's chat, on the right: the same key as a
            // workspace's pane on that side.
            switch(
                "toggle-review-chat",
                IconName::PanelRight,
                "the chat",
                "secondary-alt-b",
                || Box::new(ToggleContextPanel),
            ),
        ],
        View::Bridge | View::Custom(_) | View::NoWorkspaces | View::Artifacts => Vec::new(),
    }
}

/// An update, only once there is one: ready to install, waiting to be
/// downloaded, or (for an install PilotHouse cannot update) on GitHub.
fn update_item(cx: &App) -> Option<AnyElement> {
    use crate::updates::{self, Status};
    let dot = |color| div().size(ph_ui::rems_from_px(6.)).rounded_full().bg(color);
    let button =
        |id: &'static str, label: String| Button::new(id).occlude().ghost().small().label(label);
    let item = match updates::Updater::status(cx) {
        Status::Ready { version, .. } => button("update-restart", "Restart to update".into())
            .child(dot(cx.theme().success))
            .tooltip(ph_ui::tip(format!(
                "{} {version} is downloaded. Restart to install it now; otherwise it installs when you quit.",
                ph_brand::NAME
            )))
            .on_click(|_, _, cx| updates::restart_to_update(cx)),
        Status::Available { version } => button("update-download", format!("Update to {version}"))
            .tooltip("Download and install the new version")
            .on_click(|_, _, cx| updates::download_update(cx)),
        Status::Manual { version } => button("update-manual", format!("{version} available"))
            .tooltip(ph_ui::tip("This copy cannot update itself: get the new version from GitHub"))
            .on_click(|_, _, cx| cx.open_url(updates::RELEASES_URL)),
        _ => return None,
    };
    Some(item.into_any_element())
}

/// The app's mark, its icon in small. Off macOS, where there is no menu
/// bar, it opens the app's menu, and stands centred over the column of
/// icons under the bar (as wide as that column, from the window's edge):
/// the mark and the icons are on one line.
fn app_icon(cx: &App) -> AnyElement {
    let mark = ph_ui::logo_mark(ph_ui::rems_from_px(20.));
    if cfg!(target_os = "macos") {
        return div().mx_1().child(mark).into_any_element();
    }
    let column = crate::rail::width_px(crate::rail::labels(cx));
    let button = Button::new("app-menu")
        .occlude()
        .ghost()
        .small()
        .child(mark)
        .tooltip(ph_ui::tip(ph_brand::NAME))
        .accessibility_label(format!("{} menu", ph_brand::NAME))
        .menu(gpui_kit::Anchor::TopLeft, |menu, _, _| {
            let item = |label: &'static str, action: fn() -> Box<dyn gpui_kit::Action>| {
                PopupMenuItem::new(label)
                    .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
            };
            menu.item(item("New workspace", || Box::new(NewWorkspace)))
                .item(item("Open folder", || Box::new(AddProject)))
                .item(item("Add folder to this workspace", || {
                    Box::new(AddFolderToWorkspace)
                }))
                .separator()
                .item(item("New chat", || Box::new(NewSession)))
                .item(item("Command palette", || Box::new(ToggleCommandPalette)))
                .item(item("Settings", || Box::new(OpenSettings)))
                .separator()
                .item(item(concat!("Quit ", ph_brand::name!()), || Box::new(Quit)))
        });
    h_flex()
        .flex_none()
        .w(ph_ui::rems_from_px(column))
        .justify_center()
        .child(button)
        .into_any_element()
}

/// What is in front, said beside the app's mark: *Bridge*, *Reviews*, an
/// organisation's view, or the workspace's name with its colour (*Chats*
/// for the space without folders). The left column of icons switches
/// between them; this names where the user is.
fn front_name(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let name: gpui_kit::SharedString = match workspace.view() {
        View::Bridge => "Bridge".into(),
        View::GitHub => "Reviews".into(),
        View::Artifacts => "Artifacts".into(),
        View::NoWorkspaces => "Workspaces".into(),
        View::Custom(id) => ph_custom::views()
            .into_iter()
            .find(|custom| custom.id == id)?
            .label
            .into(),
        View::Workspaces => {
            let id = workspace.active_project_id()?;
            workspace
                .projects()
                .iter()
                .find(|project| project.id == id)?
                .name
                .clone()
                .into()
        }
    };
    let theme = cx.theme();
    // One of the user's workspaces: its name opens the list of them. The
    // Bridge, Reviews and Chats are places of their own, named plainly.
    let switches = workspace.view() == View::Workspaces
        && workspace
            .active_project_id()
            .is_some_and(|id| workspace.listed().any(|project| project.id == id));
    let tint = crate::frame::tint(workspace, cx);
    let Some((id, color)) = workspace.active_project_id().zip(tint).filter(|_| switches) else {
        return Some(
            h_flex()
                .min_w_0()
                .gap_1p5()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                // A workspace's colour, as in the frame's light.
                .children(tint.map(|color| {
                    div()
                        .flex_none()
                        .size(ph_ui::rems_from_px(8.))
                        .rounded_full()
                        .bg(color)
                }))
                .child(div().min_w_0().truncate().child(name))
                .into_any_element(),
        );
    };
    // The workspace is what everything below belongs to, and its name is a
    // control: so it is said in full strength, with the workspace's mark
    // (as in the list it opens) and an outline. As a dim word with a dot it
    // went unnoticed, and so did that it can be pressed.
    let label = h_flex()
        .min_w_0()
        .gap_1p5()
        .text_sm()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(theme.foreground)
        .child(crate::workspace_switcher::badge(
            &name, color, true, 18., cx,
        ))
        .child(div().min_w_0().truncate().child(name))
        .child(
            Icon::new(IconName::ChevronDown)
                .xsmall()
                .text_color(theme.muted_foreground),
        );
    let switcher = workspace.switcher().clone();
    let owner = cx.entity().downgrade();
    let popover = Popover::new("workspace-switcher-popover")
        .offset(ph_ui::POPUP_GAP)
        .anchor(gpui_kit::Anchor::TopLeft)
        .open(workspace.is_switcher_open())
        .on_open_change(cx.listener(|workspace, open: &bool, window, cx| {
            workspace.set_switcher_open(*open, window, cx)
        }))
        .trigger(
            Button::new("workspace-switcher")
                .outline()
                .small()
                .child(label)
                .tooltip(if cfg!(target_os = "macos") {
                    "Switch workspace (⌘E)"
                } else {
                    "Switch workspace (Ctrl+E)"
                })
                .accessibility_label("Switch workspace"),
        )
        .content(move |_, _, _| switcher.clone());
    Some(
        div()
            .id("workspace-name")
            // The bar is the window's handle: what is pressed here is not.
            .occlude()
            .min_w_0()
            .child(popover)
            // What can be done with the workspace in front, as in its row
            // of the list.
            .context_menu(move |menu, window, cx| {
                crate::bridge::workspace_menu_items(menu, owner.clone(), id, window, cx)
            })
            .into_any_element(),
    )
}
