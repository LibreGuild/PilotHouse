//! Embedded terminals.
//!
//! [`TerminalPanel`] is the Terminal tab: tabs of [`TerminalView`]s, each
//! showing a [`Terminal`], a login shell in a workspace folder on a real PTY.
//! The shells belong to the workspace ([`TerminalHub`]), not to the tab, so
//! they keep running while it is closed or shown in another window. Emulation (escape codes,
//! scrollback, the grid) is `alacritty_terminal`; PilotHouse only draws the
//! grid and turns key presses into bytes.
//!
//! Drag to select (double-click a word, triple-click a line) and ⌘C (Ctrl+Shift+C
//! off macOS) copies. Not yet supported: mouse reporting to programs.
//!
//! Agent CLIs PilotHouse has no chat for ([`cli_agents`]) start from the
//! tab's menu, each in its own terminal.

pub mod cli_agents;
mod keys;
mod palette;
mod terminal;
mod terminal_panel;
mod terminal_view;
mod workspace_terminals;

pub use terminal::{Terminal, TerminalEvent};
pub use terminal_panel::{TerminalPanel, TerminalPanelEvent};
pub use terminal_view::{TerminalView, TerminalViewEvent};
pub use workspace_terminals::{TerminalHub, WorkspaceTerminals};

use gpui_kit::{App, KeyBinding, actions};

actions!(
    terminal,
    [
        /// Paste the clipboard into the shell (⌘V).
        TerminalPaste,
        /// Copy the selected text (⌘C; Ctrl+Shift+C off macOS, where Ctrl+C
        /// stays the interrupt).
        TerminalCopy,
        /// Send the selected text (an error, a stack trace) to the chat (⌘L).
        TerminalAsk,
    ]
);

pub fn init(cx: &mut App) {
    let context = Some(terminal_view::TERMINAL_CONTEXT);
    let copy = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-shift-c"
    };
    cx.bind_keys([
        KeyBinding::new("secondary-v", TerminalPaste, context),
        KeyBinding::new(copy, TerminalCopy, context),
        KeyBinding::new("secondary-l", TerminalAsk, context),
    ]);
}
