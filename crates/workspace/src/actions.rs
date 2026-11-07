//! Workspace commands. Each command is one GPUI action, so the keyboard, the
//! command palette and buttons all run the same handler on [`crate::Workspace`].

use gpui_kit::{Action, App, KeyBinding, actions};
use serde::Deserialize;

use crate::AppState;

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
        /// What the agents used: tokens, the estimate at API prices and
        /// limits, by agent, workspace or model.
        ShowUsage,
        /// A small chat window over every app (also from anywhere, with its
        /// shortcut in Settings → Chat).
        QuickChat,
        /// A new presentation in Studio (title, size, theme).
        NewPresentation,
        /// A presentation from the presentations folder, in Studio.
        OpenPresentation,
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
        /// Show or hide the column of tools beside the chat.
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
        /// The chosen device's log (a tool of its own, at the bottom).
        ShowLogcat,
        /// Open or close the bottom panel (the terminal, Logcat).
        ToggleBottomPanel,
        /// Show the terminal, or hide the bottom panel when it is in front.
        ToggleTerminal,
        /// Send a screenshot of the visible browser or device (with its marks) to chat.
        CaptureToChat,
        OpenSettings,
        /// Settings, at its GitHub page (accounts and tokens).
        OpenGitHubSettings,
        /// Settings, at its Storage page (what is on disk, and cleaning up).
        OpenStorageSettings,
        UseSystemAppearance,
        UseLightAppearance,
        UseDarkAppearance,
        /// A new chat to design a theme with an agent, shown live as it changes.
        DesignTheme,
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
        /// The Agents pop-up: chats at work or waiting, across workspaces (⌘⇧A).
        ShowAgentsView,
        /// Move the workspace in front to a window of its own.
        OpenInNewWindow,
        Quit,
        /// Everything a little larger (⌘=, Ctrl+= elsewhere).
        ZoomIn,
        /// Everything a little smaller (⌘-, Ctrl+-).
        ZoomOut,
        /// The default sizes again (⌘0, Ctrl+0).
        ResetZoom,
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
        KeyBinding::new(
            ph_ui::keys("secondary-shift-p"),
            ToggleCommandPalette,
            context,
        ),
        KeyBinding::new(ph_ui::keys("secondary-k"), ToggleCommandPalette, context),
        KeyBinding::new(ph_ui::keys("secondary-o"), AddProject, context),
        KeyBinding::new(ph_ui::keys("secondary-n"), NewSession, context),
        KeyBinding::new(ph_ui::keys("secondary-;"), SideChat, context),
        KeyBinding::new(ph_ui::keys("secondary-shift-k"), SearchChats, context),
        KeyBinding::new(ph_ui::keys("secondary-b"), ToggleSidebar, context),
        // As in VS Code: ⌘J the bottom panel, ⌃` the terminal, ⌥⌘B the
        // column of tools beside the chat.
        KeyBinding::new(ph_ui::keys("secondary-j"), ToggleBottomPanel, context),
        KeyBinding::new(ph_ui::keys("ctrl-`"), ToggleTerminal, context),
        KeyBinding::new(ph_ui::keys("secondary-alt-b"), ToggleContextPanel, context),
        KeyBinding::new(ph_ui::keys("secondary-shift-j"), ToggleZoom, context),
        KeyBinding::new(ph_ui::keys("secondary-1"), ShowFiles, context),
        KeyBinding::new(ph_ui::keys("secondary-2"), ShowChanges, context),
        KeyBinding::new(ph_ui::keys("secondary-3"), ShowBoard, context),
        KeyBinding::new(ph_ui::keys("secondary-4"), ShowTerminal, context),
        KeyBinding::new(ph_ui::keys("secondary-5"), ShowBrowser, context),
        KeyBinding::new(ph_ui::keys("secondary-6"), ShowDevices, context),
        KeyBinding::new(ph_ui::keys("secondary-7"), ShowRuns, context),
        KeyBinding::new(ph_ui::keys("secondary-8"), ShowPulls, context),
        KeyBinding::new(ph_ui::keys("secondary-9"), ShowTests, context),
        KeyBinding::new(ph_ui::keys("secondary-shift-s"), CaptureToChat, context),
        KeyBinding::new(ph_ui::keys("secondary-,"), OpenSettings, context),
        KeyBinding::new(ph_ui::keys("secondary-/"), ShowShortcuts, context),
        KeyBinding::new(ph_ui::keys("secondary-q"), Quit, None),
        // Zoom, as in VS Code, everywhere in the window. ⌘+ is ⌘⇧= on most
        // keyboards; both work.
        KeyBinding::new(ph_ui::keys("secondary-="), ZoomIn, None),
        KeyBinding::new(ph_ui::keys("secondary-+"), ZoomIn, None),
        KeyBinding::new(ph_ui::keys("secondary-shift-="), ZoomIn, None),
        KeyBinding::new(ph_ui::keys("secondary--"), ZoomOut, None),
        KeyBinding::new(ph_ui::keys("secondary-0"), ResetZoom, None),
        KeyBinding::new(ph_ui::keys("ctrl-tab"), PreviousWorkspace, context),
        KeyBinding::new(
            ph_ui::keys("secondary-shift-e"),
            ShowWorkspacesView,
            context,
        ),
        KeyBinding::new(ph_ui::keys("secondary-shift-g"), ShowGitHubView, context),
        KeyBinding::new(ph_ui::keys("secondary-shift-a"), ShowAgentsView, context),
    ]);
    // ⌘/Ctrl+digits show tools, so workspaces take the other modifier.
    const KEYS: [&str; 9] = if cfg!(target_os = "macos") {
        [
            "ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6", "ctrl-7", "ctrl-8",
            "ctrl-9",
        ]
    } else {
        [
            "alt-1", "alt-2", "alt-3", "alt-4", "alt-5", "alt-6", "alt-7", "alt-8", "alt-9",
        ]
    };
    cx.bind_keys(
        KEYS.iter()
            .enumerate()
            .map(|(index, keys)| KeyBinding::new(keys, SwitchToWorkspace { index }, context)),
    );
    cx.on_action(|_: &Quit, cx| {
        // Studio's unsaved work is saved, or asked about, first; once the
        // window the key came from is free to ask.
        cx.defer(|cx| {
            if crate::studio_window::ready_to_quit(cx) {
                cx.quit();
            }
        });
    });
    cx.on_action(|_: &QuickChat, cx| crate::quick_chat::toggle(cx));
    cx.on_action(|_: &ZoomIn, cx| AppState::update_preferences(cx, |p| p.zoom(1)));
    cx.on_action(|_: &ZoomOut, cx| AppState::update_preferences(cx, |p| p.zoom(-1)));
    cx.on_action(|_: &ResetZoom, cx| AppState::update_preferences(cx, |p| p.reset_zoom()));
}
