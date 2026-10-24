//! The ⌘K command palette: every workspace command, searchable in one place.
//!
//! Rows run workspace actions, so a palette entry, its keyboard shortcut and
//! any button for it always do the same thing.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex,
    kbd::Kbd,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, App, AppContext as _, Entity, Focusable as _, ParentElement as _, SharedString,
    Styled as _, Window, div,
};
use ph_store::projects::ProjectId;
use ph_ui::Icons;

use crate::actions::*;
use crate::context_panel::ContextTab;
use crate::workspace::Workspace;

/// What a palette row does when confirmed.
enum Run {
    Action(Box<dyn Action>),
    OpenProject(ProjectId),
    /// Start an agent CLI without a chat in a terminal.
    StartCli(&'static ph_terminal::cli_agents::CliAgent),
    /// Open the Utilities dialog at one utility.
    Utility(&'static str),
}

struct Entry {
    label: SharedString,
    icon: Icon,
    keywords: &'static [&'static str],
    run: Run,
}

impl Entry {
    fn action(
        label: &str,
        icon: impl Into<Icon>,
        action: impl Action,
        keywords: &'static [&'static str],
    ) -> Self {
        Self {
            label: label.to_string().into(),
            icon: icon.into(),
            keywords,
            run: Run::Action(Box::new(action)),
        }
    }
}

/// Builds the palette's groups from the current workspace state.
fn groups(workspace: &Workspace) -> Vec<(&'static str, Vec<Entry>)> {
    let mut groups = Vec::new();

    let mut general = vec![
        Entry::action(
            "New chat",
            Icons::SquarePen,
            NewSession,
            &["session", "conversation"],
        ),
        Entry::action(
            "Open folder as workspace…",
            Icons::FolderPlus,
            AddProject,
            &["open", "folder", "project", "workspace"],
        ),
        Entry::action(
            "New workspace",
            Icons::SquarePen,
            NewWorkspace,
            &["empty", "chat", "workspace", "project"],
        ),
        Entry::action(
            "Add folder to workspace…",
            Icons::FolderPlus,
            AddFolderToWorkspace,
            &["multi", "root", "folder", "repository"],
        ),
        Entry::action(
            "Open workspace in new window",
            Icons::ExternalLink,
            OpenInNewWindow,
            &["window", "move", "monitor", "detach"],
        ),
        Entry::action(
            "Rename workspace…",
            IconName::Replace,
            RenameWorkspace,
            &["name", "workspace", "project"],
        ),
        Entry::action(
            "Settings",
            IconName::Settings,
            OpenSettings,
            &["preferences"],
        ),
    ];
    general.push(Entry::action(
        "Search chats…",
        IconName::Search,
        SearchChats,
        &["find", "history", "messages", "conversation"],
    ));
    general.push(Entry::action(
        "Architecture map…",
        IconName::Network,
        ShowArchitecture,
        &[
            "architecture",
            "dependencies",
            "modules",
            "crates",
            "packages",
            "overview",
        ],
    ));
    general.push(Entry::action(
        "Parallel runs…",
        IconName::GalleryVerticalEnd,
        ShowParallelRuns,
        &["compare", "agents", "worktree", "side by side"],
    ));
    general.push(Entry::action(
        "Agent instructions…",
        IconName::BookOpen,
        ShowInstructions,
        &["agents.md", "claude.md", "gemini.md", "sync", "rules"],
    ));
    general.push(Entry::action(
        "Schedules…",
        Icons::CalendarClock,
        ShowSchedules,
        &["schedule", "cron", "timer", "summary", "nightly", "every"],
    ));
    general.push(Entry::action(
        "Network drives…",
        Icons::HardDrive,
        ShowNetworkDrives,
        &[
            "ssh", "remote", "server", "samba", "smb", "share", "mapped", "drive",
        ],
    ));
    general.push(Entry::action(
        "Keyboard shortcuts",
        Icons::Keyboard,
        ShowShortcuts,
        &["keys", "keymap", "hotkeys", "help", "cheat sheet"],
    ));
    general.push(Entry::action(
        "Usage…",
        IconName::ChartPie,
        ShowUsage,
        &["tokens", "cost", "spend", "context"],
    ));
    general.push(Entry::action(
        "Toggle sidebar",
        IconName::PanelLeft,
        ToggleSidebar,
        &["hide", "show"],
    ));
    general.push(Entry::action(
        "Toggle panel",
        IconName::PanelRight,
        ToggleContextPanel,
        &["hide", "show", "context"],
    ));
    general.push(Entry::action(
        "Maximize panel",
        IconName::Maximize,
        ToggleZoom,
        &["zoom", "full"],
    ));
    groups.push(("General", general));

    if workspace.active_space().is_some() {
        let views = ContextTab::ALL
            .into_iter()
            .map(|tab| Entry {
                label: format!("Show {}", tab.label()).into(),
                icon: tab.icon().into(),
                keywords: &["panel", "tab"],
                run: Run::Action(tab.action()),
            })
            .collect();
        groups.push(("Panels", views));
        groups.push((
            "Files",
            vec![
                Entry::action(
                    "Go to file…",
                    IconName::File,
                    ph_files::QuickOpen,
                    &["open", "find", "fuzzy"],
                ),
                Entry::action(
                    "Search in workspace…",
                    IconName::Search,
                    ph_files::FindInProject,
                    &["grep", "text"],
                ),
            ],
        ));
    }

    let projects: Vec<Entry> = workspace
        .projects()
        .iter()
        .filter(|project| Some(project.id) != workspace.active_project_id())
        .map(|project| Entry {
            label: format!("Open {}", project.name).into(),
            icon: IconName::Folder.into(),
            keywords: &["workspace", "project", "switch"],
            run: Run::OpenProject(project.id),
        })
        .collect();
    if !projects.is_empty() {
        groups.push(("Workspaces", projects));
    }

    groups.push((
        "Utilities",
        std::iter::once(Entry::action(
            "Utilities…",
            Icons::Wrench,
            ShowUtilities,
            &["tools", "convert", "format", "encode", "decode"],
        ))
        .chain(ph_utils::UTILITIES.iter().map(|utility| Entry {
            label: format!("{}: {}", utility.group, utility.name).into(),
            icon: Icons::Wrench.into(),
            keywords: utility.keywords,
            run: Run::Utility(utility.id),
        }))
        .collect(),
    ));

    if workspace.active_space().is_some() {
        let clis: Vec<Entry> = ph_terminal::cli_agents::installed()
            .into_iter()
            .map(|agent| Entry {
                label: format!("Start {} in a terminal", agent.name).into(),
                icon: IconName::Bot.into(),
                keywords: &["agent", "cli", "terminal"],
                run: Run::StartCli(agent),
            })
            .collect();
        if !clis.is_empty() {
            groups.push(("Agent CLIs", clis));
        }
    }

    groups.push((
        "Appearance",
        vec![
            Entry::action(
                "Use system appearance",
                IconName::Palette,
                UseSystemAppearance,
                &["theme", "auto"],
            ),
            Entry::action(
                "Use light appearance",
                IconName::Sun,
                UseLightAppearance,
                &["theme"],
            ),
            Entry::action(
                "Use dark appearance",
                IconName::Moon,
                UseDarkAppearance,
                &["theme"],
            ),
        ],
    ));
    groups
}

/// A palette row: icon, label and the command's shortcut on the trailing edge.
fn row(entry: &Entry) -> CommandItem {
    let label = entry.label.clone();
    let icon = entry.icon.clone();
    let action = match &entry.run {
        Run::Action(action) => Some(action.boxed_clone()),
        Run::OpenProject(_) | Run::StartCli(_) | Run::Utility(_) => None,
    };
    CommandItem::new()
        .label(entry.label.clone())
        .keywords(entry.keywords.iter().copied())
        .child(move |window, cx| {
            let shortcut = action.as_ref().and_then(|action| {
                Kbd::binding_for_action(action.as_ref(), Some(WORKSPACE_CONTEXT), window)
            });
            h_flex()
                .w_full()
                .gap_2()
                .child(icon.clone().text_color(cx.theme().muted_foreground))
                .child(div().flex_1().truncate().child(label.clone()))
                .when_some(shortcut, |this, shortcut| this.child(shortcut))
        })
}

/// Opens the palette. Takes the workspace by reference because it is called
/// from a workspace action handler, while the entity is already borrowed.
pub(crate) fn open(
    workspace: &Workspace,
    handle: Entity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    let groups = Rc::new(groups(workspace));
    let workspace = handle;

    // Focus the search field once, after the dialog has mounted.
    let focus_state = state.clone();
    window.defer(cx, move |window, cx| {
        focus_state.read(cx).focus_handle(cx).focus(window, cx);
    });

    window.open_dialog(cx, move |dialog, _, cx| {
        let confirm_groups = groups.clone();
        let workspace = workspace.clone();
        let mut command = Command::new(&state)
            .bordered(false)
            .rounded(cx.theme().radius_lg)
            .placeholder("Type a command or search…")
            .on_confirm(move |index_path, window, cx| {
                let Some(entry) = confirm_groups
                    .get(index_path.section)
                    .and_then(|(_, entries)| entries.get(index_path.row))
                else {
                    return;
                };
                // Closing returns focus to the workspace, so the action below
                // dispatches along the workspace's key context.
                window.close_dialog(cx);
                match &entry.run {
                    Run::Action(action) => window.dispatch_action(action.boxed_clone(), cx),
                    Run::OpenProject(id) => {
                        let id = *id;
                        workspace
                            .update(cx, |workspace, cx| workspace.open_project(id, window, cx));
                    }
                    Run::Utility(id) => crate::utilities::open(Some(id), window, cx),
                    Run::StartCli(agent) => {
                        let space = workspace.read(cx).active_space().cloned();
                        if let Some(space) = space {
                            space.update(cx, |space, cx| space.start_cli(agent, window, cx));
                        }
                    }
                }
            });
        for (title, entries) in groups.iter() {
            command = command.group(
                CommandGroup::new()
                    .label(*title)
                    .items(entries.iter().map(row)),
            );
        }
        dialog.close_button(false).p_0().child(command)
    });
}
