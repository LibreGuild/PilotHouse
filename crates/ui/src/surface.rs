use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Hsla, IntoElement, ParentElement, Pixels, RenderOnce, Styled as _, Window,
    div, px,
};

/// A floating panel: rounded, hairline-bordered, clipped to its corners.
///
/// The workspace places its main regions (the list, the panes) on the
/// window's frame as surfaces, which gives the window its layered look.
#[derive(IntoElement, Default)]
pub struct Surface {
    children: Vec<AnyElement>,
    behind: Option<(Hsla, Hsla)>,
}

impl Surface {
    pub fn new() -> Self {
        Self::default()
    }

    /// What the window shows behind the panel's top corners and behind its
    /// bottom ones. Children are clipped to the panel's rectangle, not to
    /// its rounding (the UI framework has no rounded clip), so one that
    /// fills the panel (a tab bar, a terminal) pokes out of the corners
    /// wherever the window behind is another colour. Given these, the
    /// corners are painted over with them.
    pub fn behind(mut self, top: Hsla, bottom: Hsla) -> Self {
        self.behind = Some((top, bottom));
        self
    }
}

impl ParentElement for Surface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Surface {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let radius = cx.theme().radius_lg;
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_xs()
            .children(self.children)
            // Last, over the children; the panel's border is drawn after
            // them and covers the seam.
            .when_some(self.behind, |this, (top, bottom)| {
                this.child(corners(radius, true, top))
                    .child(corners(radius, false, bottom))
            })
    }
}

/// The panel's two top (or bottom) corners, outside its rounding, in
/// `color`: a thick frame drawn around the panel whose hole is the panel's
/// own shape. Only its corners reach inside the panel, and the strip they
/// are drawn in keeps one pair of them. One quad a pair, no path.
fn corners(radius: Pixels, top: bool, color: Hsla) -> impl IntoElement {
    // How far the frame reaches beyond the panel; any width does.
    const OUT: Pixels = px(8.);
    // The panel's own border: its children are laid out inside it.
    const BORDER: Pixels = px(1.);
    let reach = OUT + BORDER;
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(radius)
        .overflow_hidden()
        .map(|this| if top { this.top_0() } else { this.bottom_0() })
        .child(
            div()
                .absolute()
                .left(-reach)
                .right(-reach)
                // Tall enough for both of its rounded ends.
                .h((radius + reach) * 2.)
                .map(|this| {
                    if top {
                        this.top(-reach)
                    } else {
                        this.bottom(-reach)
                    }
                })
                .rounded(radius + OUT)
                // Half a pixel into the panel's border, which hides the
                // edge.
                .border(OUT + px(0.5))
                .border_color(color),
        )
}
