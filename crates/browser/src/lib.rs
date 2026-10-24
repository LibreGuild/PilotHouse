//! The embedded browser.
//!
//! Each project has one browser: a headless Chrome with its own profile in
//! the data directory, so logins persist per project. PilotHouse talks to it
//! over the Chrome DevTools Protocol:
//!
//! - [`chrome`]: find and launch Chrome (or reattach after a crash).
//! - [`cdp`]: a small DevTools client on one WebSocket.
//! - [`Browser`]: page state, logs, input and the actions agents use.
//! - [`BrowserPanel`]: the pane (address bar, live view, annotation, logs).
//! - [`control`]: `browser.*` control methods behind the MCP tools.
//!
//! Nothing here talks to the internet on its own: Chrome's background
//! services are switched off and the address bar never searches.

mod browser;
mod browser_panel;
pub mod cdp;
pub mod chrome;
mod control;
mod hub;
mod input;
pub mod logs;
mod scripts;

pub use browser::{Browser, BrowserEvent, BrowserStatus, Emulation, Viewport, normalize_url};
pub use browser_panel::BrowserPanel;
pub use hub::BrowserHub;

/// Sets up the hub and the control methods. Call once at startup.
pub fn init(store: ph_store::Store, cx: &mut gpui_kit::App) {
    BrowserHub::init(store, cx);
    control::register(cx);
}
