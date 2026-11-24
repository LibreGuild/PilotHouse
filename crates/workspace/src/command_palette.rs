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
    /// A review's chat, shown in Reviews (its repository is no workspace).
    OpenReview(ph_store::sessions::SessionId),
    /// Start an agent CLI without a chat in a terminal.
    /// Open the Utilities dialog at one utility.
    Utility(&'static str),
    /// One of the organisation's own commands (`ph_custom::commands`), by
    /// its place in the list.
    Custom(usize),
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
    let mut live = crate::bridge::live_cards(workspace, cx);
    live.sort_by_key(|card| rank(card.summary.state));
    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for (card, session) in live.iter().filter_map(|card| match card.target {
        crate::bridge::Target::Chat(session) if card.summary.state != ChatState::Idle => {
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
            run: if card.github {
                Run::OpenReview(session)
            } else {
                Run::OpenChat(card.project, session)
            },
        });
    }
    let mut stored: Vec<(String, ph_store::sessions::Session)> = workspace
        .projects()
        .iter()
        .map(|project| (project.id, project.name.clone()))
        .flat_map(|(id, place)| {
            workspace
                .sessions_of(id, cx)
                .iter()
                .map(|session| (place.clone(), session.clone()))
                .collect::<Vec<_>>()
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
            "New task in a worktree",
            Icons::GitBranch,
            NewTask,
            &["worktree", "branch", "task", "parallel"],
        ),
        Entry::action(
            "Finish this task",
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
            "Switch workspace…",
            Icons::Folders,
            SwitchWorkspace,
            &[
                "switch workspace",
                "workspaces",
                "recent",
                "project",
                "open recent",
            ],
        ),
        Entry::action(
            "Open a folder as a workspace",
            Icons::FolderPlus,
            AddProject,
            &[
                "open folder as workspace",
                "open",
                "folder",
                "project",
                "workspace",
            ],
        ),
        Entry::action(
            "New workspace",
            Icons::SquarePen,
            NewWorkspace,
            &["empty", "chat", "workspace", "project"],
        ),
        Entry::action(
            "Add a folder to this workspace",
            Icons::FolderPlus,
            AddFolderToWorkspace,
            &[
                "add folder to workspace",
                "multi",
                "root",
                "folder",
                "repository",
            ],
        ),
        Entry::action(
            "Open this workspace in a new window",
            Icons::ExternalLink,
            OpenInNewWindow,
            &[
                "open workspace in new window",
                "window",
                "move",
                "monitor",
                "detach",
            ],
        ),
        Entry::action(
            "Rename this workspace",
            IconName::Replace,
            RenameWorkspace,
            &["rename workspace", "name", "workspace", "project"],
        ),
        Entry::action(
            "Settings",
            IconName::Settings,
            OpenSettings,
            &["preferences"],
        ),
    ];
    general.push(Entry::action(
        "New slides",
        Icons::Presentation,
        NewSlides,
        &[
            "presentation",
            "deck",
            "powerpoint",
            "pptx",
            "keynote",
            "artifact",
        ],
    ));
    general.push(Entry::action(
        "New whiteboard",
        Icons::StickyNote,
        NewWhiteboard,
        &[
            "board",
            "canvas",
            "sticky",
            "notes",
            "diagram",
            "flowchart",
            "brainstorm",
            "artifact",
        ],
    ));
    general.push(Entry::action(
        "Search chats",
        IconName::Search,
        SearchChats,
        &["find", "history", "messages", "conversation"],
    ));
    general.push(Entry::action(
        "Parallel runs",
        IconName::GalleryVerticalEnd,
        ShowParallelRuns,
        &["compare", "agents", "worktree", "side by side"],
    ));
    general.push(Entry::action(
        "Agent instructions",
        IconName::BookOpen,
        ShowInstructions,
        &["agents.md", "claude.md", "gemini.md", "sync", "rules"],
    ));
    general.push(Entry::action(
        "Schedules",
        Icons::CalendarClock,
        ShowSchedules,
        &["schedule", "cron", "timer", "summary", "nightly", "every"],
    ));
    general.push(Entry::action(
        "Network drives",
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
        "Welcome tour",
        Icons::Sparkles,
        ShowTour,
        &["onboarding", "what's new", "features", "help", "intro"],
    ));
    general.push(Entry::action(
        "Usage",
        IconName::ChartPie,
        ShowUsage,
        &["tokens", "cost", "spend", "context"],
    ));
    general.push(Entry::action(
        "Quick chat",
        Icons::MessageSquare,
        QuickChat,
        &["floating", "popup", "overlay", "ask", "spotlight", "hotkey"],
    ));
    general.push(Entry::action(
        "Show or hide the chat",
        IconName::PanelLeft,
        ToggleSidebar,
        &[
            "toggle sidebar",
            "hide the sidebar",
            "toggle",
            "hide",
            "show",
            "fold",
            "list",
        ],
    ));
    general.push(Entry::action(
        "Show or hide the result pane",
        IconName::PanelRight,
        ToggleContextPanel,
        &[
            "toggle tools",
            "hide the tools",
            "toggle",
            "hide",
            "show",
            "context",
            "panel",
            "column",
        ],
    ));
    general.push(Entry::action(
        "Show or hide the terminal",
        IconName::SquareTerminal,
        ToggleTerminal,
        &[
            "toggle terminal",
            "toggle bottom panel",
            "bottom panel",
            "toggle",
            "shell",
            "console",
        ],
    ));
    general.push(Entry::action(
        "Let the result pane fill the window",
        IconName::Maximize,
        ToggleZoom,
        &["maximize panel", "maximize", "zoom", "full", "editor"],
    ));
    general.push(Entry::action(
        "Reset the layout",
        Icons::LayoutGrid,
        ResetLayout,
        &["reset layout", "panes", "default", "restore", "arrange"],
    ));
    groups.push(("General", general));

    if workspace.active_space().is_some() {
        let views = ContextTab::available()
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
                    "Go to a file",
                    IconName::File,
                    ph_files::QuickOpen,
                    &["go to file", "open", "find", "fuzzy"],
                ),
                Entry::action(
                    "Search in the workspace",
                    IconName::Search,
                    ph_files::FindInProject,
                    &["search in workspace", "grep", "text"],
                ),
            ],
        ));
    }

    groups.push((
        "Views",
        vec![
            Entry::action(
                "Show the Bridge",
                ph_ui::Icons::ShipWheel,
                ShowWorkspacesView,
                &[
                    "workspaces view",
                    "show workspaces",
                    "agents",
                    "tasks",
                    "home",
                    "view",
                    "board",
                ],
            ),
            Entry::action(
                "Show Chats (the chats that need no folder)",
                ph_ui::Icons::MessagesSquare,
                ShowChatsView,
                &["view", "chat", "chats", "no folder", "loose", "scratch"],
            ),
            Entry::action(
                "Let agents use your Chrome",
                IconName::Globe,
                ph_browser::OpenChromeExtensionSettings,
                &[
                    "chrome",
                    "edge",
                    "brave",
                    "browser",
                    "extension",
                    "agents",
                    "tab group",
                ],
            ),
            Entry::action(
                "Show Reviews",
                ph_ui::Icons::GitPullRequest,
                ShowGitHubView,
                &[
                    "github view",
                    "show github",
                    "github",
                    "view",
                    "pull",
                    "review",
                    "pr",
                ],
            ),
            Entry::action(
                "Show Artifacts",
                ph_ui::Icons::Shapes,
                ShowArtifacts,
                &["artifact", "page", "published", "made", "library"],
            ),
        ]
        .into_iter()
        // The organisation's own views.
        .chain(ph_custom::views().into_iter().map(|custom| Entry {
            label: format!("Show the {} view", custom.label).into(),
            icon: (custom.icon)(),
            keywords: &["view"],
            run: Run::Action(Box::new(ph_custom::ShowView {
                id: custom.id.to_string(),
            })),
        }))
        .collect(),
    ));
    // The organisation's own commands.
    groups.push((
        "Custom",
        ph_custom::commands()
            .into_iter()
            .enumerate()
            .map(|(index, command)| Entry {
                label: command.label.into(),
                icon: (command.icon)(),
                keywords: command.keywords,
                run: Run::Custom(index),
            })
            .collect(),
    ));

    let projects: Vec<Entry> = workspace
        .projects()
        .iter()
        // Not the one on screen; the one behind the Bridge is offered.
        .filter(|project| {
            workspace.view() != crate::workspace::View::Workspaces
                || Some(project.id) != workspace.active_project_id()
        })
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
            "Utilities",
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
            Entry::action(
                "Design a theme with an agent",
                IconName::Palette,
                DesignTheme,
                &["theme", "colors", "customize", "mod", "look"],
            ),
        ],
    ));
    // Not what this build leaves out (the brand's features).
    for (_, entries) in &mut groups {
        entries.retain(|entry| match &entry.run {
            Run::Action(action) => crate::features::allows(action.as_ref()),
            Run::Utility(_) => ph_brand::enabled(ph_brand::Feature::Utilities),
            Run::OpenProject(_) | Run::OpenChat(..) | Run::OpenReview(_) | Run::Custom(_) => true,
        });
    }
    groups.retain(|(_, entries)| !entries.is_empty());
    groups
}

/// A palette row: icon, label and the command's shortcut on the trailing edge.
fn row(entry: &Entry) -> CommandItem {
    let label = entry.label.clone();
    let icon = entry.icon.clone();
    let action = match &entry.run {
        Run::Action(action) => Some(action.boxed_clone()),
        Run::OpenProject(_)
        | Run::OpenChat(..)
        | Run::OpenReview(_)
        | Run::Utility(_)
        | Run::Custom(_) => None,
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
/// The palette's width and the most its list is high, at the default
/// interface size.
const PALETTE_WIDTH: f32 = 680.;
const PALETTE_LIST: f32 = 460.;

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

    window.open_dialog(cx, move |dialog, window, cx| {
        let confirm_groups = groups.clone();
        let workspace = workspace.clone();
        // Wide enough to read a chat's title or a command with its key,
        // and long enough to show a dozen rows: the kit's own size (448 by
        // 300) cut titles short and showed eight. Never more than the
        // window has.
        let view = window.viewport_size();
        let width = ph_ui::scaled_px(PALETTE_WIDTH, window).min(view.width - gpui_kit::px(32.));
        let list = ph_ui::scaled_px(PALETTE_LIST, window).min(view.height * 0.6);
        let mut command = Command::new(&state)
            .bordered(false)
            .rounded(cx.theme().radius_lg)
            .max_h(list)
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
                    Run::OpenReview(session) => {
                        let session = *session;
                        workspace.update(cx, |workspace, cx| {
                            workspace.show_session(session, window, cx)
                        });
                    }
                    Run::Utility(id) => ph_utils::dialog::open(
                        Some(id),
                        crate::AppState::store(cx).clone(),
                        window,
                        cx,
                    ),
                    Run::Custom(index) => {
                        if let Some(command) = ph_custom::commands().into_iter().nth(*index) {
                            let context = ph_custom::CustomContext {
                                store: crate::AppState::store(cx).clone(),
                            };
                            (command.run)(&context, window, cx);
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
        dialog.close_button(false).w(width).p_0().child(command)
    });
}
