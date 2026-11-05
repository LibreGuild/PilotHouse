//! A one-line label cut short with an ellipsis when it does not fit, whose
//! full text shows in a tooltip once the pointer rests on it: only when it
//! was cut, so whole names never get a tooltip repeating them.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{
    AppContext as _, ElementId, EmptyView, InteractiveElement as _, ParentElement as _, Pixels,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, TextStyle, canvas, div,
    px,
};

/// How long the pointer rests before the full text shows.
const DELAY: Duration = Duration::from_millis(1200);

/// `text` on one line, truncated with an ellipsis; resting on it shows the
/// whole of it when it was cut. Style it like any `div` (colour, size).
pub fn truncated_label(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
) -> Stateful<gpui_kit::Div> {
    let text: SharedString = text.into();
    // The width it had and the text style it was drawn with, last frame.
    let fit: Rc<RefCell<Option<(Pixels, TextStyle)>>> = Rc::default();
    let measure = fit.clone();
    let full = text.clone();
    div()
        .id(id)
        .relative()
        .min_w_0()
        .truncate()
        .child(text)
        .child(
            canvas(
                move |bounds, window, _| {
                    *measure.borrow_mut() = Some((bounds.size.width, window.text_style()));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .tooltip(move |window, cx| {
            let cut = fit.borrow().as_ref().is_some_and(|(width, style)| {
                let size = style.font_size.to_pixels(window.rem_size());
                let line = window.text_system().shape_line(
                    full.clone(),
                    size,
                    &[style.to_run(full.len())],
                    None,
                );
                line.width > *width + px(0.5)
            });
            if cut {
                Tooltip::new(full.clone()).build(window, cx)
            } else {
                cx.new(|_| EmptyView).into()
            }
        })
        .tooltip_show_delay(DELAY)
}
