//! How wide a piece of the interface is, so it can lay itself out for the
//! room it has: a tool beside the chat is a narrow column on a small window
//! and a wide pane on a large one, and GPUI has no container queries.
//!
//! [`width_of`] gives the width an element had when it was last drawn and a
//! probe to put inside it; the view is drawn again when the width changes.
//! Widths are compared in pixels at the default interface size
//! ([`design_px`]), so a breakpoint means the same at every interface size.

use gpui_kit::{AnyElement, App, ElementId, IntoElement as _, Pixels, Styled as _, Window, canvas};

/// The width element `id` had when it was last drawn (`None` before its
/// first frame), and the probe that measures it: add the probe as a child
/// of the element, which must be positioned (`.relative()`).
pub fn width_of(
    id: impl Into<ElementId>,
    window: &mut Window,
    cx: &mut App,
) -> (Option<Pixels>, AnyElement) {
    let state = window.use_keyed_state(id, cx, |_, _| None::<Pixels>);
    let known = *state.read(cx);
    let probe = canvas(
        move |bounds, _, cx| {
            // Whole pixels: a width that wobbles by a fraction while a pane
            // is dragged must not draw the view again for nothing.
            let width = bounds.size.width.round();
            if *state.read(cx) != Some(width) {
                state.update(cx, |known, cx| {
                    *known = Some(width);
                    cx.notify();
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .into_any_element();
    (known, probe)
}

/// `width` in pixels at the default interface size: what breakpoints are
/// written in.
pub fn design_px(width: Pixels, window: &Window) -> f32 {
    f32::from(width) / f32::from(window.rem_size()) * 16.
}

/// Whether element `id` was narrower than `below` (pixels at the default
/// interface size) when it was last drawn, and its probe (see [`width_of`]).
/// Not narrow until it has been measured: the first frame is drawn in full.
pub fn narrower_than(
    id: impl Into<ElementId>,
    below: f32,
    window: &mut Window,
    cx: &mut App,
) -> (bool, AnyElement) {
    let (width, probe) = width_of(id, window, cx);
    let narrow = width.is_some_and(|width| design_px(width, window) < below);
    (narrow, probe)
}
