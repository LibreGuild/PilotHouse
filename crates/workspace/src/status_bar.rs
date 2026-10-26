//! The bar at the bottom of the window: the git branch (click for the
//! branch picker), the project folder, where its work runs when it is on a
//! network drive, how CI went for the commit checked out, the ports the
//! workspace's processes listen on, the detected stack, and how close each
//! agent subscription is to its limits.

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
    // The GitHub view and the Chats space have no folder to tell of.
    let space_entity = workspace
        .active_space()
        .filter(|space| {
            workspace.view() == crate::workspace::View::Workspaces
                && space.read(cx).project().is_workspace()
        })
        .cloned();
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
    let drive = space.and_then(|space| drive_item(space, cx));
    let ci = space.and_then(|space| ci_item(space.signals().ci.as_ref(), cx));
    let ports = space.and_then(|space| ports_item(&space.signals().ports, cx));
    let merged = space
        .filter(|space| space.signals().merged_task)
        .map(|_| merged_item(cx));
    let branch = space_entity.and_then(|space| branch_item(&space, cx));

    StatusBar::new()
        .left(
            h_flex()
                .gap_3()
                .min_w_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .children(branch)
                .children(merged)
                .children(ci)
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
                        .when_some(path, |this, path| {
                            this.child(Icon::new(Icons::Folder).xsmall())
                                .child(div().truncate().child(path))
                        }),
                )
                .children(drive)
                .children(ports),
        )
        .right(
            h_flex()
                .gap_3()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .children(limits(cx))
                .when(!stacks.is_empty(), |this| this.child(stacks.join(" · ")))
                .child(format!("PilotHouse {}", ph_ui::version())),
        )
}

/// Each agent's limit windows as small meters ("Claude 5h ▰▰▱ 62%"), amber
/// when close and red when reached; reset times in the tooltip.
fn limits(cx: &Context<Workspace>) -> Vec<AnyElement> {
    let theme = cx.theme();
    ph_agent::rate_limits::RateLimits::all(cx)
        .into_iter()
        .enumerate()
        .map(|(ix, (agent, windows))| {
            let tip = windows
                .iter()
                .map(|limit| {
                    let resets = limit
                        .resets_at
                        .and_then(|at| jiff::Timestamp::from_second(at).ok())
                        .map(|at| {
                            at.to_zoned(jiff::tz::TimeZone::system())
                                .strftime(" · resets %a %H:%M")
                                .to_string()
                        })
                        .unwrap_or_default();
                    let used = limit
                        .used
                        .map(|used| format!("{:.0}% used", used * 100.0))
                        .unwrap_or_else(|| "usage not reported".into());
                    format!("{agent} {}: {used}{resets}", limit.window)
                })
                .collect::<Vec<_>>()
                .join("\n");
            h_flex()
                .id(("status-limits", ix))
                .gap_1p5()
                .child(agent)
                .children(windows.into_iter().map(|limit| {
                    let color = match limit.state {
                        ph_agent::rate_limits::LimitState::Reached => theme.danger,
                        ph_agent::rate_limits::LimitState::Warning => theme.warning,
                        ph_agent::rate_limits::LimitState::Fine => theme.muted_foreground,
                    };
                    h_flex().gap_1().child(limit.window.clone()).when_some(
                        limit.used,
                        |this, used| {
                            this.child(
                                div()
                                    .w(gpui_kit::px(28.))
                                    .h(gpui_kit::px(4.))
                                    .rounded_full()
                                    .bg(theme.muted)
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .w(gpui_kit::relative(used))
                                            .bg(color),
                                    ),
                            )
                            .child(
                                div()
                                    .text_color(color)
                                    .child(format!("{:.0}%", used * 100.0)),
                            )
                        },
                    )
                }))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .into_any_element()
        })
        .collect()
}

/// CI of the commit checked out: a check when all passed, a spinner while
/// some run, red when one failed. Opens the failed (or running) check.
fn ci_item(ci: Option<&crate::signals::Ci>, cx: &Context<Workspace>) -> Option<AnyElement> {
    let ci = ci?;
    let theme = cx.theme();
    let (icon, label, color): (AnyElement, String, gpui_kit::Hsla) = if !ci.failed.is_empty() {
        (
            Icon::new(gpui_kit::component::IconName::CircleX)
                .xsmall()
                .text_color(theme.danger)
                .into_any_element(),
            format!("CI {} failed", ci.failed.len()),
            theme.danger,
        )
    } else if ci.pending > 0 {
        (
            Spinner::new().xsmall().color(theme.info).into_any_element(),
            "CI running".into(),
            theme.muted_foreground,
        )
    } else {
        (
            Icon::new(gpui_kit::component::IconName::CircleCheck)
                .xsmall()
                .text_color(theme.success)
                .into_any_element(),
            "CI".into(),
            theme.muted_foreground,
        )
    };
    let mut tip = format!(
        "Checks of the commit checked out: {} passed, {} running, {} failed",
        ci.passed,
        ci.pending,
        ci.failed.len()
    );
    for (name, _) in &ci.failed {
        tip.push_str(&format!("\n✕ {name}"));
    }
    let url = ci.url.clone();
    Some(
        h_flex()
            .id("status-ci")
            .gap_1()
            .text_color(color)
            .child(icon)
            .child(label)
            .when_some(url, |this, url| {
                this.cursor_pointer()
                    .on_click(move |_, _, cx| cx.open_url(&url))
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .into_any_element(),
    )
}

/// A task whose branch is merged: an offer to finish it.
fn merged_item(cx: &Context<Workspace>) -> AnyElement {
    Button::new("status-task-merged")
        .ghost()
        .xsmall()
        .icon(
            Icon::new(gpui_kit::component::IconName::CircleCheck)
                .xsmall()
                .text_color(cx.theme().success),
        )
        .label("Merged · Finish task")
        .tooltip("The task's branch is in its base: remove its worktree and workspace")
        .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::FinishTask), cx))
        .into_any_element()
}

/// The ports the workspace's processes listen on; opens the Run tool.
fn ports_item(ports: &[u16], _: &Context<Workspace>) -> Option<AnyElement> {
    if ports.is_empty() {
        return None;
    }
    let label = ports
        .iter()
        .take(2)
        .map(|port| format!(":{port}"))
        .chain((ports.len() > 2).then(|| format!("+{}", ports.len() - 2)))
        .collect::<Vec<_>>()
        .join(" ");
    Some(
        Button::new("status-ports")
            .ghost()
            .xsmall()
            .icon(Icon::new(Icons::Globe).xsmall())
            .label(label)
            .tooltip("Listening here: open, copy or stop in the Run tool")
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(crate::actions::ShowRuns), cx)
            })
            .into_any_element(),
    )
}

/// Where the workspace's work runs when its folder is on a network drive:
/// the server (mapped), or an offer to set that up. Opens Network drives.
fn drive_item(
    space: &crate::workspace_screen::WorkspaceScreen,
    cx: &Context<Workspace>,
) -> Option<AnyElement> {
    let (label, tip, color) = if let Some(server) = space.server() {
        (
            format!("SSH · {}", server.mapping.host),
            format!(
                "Git, terminals and runs work on {} in {}. Files are read through the drive; agents run on this computer.",
                server.mapping.destination(),
                server.path
            ),
            cx.theme().info,
        )
    } else {
        let drive = space.network_drive()?;
        (
            "Network drive".to_string(),
            format!(
                "This folder is {}'s share “{}”. Set it up to run Git, terminals and runs on the server over SSH.",
                drive.server, drive.share
            ),
            cx.theme().muted_foreground,
        )
    };
    Some(
        Button::new("status-network-drive")
            .ghost()
            .xsmall()
            .icon(Icon::new(Icons::HardDrive).xsmall())
            .label(label)
            .text_color(color)
            .tooltip(tip)
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(crate::actions::ShowNetworkDrives), cx)
            })
            .into_any_element(),
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
