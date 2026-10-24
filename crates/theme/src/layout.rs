//! Pane geometry for the workspace shell.
//!
//! These are physical layout boundaries (minimum, default and maximum widths of
//! resizable regions), which the gpui-kit design guide asks every major region
//! to define. They are the only `px` values the shell uses.

use gpui_kit::{Pixels, px};

/// The window title bar: taller than gpui-component's 34 px so its
/// controls match the size of the rest of the shell.
pub const TITLE_BAR_HEIGHT: Pixels = px(40.);

/// Where macOS draws the window buttons, centred in [`TITLE_BAR_HEIGHT`].
pub const TRAFFIC_LIGHTS: (Pixels, Pixels) = (px(14.), px(13.));

/// Left rail: projects and sessions.
pub const SIDEBAR_WIDTH: Pixels = px(248.);

/// Right context panel (files, changes, board, terminal).
pub const CONTEXT_PANEL_DEFAULT: Pixels = px(560.);
pub const CONTEXT_PANEL_MIN: Pixels = px(320.);
pub const CONTEXT_PANEL_MAX: Pixels = px(1200.);

/// The chat's narrowest width.
pub const CHAT_MIN: Pixels = px(380.);

/// The editor column's narrowest width (beside the chat, while a file is
/// open).
pub const EDITOR_MIN: Pixels = px(360.);

/// Chat messages are capped at a comfortable reading width.
pub const CHAT_MAX_CONTENT: Pixels = px(860.);

/// Default window size on first launch.
pub const WINDOW_DEFAULT_WIDTH: Pixels = px(1440.);
pub const WINDOW_DEFAULT_HEIGHT: Pixels = px(900.);
pub const WINDOW_MIN_WIDTH: Pixels = px(960.);
pub const WINDOW_MIN_HEIGHT: Pixels = px(600.);
