//! The left sidebar for the workspace in front: its files on top (each
//! folder a root of the tree), its chats below, grouped by day. Either
//! section folds; with both open, the line between them drags. Chats that
//! PilotHouse started itself (schedules, test recording and repair, pull
//! request reviews) fold into *Runs*, so the user's own stay in view.
//!
//! Workspaces are switched from the title bar, not here.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    resizable::{resizable_panel, v_resizable},
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use jiff::{ToSpan as _, Zoned};
use ph_store::sessions::Session;
use ph_theme::layout;
use ph_ui::Icons;

use crate::actions::{AddFolderToWorkspace, AddProject, NewSession, NewWorkspace, SearchChats};
use crate::project_space::ProjectSpace;
use crate::workspace::Workspace;

pub(crate) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let body = match workspace.active_space().cloned() {
        Some(space) => sections(&space, cx),
        None => no_workspace(cx),
    };
    v_flex()
        .id("workspace-sidebar")
        .flex_none()
        .w(layout::SIDEBAR_WIDTH)
        .h_full()
        .pl_2()
        .pb_2()
        .child(body)
}

/// Files over chats, as folded and sized.
fn sections(space: &Entity<ProjectSpace>, cx: &mut Context<Workspace>) -> AnyElement {
    let (tree, files_folded, chats_folded, project_id) = {
        let space = space.read(cx);
        (
            space.file_tree().cloned(),
            space.files_folded(),
            space.chats_folded(),
            space.project().id,
        )
    };
    let chats = chats_section(space, chats_folded, cx);
    let Some(tree) = tree else {
        // No folders yet: say how to add one, and the chats take the rest.
        return v_flex()
            .size_full()
            .child(no_folders(cx))
            .child(div().flex_1().min_h_0().child(chats))
            .into_any_element();
    };
    let files = files_section(space, &tree, files_folded, cx);
    match (files_folded, chats_folded) {
        (false, false) => v_resizable(("sidebar-split", project_id as u64))
            .child(
                resizable_panel()
                    .size(px(380.))
                    .size_range(px(120.)..gpui_kit::Pixels::MAX)
                    .child(files),
            )
            .child(
                resizable_panel()
                    .size_range(px(120.)..gpui_kit::Pixels::MAX)
                    .child(chats),
            )
            .into_any_element(),
        // A folded section keeps its header; the open one takes the rest.
        (true, false) => v_flex()
            .size_full()
            .child(div().flex_none().child(files))
            .child(div().flex_1().min_h_0().child(chats))
            .into_any_element(),
        (false, true) => v_flex()
            .size_full()
            .child(div().flex_1().min_h_0().child(files))
            .child(div().flex_none().child(chats))
            .into_any_element(),
        (true, true) => v_flex()
            .size_full()
            .child(files)
            .child(chats)
            .into_any_element(),
    }
}

fn files_section(
    space: &Entity<ProjectSpace>,
    tree: &Entity<ph_files::FileTree>,
    folded: bool,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let files = tree.read(cx).files().clone();
    let searching = files.read(cx).is_searching();
    let (fold, search) = (space.clone(), files.clone());
    let header = section_header(
        "sidebar-files",
        if searching { "Search" } else { "Files" },
        folded,
        move |cx| {
            fold.update(cx, |space, cx| {
                let (chats, runs) = (space.chats_folded(), space.runs_open());
                space.set_sidebar_folds(!folded, chats, runs, cx)
            })
        },
        cx,
    )
    .child(
        Button::new("files-search")
            .ghost()
            .xsmall()
            .icon(if searching {
                Icons::Files
            } else {
                IconName::Search.into()
            })
            .tooltip(if searching {
                "Back to the files"
            } else {
                "Search in workspace (⌘⇧F)"
            })
            .accessibility_label("Search in workspace")
            .on_click(move |_, window, cx| {
                search.update(cx, |files, cx| {
                    if files.is_searching() {
                        files.show_tree(cx)
                    } else {
                        files.find_in_project(window, cx)
                    }
                })
            }),
    )
    .child(
        Button::new("files-add-folder")
            .ghost()
            .xsmall()
            .icon(Icons::FolderPlus)
            .tooltip("Add a folder to this workspace")
            .accessibility_label("Add folder")
            .on_click(|_, window, cx| window.dispatch_action(Box::new(AddFolderToWorkspace), cx)),
    );
    v_flex()
        .size_full()
        .child(header)
        .when(!folded, |this| {
            this.child(div().flex_1().min_h_0().child(tree.clone()))
        })
        .into_any_element()
}

fn chats_section(
    space: &Entity<ProjectSpace>,
    folded: bool,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let fold = space.clone();
    let header = section_header(
        "sidebar-chats",
        "Chats",
        folded,
        move |cx| {
            fold.update(cx, |space, cx| {
                let (files, runs) = (space.files_folded(), space.runs_open());
                space.set_sidebar_folds(files, !folded, runs, cx)
            })
        },
        cx,
    )
    .child(
        Button::new("sidebar-search-chats")
            .ghost()
            .xsmall()
            .icon(IconName::Search)
            .tooltip("Search chats (⌘⇧K)")
            .accessibility_label("Search chats")
            .on_click(|_, window, cx| window.dispatch_action(Box::new(SearchChats), cx)),
    )
    .child(
        Button::new("sidebar-new-chat")
            .ghost()
            .xsmall()
            .icon(Icons::SquarePen)
            .tooltip("New chat (⌘N)")
            .accessibility_label("New chat")
            .on_click(|_, window, cx| window.dispatch_action(Box::new(NewSession), cx)),
    );
    if folded {
        return header.into_any_element();
    }
    v_flex()
        .size_full()
        .child(header)
        .child(
            v_flex()
                .id("sidebar-chat-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .pr_1()
                .pb_2()
                .children(chat_list(space, cx)),
        )
        .into_any_element()
}

/// The chats in groups by day, the user's own first and PilotHouse's in
/// *Runs* at the end.
fn chat_list(space: &Entity<ProjectSpace>, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let (sessions, active, runs_open, project_id) = {
        let space = space.read(cx);
        (
            space.sessions().to_vec(),
            space.active_session(),
            space.runs_open(),
            space.project().id,
        )
    };
    let chats = ph_agent::ChatHub::existing(project_id, cx);
    let (runs, own): (Vec<Session>, Vec<Session>) = sessions.into_iter().partition(is_automated);
    let today = Zoned::now();
    let mut rows = Vec::new();
    let mut group = None;
    if own.is_empty() {
        rows.push(
            div()
                .px_2()
                .py_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No chats yet.")
                .into_any_element(),
        );
    }
    for session in &own {
        let day = day_group(session.updated_at, &today);
        if group != Some(day) {
            group = Some(day);
            rows.push(group_label(day, cx));
        }
        rows.push(chat_row(space, session, active, chats.as_ref(), cx));
    }
    if !runs.is_empty() {
        let toggle = space.clone();
        rows.push(
            h_flex()
                .id("sidebar-runs")
                .mt_2()
                .px_2()
                .h_6()
                .gap_1()
                .rounded(cx.theme().radius)
                .cursor_pointer()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .hover(|style| style.bg(cx.theme().sidebar_accent))
                .on_click(move |_, _, cx| {
                    toggle.update(cx, |space, cx| {
                        let (files, chats) = (space.files_folded(), space.chats_folded());
                        space.set_sidebar_folds(files, chats, !runs_open, cx)
                    })
                })
                .child(
                    Icon::new(if runs_open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall(),
                )
                .child(format!("Runs · {}", runs.len()))
                .into_any_element(),
        );
        if runs_open {
            for session in &runs {
                rows.push(chat_row(space, session, active, chats.as_ref(), cx));
            }
        }
    }
    rows
}

fn chat_row(
    space: &Entity<ProjectSpace>,
    session: &Session,
    active: Option<i64>,
    chats: Option<&Entity<ph_agent::WorkspaceChats>>,
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
    let space = space.clone();
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
        .px_2()
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
        .on_click(move |_, _, cx| space.update(cx, |space, cx| space.select_session(id, cx)))
        .child(marker)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(session.title.clone()),
        )
        .into_any_element()
}

/// A section's title row: a chevron and name that fold it, then `actions`.
fn section_header(
    id: &'static str,
    title: &'static str,
    folded: bool,
    toggle: impl Fn(&mut App) + 'static,
    cx: &App,
) -> gpui_kit::Div {
    h_flex().flex_none().h_8().pr_1().gap_0p5().child(
        h_flex()
            .id(id)
            .flex_1()
            .h_full()
            .px_1()
            .gap_1()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .text_xs()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(cx.theme().muted_foreground)
            .hover(|style| style.text_color(cx.theme().foreground))
            .on_click(move |_, _, cx| toggle(cx))
            // Findable in UI tests and automation.
            .test_support()
            .child(
                Icon::new(if folded {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .xsmall(),
            )
            .child(title.to_uppercase()),
    )
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

/// The Files section of a workspace without folders.
fn no_folders(cx: &App) -> AnyElement {
    v_flex()
        .flex_none()
        .pr_1()
        .pb_2()
        .child(
            h_flex()
                .h_8()
                .px_1()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .child("FILES"),
        )
        .child(
            div()
                .px_2()
                .pb_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No folders in this workspace."),
        )
        .child(
            Button::new("sidebar-add-first-folder")
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .icon(Icons::FolderPlus)
                .label("Add a folder…")
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(AddFolderToWorkspace), cx)
                }),
        )
        .into_any_element()
}

/// Shown when no workspace is open.
fn no_workspace(cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .pt_2()
        .pr_2()
        .child(
            div()
                .px_2()
                .pb_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No workspace open."),
        )
        .child(
            Button::new("sidebar-open-folder")
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .icon(Icons::FolderOpen)
                .label("Open a folder…")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(AddProject), cx)),
        )
        .child(
            Button::new("sidebar-new-workspace")
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .icon(Icons::SquarePen)
                .label("New workspace")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewWorkspace), cx)),
        )
        .into_any_element()
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
