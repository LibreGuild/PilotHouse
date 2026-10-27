//! Every pending agent question, across workspaces, for the inbox.

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{AnyElement, App, IntoElement, ParentElement as _, Styled as _, div};

use crate::entry_view::{form_answers, permission_buttons};
use crate::permission::ApprovalInbox;

/// How many agent questions wait for an answer.
pub fn pending_count(cx: &mut App) -> usize {
    let inbox = ApprovalInbox::global(cx);
    inbox.read(cx).pending(cx).len()
}

/// One row per waiting question: where it came from, what the agent wants,
/// and the buttons to answer.
pub fn rows(cx: &mut App) -> Vec<AnyElement> {
    let inbox = ApprovalInbox::global(cx);
    let pending = inbox.read(cx).pending(cx);
    pending
        .iter()
        .map(|handle| {
            let request = handle.read(cx);
            v_flex()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(request.source.clone()),
                )
                .child(div().text_sm().child(request.title.clone()))
                .child(match request.form() {
                    Some(form) => form_answers(handle, form, cx),
                    None => permission_buttons(handle, request),
                })
                .into_any_element()
        })
        .collect()
}
