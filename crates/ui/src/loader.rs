//! PilotHouse's own "working" mark: a pennant of four bars that flutters,
//! after the pennant of the app's icon.
//!
//! It stands where a small spinner did. A 12 px ring went unnoticed beside
//! text; the pennant is a little wider, each bar in a colour of its own,
//! and its wave reads as motion at a glance without spinning.
//!
//! The colours are the pennant's own ([`HUES`]): the violet of the app's
//! mark at the mast, running to a hot pink at the fly. One family of hue,
//! so the mark reads as one thing and as this app's. (It was purple, blue,
//! cyan and green: four hues side by side, which read as the four-colour
//! marks of other products.) They keep clear of the colours that mean
//! something here: red and amber for what went wrong or waits, green for
//! what is done, the blue of what is at work.
//!
//! With motion reduced (the system's setting: *Reduce motion* on macOS,
//! *Animation effects* switched off on Windows, which many machines have)
//! nothing moves, but the mark must not stand still either: a still mark
//! reads as "stuck". The bars keep their shape and light up one after
//! another, a few times a second.

use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, Sizable, Size, h_flex};
use gpui_kit::{
    Animation, AnimationExt as _, App, Hsla, IntoElement, ParentElement as _, RenderOnce,
    Styled as _, Task, Window, div,
};

/// One flutter of the pennant.
const WAVE: Duration = Duration::from_millis(1100);

/// With motion reduced: how long each bar is the lit one.
const STEP: Duration = Duration::from_millis(380);

/// Each bar's height as a part of the mark's: tall at the mast, tapering to
/// the fly, as a pennant does.
const BARS: [f32; 4] = [1.0, 0.82, 0.62, 0.42];

/// Each bar's hue in degrees, from the mast to the fly: violet, purple,
/// fuchsia, hot pink.
const HUES: [f32; 4] = [262., 286., 310., 334.];

/// The pennant's colours on a dark or a light background: vivid, and as
/// light as reads on each.
fn pennant(dark: bool) -> [Hsla; 4] {
    let (saturation, lightness) = if dark { (0.95, 0.69) } else { (0.86, 0.55) };
    HUES.map(|hue| gpui_kit::hsla(hue / 360., saturation, lightness, 1.))
}

/// What the bars show at one moment.
#[derive(Clone, Copy)]
enum Beat {
    /// How far through its flutter the pennant is (0 to 1).
    Wave(f32),
    /// Motion is reduced: this bar is lit.
    Lit(usize),
}

/// Which bar is lit while motion is reduced, moved on by a slow timer (not
/// by frames: nothing here needs sixty redraws a second).
struct Still {
    lit: usize,
    _tick: Task<()>,
}

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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let height = self.height();
        // A colour asked for (on a filled button, say) is every bar's;
        // else each has the pennant's own.
        let colors = match self.color {
            Some(color) => [color; BARS.len()],
            None => pennant(cx.theme().is_dark()),
        };
        let bar_width = (height / 5.).max(2.);
        let bars = move |beat: Beat| {
            BARS.iter().enumerate().map(move |(ix, part)| {
                // A wave running from the mast to the fly: each bar swells
                // and settles a little after the one before it.
                let (swell, light) = match beat {
                    Beat::Wave(phase) => {
                        let turn = (phase - ix as f32 * 0.16) * std::f32::consts::TAU;
                        let swell = 0.62 + 0.38 * (0.5 + 0.5 * turn.sin());
                        (swell, 0.55 + 0.45 * swell.min(1.))
                    }
                    Beat::Lit(lit) if lit == ix => (1., 1.),
                    Beat::Lit(_) => (1., 0.4),
                };
                div()
                    .flex_none()
                    .w(crate::rems_from_px(bar_width))
                    .h(crate::rems_from_px((height * part * swell).max(2.)))
                    .rounded_full()
                    .bg(colors[ix].opacity(light))
            })
        };
        let mark = h_flex()
            .flex_none()
            .h(crate::rems_from_px(height))
            .gap(crate::rems_from_px((bar_width * 0.6).max(1.5)))
            .items_center();
        if cx.reduce_motion() {
            // Marks drawn in one place share the timer, and keep step.
            let still = window.use_keyed_state("pennant-still", cx, |_, cx| Still {
                lit: 0,
                _tick: cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(STEP).await;
                        let moved = this.update(cx, |still: &mut Still, cx| {
                            still.lit = (still.lit + 1) % BARS.len();
                            cx.notify();
                        });
                        if moved.is_err() {
                            break;
                        }
                    }
                }),
            });
            let lit = still.read(cx).lit;
            return mark.children(bars(Beat::Lit(lit))).into_any_element();
        }
        mark.with_animation(
            "pennant",
            Animation::new(WAVE).repeat(),
            move |mark, phase| mark.children(bars(Beat::Wave(phase))),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pennant_has_colours_of_its_own() {
        // One family of hue, from violet to pink, in order.
        assert!(HUES.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(HUES.iter().all(|hue| (250. ..=345.).contains(hue)));
        // Clear of what colours mean here: red and amber (wrong, waiting),
        // green (done), the blue of what is at work.
        let meant = [(345., 360.), (0., 70.), (80., 170.), (190., 245.)];
        for hue in HUES {
            assert!(
                !meant.iter().any(|(from, to)| (*from..=*to).contains(&hue)),
                "{hue} is a colour that means something"
            );
        }
        // Solid, and lighter on a dark background than on a light one.
        for (dark, light) in pennant(true).into_iter().zip(pennant(false)) {
            assert_eq!((dark.a, light.a), (1., 1.));
            assert!(dark.l > light.l);
        }
    }
}
