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
        /// Fold the chat away or bring it back (⌘B); in Reviews, its list.
        /// (Named for the list of workspaces and chats it showed and hid.)
        ToggleSidebar,
        /// Show or hide the result pane beside the chat (⌥⌘B).
        ToggleContextPanel,
        /// Let the result pane have the window, and back (⌘⇧J).
        ToggleZoom,
        /// Put the panes back where a new workspace has them.
        ResetLayout,
        ShowFiles,
        ShowChanges,
        ShowTerminal,
        ShowBrowser,
        ShowDevices,
        ShowRuns,
        ShowPulls,
        ShowTests,
        /// The chosen device's log (a tool of its own, under More).
        ShowLogcat,
        /// The terminal (⌘J, ⌃`); with it in front already, what was
        /// there before it.
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
        /// The Bridge: every task of every workspace (⌘⇧E, ⌘⇧A). (Named
        /// for the list of workspaces that was in its place.)
        ShowWorkspacesView,
        /// The workspace for chats that need no folder ("Chats"), made
        /// when there is none (⌘⇧D). Named for what it showed before that
        /// was a workspace: keymaps and menus refer to it by this name.
        ShowChatsView,
        /// The GitHub view: pull requests across repositories, reviews (⌘⇧G).
        ShowGitHubView,
        /// Move the workspace in front to a window of its own.
        OpenInNewWindow,
        Quit,
        /// Everything a little larger (⌘=, Ctrl+= elsewhere).
        ZoomIn,
        /// Everything a little smaller (⌘-, Ctrl+-).
        ZoomOut,
        /// The default sizes again (⌘0, Ctrl+0).
        ResetZoom,
        /// How wide the chat's messages get (the chat's ⋯ menu, and
        /// Settings → Chat): the column, a wider one, the whole pane.
        ChatWidthNormal,
        ChatWidthWide,
        ChatWidthFull,
    ]
);

/// Show the workspace at `index` in the column of icons (⌃1…⌃9 on macOS, Alt+1…9
/// elsewhere).
#[derive(Action, Clone, PartialEq, Deserialize)]
#[action(namespace = workspace, no_json)]
pub struct SwitchToWorkspace {
    pub index: usize,
}

/// The workspace's key context less a terminal's (`ph_terminal`'s
/// "Terminal"), for keys a terminal keeps.
const WORKSPACE_OUTSIDE_TERMINAL: &str = "Workspace && !Terminal";
/// Anywhere but in a terminal.
const OUTSIDE_TERMINAL: &str = "!Terminal";

/// The context to bind `keys` (as GPUI reads them) in, for a binding meant
/// for `context`, on a Mac when `mac`. Off a Mac the app's shortcuts are
/// Ctrl keys, and Ctrl with a letter, digit or sign is what a shell and the
/// agents' CLIs read (Ctrl+W deletes a word, Ctrl+B is tmux's prefix, Ctrl+J
/// a new line): a binding acts before the terminal sees the key, so while a
/// terminal has the keyboard those are left to it. Ctrl+` and Ctrl+Tab
/// stay, as in VS Code: they mean nothing to a shell. On a Mac the app's
/// keys are ⌘, which a terminal does not read.
pub(crate) fn context_for(
    keys: &str,
    context: Option<&'static str>,
    mac: bool,
) -> Option<&'static str> {
    let (modifiers, key) = match keys.strip_suffix("--") {
        Some(modifiers) => (modifiers, "-"),
        None => keys.rsplit_once('-').unwrap_or(("", keys)),
    };
    let shells = !mac && !keys.contains(' ') && modifiers == "ctrl" && !matches!(key, "`" | "tab");
    match context {
        _ if !shells => context,
        None => Some(OUTSIDE_TERMINAL),
        Some(WORKSPACE_CONTEXT) => Some(WORKSPACE_OUTSIDE_TERMINAL),
        Some(other) => Some(other),
    }
}

/// `binding` written with `secondary-` as GPUI reads it on a Mac when
/// `mac`.
fn resolve(binding: &'static str, mac: bool) -> &'static str {
    if mac == cfg!(target_os = "macos") {
        return ph_ui::keys(binding);
    }
    // Only the tests ask for the other systems' keys.
    let key = if mac { "cmd" } else { "ctrl" };
    Box::leak(binding.replace("secondary", key).into_boxed_str())
}

/// Binds `binding` (written with `secondary-`) to `action` in `context`,
/// as it is on a Mac when `mac`.
fn bind<A: Action>(
    binding: &'static str,
    action: A,
    context: Option<&'static str>,
    mac: bool,
) -> KeyBinding {
    let keys = resolve(binding, mac);
    KeyBinding::new(keys, action, context_for(keys, context, mac))
}

/// The keys of the workspace's commands, on a Mac when `mac`.
pub(crate) fn bindings(mac: bool) -> Vec<KeyBinding> {
    let context = Some(WORKSPACE_CONTEXT);
    let mut bindings = vec![
        // The last binding for an action is the one menus and hints display.
        bind("secondary-shift-p", ToggleCommandPalette, context, mac),
        bind("secondary-k", ToggleCommandPalette, context, mac),
        bind("secondary-o", AddProject, context, mac),
        bind("secondary-n", NewSession, context, mac),
        bind("secondary-;", SideChat, context, mac),
        bind("secondary-shift-k", SearchChats, context, mac),
        bind("secondary-b", ToggleSidebar, context, mac),
        // As in VS Code: ⌃` and ⌘J the terminal (⌘J last: it is the one
        // menus and hints show), ⌥⌘B the pane beside the chat.
        bind("ctrl-`", ToggleTerminal, context, mac),
        bind("secondary-j", ToggleTerminal, context, mac),
        bind("secondary-alt-b", ToggleContextPanel, context, mac),
        bind("secondary-shift-j", ToggleZoom, context, mac),
        bind("secondary-1", ShowFiles, context, mac),
        bind("secondary-2", ShowChanges, context, mac),
        bind("secondary-4", ShowTerminal, context, mac),
        bind("secondary-5", ShowBrowser, context, mac),
        bind("secondary-6", ShowDevices, context, mac),
        bind("secondary-7", ShowRuns, context, mac),
        bind("secondary-8", ShowPulls, context, mac),
        bind("secondary-9", ShowTests, context, mac),
        bind("secondary-shift-s", CaptureToChat, context, mac),
        bind("secondary-,", OpenSettings, context, mac),
        bind("secondary-/", ShowShortcuts, context, mac),
        bind("secondary-q", Quit, None, mac),
        // Zoom, as in VS Code, everywhere in the window. ⌘+ is ⌘⇧= on most
        // keyboards; both work.
        bind("secondary-=", ZoomIn, None, mac),
        bind("secondary-+", ZoomIn, None, mac),
        bind("secondary-shift-=", ZoomIn, None, mac),
        bind("secondary--", ZoomOut, None, mac),
        bind("secondary-0", ResetZoom, None, mac),
        bind("ctrl-tab", PreviousWorkspace, context, mac),
        // ⌘⇧A was the Agents pop-up's key; ⌘⇧E last, the one hints show.
        bind("secondary-shift-a", ShowWorkspacesView, context, mac),
        bind("secondary-shift-e", ShowWorkspacesView, context, mac),
        bind("secondary-shift-d", ShowChatsView, context, mac),
        bind("secondary-shift-g", ShowGitHubView, context, mac),
    ];
    // ⌘/Ctrl+digits show tools, so workspaces take the other modifier.
    let keys: [&'static str; 9] = if mac {
        [
            "ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6", "ctrl-7", "ctrl-8",
            "ctrl-9",
        ]
    } else {
        [
            "alt-1", "alt-2", "alt-3", "alt-4", "alt-5", "alt-6", "alt-7", "alt-8", "alt-9",
        ]
    };
    bindings.extend(
        keys.into_iter()
            .enumerate()
            .map(|(index, keys)| bind(keys, SwitchToWorkspace { index }, context, mac)),
    );
    bindings
}

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys(bindings(cfg!(target_os = "macos")));
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
    for width in ph_theme::ChatWidth::ALL {
        let set = move |cx: &mut App| AppState::update_preferences(cx, |p| p.chat_width = width);
        match width {
            ph_theme::ChatWidth::Comfortable => {
                cx.on_action(move |_: &ChatWidthNormal, cx| set(cx))
            }
            ph_theme::ChatWidth::Wide => cx.on_action(move |_: &ChatWidthWide, cx| set(cx)),
            ph_theme::ChatWidth::Full => cx.on_action(move |_: &ChatWidthFull, cx| set(cx)),
        };
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{KeyContext, Keystroke};

    use super::*;

    /// Whether `keys` runs `action` with the keyboard in `stack` (the key
    /// contexts from the window's root to what has the keyboard).
    fn acts(
        bindings: &[KeyBinding],
        keys: &'static str,
        action: &dyn Action,
        stack: &[&'static str],
    ) -> bool {
        let typed = [Keystroke::parse(keys).unwrap()];
        let stack: Vec<KeyContext> = stack
            .iter()
            .map(|context| KeyContext::parse(context).unwrap())
            .collect();
        bindings.iter().any(|binding| {
            binding.match_keystrokes(&typed) == Some(false)
                && binding.action().partial_eq(action)
                && binding
                    .predicate()
                    .is_none_or(|predicate| predicate.depth_of(&stack).is_some())
        })
    }

    #[test]
    fn a_terminal_keeps_the_ctrl_keys_a_shell_reads() {
        let bindings = bindings(false);
        let workspace = ["Root", "Workspace"];
        let terminal = ["Root", "Workspace", "Terminal"];
        let file = ["Root"];
        for (keys, action) in [
            ("ctrl-k", &ToggleCommandPalette as &dyn Action),
            ("ctrl-b", &ToggleSidebar),
            ("ctrl-j", &ToggleTerminal),
            ("ctrl-n", &NewSession),
            ("ctrl-o", &AddProject),
            ("ctrl-1", &ShowFiles),
            ("ctrl--", &ZoomOut),
        ] {
            assert!(acts(&bindings, keys, action, &workspace), "{keys}");
            assert!(!acts(&bindings, keys, action, &terminal), "{keys}");
        }
        // Quit works in any window, but not from a terminal.
        assert!(acts(&bindings, "ctrl-q", &Quit, &file));
        assert!(!acts(&bindings, "ctrl-q", &Quit, &terminal));
        // What a shell does not read still works there.
        assert!(acts(&bindings, "ctrl-`", &ToggleTerminal, &terminal));
        assert!(acts(&bindings, "ctrl-tab", &PreviousWorkspace, &terminal));
        assert!(acts(
            &bindings,
            "ctrl-shift-p",
            &ToggleCommandPalette,
            &terminal
        ));
        assert!(acts(
            &bindings,
            "alt-2",
            &SwitchToWorkspace { index: 1 },
            &terminal
        ));
    }

    #[test]
    fn on_a_mac_the_terminal_leaves_the_command_keys_alone() {
        let bindings = bindings(true);
        let terminal = ["Root", "Workspace", "Terminal"];
        assert!(acts(&bindings, "cmd-k", &ToggleCommandPalette, &terminal));
        assert!(acts(&bindings, "cmd-q", &Quit, &terminal));
        assert!(acts(&bindings, "cmd-j", &ToggleTerminal, &terminal));
        assert!(acts(
            &bindings,
            "ctrl-2",
            &SwitchToWorkspace { index: 1 },
            &terminal
        ));
        assert_eq!(context_for("cmd-w", None, true), None);
        assert_eq!(context_for("ctrl-w", None, false), Some(OUTSIDE_TERMINAL));
        assert_eq!(context_for("ctrl-shift-w", None, false), None);
    }
}
