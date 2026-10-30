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

mod agent_chrome;
mod agent_control;
mod agent_scripts;
mod browser;
mod browser_panel;
pub mod cdp;
pub mod chrome;
mod control;
mod hub;
mod input;
pub mod logs;
mod scripts;

pub use agent_chrome::{AgentChrome, AgentChromeEvent, AgentChromeStatus};
pub use browser::{Browser, BrowserEvent, BrowserStatus, Emulation, Viewport, normalize_url};
pub use browser_panel::BrowserPanel;
pub use hub::BrowserHub;

gpui_kit::actions!(
    browser,
    [
        /// Show agents' Chrome (starting it): to sign in to sites agents
        /// use, or to watch them.
        OpenAgentChrome,
    ]
);

/// Sets up the hub and the control methods. Call once at startup.
pub fn init(store: ph_store::Store, cx: &mut gpui_kit::App) {
    BrowserHub::init(store.clone(), cx);
    AgentChrome::init(store, cx);
    control::register(cx);
    agent_control::register(cx);
    cx.on_action(|_: &OpenAgentChrome, cx| {
        use gpui_kit::TaskExt as _;
        if let Some(chrome) = AgentChrome::global(cx) {
            chrome
                .update(cx, |chrome, cx| chrome.show(cx))
                .detach_and_log_err(cx);
        }
    });
}
