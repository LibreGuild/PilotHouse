//! The left sidebar of the Workspaces view: everything at once, so nothing
//! needs switching to be seen.
//!
//! - **New chat** (⌘N) and a search field (⌘⇧K) on top.
//! - **Pinned**: chats pinned from any workspace, kept on top.
//! - **Chats**: the Chats space, for chats outside any workspace (no folder
//!   needed), grouped by day.
//! - **Workspaces**: each with its colour and what it is doing; unfolded,
//!   its newest chats (all of them on request). Picking a chat opens its
//!   workspace at it. Each row's menu adds folders, renames, moves it to a
//!   window of its own, recolours or removes it.
//! - Each chat's menu pins, renames or deletes it. Every section folds away
//!   from its header.
//!
//! Chats that PilotHouse started itself (schedules, test recording and
//! repair, pull request reviews) fold into *Runs*, so the user's own stay in
//! view. A workspace's files are its Files tool, on the right.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window, div, px,
};
use jiff::{ToSpan as _, Zoned};
use ph_agent::ChatState;
use ph_store::projects::{Project, ProjectId};
use ph_store::sessions::{Session, SessionId};
use ph_theme::layout;
use ph_ui::{Icons, MenuButton as _};

use crate::actions::{
    AddFolderToWorkspace, AddProject, NewSession, NewWorkspace, OpenInNewWindow, RemoveWorkspace,
    RenameWorkspace, SearchChats,
};
use crate::workspace::{View, Workspace};

/// The chats an unfolded workspace lists before *Show all*.
const NEWEST: usize = 5;

/// Section names, as [`Workspace::folded`] keeps them.
const CHATS: &str = "chats";
const WORKSPACES: &str = "workspaces";
const PINNED: &str = "pinned";

/// The sidebar's width, as the user last dragged it (the workspace's and
/// the GitHub view's sidebars share it).
#[derive(Clone, Copy)]
struct SidebarWidth {
    width: Pixels,
    /// Bumped by each drag step: only the last one is saved.
    edit: u64,
}

impl gpui_kit::Global for SidebarWidth {}

/// Reads the saved width (at startup).
pub(crate) fn load_width(cx: &mut App) {
    let saved: f32 = crate::AppState::store(cx).setting(crate::app_state::keys::SIDEBAR_WIDTH);
    let width = if saved > 0. {
        px(saved).clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX)
    } else {
        layout::SIDEBAR_WIDTH
    };
    cx.set_global(SidebarWidth { width, edit: 0 });
}

pub(crate) fn width(cx: &App) -> Pixels {
    cx.try_global::<SidebarWidth>()
        .map_or(layout::SIDEBAR_WIDTH, |sidebar| sidebar.width)
}

/// What is dragged when the sidebar's edge is.
pub(crate) struct SidebarEdge;

impl Render for SidebarEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

/// The grip on the sidebar's right edge; the sidebar's container passes
/// the drag to [`drag_width`].
pub(crate) fn resize_edge(cx: &App) -> impl IntoElement {
    let hover = cx.theme().border;
    div()
        .id("sidebar-resize")
        .absolute()
        .top_0()
        .right(px(-3.))
        .w(px(6.))
        .h_full()
        .cursor_col_resize()
        .hover(move |style| style.bg(hover))
        .on_drag(SidebarEdge, |_, _, _, cx| cx.new(|_| SidebarEdge))
}

/// The edge was dragged to `width`: the sidebars follow, and the width is
/// saved once the drag rests.
pub(crate) fn drag_width(width: Pixels, cx: &mut App) {
    let width = width.clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX);
    let edit = cx
        .try_global::<SidebarWidth>()
        .map_or(0, |sidebar| sidebar.edit)
        + 1;
    cx.set_global(SidebarWidth { width, edit });
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(400))
            .await;
        cx.update(|cx| {
            if cx
                .try_global::<SidebarWidth>()
                .is_some_and(|sidebar| sidebar.edit == edit)
            {
                _ = crate::AppState::store(cx)
                    .set_setting(crate::app_state::keys::SIDEBAR_WIDTH, &f32::from(width));
            }
        });
    })
    .detach();
}

pub(crate) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let mut rows: Vec<AnyElement> = Vec::new();
    if !workspace.pinned.is_empty() {
        rows.extend(pinned_rows(workspace, cx));
    }
    if let Some(chats) = workspace.chats_space() {
        let folded = workspace.folded.contains(CHATS);
        rows.push(chats_header(chats.id, folded, cx));
        if !folded {
            rows.extend(chat_rows(workspace, chats.id, None, cx));
        }
    }
    let folded = workspace.folded.contains(WORKSPACES);
    rows.push(workspaces_header(workspace.projects().len(), folded, cx));
    if !folded {
        if workspace.projects().is_empty() {
            rows.push(no_workspaces(cx));
        }
        for (ix, project) in workspace.projects().iter().enumerate() {
            rows.push(workspace_row(workspace, project, ix, cx));
            if workspace.expanded.contains(&project.id) {
                // A workspace's chats hang from it on a guide line, so they
                // read as its own and not as the next section.
                rows.push(
                    v_flex()
                        .ml(px(11.))
                        .pl(px(6.))
                        .mb_1()
                        .border_l_1()
                        .border_color(cx.theme().sidebar_border)
                        .children(chat_rows(workspace, project.id, Some(NEWEST), cx))
                        .into_any_element(),
                );
            }
        }
    }
    v_flex()
        .id("workspace-sidebar")
        .flex_none()
        .w(width(cx))
        .h_full()
        .pl_2()
        .pb_2()
        .child(top(cx))
        .child(
            v_flex()
                .id("sidebar-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .pr_1()
                .pb_2()
                .children(rows),
        )
}

/// New chat and search, as plain rows with their shortcut on the right:
/// they lead the list without pulling the eye from the work.
fn top(cx: &App) -> impl IntoElement {
    let row = |id: &'static str,
               icon: Icon,
               label: &'static str,
               keys: (&'static str, &'static str),
               cx: &App| {
        let theme = cx.theme();
        h_flex()
            .id(id)
            .h_7()
            .px_2()
            .gap_2()
            .rounded(theme.radius)
            .cursor_pointer()
            .text_sm()
            .text_color(theme.sidebar_foreground)
            .hover(|style| style.bg(theme.sidebar_accent))
            .child(icon.small().text_color(theme.muted_foreground))
            .child(div().flex_1().child(label))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground.opacity(0.8))
                    .child(if cfg!(target_os = "macos") {
                        keys.0
                    } else {
                        keys.1
                    }),
            )
            .aria_label(label)
            .test_support()
    };
    v_flex()
        .flex_none()
        .mr_1()
        .pt_1()
        .pb_1()
        .border_b_1()
        .border_color(cx.theme().sidebar_border)
        .child(
            row(
                "sidebar-new-chat",
                Icon::new(Icons::SquarePen),
                "New chat",
                ("⌘N", "Ctrl+N"),
                cx,
            )
            .on_click(|_, window, cx| window.dispatch_action(Box::new(NewSession), cx)),
        )
        .child(
            row(
                "sidebar-search-chats",
                Icon::new(IconName::Search),
                "Search chats",
                ("⌘⇧K", "Ctrl+Shift+K"),
                cx,
            )
            .on_click(|_, window, cx| window.dispatch_action(Box::new(SearchChats), cx)),
        )
}

/// Pinned chats, from any workspace, each showing where it lives.
fn pinned_rows(workspace: &Workspace, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let entity = cx.entity().downgrade();
    let active = workspace
        .active_space()
        .filter(|_| workspace.view() == View::Workspaces)
        .and_then(|space| space.read(cx).active_session());
    let folded = workspace.folded.contains(PINNED);
    let mut rows = vec![section(
        "sidebar-pinned",
        "Pinned",
        (PINNED, folded),
        folded.then_some(workspace.pinned.len()),
        div(),
        cx,
    )];
    if folded {
        return rows;
    }
    for session in &workspace.pinned {
        let place = workspace
            .projects()
            .iter()
            .find(|project| project.id == session.project_id)
            .map(|project| project.name.clone());
        let chats = ph_agent::ChatHub::existing(session.project_id, cx);
        rows.push(chat_row(
            &entity,
            session.project_id,
            session,
            active,
            ChatRow {
                pinned: true,
                settled: workspace.is_settled(session.id),
                place,
            },
            chats.as_ref(),
            cx,
        ));
    }
    rows
}

fn chats_header(chats: ProjectId, folded: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let workspace = cx.entity().downgrade();
    section(
        "sidebar-chats",
        "Chats",
        (CHATS, folded),
        None,
        Button::new("sidebar-new-loose-chat")
            .ghost()
            .xsmall()
            .icon(IconName::Plus)
            .tooltip("New chat outside any workspace")
            .accessibility_label("New chat outside any workspace")
            .on_click(move |_, window, cx| {
                _ = workspace.update(cx, |workspace, cx| workspace.new_chat_in(chats, window, cx));
            }),
        cx,
    )
}

fn workspaces_header(count: usize, folded: bool, cx: &mut Context<Workspace>) -> AnyElement {
    section(
        "sidebar-workspaces",
        "Workspaces",
        (WORKSPACES, folded),
        (folded && count > 0).then_some(count),
        Button::new("sidebar-add-workspace")
            .ghost()
            .xsmall()
            .icon(IconName::Plus)
            .tooltip("New workspace or open a folder")
            .accessibility_label("Add a workspace")
            .menu(gpui_kit::Anchor::TopRight, |menu, _, _| {
                menu.item(
                    PopupMenuItem::new("Open folder as a workspace…")
                        .icon(Icon::new(Icons::FolderOpen))
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(AddProject), cx)),
                )
                .item(
                    PopupMenuItem::new("New workspace without folders")
                        .icon(Icon::new(Icons::SquarePen))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(NewWorkspace), cx)
                        }),
                )
            }),
        cx,
    )
}

/// A workspace: fold chevron, colour, name, what it is doing, and on hover
/// a new-chat button and its menu.
fn workspace_row(
    workspace: &Workspace,
    project: &Project,
    ix: usize,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = project.id;
    let expanded = workspace.expanded.contains(&id);
    let active = workspace.view() == View::Workspaces && workspace.active_project_id() == Some(id);
    let dark = cx.theme().is_dark();
    let color = ph_ui::workspace_color(project.shade(ph_ui::WORKSPACE_COLORS.len()), dark);
    let status = workspace.status(id);
    // The label says what is going on; a green dot alone says there is
    // news nobody has looked at.
    let marker: Option<AnyElement> =
        (status.waiting == 0 && !status.failed && status.working == 0 && status.unseen > 0)
            .then(|| dot(cx.theme().success).into_any_element());
    let label = status_label(&status, cx);
    let summary = status.summary(cx).map(|(text, _)| text);
    let tooltip = match (summary, ix < 9) {
        (Some(text), true) => format!("{} · {text} ({})", project.name, switch_hint(ix)),
        (Some(text), false) => format!("{} · {text}", project.name),
        (None, true) => format!("{} ({})", project.name, switch_hint(ix)),
        (None, false) => project.name.clone(),
    };
    let entity = cx.entity().downgrade();
    let (fold, open, new_chat) = (entity.clone(), entity.clone(), entity.clone());
    let group = SharedString::from(format!("workspace-row-{id}"));
    h_flex()
        .id(("sidebar-workspace", id as u64))
        .group(group.clone())
        .relative()
        .mt_0p5()
        .h_7()
        .pl_0p5()
        .pr_0p5()
        .gap_1()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .text_sm()
        // Heavier than the chats under it.
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .when(active, |this| {
            this.bg(cx.theme().sidebar_accent.opacity(0.35))
        })
        .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.6)))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .on_click(move |_, window, cx| {
            _ = open.update(cx, |workspace, cx| workspace.open_project(id, window, cx));
        })
        .child(
            div()
                .id(("sidebar-fold", id as u64))
                .flex_none()
                .p_0p5()
                .rounded(cx.theme().radius)
                .text_color(cx.theme().muted_foreground)
                .hover(|style| style.text_color(cx.theme().foreground))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    _ = fold.update(cx, |workspace, cx| workspace.toggle_expanded(id, cx));
                })
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall(),
                ),
        )
        .child(dot(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(project.name.clone()),
        )
        .children(marker)
        .children(label.map(|(text, color)| {
            div()
                .flex_none()
                .pr_1()
                .text_xs()
                .text_color(color)
                .group_hover(group.clone(), |style| style.invisible())
                .child(text)
        }))
        .child(
            h_flex()
                .absolute()
                .right_0p5()
                .gap_0p5()
                .invisible()
                .group_hover(group, |style| style.visible())
                .child(
                    Button::new(("sidebar-workspace-chat", id as u64))
                        .ghost()
                        .xsmall()
                        .icon(Icons::SquarePen)
                        .tooltip("New chat in this workspace")
                        .accessibility_label("New chat in this workspace")
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            _ = new_chat
                                .update(cx, |workspace, cx| workspace.new_chat_in(id, window, cx));
                        }),
                )
                .child(workspace_menu(entity, project, cx)),
        )
        .test_support()
        .into_any_element()
}

/// What can be done to a workspace. Each item opens it first, then runs
/// the action on the workspace in front.
fn workspace_menu(
    workspace: WeakEntity<Workspace>,
    project: &Project,
    _: &App,
) -> impl IntoElement {
    let id = project.id;
    Button::new(("sidebar-workspace-menu", id as u64))
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .accessibility_label("Workspace menu")
        .menu(gpui_kit::Anchor::TopRight, move |menu, window, cx| {
            let item =
                |label: &'static str, icon: Icons, action: fn() -> Box<dyn gpui_kit::Action>| {
                    let workspace = workspace.clone();
                    PopupMenuItem::new(label).icon(Icon::new(icon)).on_click(
                        move |_, window, cx| {
                            _ = workspace
                                .update(cx, |workspace, cx| workspace.open_project(id, window, cx));
                            window.dispatch_action(action(), cx);
                        },
                    )
                };
            let colours = {
                let workspace = workspace.clone();
                gpui_kit::component::menu::PopupMenu::build(window, cx, move |menu, _, _| {
                    ph_ui::WORKSPACE_COLORS.iter().enumerate().fold(
                        menu,
                        |menu, (ix, (name, _))| {
                            let workspace = workspace.clone();
                            menu.item(PopupMenuItem::new(*name).on_click(move |_, _, cx| {
                                _ = workspace.update(cx, |workspace, cx| {
                                    workspace.set_color(id, Some(ix as u8), cx)
                                });
                            }))
                        },
                    )
                })
            };
            let task = crate::tasks::of(crate::AppState::store(cx), id);
            let menu = menu
                .item(item("Add a folder…", Icons::FolderPlus, || {
                    Box::new(AddFolderToWorkspace)
                }))
                .item(item("New task…", Icons::GitBranch, || {
                    Box::new(crate::actions::NewTask)
                }))
                .when(task.is_some(), |menu| {
                    menu.item(item("Finish task…", Icons::Check, || {
                        Box::new(crate::actions::FinishTask)
                    }))
                });
            menu.item(item("Rename…", Icons::Pencil, || {
                Box::new(RenameWorkspace)
            }))
            .item(item("Open in new window", Icons::ExternalLink, || {
                Box::new(OpenInNewWindow)
            }))
            .item(PopupMenuItem::submenu("Colour", colours))
            .separator()
            .item(item("Remove from PilotHouse", Icons::Trash, || {
                Box::new(RemoveWorkspace)
            }))
        })
}

/// The chats of `project` (the Chats space, or a workspace), the user's own
/// by day and PilotHouse's in *Runs*; with `limit`, only the newest unless
/// all were asked for.
fn chat_rows(
    workspace: &Workspace,
    project: ProjectId,
    limit: Option<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    let sessions = workspace.sessions_of(project, cx);
    let active = (workspace.view() == View::Workspaces
        && workspace.active_project_id() == Some(project))
    .then(|| {
        workspace
            .active_space()
            .and_then(|space| space.read(cx).active_session())
    })
    .flatten();
    let nested = limit.is_some();
    let chats = ph_agent::ChatHub::existing(project, cx);
    let (runs, own): (Vec<Session>, Vec<Session>) = sessions.into_iter().partition(is_automated);
    // Chats marked done list apart, unless at work again.
    let busy = |session: &Session| {
        chats.as_ref().is_some_and(|chats| {
            chats
                .read(cx)
                .summary(session.id, cx)
                .is_some_and(|summary| {
                    matches!(summary.state, ChatState::Working | ChatState::NeedsYou)
                })
        })
    };
    let (done, own): (Vec<Session>, Vec<Session>) = own
        .into_iter()
        .partition(|session| workspace.is_settled(session.id) && !busy(session));
    let show_all = workspace.show_all.contains(&project);
    let shown = match limit {
        Some(limit) if !show_all => own.len().min(limit),
        _ => own.len(),
    };
    let entity = cx.entity().downgrade();
    let today = Zoned::now();
    let mut rows = Vec::new();
    if own.is_empty() && runs.is_empty() && done.is_empty() {
        let start = entity.clone();
        rows.push(if nested {
            h_flex()
                .id(SharedString::from(format!("sidebar-first-chat-{project}")))
                .h_6()
                .pl(px(6.))
                .pr_2()
                .gap_1p5()
                .rounded(cx.theme().radius)
                .cursor_pointer()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .hover(|style| {
                    style
                        .bg(cx.theme().sidebar_accent)
                        .text_color(cx.theme().foreground)
                })
                .child(Icon::new(Icons::Plus).xsmall())
                .child("New chat")
                .on_click(move |_, window, cx| {
                    _ = start.update(cx, |workspace, cx| {
                        workspace.new_chat_in(project, window, cx)
                    });
                })
                .into_any_element()
        } else {
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Chats that need no folder.")
                .into_any_element()
        });
    }
    let mut group = None;
    for session in own.iter().take(shown) {
        // Day groups only in the Chats list; a workspace lists its newest.
        if !nested {
            let day = day_group(session.updated_at, &today);
            if group != Some(day) {
                group = Some(day);
                rows.push(group_label(day, cx));
            }
        }
        rows.push(chat_row(
            &entity,
            project,
            session,
            active,
            ChatRow {
                pinned: workspace.is_pinned(session.id),
                settled: workspace.is_settled(session.id),
                place: None,
            },
            chats.as_ref(),
            cx,
        ));
    }
    if own.len() > shown || (show_all && limit.is_some_and(|limit| own.len() > limit)) {
        let more = own.len() > shown;
        let toggle = entity.clone();
        rows.push(
            link(
                SharedString::from(format!("sidebar-more-{project}")),
                if more {
                    format!("Show all ({})", own.len())
                } else {
                    "Show fewer".into()
                },
                nested,
                cx,
            )
            .on_click(move |_, _, cx| {
                _ = toggle.update(cx, |workspace, cx| {
                    if !workspace.show_all.remove(&project) {
                        workspace.show_all.insert(project);
                    }
                    cx.notify();
                });
            })
            .into_any_element(),
        );
    }
    if !done.is_empty() {
        let open = workspace.done_open.contains(&project);
        let toggle = entity.clone();
        rows.push(
            link(
                SharedString::from(format!("sidebar-done-{project}")),
                format!("Done · {}", done.len()),
                nested,
                cx,
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall(),
            )
            .on_click(move |_, _, cx| {
                _ = toggle.update(cx, |workspace, cx| {
                    if !workspace.done_open.remove(&project) {
                        workspace.done_open.insert(project);
                    }
                    cx.notify();
                });
            })
            .into_any_element(),
        );
        if open {
            for session in &done {
                rows.push(chat_row(
                    &entity,
                    project,
                    session,
                    active,
                    ChatRow {
                        pinned: workspace.is_pinned(session.id),
                        settled: workspace.is_settled(session.id),
                        place: None,
                    },
                    chats.as_ref(),
                    cx,
                ));
            }
        }
    }
    if !runs.is_empty() {
        let open = workspace.runs_open.contains(&project);
        let toggle = entity.clone();
        rows.push(
            link(
                SharedString::from(format!("sidebar-runs-{project}")),
                format!("Runs · {}", runs.len()),
                nested,
                cx,
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall(),
            )
            .on_click(move |_, _, cx| {
                _ = toggle.update(cx, |workspace, cx| {
                    if !workspace.runs_open.remove(&project) {
                        workspace.runs_open.insert(project);
                    }
                    cx.notify();
                });
            })
            .into_any_element(),
        );
        if open {
            for session in &runs {
                rows.push(chat_row(
                    &entity,
                    project,
                    session,
                    active,
                    ChatRow {
                        pinned: workspace.is_pinned(session.id),
                        settled: workspace.is_settled(session.id),
                        place: None,
                    },
                    chats.as_ref(),
                    cx,
                ));
            }
        }
    }
    rows
}

/// How a chat row shows.
struct ChatRow {
    pinned: bool,
    /// The workspace it belongs to, shown in the Pinned list.
    place: Option<String>,
    /// Marked done.
    settled: bool,
}

fn chat_row(
    workspace: &WeakEntity<Workspace>,
    project: ProjectId,
    session: &Session,
    active: Option<SessionId>,
    row: ChatRow,
    chats: Option<&gpui_kit::Entity<ph_agent::WorkspaceChats>>,
    cx: &App,
) -> AnyElement {
    let id = session.id;
    let selected = active == Some(id);
    let summary = chats.and_then(|chats| chats.read(cx).summary(id, cx));
    let state = summary
        .as_ref()
        .map_or(ChatState::Idle, |summary| summary.state);
    let busy = matches!(state, ChatState::Working | ChatState::NeedsYou);
    let agent = summary
        .as_ref()
        .map(|summary| summary.agent)
        .or_else(|| ph_agent::agents::find(&session.agent_id));
    let line = summary
        .and_then(|summary| summary.line)
        .filter(|line| !line.is_empty());
    let line_color = match state {
        ChatState::NeedsYou => cx.theme().warning,
        ChatState::Failed => cx.theme().danger,
        _ => cx.theme().muted_foreground,
    };
    // Shown under the title when there is something to say: where a
    // pinned chat lives, and what it is doing or said last.
    let second = match (row.place, line) {
        (Some(place), Some(line)) => Some(format!("{place} · {line}")),
        (Some(place), None) => Some(place),
        (None, line) => line,
    };
    let open = workspace.clone();
    let group = SharedString::from(format!("chat-row-{id}"));
    let indent = px(6.);
    v_flex()
        .id(SharedString::from(format!("chat-{id}")))
        .group(group.clone())
        .relative()
        .pl(indent)
        .pr_1()
        .py_1()
        .gap_0p5()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .when(selected, |this| {
            this.bg(cx.theme().sidebar_accent)
                .text_color(cx.theme().sidebar_accent_foreground)
        })
        .when(!selected, |this| {
            this.hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.6)))
        })
        .on_click(move |_, window, cx| {
            _ = open.update(cx, |workspace, cx| {
                workspace.open_chat(project, id, window, cx)
            });
        })
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(state_glyph(state, row.pinned, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(
                            matches!(state, ChatState::Done | ChatState::NeedsYou),
                            |this| this.font_weight(gpui_kit::FontWeight::MEDIUM),
                        )
                        .child(session.title.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .group_hover(group.clone(), |style| style.invisible())
                        .child(age(session.updated_at)),
                ),
        )
        .when_some(second, |this, second| {
            this.child(
                h_flex()
                    .pl(px(22.))
                    .gap_1p5()
                    .text_xs()
                    .text_color(line_color)
                    .children(agent.map(|agent| agent.mark(px(12.))))
                    .child(div().flex_1().min_w_0().truncate().child(second)),
            )
        })
        .child(
            div()
                .id(SharedString::from(format!("chat-actions-{id}")))
                .absolute()
                .top(px(3.))
                .right_0p5()
                .invisible()
                .group_hover(group, |style| style.visible())
                // The menu opens without opening the chat.
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(chat_menu(
                    workspace.clone(),
                    project,
                    session.clone(),
                    (row.pinned, row.settled),
                    busy,
                )),
        )
        .test_support()
        .into_any_element()
}

/// A chat's state as a 14 px glyph: a spinner while it works, amber when
/// it needs the user, red when it failed, green when there is news; quiet
/// otherwise.
fn state_glyph(state: ChatState, pinned: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let frame = || {
        div()
            .flex_none()
            .size(px(14.))
            .flex()
            .items_center()
            .justify_center()
    };
    match state {
        ChatState::Working => frame()
            .child(Spinner::new().xsmall().color(theme.info))
            .into_any_element(),
        ChatState::NeedsYou => frame()
            .child(crate::agents_board::question_mark(cx))
            .into_any_element(),
        ChatState::Failed => frame().child(dot(theme.danger)).into_any_element(),
        ChatState::Done => frame().child(dot(theme.success)).into_any_element(),
        ChatState::Idle => frame()
            .child(
                Icon::new(if pinned {
                    Icons::Pin
                } else {
                    Icons::MessageSquare
                })
                .xsmall()
                .text_color(theme.muted_foreground),
            )
            .into_any_element(),
    }
}

/// A workspace's most urgent state in a word or two, for its row.
fn status_label(status: &crate::status::WorkspaceStatus, cx: &App) -> Option<(String, Hsla)> {
    let theme = cx.theme();
    if status.waiting > 0 {
        return Some(("needs you".into(), theme.warning));
    }
    if status.failed {
        return Some(("failing".into(), theme.danger));
    }
    if status.working > 0 {
        return Some((
            format!("{} working", status.working),
            theme.muted_foreground,
        ));
    }
    None
}

/// How long ago `millis` was, in a list's few characters: now, 5m, 3h,
/// Tue, Oct 3.
pub(crate) fn age(millis: i64) -> String {
    let Ok(at) = jiff::Timestamp::from_millisecond(millis) else {
        return String::new();
    };
    let now = jiff::Timestamp::now();
    let minutes = (now.as_second() - at.as_second()) / 60;
    match minutes {
        ..1 => "now".into(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        1440..10080 => at
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%a")
            .to_string(),
        _ => at
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%b %-d")
            .to_string(),
    }
}

/// What can be done to a chat: pin, mark done, rename, delete (not while
/// it works).
fn chat_menu(
    workspace: WeakEntity<Workspace>,
    project: ProjectId,
    session: Session,
    (pinned, settled): (bool, bool),
    busy: bool,
) -> impl IntoElement {
    let id = session.id;
    Button::new(SharedString::from(format!("chat-menu-{id}")))
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .accessibility_label("Chat menu")
        .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            let (pin, settle, rename, delete) = (
                workspace.clone(),
                workspace.clone(),
                workspace.clone(),
                workspace.clone(),
            );
            let (rename_session, delete_session) = (session.clone(), session.clone());
            menu.item(
                PopupMenuItem::new(if pinned { "Unpin" } else { "Pin to top" })
                    .icon(Icon::new(if pinned { Icons::PinOff } else { Icons::Pin }))
                    .on_click(move |_, _, cx| {
                        _ = pin.update(cx, |workspace, cx| workspace.toggle_pin(id, cx));
                    }),
            )
            .item(
                PopupMenuItem::new(if settled { "Not done" } else { "Mark as done" })
                    .icon(Icon::new(if settled { Icons::Undo2 } else { Icons::Check }))
                    .on_click(move |_, _, cx| {
                        _ = settle.update(cx, |workspace, cx| {
                            workspace.toggle_settled(project, id, cx)
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("Rename…")
                    .icon(Icon::new(Icons::Pencil))
                    .on_click(move |_, window, cx| {
                        _ = rename.update(cx, |workspace, cx| {
                            workspace.rename_chat(project, &rename_session, window, cx)
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(if busy {
                    "Delete (still working)"
                } else {
                    "Delete…"
                })
                .icon(Icon::new(Icons::Trash))
                .disabled(busy)
                .on_click(move |_, window, cx| {
                    _ = delete.update(cx, |workspace, cx| {
                        workspace.delete_chat(project, &delete_session, window, cx)
                    });
                }),
            )
        })
}

/// A section's title row, then `action`. The title folds the section
/// (`fold`: its name and whether it is folded); `count` shows how much a
/// folded one hides.
fn section(
    id: &'static str,
    title: &'static str,
    (name, folded): (&'static str, bool),
    count: Option<usize>,
    action: impl IntoElement,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let workspace = cx.entity().downgrade();
    let group = SharedString::from(format!("{id}-header"));
    h_flex()
        .id(id)
        .group(group.clone())
        .flex_none()
        .h_8()
        .mt_2()
        .px_1()
        .gap_1()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .cursor_pointer()
        .hover(|style| style.text_color(cx.theme().foreground))
        .on_click(move |_, _, cx| {
            _ = workspace.update(cx, |workspace, cx| workspace.toggle_section(name, cx));
        })
        .child(
            // Folded sections always show it; open ones on hover.
            div()
                .when(!folded, |this| {
                    this.invisible()
                        .group_hover(group.clone(), |style| style.visible())
                })
                .child(
                    Icon::new(if folded {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .xsmall(),
                ),
        )
        .child(title.to_uppercase())
        .when_some(count, |this, count| {
            this.child(
                div()
                    .px_1p5()
                    .rounded_full()
                    .bg(cx.theme().muted)
                    .child(count.to_string()),
            )
        })
        .child(div().flex_1())
        // Its button acts without folding the section.
        .child(
            div()
                .id(SharedString::from(format!("{id}-action")))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(action),
        )
        // Findable in UI tests and automation.
        .test_support()
        .into_any_element()
}

/// A small muted row (Show all, Runs).
fn link(
    id: SharedString,
    text: String,
    nested: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    h_flex()
        .id(id)
        .h_6()
        .when(nested, |this| this.pl(px(6.)))
        .when(!nested, |this| this.pl_2())
        .pr_2()
        .gap_1()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .hover(|style| style.bg(cx.theme().sidebar_accent))
        .child(text)
}

fn group_label(day: Day, cx: &App) -> AnyElement {
    div()
        .px_2()
        .pt_2()
        .pb_0p5()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(day.label())
        .into_any_element()
}

/// Shown while there is no workspace.
fn no_workspaces(cx: &App) -> AnyElement {
    v_flex()
        .gap_0p5()
        .items_start()
        .child(
            div()
                .px_2()
                .pb_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Open a folder to work on its code: files, Git, runs and tests."),
        )
        .child(
            Button::new("sidebar-open-folder")
                .ghost()
                .small()
                .icon(Icons::FolderOpen)
                .label("Open a folder…")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(AddProject), cx)),
        )
        .child(
            Button::new("sidebar-new-workspace")
                .ghost()
                .small()
                .icon(Icons::SquarePen)
                .label("New workspace")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewWorkspace), cx)),
        )
        .into_any_element()
}

fn dot(color: Hsla) -> impl IntoElement {
    div().flex_none().size(px(8.)).rounded_full().bg(color)
}

/// ⌃1…⌃9 on macOS, Alt+1…9 elsewhere (⌘/Ctrl+digits show tools).
fn switch_hint(ix: usize) -> String {
    if cfg!(target_os = "macos") {
        format!("⌃{}", ix + 1)
    } else {
        format!("Alt+{}", ix + 1)
    }
}

/// A chat PilotHouse started rather than the user: a schedule's, a UI
/// test's recording or repair, a pull request review.
fn is_automated(session: &Session) -> bool {
    session.kind == "review"
        || session.title.ends_with("(scheduled)")
        || session.title.starts_with("Record test:")
        || session.title.starts_with("Repair test:")
}

/// How long ago a chat was last used, as the list groups it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Day {
    Today,
    Yesterday,
    ThisWeek,
    Older,
}

impl Day {
    fn label(self) -> &'static str {
        match self {
            Day::Today => "Today",
            Day::Yesterday => "Yesterday",
            Day::ThisWeek => "This week",
            Day::Older => "Older",
        }
    }
}

/// The group of a chat last used at `millis`, seen from `now`.
fn day_group(millis: i64, now: &Zoned) -> Day {
    let Ok(at) = jiff::Timestamp::from_millisecond(millis) else {
        return Day::Older;
    };
    let date = at.to_zoned(now.time_zone().clone()).date();
    let today = now.date();
    if date >= today {
        Day::Today
    } else if today
        .checked_sub(1.day())
        .is_ok_and(|yesterday| date == yesterday)
    {
        Day::Yesterday
    } else if today
        .checked_sub(7.days())
        .is_ok_and(|week_ago| date > week_ago)
    {
        Day::ThisWeek
    } else {
        Day::Older
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chats_group_by_day() {
        let now: Zoned = "2026-10-24T15:00[UTC]".parse().unwrap();
        let at = |text: &str| text.parse::<jiff::Timestamp>().unwrap().as_millisecond();
        assert_eq!(day_group(at("2026-10-24T08:00Z"), &now), Day::Today);
        assert_eq!(day_group(at("2026-10-23T23:30Z"), &now), Day::Yesterday);
        assert_eq!(day_group(at("2026-10-19T10:00Z"), &now), Day::ThisWeek);
        assert_eq!(day_group(at("2026-10-10T10:00Z"), &now), Day::Older);
    }

    #[test]
    fn chats_pilothouse_started_are_runs() {
        let session = |title: &str, kind: &str| Session {
            id: 1,
            project_id: 1,
            title: title.into(),
            agent_id: "claude".into(),
            acp_session_id: None,
            updated_at: 0,
            cwd: None,
            kind: kind.into(),
        };
        assert!(is_automated(&session(
            "Nightly summary (scheduled)",
            "chat"
        )));
        assert!(is_automated(&session("Repair test: Sign in", "chat")));
        assert!(is_automated(&session("Review #12", "review")));
        assert!(!is_automated(&session("Fix the login bug", "chat")));
    }
}
