use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div,
};

/// A centered "nothing here yet" message with an optional call to action.
///
/// Used for empty panels: no project, no files open, empty board.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: Option<Icon>,
    title: SharedString,
    description: Option<SharedString>,
    action: Option<AnyElement>,
}

impl EmptyState {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            title: title.into(),
            description: None,
            action: None,
        }
    }

    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .when_some(self.icon, |this, icon| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size_10()
                        .rounded(cx.theme().radius_lg)
                        .bg(cx.theme().muted)
                        .text_color(cx.theme().muted_foreground)
                        .child(icon.size_5()),
                )
            })
            .child(
                // Full width up to the cap, so a long message (an error
                // with a path in it) wraps instead of running out of view.
                v_flex()
                    .items_center()
                    .gap_1()
                    .w_full()
                    .max_w_80()
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_base()
                            .font_medium()
                            .child(self.title),
                    )
                    .when_some(self.description, |this, description| {
                        this.child(
                            div()
                                .w_full()
                                .text_sm()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(description),
                        )
                    }),
            )
            .when_some(self.action, |this, action| {
                this.child(div().pt_1().child(action))
            })
    }
}
