//! Run configurations and ports.
//!
//! - [`detect`]: commands found in the project (`package.json` scripts,
//!   cargo, go, gradle, make, just, compose…). Users can add their own.
//! - [`ports`]: listening TCP ports, for "open in browser" and "stop".
//! - [`RunsPanel`]: the Run tab. Configurations run in a named terminal.
//! - `control`: `runs.ports` for agents.

pub mod detect;
pub mod ports;
mod runs_panel;

pub use detect::RunConfig;
pub use runs_panel::{RunsPanel, RunsPanelEvent};

/// Registers the control methods. Call once at startup.
pub fn init(cx: &mut gpui_kit::App) {
    use gpui_kit::AppContext as _;
    ph_control::ControlServer::handle_async(
        "runs.ports",
        |_, cx| {
            cx.background_spawn(async {
                let ports = ports::listening();
                Ok(serde_json::json!({
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
