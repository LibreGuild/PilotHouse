//! The picture on the card that offers a new deck, at the head of the
//! Artifacts list: two slides on a ground of colour, drawn from the theme's
//! own colours, so it sits in any theme and needs no image file.

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, div, linear_color_stop,
    linear_gradient,
};

/// The card's picture: it fills the box it is put in, which is `width`
/// pixels wide at the default interface size and 16:10, as the pictures of
/// the artifacts under it. Everything in it is sized to that width.
pub fn card(width: f32, cx: &App) -> AnyElement {
    /// The width the measures below are written for.
    const DRAWN_FOR: f32 = 224.;
    let scale = width / DRAWN_FOR;
    let r = |px: f32| ph_ui::rems_from_px(px * scale);
    let theme = cx.theme();
    let (from, to, glow) = (theme.magenta, theme.blue, theme.cyan_light);
    let (sheet, ink, faint) = (theme.background, theme.foreground, theme.muted_foreground);
    // A line of words on a slide.
    let line =
        |width: f32, height: f32, color| div().w(r(width)).h(r(height)).rounded_full().bg(color);
    let bar = |height: f32, color| div().w(r(9.)).h(r(height)).rounded(r(2.)).bg(color);
    // Rounded themselves: a frame's corners do not cut what is in it.
    let radius = theme.radius;
    div()
        .relative()
        .size_full()
        .rounded(radius)
        .bg(linear_gradient(
            135.,
            linear_color_stop(from, 0.),
            linear_color_stop(to, 1.),
        ))
        // A second colour coming in from a corner, so the ground is not
        // one flat sweep.
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .rounded(radius)
                .bg(linear_gradient(
                    45.,
                    linear_color_stop(glow.opacity(0.), 0.35),
                    linear_color_stop(glow.opacity(0.6), 1.),
                )),
        )
        // The slide behind.
        .child(
            div()
                .absolute()
                .left(r(74.))
                .top(r(20.))
                .w(r(122.))
                .h(r(76.))
                .rounded(r(7.))
                .bg(sheet.opacity(0.5)),
        )
        // The slide in front: a heading, a few lines, a small chart.
        .child(
            v_flex()
                .absolute()
                .left(r(34.))
                .top(r(36.))
                .w(r(136.))
                .h(r(85.))
                .p(r(11.))
                .rounded(r(7.))
                .bg(sheet)
                .shadow_lg()
                .child(line(26., 3., from))
                .child(div().h(r(6.)))
                .child(line(84., 7., ink.opacity(0.88)))
                .child(div().h(r(10.)))
                .child(
                    h_flex()
                        .items_end()
                        .justify_between()
                        .child(
                            v_flex()
                                .gap(r(5.))
                                .child(line(50., 3., faint.opacity(0.6)))
                                .child(line(42., 3., faint.opacity(0.6)))
                                .child(line(32., 3., faint.opacity(0.6))),
                        )
                        .child(
                            h_flex()
                                .items_end()
                                .gap(r(3.))
                                .child(bar(13., from))
                                .child(bar(25., to))
                                .child(bar(18., theme.cyan)),
                        ),
                ),
        )
        // What the card does: one more.
        .child(
            div()
                .absolute()
                .right(r(12.))
                .bottom(r(12.))
                .size(r(28.))
                .rounded_full()
                .bg(sheet)
                .shadow_md()
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Plus).size(r(15.)).text_color(ink)),
        )
        .into_any_element()
}
