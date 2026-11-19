//! PilotHouse design system.
//!
//! Owns the themes (PilotHouse's own, and the user's own: [`themes`]), the
//! user's choice of one, the fonts the app carries ([`fonts`]), and the few
//! layout tokens that are not part of the gpui-component theme (pane widths).
//!
//! Application code never uses raw colors: it reads semantic roles from
//! `cx.theme()` (gpui-component's `ActiveTheme`). PilotHouse's palettes live
//! in `themes/pilothouse.json`.

mod appearance;
pub mod fonts;
pub mod layout;
pub mod preferences;
pub mod themes;
pub mod vscode;

pub use appearance::{Appearance, apply_appearance, init, system_is_dark};
pub use preferences::{
    Accent, AutoSave, ChatWidth, Corners, DarkTheme, Density, EditorPreferences, Preferences,
    dense, density,
};
