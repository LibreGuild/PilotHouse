//! A dialog showing generated text (a changelog, a PR description) to copy.

use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

pub fn open(title: &'static str, text: String, window: &mut Window, cx: &mut App) {
    let text: SharedString = text.into();
    window.open_dialog(cx, move |dialog, _, cx| {
        let copy = text.clone();
        dialog.title(title).w(px(760.)).child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .id("generated-text")
                        .max_h(px(520.))
                        .overflow_y_scroll()
                        .p_3()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().secondary)
                        .text_sm()
                        .child(text.clone()),
                )
                .child(
                    h_flex().justify_end().child(
                        ph_ui::CopyButton::new("copy-generated", copy.to_string())
                            .small()
                            .primary()
                            .label("Copy"),
                    ),
                ),
        )
    });
}
