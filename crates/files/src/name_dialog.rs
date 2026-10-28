//! The dialog that asks for a file or folder name (new, rename). A name
//! that will not do keeps it open with the reason under the field.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, ParentElement as _, SharedString, Styled as _, Window, div, px,
};

/// What the dialog calls with the name; an error keeps the dialog open and
/// is shown under the field.
pub type OnName = Rc<dyn Fn(String, &mut Window, &mut App) -> Result<(), String>>;

/// Asks for a name. `initial` starts the field; `select_stem` selects the
/// name without its extension (renaming `main.rs` selects `main`).
pub fn ask_name(
    title: impl Into<SharedString>,
    ok: &'static str,
    initial: String,
    select_stem: bool,
    window: &mut Window,
    cx: &mut App,
    done: OnName,
) {
    let stem = match initial.rfind('.') {
        Some(dot) if select_stem && dot > 0 => dot,
        _ => initial.len(),
    };
    let input = cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_value(initial, window, cx);
        input.set_selected_range(0..stem, cx);
        input
    });
    let error: Rc<RefCell<Option<SharedString>>> = Rc::default();
    let title = title.into();
    let focus = input.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let (input, done, error) = (input.clone(), done.clone(), error.clone());
        let shown = error.borrow().clone();
        dialog
            .title(title.clone())
            .w(px(440.))
            .child(
                v_flex()
                    .gap_1p5()
                    .child(Input::new(&input))
                    .when_some(shown, |this, shown| {
                        this.child(div().text_xs().text_color(cx.theme().danger).child(shown))
                    }),
            )
            .footer(ph_ui::dialog_buttons(ok, true))
            .on_ok(move |_, window, cx| {
                let name = input.read(cx).value().trim().to_string();
                match done(name, window, cx) {
                    Ok(()) => true,
                    Err(reason) => {
                        *error.borrow_mut() = Some(reason.into());
                        window.refresh();
                        false
                    }
                }
            })
    });
    focus.update(cx, |input, cx| input.focus(window, cx));
}
