//! The native menu bar. Items dispatch the same actions as shortcuts and the
//! command palette, so all three stay in sync.

use gpui_kit::{App, Menu, MenuItem};
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
        MenuItem::action("Toggle Sidebar", ToggleSidebar),
        MenuItem::action("Toggle Panel", ToggleContextPanel),
        MenuItem::separator(),
    ];
    if enabled(Feature::Files) {
        items.push(MenuItem::action("Files", ShowFiles));
    }
    if enabled(Feature::Changes) {
        items.push(MenuItem::action("Changes", ShowChanges));
    }
    if enabled(Feature::Board) {
        items.push(MenuItem::action("Board", ShowBoard));
    }
    if enabled(Feature::Terminal) {
        items.push(MenuItem::action("Terminal", ShowTerminal));
    }
    items
}
