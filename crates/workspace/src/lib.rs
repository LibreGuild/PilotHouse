//! The PilotHouse window shell.
//!
//! [`Workspace`] is the root view of a window. It owns the list of
//! workspaces and one [`WorkspaceScreen`] per opened one, and lays out the
//! title bar (with the Workspaces and GitHub views), the sidebar of
//! workspaces and chats, the screen in front and the status bar. The
//! [`GithubView`] shows pull requests across repositories and their reviews. Feature crates (files, board, terminal, agents...) plug
//! their views into a `WorkspaceScreen`; the shell itself holds no feature logic.
//!
//! See `docs/architecture.md` for how the pieces fit together.

mod actions;
mod agent_sessions;
mod agents_board;
mod app_state;
mod arch_map;
mod attention;
mod chat_search;
mod command_palette;
mod context_panel;
mod copies;
mod detect;
mod file_window;
mod github_view;
mod housekeeping;
mod inbox;
mod instructions_view;
mod network_drives;
mod open;
mod parallel;
mod rename_dialog;
mod review;
mod schedules;
mod settings_dialog;
mod shell_integration;
mod shortcuts;
mod sidebar;
mod signals;
mod status;
mod status_bar;
mod storage;
mod tasks;
mod terminal_agents;
mod title_bar;
mod updates;
mod usage_view;
mod utilities;
mod windows;
mod workspace;
mod workspace_screen;

pub use actions::*;
pub use app_state::AppState;
pub use context_panel::ContextTab;
pub use detect::Stack;
pub use github_view::GithubView;
pub use open::{main_workspace, open_paths, set_main_window};
pub use windows::{open_window, window_options};
pub use workspace::{View, Workspace};
pub use workspace_screen::WorkspaceScreen;

use gpui_kit::App;

/// Registers key bindings and app-level action handlers. Call once at startup,
/// after `gpui_kit::init` and before opening the window.
pub fn init(cx: &mut App) {
    review::install(AppState::store(cx).clone(), cx);
    attention::install(cx);
    terminal_agents::install(cx);
    ph_agent::rate_limits::RateLimits::watch_codex(cx);
    updates::install(cx);
    actions::bind_keys(cx);
    network_drives::install(cx);
    sidebar::load_width(cx);
    ph_files::init(cx, WORKSPACE_CONTEXT);
    file_window::bind_keys(cx);
    ph_terminal::init(cx);
    ph_terminal::TerminalHub::keep_between_runs(ph_store::paths::data_dir().join("terminals"), cx);
}
