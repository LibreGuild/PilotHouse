//! Contents of the window title bar: the app icon, the workspace switcher
//! (the only one: every workspace with what it is doing, and making new
//! ones), the command palette trigger, and window-level toggles. The bar
//! wears a faint wash of the workspace's colour.

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Enter, Input, MoveDown, MoveUp},
    kbd::Kbd,
    menu::PopupMenuItem,
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Background, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Rgba, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, linear_color_stop, linear_gradient, px,
};
use ph_store::projects::{Project, ProjectId};
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::actions::*;
use crate::status::WorkspaceStatus;
use crate::workspace::Workspace;

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
        .child(switcher(workspace, cx))
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

/// One workspace as the switcher lists it.
#[derive(Clone)]
struct Row {
    id: ProjectId,
    name: String,
    shade: usize,
    status: WorkspaceStatus,
}

/// The workspace in front, as a button that lists every workspace with
/// what it is doing, and makes new ones. A badge counts the others that
/// want the user.
fn switcher(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let active = workspace.active_project_id();
    let colors = ph_ui::WORKSPACE_COLORS.len();
    let rows: Vec<Row> = workspace
        .projects()
        .iter()
        .map(|project| Row {
            id: project.id,
            name: project.name.clone(),
            shade: project.shade(colors),
            status: workspace.status(project.id),
        })
        .collect();
    let current = rows.iter().find(|row| Some(row.id) == active).cloned();
    // The filter keeps each row's place in the full list (its shortcut).
    let query = workspace
        .switcher_search
        .read(cx)
        .value()
        .trim()
        .to_lowercase();
    let listed: Vec<(usize, Row)> = rows
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, row)| query.is_empty() || row.name.to_lowercase().contains(&query))
        .collect();
    let highlight = workspace
        .switcher_highlight
        .min(listed.len().saturating_sub(1));
    let search = workspace.switcher_search.clone();
    let others_want_you = rows
        .iter()
        .filter(|row| Some(row.id) != active && row.status.wants_you())
        .count();
    let dark = cx.theme().is_dark();

    let label = current
        .as_ref()
        .map(|row| row.name.clone())
        .unwrap_or_else(|| "No workspace".into());
    let trigger = Button::new("project-switcher")
        .ghost()
        .small()
        .child(
            h_flex()
                .gap_1p5()
                .when_some(current.as_ref(), |this, row| {
                    this.child(color_dot(ph_ui::workspace_color(row.shade, dark)))
                })
                .child(label),
        )
        .dropdown_caret(true)
        .tooltip("Switch workspace")
        .accessibility_label("Switch workspace");
    let entity = cx.entity().downgrade();
    let opened = entity.clone();
    let popover = Popover::new("workspace-switcher")
        .offset(ph_ui::POPUP_GAP)
        .anchor(gpui_kit::Anchor::TopLeft)
        .trigger(trigger)
        .on_open_change(move |open, window, cx| {
            if *open {
                _ = opened.update(cx, |workspace, cx| workspace.switcher_opened(window, cx));
            }
        })
        .content(move |_, _, cx| {
            let popover = cx.entity().downgrade();
            let dark = cx.theme().is_dark();
            let close = move |window: &mut Window, cx: &mut App| {
                _ = popover.update(cx, |popover, cx| popover.dismiss(window, cx));
            };
            // ↑/↓ move the highlight, Enter opens it.
            let step = |by: isize| {
                let (entity, count) = (entity.clone(), listed.len());
                move |window: &mut Window, cx: &mut App| {
                    let _ = window;
                    cx.stop_propagation();
                    _ = entity.update(cx, |workspace, cx| {
                        let next =
                            workspace.switcher_highlight.min(count.saturating_sub(1)) as isize + by;
                        workspace.switcher_highlight =
                            next.clamp(0, count.saturating_sub(1) as isize) as usize;
                        cx.notify();
                    });
                }
            };
            let (up, down) = (step(-1), step(1));
            let enter = {
                let (entity, close) = (entity.clone(), close.clone());
                let target = listed.get(highlight).map(|(_, row)| row.id);
                move |window: &mut Window, cx: &mut App| {
                    cx.stop_propagation();
                    if let Some(id) = target {
                        _ = entity
                            .update(cx, |workspace, cx| workspace.open_project(id, window, cx));
                        close(window, cx);
                    }
                }
            };
            let list = listed.iter().enumerate().map(|(shown, (ix, row))| {
                let ix = *ix;
                let (entity, close, id) = (entity.clone(), close.clone(), row.id);
                let selected = Some(row.id) == active;
                let highlighted = shown == highlight;
                let summary = row.status.summary(cx);
                h_flex()
                    .id(("switch-workspace", ix))
                    .w_full()
                    .h_8()
                    .px_2()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .when(selected, |this| this.bg(cx.theme().secondary))
                    .when(highlighted && !selected, |this| {
                        this.bg(cx.theme().secondary_hover)
                    })
                    .hover(|style| style.bg(cx.theme().secondary_hover))
                    .on_click(move |_, window, cx| {
                        _ = entity
                            .update(cx, |workspace, cx| workspace.open_project(id, window, cx));
                        close(window, cx);
                    })
                    .child(color_dot(ph_ui::workspace_color(row.shade, dark)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(row.name.clone()),
                    )
                    .when_some(summary, |this, (text, color)| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .text_color(color)
                                .child(color_dot(color))
                                .child(text),
                        )
                    })
                    .when(ix < 9, |this| {
                        this.child(
                            div()
                                .w_7()
                                .text_right()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(switch_hint(ix)),
                        )
                    })
            });
            let action = |id: &'static str,
                          label: &'static str,
                          icon: Icons,
                          make: fn() -> Box<dyn gpui_kit::Action>| {
                let close = close.clone();
                h_flex()
                    .id(id)
                    .w_full()
                    .h_8()
                    .px_2()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .text_sm()
                    .hover(|style| style.bg(cx.theme().secondary_hover))
                    .on_click(move |_, window, cx| {
                        close(window, cx);
                        window.dispatch_action(make(), cx);
                    })
                    .child(
                        Icon::new(icon)
                            .small()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(label)
            };
            v_flex()
                .w(px(340.))
                .gap_0p5()
                .capture_action(move |_: &MoveUp, window, cx| up(window, cx))
                .capture_action(move |_: &MoveDown, window, cx| down(window, cx))
                .capture_action(move |_: &Enter, window, cx| enter(window, cx))
                .child(section_label("Workspaces", cx))
                .child(
                    div().px_1().pb_1().child(
                        Input::new(&search)
                            .small()
                            .prefix(Icon::new(IconName::Search).xsmall()),
                    ),
                )
                .children(list)
                .child(div().my_1().h_px().bg(cx.theme().border))
                .child(action(
                    "switcher-new-workspace",
                    "New workspace",
                    Icons::SquarePen,
                    || Box::new(NewWorkspace),
                ))
                .child(action(
                    "switcher-open-folder",
                    "Open folder as a workspace…",
                    Icons::FolderOpen,
                    || Box::new(AddProject),
                ))
                .when(active.is_some(), |this| {
                    this.child(div().my_1().h_px().bg(cx.theme().border))
                        .child(section_label("This workspace", cx))
                        .child(action(
                            "switcher-add-folder",
                            "Add a folder…",
                            Icons::FolderPlus,
                            || Box::new(AddFolderToWorkspace),
                        ))
                        .child(action(
                            "switcher-rename",
                            "Rename…",
                            Icons::Pencil,
                            || Box::new(RenameWorkspace),
                        ))
                        .child(action(
                            "switcher-new-window",
                            "Open in new window",
                            Icons::ExternalLink,
                            || Box::new(OpenInNewWindow),
                        ))
                        .child(color_swatches(entity.clone(), active, &rows, cx))
                        .child(action(
                            "switcher-remove",
                            "Remove from PilotHouse",
                            Icons::Trash,
                            || Box::new(RemoveWorkspace),
                        ))
                })
        });
    let badge = (others_want_you > 0).then(|| {
        div()
            .absolute()
            .top(px(2.))
            .right(px(2.))
            .size(px(8.))
            .rounded_full()
            .bg(cx.theme().warning)
    });
    div().relative().child(popover).children(badge)
}

/// ⌃1…⌃9 on macOS, Alt+1…9 elsewhere (⌘/Ctrl+digits show tools).
fn switch_hint(ix: usize) -> String {
    if cfg!(target_os = "macos") {
        format!("⌃{}", ix + 1)
    } else {
        format!("Alt+{}", ix + 1)
    }
}

/// The colours to pick for the workspace in front.
fn color_swatches(
    workspace: gpui_kit::WeakEntity<Workspace>,
    active: Option<ProjectId>,
    rows: &[Row],
    cx: &App,
) -> impl IntoElement {
    let dark = cx.theme().is_dark();
    let current = rows
        .iter()
        .find(|row| Some(row.id) == active)
        .map(|row| row.shade);
    h_flex()
        .px_2()
        .py_1()
        .gap_1p5()
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Colour"),
        )
        .children(
            ph_ui::WORKSPACE_COLORS
                .iter()
                .enumerate()
                .map(|(ix, (name, _))| {
                    let workspace = workspace.clone();
                    let color = ph_ui::workspace_color(ix, dark);
                    div()
                        .id(SharedString::from(format!("workspace-color-{ix}")))
                        .size(px(16.))
                        .rounded_full()
                        .bg(color)
                        .cursor_pointer()
                        .when(current == Some(ix), |this| {
                            this.border_2().border_color(cx.theme().foreground)
                        })
                        .hover(|style| style.opacity(0.8))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(*name).build(window, cx)
                        })
                        .on_click(move |_, _, cx| {
                            if let Some(id) = active {
                                _ = workspace.update(cx, |workspace, cx| {
                                    workspace.set_color(id, Some(ix as u8), cx)
                                });
                            }
                        })
                }),
        )
}

fn color_dot(color: Hsla) -> impl IntoElement {
    div().flex_none().size(px(8.)).rounded_full().bg(color)
}

fn section_label(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .pt_1()
        .pb_0p5()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}
