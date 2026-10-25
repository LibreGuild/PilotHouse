//! The left sidebar of the Workspaces view: everything at once, so nothing
//! needs switching to be seen.
//!
//! - **New chat** (⌘N) and chat search on top.
//! - **Chats**: the Chats space, for chats outside any workspace (no folder
//!   needed), grouped by day.
//! - **Workspaces**: each with its colour and what it is doing; unfolded,
//!   its newest chats (all of them on request). Picking a chat opens its
//!   workspace at it. Each row's menu adds folders, renames, moves it to a
//!   window of its own, recolours or removes it.
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

pub(crate) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let mut rows: Vec<AnyElement> = Vec::new();
    if let Some(chats) = workspace.chats_space() {
        rows.push(chats_header(chats.id, cx));
        rows.extend(chat_rows(workspace, chats.id, None, cx));
    }
    rows.push(workspaces_header(cx));
    if workspace.projects().is_empty() {
        rows.push(no_workspaces(cx));
    }
    for (ix, project) in workspace.projects().iter().enumerate() {
        rows.push(workspace_row(workspace, project, ix, cx));
        if workspace.expanded.contains(&project.id) {
            rows.extend(chat_rows(workspace, project.id, Some(NEWEST), cx));
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

/// New chat and search.
fn top(cx: &App) -> impl IntoElement {
    h_flex()
        .flex_none()
        .pr_1()
        .pb_1()
        .gap_1()
        .child(
            Button::new("sidebar-new-chat")
                .outline()
                .small()
                .flex_1()
                .icon(Icons::SquarePen)
                .label("New chat")
                .tooltip("New chat here (⌘N)")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewSession), cx)),
        )
        .child(
            Button::new("sidebar-search-chats")
                .ghost()
                .small()
                .icon(IconName::Search)
                .tooltip("Search chats (⌘⇧K)")
                .accessibility_label("Search chats")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(SearchChats), cx)),
        )
        .child(div().h_px().bg(cx.theme().border))
}

fn chats_header(chats: ProjectId, cx: &mut Context<Workspace>) -> AnyElement {
    let workspace = cx.entity().downgrade();
    section(
        "sidebar-chats",
        "Chats",
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

fn workspaces_header(cx: &mut Context<Workspace>) -> AnyElement {
    section(
        "sidebar-workspaces",
        "Workspaces",
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
            nested,
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
                    nested,
                    chats.as_ref(),
                    cx,
                ));
            }
        }
    }
    rows
}

fn chat_row(
    workspace: &WeakEntity<Workspace>,
    project: ProjectId,
    session: &Session,
    active: Option<SessionId>,
    nested: bool,
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
    let workspace = workspace.clone();
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
        Icon::new(Icons::MessageSquare)
            .xsmall()
            .text_color(cx.theme().muted_foreground)
            .into_any_element()
    };
    h_flex()
        .id(SharedString::from(format!("chat-{id}")))
        .h_7()
        .when(nested, |this| this.pl_6())
        .when(!nested, |this| this.pl_2())
        .pr_2()
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
            _ = workspace.update(cx, |workspace, cx| {
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
        .test_support()
        .into_any_element()
}

/// A section's title row, then `action`.
fn section(
    id: &'static str,
    title: &'static str,
    action: impl IntoElement,
    cx: &App,
) -> AnyElement {
    h_flex()
        .id(id)
        .flex_none()
        .h_8()
        .mt_1()
        .px_1()
        .gap_0p5()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(div().flex_1().child(title.to_uppercase()))
        .child(action)
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
