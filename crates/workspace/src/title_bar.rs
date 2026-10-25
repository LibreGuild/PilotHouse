//! Contents of the window title bar: the app icon, the two views
//! (Workspaces and GitHub), the command palette trigger, and window-level
//! toggles. In the Workspaces view the bar wears a faint wash of the
//! workspace's colour. Workspaces are picked in the sidebar.

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
    AnyElement, App, Background, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Rgba, StatefulInteractiveElement as _, Styled as _, Window, div,
    linear_color_stop, linear_gradient, px,
};
use ph_store::projects::Project;
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

    h_flex()
        .w_full()
        .h_full()
        .pr_2p5()
        .gap_1()
        .child(app_icon(cx))
        .child(
            Button::new("toggle-sidebar")
                .ghost()
                .small()
                .icon(if workspace.is_sidebar_collapsed() {
                    IconName::PanelLeftOpen
                } else {
                    IconName::PanelLeftClose
                })
                .tooltip("Toggle sidebar (⌘B)")
                .accessibility_label("Toggle sidebar")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx)),
        )
        .child(view_toggle(workspace, cx))
        .child(div().flex_1())
        // Command palette trigger, centred feel without stealing the drag area.
        .child(
            h_flex()
                .id("open-command-palette")
                .w_96()
                .max_w_1_2()
                .h_7()
                .px_2p5()
                .gap_2()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().muted))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(ToggleCommandPalette), cx)
                })
                .child(Icon::new(IconName::Search).small())
                .child(div().flex_1().child("Search or run a command"))
                .when_some(palette_hint, |this, hint| this.child(hint)),
        )
        .child(div().flex_1())
        .child(crate::inbox::button(&workspace.inbox, cx))
        .child(
            Button::new("toggle-appearance")
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
            Button::new("toggle-context-panel")
                .ghost()
                .small()
                .icon(IconName::PanelRight)
                .tooltip("Toggle tools (⌘J)")
                .accessibility_label("Toggle panel")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleContextPanel), cx)),
        )
        .child(
            Button::new("open-settings")
                .ghost()
                .small()
                .icon(Icons::Settings)
                .tooltip("Settings")
                .accessibility_label("Settings")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
        )
}

/// The title bar's background: the theme's, washed with the workspace's
/// colour (none when no workspace is open).
pub(crate) fn background(project: Option<&Project>, cx: &App) -> Background {
    let theme = cx.theme();
    let tint = project.map(|project| {
        ph_ui::workspace_tint(
            project.shade(ph_ui::WORKSPACE_COLORS.len()),
            theme.is_dark(),
        )
    });
    let wash = |color: Hsla| tint.map_or(color, |tint| color.blend(tint));
    // As gpui-component draws it: a slight fall from the window background.
    let (bar, back) = (theme.title_bar.to_rgb(), theme.background.to_rgb());
    let mixed = Hsla::from(Rgba {
        r: bar.r * 0.55 + back.r * 0.45,
        g: bar.g * 0.55 + back.g * 0.45,
        b: bar.b * 0.55 + back.b * 0.45,
        a: bar.a * 0.55 + back.a * 0.45,
    });
    linear_gradient(
        180.,
        linear_color_stop(wash(mixed), 0.),
        linear_color_stop(wash(theme.title_bar), 1.),
    )
}

/// The app's mark. Off macOS, where there is no menu bar, it opens the
/// app's menu.
fn app_icon(_: &App) -> AnyElement {
    let mark = div().flex_none().mr_1().child(ph_ui::app_icon(px(22.)));
    if cfg!(target_os = "macos") {
        return mark.into_any_element();
    }
    Button::new("app-menu")
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

/// The two views, side by side: Workspaces and GitHub (with how many pull
/// requests wait for the user's review).
fn view_toggle(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let current = workspace.view();
    let waiting = workspace
        .github_view()
        .map_or(0, |github| github.read(cx).waiting_for_review());
    let wants_you = workspace
        .projects()
        .iter()
        .map(|project| project.id)
        .chain(workspace.chats_space().map(|chats| chats.id))
        .filter(|id| Some(*id) != workspace.active_project_id() || current != View::Workspaces)
        .any(|id| workspace.status(id).wants_you());
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
            .on_click(move |_, window, cx| {
                let action: Box<dyn gpui_kit::Action> = match view {
                    View::Workspaces => Box::new(ShowWorkspacesView),
                    View::GitHub => Box::new(ShowGitHubView),
                };
                window.dispatch_action(action, cx)
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
    let dot = wants_you.then(|| {
        div()
            .absolute()
            .top(px(2.))
            .right(px(2.))
            .size(px(7.))
            .rounded_full()
            .bg(cx.theme().warning)
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
