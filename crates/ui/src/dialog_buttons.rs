//! The Cancel and OK buttons of a form dialog.
//!
//! A plain `Dialog` draws no buttons of its own (only an `AlertDialog`
//! does); its `on_ok` runs on Enter. These buttons go in its footer: OK runs
//! the same `on_ok`, so a click and Enter do the same thing.

use gpui_kit::component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogFooter},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{IntoElement, ParentElement as _, SharedString, Styled as _};

/// Cancel (closes the dialog), when `cancel`, and `ok` (runs its `on_ok`).
pub fn dialog_buttons(ok: impl Into<SharedString>, cancel: bool) -> impl IntoElement {
    DialogFooter::new()
        .w_full()
        .child(gpui_kit::div().flex_1())
        .when(cancel, |footer| {
            footer.child(
                Button::new("dialog-cancel")
                    .ghost()
                    .label("Cancel")
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
        })
        // The action fills its parent, so the parent is sized to the button.
        .child(
            gpui_kit::div().flex_none().child(
                DialogAction::new().child(Button::new("dialog-ok").primary().label(ok.into())),
            ),
        )
}
