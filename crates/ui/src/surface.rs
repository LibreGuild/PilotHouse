use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{AnyElement, App, IntoElement, ParentElement, RenderOnce, Styled as _, Window, div};

/// A floating panel: rounded, hairline-bordered, clipped to its corners.
///
/// The workspace places its main regions (chat, context panel) on the app
/// background as surfaces, which gives the window its layered look.
#[derive(IntoElement, Default)]
pub struct Surface {
    children: Vec<AnyElement>,
}

impl Surface {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ParentElement for Surface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Surface {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_xs()
            .children(self.children)
    }
}
