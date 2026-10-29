//! Shared UI for PilotHouse feature crates.
//!
//! Only presentation lives here: the bundled icon set and small value-like
//! components that several features render the same way. Anything with its own
//! state or workflow belongs in a feature crate.

mod agent_logos;
mod assets;
mod dialog_buttons;
mod empty_state;
mod file_icons;
mod menu;
mod pane_header;
pub mod platform;
mod surface;
pub mod time;
mod workspace_colors;

pub use agent_logos::{agent_logo, app_icon, logo_mark};
pub use assets::PilotHouseAssets;
pub use dialog_buttons::dialog_buttons;
pub use empty_state::EmptyState;
pub use file_icons::{
    file_icon, file_icon_element, file_svg_name, folder_icon_element, folder_svg_name,
};
pub use menu::{MenuButton, POPUP_GAP, SpacedMenu};
pub use pane_header::PaneHeader;
pub use surface::Surface;
pub use workspace_colors::{WORKSPACE_COLORS, workspace_color};

/// The complete Lucide icon catalog. Only the icons listed in
/// [`PilotHouseAssets`] (plus gpui-component's defaults) are embedded, so use a
/// name from that list or it will render blank.
pub use gpui_kit::assets::IconName as Icons;

/// The version as people read it: `1.0.0 beta` for `1.0.0-beta`.
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").replacen('-', " ", 1)
}
