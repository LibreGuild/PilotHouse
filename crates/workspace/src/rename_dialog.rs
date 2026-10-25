//! A small dialog asking for a new name.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{App, AppContext as _, ParentElement as _, Window, px};

/// Asks for a name, starting from `current`; `done` gets it on OK (or
/// Enter, which presses OK).
pub fn open(
    title: &'static str,
    current: String,
    window: &mut Window,
    cx: &mut App,
    done: impl Fn(String, &mut Window, &mut App) + 'static,
) {
    let input = cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_value(current, window, cx);
        input
    });
    let done = std::rc::Rc::new(done);
    let focus = input.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let (input, done) = (input.clone(), done.clone());
        dialog
            .title(title)
            .w(px(420.))
            .child(Input::new(&input))
            .footer(ph_ui::dialog_buttons("Rename", true))
            .on_ok(move |_, window, cx| {
                let name = input.read(cx).value().to_string();
                done(name, window, cx);
                true
            })
    });
    focus.update(cx, |input, cx| input.focus(window, cx));
}
