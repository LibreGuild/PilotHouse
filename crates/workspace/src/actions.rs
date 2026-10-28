//! Workspace commands. Each command is one GPUI action, so the keyboard, the
//! command palette and buttons all run the same handler on [`crate::Workspace`].

use gpui_kit::{Action, App, KeyBinding, actions};
use serde::Deserialize;

/// Key context set on the workspace root; bindings below only fire inside it.
pub const WORKSPACE_CONTEXT: &str = "Workspace";

actions!(
    workspace,
    [
        /// Pick a folder and open it as a workspace (one already listed is
        /// reused).
        AddProject,
        /// A workspace without folders, to chat or use the browser and
        /// devices; folders can be added later.
        NewWorkspace,
        /// Pick a folder and add it to the workspace in front.
        AddFolderToWorkspace,
        /// Give the workspace in front another name.
        RenameWorkspace,
        /// Open the command palette.
        ToggleCommandPalette,
        /// Start a new chat session in the current project.
        NewSession,
        /// Ask something aside, in a chat that has read the one in front.
        SideChat,
        /// Search the words of every chat (⌘⇧K).
        SearchChats,
        /// Tokens and cost by agent or project.
        ShowUsage,
        /// The keyboard shortcuts sheet.
        ShowShortcuts,
        /// Network drives whose work runs on a server over SSH.
        ShowNetworkDrives,
        /// Agent prompts and test runs at set times.
        ShowSchedules,
        /// Offline tools: formats, encodings, hashes, times, regex, SQLite.
        ShowUtilities,
        /// The project's agent instruction files, and syncing them.
        ShowInstructions,
        /// Compare and keep the work of agents run side by side.
        ShowParallelRuns,
        /// The project's parts and how they depend on each other.
        ShowArchitecture,
        /// Show or hide the project sidebar.
        ToggleSidebar,
        /// Show or hide the context panel next to the chat.
        ToggleContextPanel,
        /// Let the context panel fill the window (and back).
        ToggleZoom,
        /// Put the panes back where a new workspace has them.
        ResetLayout,
        ShowFiles,
        ShowChanges,
        ShowBoard,
        ShowTerminal,
        ShowBrowser,
        ShowDevices,
        ShowRuns,
        ShowPulls,
        ShowTests,
        /// Send a screenshot of the visible browser or device (with its marks) to chat.
        CaptureToChat,
        OpenSettings,
        UseSystemAppearance,
        UseLightAppearance,
        UseDarkAppearance,
        /// Remove the workspace in front from PilotHouse (its folders stay).
        RemoveWorkspace,
        /// A new task: a branch in its own worktree, as a workspace.
        NewTask,
        /// The task in front is done: its worktree and workspace go.
        FinishTask,
        /// Back to the workspace shown before this one (Ctrl+Tab).
        PreviousWorkspace,
        /// The Workspaces view: workspaces and their chats (⌘⇧E).
        ShowWorkspacesView,
        /// The GitHub view: pull requests across repositories, reviews (⌘⇧G).
        ShowGitHubView,
        /// The Agents view: chats at work or waiting, across workspaces (⌘⇧A).
        ShowAgentsView,
        /// Move the workspace in front to a window of its own.
        OpenInNewWindow,
        Quit,
    ]
);

/// Show the workspace at `index` in the sidebar (⌃1…⌃9 on macOS, Alt+1…9
/// elsewhere).
#[derive(Action, Clone, PartialEq, Deserialize)]
#[action(namespace = workspace, no_json)]
pub struct SwitchToWorkspace {
    pub index: usize,
}

pub(crate) fn bind_keys(cx: &mut App) {
    let context = Some(WORKSPACE_CONTEXT);
    cx.bind_keys([
        // The last binding for an action is the one menus and hints display.
        KeyBinding::new("secondary-shift-p", ToggleCommandPalette, context),
        KeyBinding::new("secondary-k", ToggleCommandPalette, context),
        KeyBinding::new("secondary-o", AddProject, context),
        KeyBinding::new("secondary-n", NewSession, context),
        KeyBinding::new("secondary-;", SideChat, context),
        KeyBinding::new("secondary-shift-k", SearchChats, context),
        KeyBinding::new("secondary-b", ToggleSidebar, context),
        KeyBinding::new("secondary-j", ToggleContextPanel, context),
        KeyBinding::new("secondary-shift-j", ToggleZoom, context),
        KeyBinding::new("secondary-1", ShowFiles, context),
        KeyBinding::new("secondary-2", ShowChanges, context),
        KeyBinding::new("secondary-3", ShowBoard, context),
        KeyBinding::new("secondary-4", ShowTerminal, context),
        KeyBinding::new("secondary-5", ShowBrowser, context),
        KeyBinding::new("secondary-6", ShowDevices, context),
        KeyBinding::new("secondary-7", ShowRuns, context),
        KeyBinding::new("secondary-8", ShowPulls, context),
        KeyBinding::new("secondary-9", ShowTests, context),
        KeyBinding::new("secondary-shift-s", CaptureToChat, context),
        KeyBinding::new("secondary-,", OpenSettings, context),
        KeyBinding::new("secondary-/", ShowShortcuts, context),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("ctrl-tab", PreviousWorkspace, context),
        KeyBinding::new("secondary-shift-e", ShowWorkspacesView, context),
        KeyBinding::new("secondary-shift-g", ShowGitHubView, context),
        KeyBinding::new("secondary-shift-a", ShowAgentsView, context),
    ]);
    // ⌘/Ctrl+digits show tools, so workspaces take the other modifier.
    let modifier = if cfg!(target_os = "macos") {
        "ctrl"
    } else {
        "alt"
    };
    cx.bind_keys((0..9).map(|index| {
        KeyBinding::new(
            &format!("{modifier}-{}", index + 1),
            SwitchToWorkspace { index },
            context,
        )
    }));
    cx.on_action(|_: &Quit, cx| cx.quit());
}
