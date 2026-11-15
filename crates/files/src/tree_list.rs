//! The files tree's list: gpui-component's `Tree` with a strip left free
//! under its rows. That wrapper fixes the inner list's style, so a tree
//! taller than the pane filled it to the last pixel and left nowhere to
//! right-click for the folder's own menu (new file, paste…) or to drop a
//! file on the top folder.

use std::rc::Rc;

use gpui_kit::base::{ScrollBounce, Tree};
use gpui_kit::component::{
    Selectable as _,
    list::ListItem,
    menu::{ContextMenuExt as _, PopupMenu},
    scroll::ScrollableElement as _,
    tree::{TreeEntry, TreeState},
};
use gpui_kit::{
    App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    StyleRefinement, Styled as _, Window, div, rems,
};

/// The strip kept free at the bottom of the pane, in rems (a row and a
/// half). The list's padding is taken off what it shows, not added after
/// its last row, so the strip is there wherever the tree is scrolled to.
const SPACE_BELOW: f32 = 2.5;

/// The tree of `state`, each row drawn by `row` and given `menu` on a
/// right-click.
pub(crate) fn tree_list(
    state: &Entity<TreeState>,
    row: impl Fn(usize, &TreeEntry, bool, &mut Window, &mut App) -> ListItem + 'static,
    menu: impl Fn(usize, &TreeEntry, PopupMenu, &mut Window, &mut Context<TreeState>) -> PopupMenu
    + 'static,
    cx: &App,
) -> impl IntoElement {
    let scroll = state.read(cx).scroll_handle().clone();
    let menu = Rc::new(menu);
    let rows = state.clone();
    div()
        .id(("files-tree", state.entity_id()))
        .size_full()
        .child(ScrollBounce::new(
            "files-tree-bounce",
            &scroll,
            Tree::new(state)
                .item(move |ix, entry, entry_state, window, cx| {
                    // Only the id is kept: cloning the entry would copy its
                    // whole subtree for every row on every frame.
                    let id = entry.item().id.clone();
                    let (menu, rows) = (menu.clone(), rows.clone());
                    let item = row(ix, entry, entry_state.is_selected(), window, cx)
                        .disabled(entry.is_disabled())
                        .selected(entry_state.is_selected())
                        .secondary_selected(entry_state.is_right_clicked());
                    div()
                        .child(item)
                        .context_menu(move |popup, window, cx| {
                            rows.update(cx, |state, cx| {
                                let entry = state
                                    .entry(ix)
                                    .filter(|entry| entry.item().id == id)
                                    .filter(|entry| !entry.is_disabled())
                                    .cloned();
                                match entry {
                                    Some(entry) => menu(ix, &entry, popup, window, cx),
                                    None => popup,
                                }
                            })
                        })
                        .into_any_element()
                })
                .list_style(
                    StyleRefinement::default()
                        .flex_grow_1()
                        .size_full()
                        .pb(rems(SPACE_BELOW)),
                )
                .relative()
                .size_full(),
        ))
        .vertical_scrollbar(&scroll)
}
