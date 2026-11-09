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
mod agents_board;
mod app_state;
mod arch_map;
mod attention;
mod browser_inbox;
mod chat_search;
mod command_palette;
mod context_panel;
mod copies;
mod detect;
mod features;
mod file_window;
mod github_view;
mod housekeeping;
mod inbox;
mod instructions_view;
mod network_drives;
mod open;
mod parallel;
mod presentations;
mod quick_chat;
mod rename_dialog;
mod review;
mod schedules;
mod session_changes;
mod settings_dialog;
mod shell_integration;
mod shortcuts;
mod sidebar;
mod signals;
mod status;
mod status_bar;
mod storage;
mod studio_window;
mod tasks;
mod terminal_agents;
pub mod theme_mods;
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
pub use quick_chat::toggle as toggle_quick_chat;

/// Opens `path` (a picture or video) in Studio without a chat (testing).
pub fn open_in_studio(path: std::path::PathBuf, cx: &mut gpui_kit::App) -> anyhow::Result<()> {
    if ph_studio::slides::space::is_powerpoint(&path) {
        let window = cx
            .windows()
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no window to open it from"))?;
        return window
            .update(cx, |_, window, cx| {
                presentations::open_powerpoint(path, window, cx)
            })
            .map_err(|err| anyhow::anyhow!("{err}"));
    }
    studio_window::open(path, None, cx)
}
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
    // Which agents are installed (new chats start with one), then their
    // models, so the picker lists them the first time.
    ph_agent::installed::install(AppState::store(cx).clone(), cx);
    updates::install(cx);
    actions::bind_keys(cx);
    network_drives::install(cx);
    sidebar::load_sizes(cx);
    ph_files::init(cx, WORKSPACE_CONTEXT);
    file_window::bind_keys(cx);
    ph_studio::bind_keys(cx);
    ph_terminal::init(cx);
    ph_terminal::TerminalHub::keep_between_runs(ph_store::paths::data_dir().join("terminals"), cx);
    quick_chat::init(cx);
    browser_inbox::install(cx);
    // The organisation's own additions (crates/custom): last, so they can
    // use all of the above. PilotHouse's own does nothing.
    let context = ph_custom::CustomContext {
        store: AppState::store(cx).clone(),
    };
    ph_custom::init(&context, cx);
}
