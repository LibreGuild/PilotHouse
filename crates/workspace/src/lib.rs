//! The PilotHouse window shell.
//!
//! [`Workspace`] is the root view of the main window. It owns the list of
//! projects and one [`WorkspaceScreen`] per opened project, and lays out the
//! title bar, the project/session sidebar, the chat column, the context panel
//! and the status bar. Feature crates (files, board, terminal, agents...) plug
//! their views into a `WorkspaceScreen`; the shell itself holds no feature logic.
//!
//! See `docs/architecture.md` for how the pieces fit together.

mod actions;
mod app_state;
mod arch_map;
mod chat_search;
mod command_palette;
mod context_panel;
mod detect;
mod file_window;
mod housekeeping;
mod inbox;
mod instructions_view;
mod open;
mod parallel;
mod rename_dialog;
mod review;
mod schedules;
mod settings_dialog;
mod shell_integration;
mod shortcuts;
mod sidebar;
mod status;
mod status_bar;
mod title_bar;
mod usage_view;
mod utilities;
mod welcome;
mod windows;
mod workspace;
mod workspace_screen;

pub use actions::*;
pub use app_state::AppState;
pub use context_panel::ContextTab;
pub use detect::Stack;
pub use open::{main_workspace, open_paths, set_main_window};
pub use windows::{open_window, window_options};
pub use workspace::Workspace;
pub use workspace_screen::WorkspaceScreen;

use gpui_kit::App;

/// Registers key bindings and app-level action handlers. Call once at startup,
/// after `gpui_kit::init` and before opening the window.
pub fn init(cx: &mut App) {
    review::install(AppState::store(cx).clone(), cx);
    actions::bind_keys(cx);
    ph_files::init(cx, WORKSPACE_CONTEXT);
    ph_terminal::init(cx);
}
