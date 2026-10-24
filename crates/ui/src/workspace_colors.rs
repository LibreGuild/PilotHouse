//! The colours workspaces can wear, so the title bar says at a glance which
//! one is in front. Hues keep clear of the status colours (red, amber, green)
//! and read on light and dark backgrounds.

use gpui_kit::{Hsla, hsla};

/// Each colour's name and hue (degrees).
pub const WORKSPACE_COLORS: [(&str, f32); 10] = [
    ("Blue", 215.),
    ("Violet", 268.),
    ("Teal", 172.),
    ("Rose", 336.),
    ("Orange", 24.),
    ("Cyan", 192.),
    ("Magenta", 300.),
    ("Sea green", 150.),
    ("Gold", 50.),
    ("Indigo", 242.),
];

/// Colour `index` (wrapped into the palette) as a solid dot.
pub fn workspace_color(index: usize, dark: bool) -> Hsla {
    let (_, hue) = WORKSPACE_COLORS[index % WORKSPACE_COLORS.len()];
    let lightness = if dark { 0.62 } else { 0.48 };
    hsla(hue / 360., 0.58, lightness, 1.)
}

/// Colour `index` as a faint wash over the title bar.
pub fn workspace_tint(index: usize, dark: bool) -> Hsla {
    workspace_color(index, dark).opacity(if dark { 0.10 } else { 0.08 })
}
