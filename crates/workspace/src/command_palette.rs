//! The ⌘K command palette: every workspace command, searchable in one place,
//! under the chats to jump to (those waiting for an answer or working first).
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
    OpenChat(ProjectId, ph_store::sessions::SessionId),
    /// Start an agent CLI without a chat in a terminal.
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

/// Most chats offered when nothing is typed (typing searches them all).
const CHATS: usize = 40;

/// Chats to jump to, those that want the user first: waiting for an
/// answer, working, with news; then every chat, newest first.
fn chats(workspace: &Workspace, cx: &App) -> Vec<Entry> {
    use ph_agent::ChatState;
    let rank = |state: ChatState| match state {
        ChatState::NeedsYou => 0,
        ChatState::Working => 1,
        ChatState::Done | ChatState::Failed => 2,
        ChatState::Idle => 3,
    };
    let mut live = crate::agents_board::cards(workspace, cx);
    live.sort_by_key(|card| rank(card.summary.state));
    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for (card, session) in live.iter().filter_map(|card| match card.target {
        crate::agents_board::Target::Chat(session) if card.summary.state != ChatState::Idle => {
            Some((card, session))
        }
        _ => None,
    }) {
        seen.insert(session);
        let state = match card.summary.state {
            ChatState::NeedsYou => "needs you",
            ChatState::Working => "working",
            ChatState::Failed => "failed",
            _ => "finished",
        };
        entries.push(Entry {
            label: format!("{} · {} · {state}", card.title, card.place).into(),
            icon: Icons::MessageSquare.into(),
            keywords: &["chat", "agent"],
            run: Run::OpenChat(card.project, session),
        });
    }
    let mut stored: Vec<(String, ph_store::sessions::Session)> = workspace
        .projects()
        .iter()
        .map(|project| (project.id, project.name.clone()))
        .chain(
            workspace
                .chats_space()
                .map(|chats| (chats.id, "Chats".to_string())),
        )
        .flat_map(|(id, place)| {
            workspace
                .sessions_of(id, cx)
                .into_iter()
                .map(move |session| (place.clone(), session))
        })
        .filter(|(_, session)| !seen.contains(&session.id))
        .collect();
    stored.sort_by_key(|(_, session)| std::cmp::Reverse(session.updated_at));
    entries.extend(
        stored
            .into_iter()
            .take(CHATS)
            .map(|(place, session)| Entry {
                label: format!("{} · {place}", session.title).into(),
                icon: Icons::MessageSquare.into(),
                keywords: &["chat"],
                run: Run::OpenChat(session.project_id, session.id),
            }),
    );
    entries
}

/// Builds the palette's groups from the current workspace state.
fn groups(workspace: &Workspace, cx: &App) -> Vec<(&'static str, Vec<Entry>)> {
    let mut groups = Vec::new();
    let chats = chats(workspace, cx);
    if !chats.is_empty() {
        groups.push(("Chats", chats));
    }

    let mut general = vec![
        Entry::action(
            "New chat",
            Icons::SquarePen,
            NewSession,
            &["session", "conversation"],
        ),
        Entry::action(
            "New task in a worktree…",
            Icons::GitBranch,
            NewTask,
            &["worktree", "branch", "task", "parallel"],
        ),
        Entry::action(
            "Finish this task…",
            Icons::Check,
            FinishTask,
            &["worktree", "task", "done", "archive"],
        ),
        Entry::action(
            "Side chat about this one",
            Icons::MessagesSquare,
            SideChat,
            &["aside", "btw", "question", "fork"],
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
    general.push(Entry::action(
        "Reset layout",
        Icons::LayoutGrid,
        ResetLayout,
        &["panes", "dock", "default", "restore", "arrange"],
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

    groups.push((
        "Views",
        vec![
            Entry::action(
                "Show the Workspaces view",
                ph_ui::Icons::LayoutGrid,
                ShowWorkspacesView,
                &["view", "sidebar", "chats"],
            ),
            Entry::action(
                "Open PilotHouse Chrome (agents' browser, to sign in or watch)",
                IconName::Globe,
                ph_browser::OpenAgentChrome,
                &["chrome", "edge", "browser", "sign in", "login", "agents"],
            ),
            Entry::action(
                "Show the agents at work",
                ph_ui::Icons::Bot,
                ShowAgentsView,
                &["agents", "board", "working", "needs", "waiting"],
            ),
            Entry::action(
                "Show the GitHub view",
                ph_ui::Icons::Github,
                ShowGitHubView,
                &["view", "pull", "review", "pr"],
            ),
        ],
    ));

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
        Run::OpenProject(_) | Run::OpenChat(..) | Run::Utility(_) => None,
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
    let groups = Rc::new(groups(workspace, cx));
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
                    Run::OpenChat(project, session) => {
                        let (project, session) = (*project, *session);
                        workspace.update(cx, |workspace, cx| {
                            workspace.open_chat(project, session, window, cx)
                        });
                    }
                    Run::Utility(id) => crate::utilities::open(Some(id), window, cx),
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
