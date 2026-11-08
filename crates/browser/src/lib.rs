//! The embedded browser, and agents' Chrome.
//!
//! Each project has one browser: a headless Chrome with its own profile in
//! the data directory, so logins persist per project. PilotHouse talks to it
//! over the Chrome DevTools Protocol:
//!
//! - [`chrome`]: find and launch Chrome (or reattach after a crash).
//! - [`cdp`]: a small DevTools client (Chrome's pipe, or the extension).
//! - [`Browser`]: page state, logs, input and the actions agents use.
//! - [`BrowserPanel`]: the pane (address bar, live view, annotation, logs).
//! - [`control`]: `browser.*` control methods behind the MCP tools.
//!
//! Agents' Chrome ([`AgentChrome`]) is the user's own Chrome instead, through
//! PilotHouse's Chrome extension ([`extension`], [`bridge`]): agents work in
//! the tabs of its PilotHouse tab group, with the user's sign-ins.
//!
//! Nothing here talks to the internet on its own: the pane's Chrome has its
//! background services switched off and the address bar never searches.

mod agent_chrome;
mod agent_control;
mod agent_scripts;
mod bridge;
mod browser;
mod browser_panel;
pub mod cdp;
pub mod chrome;
mod control;
pub mod extension;
mod extension_view;
mod hub;
mod input;
pub mod logs;
mod scripts;
mod sourcemap;
pub mod workflows;

pub use agent_chrome::{
    AgentChrome, AgentChromeEvent, AgentChromeStatus, BrowserHost, ChatNews, ChatNote, Delivery,
    SendTo, TabText,
};
pub use browser::{Browser, BrowserEvent, BrowserStatus, Emulation, Viewport, normalize_url};
pub use browser_panel::BrowserPanel;
pub use extension_view::ChromeExtensionView;
pub use hub::BrowserHub;

gpui_kit::actions!(
    browser,
    [
        /// Settings, at the Chrome extension page: setting up agents' use of
        /// the user's own Chrome.
        OpenChromeExtensionSettings,
    ]
);

/// Sets up the hub and the control methods. Call once at startup.
pub fn init(store: ph_store::Store, cx: &mut gpui_kit::App) {
    BrowserHub::init(store.clone(), cx);
    // Agents in the user's own Chrome: not in a build whose brand leaves the
    // extension out (no bridge, no extension files; its tools and Settings
    // page are gone too).
    if ph_brand::enabled(ph_brand::Feature::ChromeExtension) {
        AgentChrome::init(store, cx);
    }
    control::register(cx);
    agent_control::register(cx);
}
