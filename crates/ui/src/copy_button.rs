//! A Copy button that says it worked: its icon turns into a tick (and its
//! label into "Copied") for a moment after a click.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, IconName, Sizable, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, ClipboardItem, ElementId, IntoElement, RenderOnce, SharedString, Window};

/// How long the tick shows.
const SHOWN: Duration = Duration::from_millis(1500);

type Value = Rc<dyn Fn(&mut Window, &mut App) -> String>;

#[derive(IntoElement)]
pub struct CopyButton {
    id: ElementId,
    value: Value,
    label: Option<SharedString>,
    tooltip: Option<SharedString>,
    size: Size,
    primary: bool,
    disabled: bool,
}

impl CopyButton {
    /// Copies `value` when clicked.
    pub fn new(id: impl Into<ElementId>, value: impl Into<String>) -> Self {
        let value: String = value.into();
        Self::with(id, move |_, _| value.clone())
    }

    /// Copies what `value` gives at the click.
    pub fn with(
        id: impl Into<ElementId>,
        value: impl Fn(&mut Window, &mut App) -> String + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            value: Rc::new(value),
            label: None,
            tooltip: None,
            size: Size::XSmall,
            primary: false,
            disabled: false,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The hint; also the name a screen reader reads for an icon alone.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Sizable for CopyButton {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl RenderOnce for CopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let copied = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let done = *copied.read(cx);
        let value = self.value;
        let button = Button::new(self.id)
            .with_size(self.size)
            .icon(if done {
                IconName::Check
            } else {
                IconName::Copy
            })
            .disabled(self.disabled)
            .when_some(self.label, |this, label| {
                this.label(if done { "Copied".into() } else { label })
            })
            .when_some(self.tooltip, |this, tooltip| {
                this.accessibility_label(tooltip.clone()).tooltip(tooltip)
            })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                let text = value(window, cx);
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                copied.update(cx, |copied, cx| {
                    *copied = true;
                    cx.notify();
                });
                let copied = copied.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(SHOWN).await;
                    copied.update(cx, |copied, cx| {
                        *copied = false;
                        cx.notify();
                    });
                })
                .detach();
            });
        if self.primary {
            button.primary()
        } else {
            button.ghost()
        }
    }
}

/// Copies `text` and says so in a short toast, for copies from menus and
/// keys (a [`CopyButton`] shows its own tick).
pub fn copy_with_toast(text: String, what: &'static str, cx: &mut App) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::notification::Notification;
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    // Later: the window is busy with the click that copied.
    cx.defer(move |cx| {
        if let Some(window) = cx.active_window() {
            _ = window.update(cx, |_, window, cx| {
                window.push_notification(Notification::success(format!("Copied {what}")), cx)
            });
        }
    });
}
