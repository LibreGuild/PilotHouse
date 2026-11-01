//! PilotHouse design system.
//!
//! Owns the themes (PilotHouse's own, and the user's own: [`themes`]), the
//! user's choice of one, and the few layout tokens that are not part of the
//! gpui-component theme (pane widths, motion durations).
//!
//! Application code never uses raw colors: it reads semantic roles from
//! `cx.theme()` (gpui-component's `ActiveTheme`). PilotHouse's palettes live
//! in `themes/pilothouse.json`.

mod appearance;
pub mod layout;
pub mod motion;
pub mod preferences;
pub mod themes;
pub mod vscode;

pub use appearance::{Appearance, apply_appearance, init, system_is_dark};
pub use preferences::{
    Accent, ChatWidth, Corners, DarkTheme, Density, EditorPreferences, Gradient, Preferences,
    WindowStyle, dense, density,
};
