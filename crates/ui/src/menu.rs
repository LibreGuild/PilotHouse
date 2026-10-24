//! Buttons that open a menu, with room between the two.
//!
//! gpui-component's `dropdown_menu_with_anchor` opens its menu about 3 px
//! from the button, so the menu's shadow lands on it. [`MenuButton::menu`]
//! is the same dropdown with [`POPUP_GAP`] between them, and every popover
//! in the app uses that gap too. Adapted from gpui-component's
//! `menu/dropdown_menu.rs` (Apache-2.0).

use std::rc::Rc;

use gpui_kit::component::Selectable;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::popover::Popover;
use gpui_kit::{
    Anchor, App, Context, DismissEvent, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, Pixels, RenderOnce, SharedString, Window, px,
};

/// The space between a button and the menu or popover it opens.
pub const POPUP_GAP: Pixels = px(8.);

type Builder = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// A button that opens a menu.
pub trait MenuButton: Selectable + InteractiveElement + IntoElement + 'static {
    /// Opens the menu `f` builds, its `anchor` corner at the button's
    /// matching corner (`BottomLeft` opens it upwards), [`POPUP_GAP`] away.
    fn menu(
        mut self,
        anchor: Anchor,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> SpacedMenu<Self> {
        let id = self.interactivity().element_id.clone().unwrap_or(0.into());
        SpacedMenu {
            id: SharedString::from(format!("menu:{id:?}")).into(),
            anchor,
            trigger: self,
            builder: Rc::new(f),
        }
    }
}

impl MenuButton for Button {}

#[derive(IntoElement)]
pub struct SpacedMenu<T: Selectable + IntoElement + 'static> {
    id: ElementId,
    anchor: Anchor,
    trigger: T,
    builder: Builder,
}

/// The menu, made when it opens and kept until it is dismissed (the
/// popover's content is asked for on every frame).
#[derive(Default)]
struct MenuState {
    menu: Option<Entity<PopupMenu>>,
}

impl<T: Selectable + IntoElement + 'static> RenderOnce for SpacedMenu<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let builder = self.builder.clone();
        let menu_state = window.use_keyed_state(self.id.clone(), cx, |_, _| MenuState::default());
        Popover::new(SharedString::from(format!("popover:{}", self.id)))
            .appearance(false)
            .overlay_closable(false)
            .offset(POPUP_GAP)
            .trigger(self.trigger)
            .anchor(self.anchor)
            .content(move |_, window, cx| {
                if let Some(menu) = menu_state.read(cx).menu.clone() {
                    return menu;
                }
                let builder = builder.clone();
                let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
                    builder(menu, window, cx)
                });
                menu_state.update(cx, |state, _| state.menu = Some(menu.clone()));
                menu.focus_handle(cx).focus(window, cx);
                // Weak handles: the listener lives as long as the menu, which
                // the state owns, so strong ones would leak both.
                let popover = cx.entity().downgrade();
                let state = menu_state.downgrade();
                window
                    .subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                        if let Some(popover) = popover.upgrade() {
                            popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        }
                        _ = state.update(cx, |state, _| state.menu = None);
                    })
                    .detach();
                menu
            })
    }
}
