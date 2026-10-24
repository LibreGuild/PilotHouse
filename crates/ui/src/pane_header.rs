use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div,
};

/// The slim header row at the top of a pane: a title on the leading edge and
/// compact actions on the trailing edge, separated from content by a hairline.
#[derive(IntoElement)]
pub struct PaneHeader {
    title: SharedString,
    subtitle: Option<SharedString>,
    actions: Vec<AnyElement>,
}

impl PaneHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            actions: Vec::new(),
        }
    }

    /// Muted text after the title, e.g. a count or a path.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl RenderOnce for PaneHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .flex_none()
            .h_10()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(div().text_sm().font_medium().truncate().child(self.title))
                    .when_some(self.subtitle, |this, subtitle| {
                        this.child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(cx.theme().muted_foreground)
                                .child(subtitle),
                        )
                    }),
            )
            .child(h_flex().flex_none().gap_1().children(self.actions))
    }
}
