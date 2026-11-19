//! Lengths that follow the interface size.
//!
//! The interface size setting is the window's rem (16 by default), and the
//! whole interface grows and shrinks with it, as a web page does when it is
//! zoomed: text, rows, icons, paddings, dialogs. A length in `px` stays as it
//! is at every size, so views use the rem helpers (`h_7()`, `gap_2()`) and,
//! for a size off that scale, the two functions here. `px` is left for what
//! must not scale: hairlines and strokes, a window's own size and the limits
//! of its panes, and marks drawn on a picture.

use gpui_kit::{Pixels, Rems, Window, rems};

/// The interface size the design is drawn at.
const BASE: f32 = 16.;

/// A length that is `px` pixels at the default interface size and follows
/// the setting from there.
pub const fn rems_from_px(px: f32) -> Rems {
    rems(px / BASE)
}

/// [`rems_from_px`] in this window's pixels, for what only takes pixels: a
/// dialog's width, arithmetic against the viewport.
pub fn scaled_px(px: f32, window: &Window) -> Pixels {
    rems_from_px(px).to_pixels(window.rem_size())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_design_pixel_is_a_sixteenth_of_the_rem() {
        assert_eq!(rems_from_px(14.), rems(0.875));
        assert_eq!(rems_from_px(560.), rems(35.));
    }
}
