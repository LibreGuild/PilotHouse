//! The PilotHouse window shell.
//!
//! [`Workspace`] is the root view of a window. It owns the list of
//! workspaces and one [`WorkspaceScreen`] per opened one, and lays out the
//! title bar, the column of icons (the Bridge, Reviews, a badge per
//! workspace), what is in front (the Bridge with every task, a workspace's
//! screen, or Reviews) and the status bar. The [`GithubView`] shows pull
//! requests across repositories and their reviews. Feature crates (files,
//! terminal, agents...) plug their views into a `WorkspaceScreen`;
//! the shell itself holds no feature logic.
//!
//! See `docs/architecture.md` for how the pieces fit together.

mod actions;
mod agent_updates;
mod app_state;
mod artifacts;
mod attention;
mod bridge;
mod browser_inbox;
mod chat_search;
mod command_palette;
mod context_panel;
mod copies;
mod detect;
mod features;
mod file_window;
mod frame;
mod github_view;
mod housekeeping;
mod inbox;
mod instructions_view;
mod network_drives;
mod no_workspaces;
mod open;
mod parallel;
mod quick_chat;
mod rail;
mod rename_dialog;
mod review;
mod schedules;
mod session_changes;
mod settings_dialog;
mod shell_integration;
mod shortcuts;
mod sidebar;
mod signals;
mod slides_window;
mod status;
mod status_bar;
mod storage;
mod studio_window;
mod task_facts;
mod tasks;
mod terminal_agents;
pub mod theme_mods;
mod title_bar;
pub mod tour;
mod updates;
mod usage_view;
mod windows;
mod workspace;
mod workspace_screen;
mod workspace_switcher;

pub use actions::*;
pub use app_state::AppState;
pub use context_panel::ContextTab;
pub use detect::Stack;
pub use github_view::GithubView;
pub use open::{main_workspace, main_workspace_or_open, open_paths, set_main_window};
pub use quick_chat::toggle as toggle_quick_chat;

/// Opens `path` (a picture or video) in Studio without a chat (testing).
pub fn open_in_studio(path: std::path::PathBuf, cx: &mut gpui_kit::App) -> anyhow::Result<()> {
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
    // Read once at the start: what counts usage reads every account's
    // files, and has no store at hand.
    ph_agent::profiles::Profiles::load(AppState::store(cx));
    ph_agent::rate_limits::RateLimits::watch_codex(cx);
    // Which agents are installed (new chats start with one), then their
    // models, so the picker lists them the first time.
    ph_agent::installed::install(AppState::store(cx).clone(), cx);
    updates::install(cx);
    agent_updates::install(cx);
    actions::bind_keys(cx);
    network_drives::install(cx);
    sidebar::load_sizes(cx);
    // Ctrl+P and Ctrl+T are a shell's too: in a terminal they stay its own.
    ph_files::init(
        cx,
        actions::context_for(
            ph_ui::keys("secondary-p"),
            Some(WORKSPACE_CONTEXT),
            cfg!(target_os = "macos"),
        )
        .unwrap_or(WORKSPACE_CONTEXT),
    );
    // Editors offer Why Is This Here? where the build explains changes.
    ph_files::set_explains(workspace_screen::explain::explains());
    file_window::bind_keys(cx);
    ph_studio::bind_keys(cx);
    ph_terminal::init(cx);
    ph_terminal::TerminalHub::keep_between_runs(ph_store::paths::data_dir().join("terminals"), cx);
    quick_chat::init(cx);
    browser_inbox::install(cx);
    artifacts::install(cx);
    // The organisation's own additions (crates/custom): last, so they can
    // use all of the above. PilotHouse's own does nothing.
    let context = ph_custom::CustomContext {
        store: AppState::store(cx).clone(),
    };
    ph_custom::init(&context, cx);
}
