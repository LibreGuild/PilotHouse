//! "Plan with agent": ask for a goal, then let the agent turn it into cards.

use gpui_kit::component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Textarea, TextareaState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ParentElement as _, Styled as _, WeakEntity, Window, div, px,
};

use crate::BoardPanel;

pub(crate) fn open(board: WeakEntity<BoardPanel>, window: &mut Window, cx: &mut App) {
    let goal = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("What should be built? The more context, the better the plan.")
            .rows(6)
    });
    // The dialog takes focus as it mounts, so focus the field right after.
    let focus = goal.clone();
    window.defer(cx, move |window, cx| {
        focus.update(cx, |goal, cx| goal.focus(window, cx))
    });

    window.open_dialog(cx, move |dialog, _, _| {
        let send_goal = goal.clone();
        let board = board.clone();
        dialog
            .title("Plan with agent")
            .w(px(560.))
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child(
                        "The agent adds the plan to To do as small, checkable cards. Nothing is built yet.",
                    ))
                    .child(Textarea::new(&goal)),
            )
            .footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel-plan")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("send-plan")
                            .primary()
                            .label("Plan it")
                            .on_click(move |_, window, cx| {
                                let text = send_goal.read(cx).value().trim().to_string();
                                if text.is_empty() {
                                    return;
                                }
                                window.close_dialog(cx);
                                _ = board.update(cx, |board, cx| board.request_plan(&text, cx));
                            }),
                    ),
            )
    });
}
