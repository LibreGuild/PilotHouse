//! The picture on the card that offers a new board, at the head of the
//! Artifacts list: two notes, a shape and the arrow between them on a
//! ground of colour, drawn from the theme's own colours, so it sits in any
//! theme and needs no image file.

use gpui_kit::component::{ActiveTheme as _, Icon, IconName};
use gpui_kit::{
    AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, div, linear_color_stop,
    linear_gradient,
};

/// The card's picture: it fills the box it is put in, which is `width`
/// pixels wide at the default interface size and 16:10. Everything in it
/// is sized to that width.
pub fn card(width: f32, cx: &App) -> AnyElement {
    /// The width the measures below are written for.
    const DRAWN_FOR: f32 = 224.;
    let scale = width / DRAWN_FOR;
    let r = |px: f32| ph_ui::rems_from_px(px * scale);
    let theme = cx.theme();
    let (from, to) = (theme.green, theme.cyan);
    let (ink, sheet) = (theme.foreground, theme.background);
    let radius = theme.radius;
    // A note: a square of colour with two lines of words on it.
    let note = |left: f32, top: f32, color: gpui_kit::Hsla| {
        div()
            .absolute()
            .left(r(left))
            .top(r(top))
            .size(r(54.))
            .rounded(r(4.))
            .bg(color)
            .p(r(8.))
            .flex()
            .flex_col()
            .gap(r(5.))
            .child(
                div()
                    .w(r(30.))
                    .h(r(4.))
                    .rounded_full()
                    .bg(ink.opacity(0.55)),
            )
            .child(div().w(r(20.)).h(r(4.)).rounded_full().bg(ink.opacity(0.3)))
    };
    div()
        .relative()
        .size_full()
        .rounded(radius)
        .bg(linear_gradient(
            135.,
            linear_color_stop(from, 0.),
            linear_color_stop(to, 1.),
        ))
        .child(note(26., 24., theme.yellow_light))
        .child(note(46., 62., theme.magenta_light))
        // The line from the notes to the shape.
        .child(
            div()
                .absolute()
                .left(r(104.))
                .top(r(69.))
                .w(r(30.))
                .h(r(3.))
                .rounded_full()
                .bg(sheet.opacity(0.9)),
        )
        // A decision: a square on its corner.
        .child(
            div()
                .absolute()
                .left(r(138.))
                .top(r(42.))
                .size(r(56.))
                .rounded(r(28.))
                .bg(sheet)
                .flex()
                .items_center()
                .justify_center()
                .child(div().w(r(24.)).h(r(4.)).rounded_full().bg(ink.opacity(0.5))),
        )
        // What the card does: makes a new one.
        .child(
            div()
                .absolute()
                .right(r(10.))
                .bottom(r(10.))
                .size(r(24.))
                .rounded_full()
                .bg(ink)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Plus).size(r(13.)).text_color(sheet)),
        )
        .into_any_element()
}
