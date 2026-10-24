//! Run configurations and ports.
//!
//! - [`detect`]: commands found in the project (`package.json` scripts,
//!   cargo, go, gradle, make, just, compose…). Users can add their own.
//! - [`ports`]: listening TCP ports, for "open in browser" and "stop", here
//!   or on a mapped network drive's server.
//! - [`forward`]: tunnels to a server's ports, to open them here.
//! - [`RunsPanel`]: the Run tab. Configurations run in a named terminal.
//! - `control`: `runs.ports` for agents.

pub mod detect;
pub mod forward;
pub mod ports;
mod runs_panel;

pub use detect::RunConfig;
pub use runs_panel::{RunsPanel, RunsPanelEvent};

/// Registers the control methods. Call once at startup. `folder_of` gives a
/// workspace's primary folder, so a workspace on a mapped network drive gets
/// its server's ports.
pub fn init(
    folder_of: impl Fn(i64) -> Option<std::path::PathBuf> + 'static,
    cx: &mut gpui_kit::App,
) {
    use gpui_kit::AppContext as _;
    ph_control::ControlServer::handle_async(
        "runs.ports",
        move |params, cx| {
            let server = params
                .get("project_id")
                .and_then(serde_json::Value::as_i64)
                .and_then(&folder_of)
                .and_then(|folder| ph_remote::remote_of(&folder))
                .map(|remote| remote.mapping);
            cx.background_spawn(async move {
                let ports = match &server {
                    Some(server) => ports::listening_on(server),
                    None => ports::listening(),
                };
                Ok(serde_json::json!({
                    "server": server.as_ref().map(|server| server.destination()),
                    "ports": ports.iter().map(|port| serde_json::json!({
                        "port": port.port, "address": port.address, "pid": port.pid,
                        "process": port.process, "cwd": port.cwd,
                    })).collect::<Vec<_>>(),
                }))
            })
        },
        cx,
    );
}
