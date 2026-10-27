//! PilotHouse design system.
//!
//! Owns the two product themes (`PilotHouse Light` and `PilotHouse Dark`), the
//! user's appearance preference, and the few layout tokens that are not part of
//! the gpui-component theme (pane widths, motion durations).
//!
//! Application code never uses raw colors: it reads semantic roles from
//! `cx.theme()` (gpui-component's `ActiveTheme`). The palette itself lives in
//! `themes/pilothouse.json`.

mod appearance;
pub mod layout;
pub mod motion;
pub mod preferences;

pub use appearance::{Appearance, apply_appearance, init};
pub use preferences::{
    ChatWidth, Corners, EditorPreferences, Preferences, WindowStyle, accent, accent_fill,
};
