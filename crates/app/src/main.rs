// A windowed app on Windows: no console window opens with it. Subcommands
// that print (`ctl`) attach to the console they were started from.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! PilotHouse entry point.
//!
//! With no arguments this starts the desktop app. Subcommands reuse the same
//! binary for helper processes:
//!
//! - `pilothouse <path>...`: open folders and files, in the running app if
//!   there is one (see `cli`).
//! - `pilothouse ctl <method> [json]`: call the running app's control socket
//!   (used by scripts and the dev automation harness).
//! - `pilothouse mcp`: the MCP server agents use for PilotHouse tools.
//! - `pilothouse agy-acp`: Google Antigravity as an ACP agent.

mod cli;
mod control_handlers;
mod logs;
mod menus;

use anyhow::Result;
use gpui_kit::App;
use ph_store::{Store, paths};
use ph_workspace::AppState;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Only `ctl` prints for a person; `mcp` and `agy-acp` must keep the pipes
    // they were given.
    #[cfg(windows)]
    if args.first().is_some_and(|command| command == "ctl") {
        attach_console();
    }
    // The helpers run beside the app and keep to stderr.
    let helper = matches!(
        args.first().map(String::as_str),
        Some("ctl" | "mcp" | "agy-acp" | "agent-hook")
    );
    logs::init(!helper);
    logs::install_crash_handler();

    let open = match cli::run(&args) {
        cli::Launch::Exit(code) => std::process::exit(code),
        cli::Launch::App(open) => open,
    };

    if let Err(err) = run_app(open) {
        eprintln!("PilotHouse could not start: {err:#}");
        std::process::exit(1);
    }
}

fn run_app(open: Vec<std::path::PathBuf>) -> Result<()> {
    // Agents and terminals need the login shell's PATH; read it while the UI starts.
    ph_env::warm_up();
    let store = Store::open(&paths::database_path())?;

    // Finder's "Open With" and `open -a` arrive as URLs (macOS), also while
    // the app is starting.
    let (opened, opened_urls) = smol::channel::unbounded::<Vec<String>>();
    let app = gpui_kit::application().with_assets(ph_ui::PilotHouseAssets);
    app.on_open_urls(move |urls| {
        let _ = opened.send_blocking(urls);
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        ph_ui::init(cx);
        ph_theme::init(cx);
        ph_browser::init(store.clone(), cx);
        ph_devices::init(cx);
        let projects = store.clone();
        ph_runs::init(
            move |id| {
                projects
                    .project(id)
                    .ok()
                    .flatten()
                    .map(|project| project.path)
            },
            cx,
        );
        ph_uitest::init(store.clone(), cx);
        AppState::install(store, cx);
        ph_workspace::init(cx);
        menus::install(cx);

        if let Err(err) = open_main_window(cx) {
            tracing::error!("opening the main window failed: {err:#}");
            cx.quit();
            return;
        }
        control_handlers::register(cx);
        if !open.is_empty()
            && let Err(err) = ph_workspace::open_paths(&open, cx)
        {
            tracing::warn!("opening {open:?} failed: {err:#}");
        }
        cx.spawn(async move |cx| {
            while let Ok(urls) = opened_urls.recv().await {
                let paths: Vec<_> = urls.iter().filter_map(|url| file_url_path(url)).collect();
                cx.update(|cx| {
                    if let Err(err) = ph_workspace::open_paths(&paths, cx) {
                        tracing::warn!("opening {paths:?} failed: {err:#}");
                    }
                });
            }
        })
        .detach();
        if let Err(err) = ph_control::ControlServer::start(&paths::control_socket_path(), cx) {
            // The app is fully usable without it; agents just can't reach the board.
            tracing::warn!("control socket unavailable: {err:#}");
        }
        cx.activate(true);
    });
    Ok(())
}

fn open_main_window(cx: &mut App) -> Result<()> {
    let (handle, workspace) = ph_workspace::open_window(None, cx)?;
    ph_workspace::set_main_window(handle, &workspace, cx);
    Ok(())
}

/// The local path of a `file://` URL.
fn file_url_path(url: &str) -> Option<std::path::PathBuf> {
    url::Url::parse(url).ok()?.to_file_path().ok()
}

/// Lets `pilothouse ctl …` print in the terminal it was run from. The MCP
/// bridge needs nothing: it talks over the pipes its agent gives it.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: plain Win32 call; fails harmlessly when there is no parent console.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
