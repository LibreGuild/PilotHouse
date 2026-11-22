//! The native menu bar. Items dispatch the same actions as shortcuts and the
//! command palette, so all three stay in sync.

use gpui_kit::base::input;
use gpui_kit::{App, Menu, MenuItem, OsAction};
use ph_workspace::*;

pub fn install(cx: &mut App) {
    // Key bindings are registered before this call, so menus show shortcuts.
    cx.set_menus(vec![
        Menu::new(ph_brand::NAME).items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action(format!("Quit {}", ph_brand::NAME), Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open Folder…", AddProject),
            MenuItem::action("New Workspace", NewWorkspace),
            MenuItem::action("Add Folder to Workspace…", AddFolderToWorkspace),
            MenuItem::action("New Chat", NewSession),
        ]),
        // The system's own editing commands. In the app's fields its keys
        // act first, as before; a web page shown in a window (a deck of
        // slides) gets them only from here: without this menu ⌘C, ⌘V and
        // ⌘A do nothing in one.
        Menu::new("Edit").items([
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items(view_items()),
    ]);
}

/// The View menu: the tools listed are the ones this build has (the brand's
/// `[features]`).
fn view_items() -> Vec<MenuItem> {
    use ph_brand::{Feature, enabled};
    let mut items = vec![
        MenuItem::action("Command Palette", ToggleCommandPalette),
        MenuItem::separator(),
        MenuItem::action("Show or Hide the Chat", ToggleSidebar),
        MenuItem::action("Show or Hide the Result Pane", ToggleContextPanel),
        MenuItem::separator(),
    ];
    if enabled(Feature::Files) {
        items.push(MenuItem::action("Files", ShowFiles));
    }
    if enabled(Feature::Changes) {
        items.push(MenuItem::action("Changes", ShowChanges));
    }
    if enabled(Feature::Terminal) {
        items.push(MenuItem::action("Terminal", ShowTerminal));
    }
    items
}
