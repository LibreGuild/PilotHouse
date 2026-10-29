//! Contents of the window title bar: the app's mark, the two views
//! (Workspaces and GitHub), the command palette trigger in the middle, and
//! window-level buttons (inbox, agents, appearance, settings, the side
//! panels). The bar is quiet chrome: plain, so attention stays on the work.
//! Workspaces are picked in the sidebar.
//!
//! On Windows the whole bar is a caption (drag) area for the system, so a
//! click on it moves the window and a double click maximises it. Every
//! control here is `occlude`d so its clicks reach the app instead.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
    menu::PopupMenuItem,
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
    let dark = cx.theme().is_dark();
    let palette_hint =
        Kbd::binding_for_action(&ToggleCommandPalette, Some(WORKSPACE_CONTEXT), window);
    // The search sits in the middle of the window, not of the space left
    // between the window's buttons: the side with less of the window's own
    // chrome gets that much more room (macOS's buttons are on the left,
    // Windows' and Linux's on the right).
    let (left_bias, right_bias) = chrome_bias(window);
    h_flex()
        .w_full()
        .h_full()
        .pr_2p5()
        .gap_1()
        .child(
            h_flex()
                .flex_1()
                .flex_basis(left_bias)
                .gap_1()
                .child(app_icon(cx))
                .child(view_toggle(workspace, cx)),
        )
        .child(
            h_flex()
                .id("open-command-palette")
                .role(gpui_kit::accesskit::Role::Button)
                .aria_label("Search or run a command")
                .occlude()
                .w_96()
                .min_w(px(160.))
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
                .justify_end()
                .gap_1()
                .children(update_item(cx))
                .child(
                    div()
                        .id("inbox-button")
                        .occlude()
                        .child(crate::inbox::button(&workspace.inbox, cx)),
                )
                .child(
                    div()
                        .id("agents-button")
                        .occlude()
                        .child(agents_button(workspace, cx)),
                )
                .child(
                    Button::new("toggle-appearance")
                        .occlude()
                        .ghost()
                        .small()
                        .icon(if dark { IconName::Sun } else { IconName::Moon })
                        .tooltip(if dark {
                            "Use light appearance"
                        } else {
                            "Use dark appearance"
                        })
                        .accessibility_label("Toggle appearance")
                        .on_click(move |_, window, cx| {
                            if dark {
                                window.dispatch_action(Box::new(UseLightAppearance), cx);
                            } else {
                                window.dispatch_action(Box::new(UseDarkAppearance), cx);
                            }
                        }),
                )
                .child(
                    Button::new("open-settings")
                        .occlude()
                        .ghost()
                        .small()
                        .icon(Icons::Settings)
                        .tooltip("Settings")
                        .accessibility_label("Settings")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(OpenSettings), cx)
                        }),
                )
                // The two side panels' switches side by side, as in VS Code.
                .child(div().w_px().h_4().mx_1().bg(cx.theme().border))
                .child(
                    Button::new("toggle-sidebar")
                        .occlude()
                        .ghost()
                        .small()
                        .icon(if workspace.is_sidebar_collapsed() {
                            IconName::PanelLeftOpen
                        } else {
                            IconName::PanelLeftClose
                        })
                        .tooltip("Toggle sidebar (⌘B)")
                        .accessibility_label("Toggle sidebar")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleSidebar), cx)
                        }),
                )
                .child(
                    Button::new("toggle-context-panel")
                        .occlude()
                        .ghost()
                        .small()
                        .icon(IconName::PanelRight)
                        .tooltip("Toggle tools (⌘J)")
                        .accessibility_label("Toggle panel")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleContextPanel), cx)
                        }),
                ),
        )
}

/// How much more room each side of the bar gets, so its middle is the
/// window's: the width of the window's own buttons on the other side.
fn chrome_bias(window: &Window) -> (gpui_kit::Pixels, gpui_kit::Pixels) {
    // gpui-component's title bar: 80 px for macOS's buttons on the left, 12
    // elsewhere; Windows and client-decorated Linux draw three 34 px
    // buttons on the right.
    if cfg!(target_os = "macos") {
        return (px(0.), px(80.));
    }
    let controls = cfg!(windows)
        || matches!(
            window.window_decorations(),
            gpui_kit::Decorations::Client { .. }
        );
    if controls {
        (px(102. - 12.), px(0.))
    } else {
        (px(0.), px(12.))
    }
}

/// The Agents button: how many chats need the user (amber) or are at work,
/// and the popup listing them all.
fn agents_button(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    use gpui_kit::component::popover::Popover;
    let (needs_you, working) = crate::agents_board::counts(workspace, cx);
    let hint = if cfg!(target_os = "macos") {
        "⌘⇧A"
    } else {
        "Ctrl+Shift+A"
    };
    let trigger = Button::new("agents")
        .ghost()
        .small()
        .icon(Icons::Bot)
        .tooltip(format!("Agents ({hint})"))
        .accessibility_label("Agents")
        .when(needs_you > 0, |this| {
            this.child(
                div()
                    .px_1p5()
                    .rounded_full()
                    .bg(cx.theme().warning.opacity(0.2))
                    .text_color(cx.theme().warning)
                    .text_xs()
                    .child(needs_you.to_string()),
            )
        })
        .when(needs_you == 0 && working > 0, |this| {
            this.child(
                gpui_kit::component::spinner::Spinner::new()
                    .xsmall()
                    .color(cx.theme().info),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(working.to_string()),
            )
        });
    let this = cx.entity().downgrade();
    Popover::new("agents-popover")
        .offset(ph_ui::POPUP_GAP)
        .anchor(gpui_kit::Anchor::TopRight)
        .open(workspace.agents_open)
        .on_open_change({
            let this = this.clone();
            move |open, _, cx| {
                _ = this.update(cx, |workspace, cx| workspace.set_agents_open(*open, cx));
            }
        })
        .trigger(trigger)
        .content(move |_, _, cx| {
            let Some(workspace) = this.upgrade() else {
                return div().into_any_element();
            };
            crate::agents_board::popup(workspace.read(cx), this.clone(), cx)
        })
}

/// An update, only once there is one: ready to install, waiting to be
/// downloaded, or (for an install PilotHouse cannot update) on GitHub.
fn update_item(cx: &App) -> Option<AnyElement> {
    use crate::updates::{self, Status};
    let dot = |color| div().size(px(6.)).rounded_full().bg(color);
    let button =
        |id: &'static str, label: String| Button::new(id).occlude().ghost().small().label(label);
    let item = match updates::Updater::status(cx) {
        Status::Ready { version, .. } => button("update-restart", "Restart to update".into())
            .child(dot(cx.theme().success))
            .tooltip(format!(
                "PilotHouse {version} is downloaded. Restart to install it now; otherwise it installs when you quit."
            ))
            .on_click(|_, _, cx| updates::restart_to_update(cx)),
        Status::Available { version } => button("update-download", format!("Update to {version}"))
            .tooltip("Download and install the new version")
            .on_click(|_, _, cx| updates::download_update(cx)),
        Status::Manual { version } => button("update-manual", format!("{version} available"))
            .tooltip("This copy cannot update itself: get the new version from GitHub")
            .on_click(|_, _, cx| cx.open_url(updates::RELEASES_URL)),
        _ => return None,
    };
    Some(item.into_any_element())
}

/// The app's mark, its icon in small. Off macOS, where there is no menu
/// bar, it opens the app's menu.
fn app_icon(_: &App) -> AnyElement {
    let mark = div().mx_1().child(ph_ui::logo_mark(px(20.)));
    if cfg!(target_os = "macos") {
        return mark.into_any_element();
    }
    Button::new("app-menu")
        .occlude()
        .ghost()
        .small()
        .child(mark)
        .tooltip("PilotHouse")
        .accessibility_label("PilotHouse menu")
        .menu(gpui_kit::Anchor::TopLeft, |menu, _, _| {
            let item = |label: &'static str, action: fn() -> Box<dyn gpui_kit::Action>| {
                PopupMenuItem::new(label)
                    .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
            };
            menu.item(item("New workspace", || Box::new(NewWorkspace)))
                .item(item("Open folder…", || Box::new(AddProject)))
                .item(item("Add folder to this workspace…", || {
                    Box::new(AddFolderToWorkspace)
                }))
                .separator()
                .item(item("New chat", || Box::new(NewSession)))
                .item(item("Command palette", || Box::new(ToggleCommandPalette)))
                .item(item("Settings…", || Box::new(OpenSettings)))
                .separator()
                .item(item("Quit PilotHouse", || Box::new(Quit)))
        })
        .into_any_element()
}

/// The two views, side by side: Workspaces and GitHub (how many pull
/// requests wait for the user's review).
fn view_toggle(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let current = workspace.view();
    let waiting = workspace
        .github_view()
        .map_or(0, |github| github.read(cx).waiting_for_review());
    // Other workspaces with something for the user: amber when an agent
    // waits for an answer, green when there is only news.
    let others: Vec<crate::status::WorkspaceStatus> = workspace
        .projects()
        .iter()
        .map(|project| project.id)
        .chain(workspace.chats_space().map(|chats| chats.id))
        .filter(|id| Some(*id) != workspace.active_project_id() || current != View::Workspaces)
        .map(|id| workspace.status(id))
        .collect();
    let news_color = if others.iter().any(|status| status.waiting > 0) {
        Some(cx.theme().warning)
    } else if others.iter().any(|status| status.unseen > 0) {
        Some(cx.theme().success)
    } else {
        None
    };
    let this = cx.entity().downgrade();
    let item = |view: View,
                id: &'static str,
                label: &'static str,
                icon: Icons,
                hint: &'static str,
                badge: Option<AnyElement>,
                cx: &App| {
        let selected = view == current;
        h_flex()
            .id(id)
            // Blocks the title bar behind it: pressing here must not start a
            // window move (which swallows the click if the mouse moves).
            .occlude()
            .relative()
            .h_6()
            .px_2p5()
            .gap_1p5()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .text_sm()
            .role(gpui_kit::accesskit::Role::Tab)
            .aria_label(label)
            .text_color(if selected {
                cx.theme().foreground
            } else {
                cx.theme().muted_foreground
            })
            .when(selected, |this| {
                this.bg(cx.theme().background)
                    .shadow_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
            })
            .when(!selected, |this| {
                this.hover(|style| style.text_color(cx.theme().foreground))
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(format!("{label} ({hint})"))
                    .build(window, cx)
            })
            // On press, like a tab: nothing can come between press and
            // release to lose it (the mouse moving, a slow frame). Straight
            // to the workspace, not as an action: an action goes where the
            // keyboard is, and that can be a view no longer shown.
            .on_mouse_down(gpui_kit::MouseButton::Left, {
                let this = this.clone();
                move |_, window, cx| {
                    cx.stop_propagation();
                    _ = this.update(cx, |workspace, cx| workspace.set_view(view, window, cx));
                }
            })
            .child(Icon::new(icon).small())
            .child(label)
            .children(badge)
            .test_support()
    };
    let (workspaces_hint, github_hint) = if cfg!(target_os = "macos") {
        ("⌘⇧E", "⌘⇧G")
    } else {
        ("Ctrl+Shift+E", "Ctrl+Shift+G")
    };
    let dot = news_color.map(|color| {
        div()
            .absolute()
            .top(px(2.))
            .right(px(2.))
            .size(px(7.))
            .rounded_full()
            .bg(color)
            .into_any_element()
    });
    let count = (waiting > 0).then(|| {
        div()
            .px_1p5()
            .rounded_full()
            .bg(cx.theme().warning.opacity(0.2))
            .text_color(cx.theme().warning)
            .text_xs()
            .child(waiting.to_string())
            .into_any_element()
    });
    h_flex()
        .id("view-toggle")
        .occlude()
        .role(gpui_kit::accesskit::Role::TabList)
        .p_0p5()
        .gap_0p5()
        .rounded(cx.theme().radius)
        .bg(cx.theme().muted)
        .child(item(
            View::Workspaces,
            "view-workspaces",
            "Workspaces",
            Icons::LayoutGrid,
            workspaces_hint,
            dot,
            cx,
        ))
        .child(item(
            View::GitHub,
            "view-github",
            "GitHub",
            Icons::Github,
            github_hint,
            count,
            cx,
        ))
}
