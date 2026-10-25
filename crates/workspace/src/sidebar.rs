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
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, div, px,
};
use jiff::{ToSpan as _, Zoned};
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
                rows.extend(chat_rows(workspace, project.id, Some(NEWEST), cx));
            }
        }
    }
    v_flex()
        .id("workspace-sidebar")
        .flex_none()
        .w(layout::SIDEBAR_WIDTH)
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

/// New chat, prominent in the accent, and a search field. Both are rows
/// of the list's width, with their shortcut on the right.
fn top(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let shortcut =
        |keys: &'static str, color: Hsla| div().flex_none().text_xs().text_color(color).child(keys);
    v_flex()
        .flex_none()
        .pr_1()
        .pt_1()
        .pb_2()
        .gap_1p5()
        .child(
            h_flex()
                .id("sidebar-new-chat")
                .h_8()
                .px_2p5()
                .gap_2()
                .rounded(theme.radius_lg)
                .cursor_pointer()
                // A calm surface with the accent only in its icon: it leads
                // the sidebar without pulling the eye from the chat.
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar_accent.opacity(0.55))
                .text_color(theme.foreground)
                .text_sm()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .hover(|style| style.bg(theme.sidebar_accent))
                .active(|style| style.bg(theme.sidebar_accent.opacity(0.8)))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("New chat in the workspace in front")
                        .build(window, cx)
                })
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewSession), cx))
                .child(Icon::new(Icons::SquarePen).small().text_color(theme.link))
                .child(div().flex_1().child("New chat"))
                .child(shortcut(
                    if cfg!(target_os = "macos") {
                        "⌘N"
                    } else {
                        "Ctrl+N"
                    },
                    theme.muted_foreground.opacity(0.8),
                ))
                .aria_label("New chat")
                .test_support(),
        )
        .child(
            h_flex()
                .id("sidebar-search-chats")
                .h_8()
                .px_2p5()
                .gap_2()
                .rounded(theme.radius_lg)
                .cursor_pointer()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background.opacity(0.5))
                .text_sm()
                .text_color(theme.muted_foreground)
                .hover(|style| style.border_color(theme.ring.opacity(0.6)))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(SearchChats), cx))
                .child(Icon::new(IconName::Search).small())
                .child(div().flex_1().child("Search chats"))
                .child(shortcut(
                    if cfg!(target_os = "macos") {
                        "⌘⇧K"
                    } else {
                        "Ctrl+Shift+K"
                    },
                    theme.muted_foreground.opacity(0.8),
                ))
                .aria_label("Search chats")
                .test_support(),
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
                nested: false,
                pinned: true,
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
    let marker: Option<AnyElement> = if status.waiting > 0 {
        Some(dot(cx.theme().warning).into_any_element())
    } else if status.failed {
        Some(dot(cx.theme().danger).into_any_element())
    } else if status.working > 0 {
        Some(Spinner::new().xsmall().into_any_element())
    } else if status.unseen > 0 {
        Some(dot(cx.theme().success).into_any_element())
    } else {
        None
    };
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
        .mt_0p5()
        .h_7()
        .pl_0p5()
        .pr_0p5()
        .gap_1()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .text_sm()
        .when(active, |this| {
            this.font_weight(gpui_kit::FontWeight::MEDIUM)
                .bg(cx.theme().sidebar_accent.opacity(0.35))
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
        .child(
            h_flex()
                .flex_none()
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
            menu.item(item("Add a folder…", Icons::FolderPlus, || {
                Box::new(AddFolderToWorkspace)
            }))
            .item(item("Rename…", Icons::Pencil, || {
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
    let show_all = workspace.show_all.contains(&project);
    let shown = match limit {
        Some(limit) if !show_all => own.len().min(limit),
        _ => own.len(),
    };
    let entity = cx.entity().downgrade();
    let today = Zoned::now();
    let mut rows = Vec::new();
    if own.is_empty() && runs.is_empty() {
        rows.push(
            div()
                .when(nested, |this| this.pl_6())
                .when(!nested, |this| this.px_2())
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if nested {
                    "No chats yet."
                } else {
                    "Chats here need no folder."
                })
                .into_any_element(),
        );
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
                nested,
                pinned: workspace.is_pinned(session.id),
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
                        nested,
                        pinned: workspace.is_pinned(session.id),
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
    /// Under its workspace (indented).
    nested: bool,
    pinned: bool,
    /// The workspace it belongs to, shown in the Pinned list.
    place: Option<String>,
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
    let (working, waiting) = chats
        .map(|chats| {
            let chats = chats.read(cx);
            (chats.is_working(id, cx), chats.is_waiting(id, cx))
        })
        .unwrap_or_default();
    let open = workspace.clone();
    let marker: AnyElement = if waiting {
        div()
            .flex_none()
            .size(px(8.))
            .mx(px(3.))
            .rounded_full()
            .bg(cx.theme().warning)
            .into_any_element()
    } else if working {
        Spinner::new().xsmall().into_any_element()
    } else {
        Icon::new(if row.pinned {
            Icons::Pin
        } else {
            Icons::MessageSquare
        })
        .xsmall()
        .text_color(cx.theme().muted_foreground)
        .into_any_element()
    };
    let group = SharedString::from(format!("chat-row-{id}"));
    h_flex()
        .id(SharedString::from(format!("chat-{id}")))
        .group(group.clone())
        .h_7()
        .when(row.nested, |this| this.pl_6())
        .when(!row.nested, |this| this.pl_2())
        .pr_0p5()
        .gap_2()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .text_sm()
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
        .child(marker)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(session.title.clone()),
        )
        .when_some(row.place, |this, place| {
            this.child(
                div()
                    .flex_none()
                    .max_w(px(80.))
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .group_hover(group.clone(), |style| style.invisible())
                    .child(place),
            )
        })
        .child(
            div()
                .id(SharedString::from(format!("chat-actions-{id}")))
                .flex_none()
                .invisible()
                .group_hover(group, |style| style.visible())
                // The menu opens without opening the chat.
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(chat_menu(
                    workspace.clone(),
                    project,
                    session.clone(),
                    row.pinned,
                    working || waiting,
                )),
        )
        .test_support()
        .into_any_element()
}

/// What can be done to a chat: pin, rename, delete (not while it works).
fn chat_menu(
    workspace: WeakEntity<Workspace>,
    project: ProjectId,
    session: Session,
    pinned: bool,
    busy: bool,
) -> impl IntoElement {
    let id = session.id;
    Button::new(SharedString::from(format!("chat-menu-{id}")))
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .accessibility_label("Chat menu")
        .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            let (pin, rename, delete) = (workspace.clone(), workspace.clone(), workspace.clone());
            let (rename_session, delete_session) = (session.clone(), session.clone());
            menu.item(
                PopupMenuItem::new(if pinned { "Unpin" } else { "Pin to top" })
                    .icon(Icon::new(if pinned { Icons::PinOff } else { Icons::Pin }))
                    .on_click(move |_, _, cx| {
                        _ = pin.update(cx, |workspace, cx| workspace.toggle_pin(id, cx));
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
        .mt_1()
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
        .when(nested, |this| this.pl_6())
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
