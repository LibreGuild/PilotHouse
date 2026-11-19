//! PilotHouse's own "working" mark: a pennant of four bars that flutters,
//! after the pennant of the app's icon.
//!
//! It stands where a small spinner did. A 12 px ring went unnoticed beside
//! text; the pennant is a little wider, in the accent colour, and its wave
//! reads as motion at a glance without spinning. With motion reduced (the
//! system's setting) it stands still.

use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, Sizable, Size, h_flex};
use gpui_kit::{
    Animation, AnimationExt as _, App, Hsla, IntoElement, ParentElement as _, RenderOnce,
    Styled as _, Window, div,
};

/// One flutter of the pennant.
const WAVE: Duration = Duration::from_millis(1100);

/// Each bar's height as a part of the mark's: tall at the mast, tapering to
/// the fly, as a pennant does.
const BARS: [f32; 4] = [1.0, 0.82, 0.62, 0.42];

/// The fluttering pennant; `xsmall()` beside captions, `small()` beside
/// row text, as it is (medium) over a label or in a button, `large()`
/// alone in an empty pane.
#[derive(IntoElement)]
pub struct Loader {
    size: Size,
    color: Option<Hsla>,
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

impl Loader {
    pub fn new() -> Self {
        Self {
            size: Size::Medium,
            color: None,
        }
    }

    /// Its colour, instead of the accent.
    pub fn color(mut self, color: impl Into<Hsla>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// The mark's height in pixels at the default interface size.
    fn height(&self) -> f32 {
        match self.size {
            Size::XSmall => 10.,
            Size::Small => 12.,
            Size::Medium => 16.,
            Size::Large => 24.,
            Size::Size(pixels) => f32::from(pixels),
        }
    }
}

impl Sizable for Loader {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl RenderOnce for Loader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let height = self.height();
        let color = self.color.unwrap_or(cx.theme().info);
        let bar_width = (height / 5.).max(2.);
        let bars = move |phase: Option<f32>| {
            BARS.iter().enumerate().map(move |(ix, part)| {
                // A wave running from the mast to the fly: each bar swells
                // and settles a little after the one before it.
                let swell = phase.map_or(1., |phase| {
                    let turn = (phase - ix as f32 * 0.16) * std::f32::consts::TAU;
                    0.62 + 0.38 * (0.5 + 0.5 * turn.sin())
                });
                div()
                    .flex_none()
                    .w(crate::rems_from_px(bar_width))
                    .h(crate::rems_from_px((height * part * swell).max(2.)))
                    .rounded_full()
                    .bg(color.opacity(0.55 + 0.45 * swell.min(1.)))
            })
        };
        let mark = h_flex()
            .flex_none()
            .h(crate::rems_from_px(height))
            .gap(crate::rems_from_px((bar_width * 0.6).max(1.5)))
            .items_center();
        if cx.reduce_motion() {
            return mark.children(bars(None)).into_any_element();
        }
        mark.with_animation(
            "pennant",
            Animation::new(WAVE).repeat(),
            move |mark, phase| mark.children(bars(Some(phase))),
        )
        .into_any_element()
    }
}
