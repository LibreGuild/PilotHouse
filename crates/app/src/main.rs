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
mod memory;
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
    // To stderr until the app itself starts (`chrome-host`'s stdout is
    // Chrome's channel to the extension): the helpers, and a run that only
    // hands paths to the running app, leave its log file alone.
    logs::init();
    logs::install_crash_handler();

    let (open, quick_chat) = match cli::run(&args) {
        cli::Launch::Exit(code) => std::process::exit(code),
        cli::Launch::App(open) => (open, false),
        cli::Launch::QuickChat => (Vec::new(), true),
    };

    if let Err(err) = run_app(open, quick_chat) {
        eprintln!("{} could not start: {err:#}", ph_brand::NAME);
        std::process::exit(1);
    }
}

/// Has the app's windows drawn so that a window of the system's inside
/// them shows (Windows).
///
/// A deck's page is the system's web view (WebView2), a child window of the
/// deck's window. GPUI draws a window through DirectComposition, in a layer
/// the system puts *over* the window's child windows: the web view was
/// there, loaded and working, under the app's own picture of the pane, and
/// the pane stayed empty. Presenting to the window itself (GPUI's other
/// way, the usual one for a Direct3D window) leaves child windows on top,
/// as on macOS, where the rest of the deck's code already counts on it (the
/// web view steps aside for the app's dialogs). What is given up is a
/// see-through window, which the app does not have.
///
/// GPUI reads this once, when the app is made, for every window. Whoever
/// sets the variable themselves (`0` for the layer back) is left alone.
#[cfg(windows)]
fn show_child_windows() {
    const SWITCH: &str = "GPUI_DISABLE_DIRECT_COMPOSITION";
    if std::env::var_os(SWITCH).is_none() {
        // SAFETY: at the start of the app, before any thread that reads
        // the environment is started (as `forget_parent_session` above).
        unsafe { std::env::set_var(SWITCH, "1") };
        ph_env::keep_to_app(SWITCH);
    }
}

fn run_app(open: Vec<std::path::PathBuf>, quick_chat: bool) -> Result<()> {
    // One app per data folder, settled before the database or a window is
    // opened: two launches at once (several files opened from Explorer, an
    // icon clicked twice) found no app to hand over to, and each would
    // become one on the same database. Held until the app exits.
    let _instance = match instance_lock() {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => std::process::exit(cli::hand_over_once_started(&open, quick_chat)),
        Err(err) => {
            tracing::warn!("no instance lock: {err:#}");
            None
        }
    };
    // Started from a terminal inside an agent's session (or by an agent):
    // the agents this app starts are not that session's children.
    ph_env::forget_parent_session();
    #[cfg(windows)]
    show_child_windows();
    logs::write_to_file();
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
    // The Dock icon clicked with every window closed (macOS): a window again.
    app.on_reopen(|cx| {
        if let Err(err) = ph_workspace::main_workspace_or_open(cx) {
            tracing::warn!("opening a window failed: {err:#}");
        }
    });
    app.run(move |cx| {
        ph_theme::before_init(cx);
        gpui_kit::init(cx);
        ph_ui::rich_copy::use_system_clipboard();
        ph_theme::init(cx);
        ph_browser::init(store.clone(), cx);
        ph_devices::init(store.clone(), cx);
        let projects = store.clone();
        ph_runs::init(
            store.clone(),
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
        memory::start(cx);
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
            // The app is fully usable without it; agents just can't reach its tools.
            tracing::warn!("control socket unavailable: {err:#}");
        }
        cx.activate(true);
        if quick_chat {
            ph_workspace::toggle_quick_chat(cx);
        }
    });
    Ok(())
}

/// Takes the data folder's instance lock (`None`: another launch holds
/// it). The system lets go of it when the app exits, also by a crash, so a
/// lock is never left behind.
fn instance_lock() -> std::io::Result<Option<std::fs::File>> {
    let path = paths::database_path().with_extension("lock");
    if let Some(parent) = path.parent() {
        paths::ensure_private_dir(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(err)) => Err(err),
    }
}

fn open_main_window(cx: &mut App) -> Result<()> {
    let (handle, workspace) = ph_workspace::open_window(None, cx)?;
    ph_workspace::set_main_window(handle, &workspace, cx);
    // What the app has, said once: after an install, and after an update
    // that added something.
    _ = handle.update(cx, |_, window, cx| {
        ph_workspace::tour::open_if_due(window, cx)
    });
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
