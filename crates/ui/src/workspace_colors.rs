//! The colours workspaces can wear: a small mark beside the workspace's name,
//! so lists say at a glance which one a row belongs to. Hues keep clear of the
//! status colours (red, amber, green) and read on light and dark backgrounds.

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
