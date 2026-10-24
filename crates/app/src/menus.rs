//! The native menu bar. Items dispatch the same actions as shortcuts and the
//! command palette, so all three stay in sync.

use gpui_kit::{App, Menu, MenuItem};
use ph_workspace::*;

pub fn install(cx: &mut App) {
    // Key bindings are registered before this call, so menus show shortcuts.
    cx.set_menus(vec![
        Menu::new("PilotHouse").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit PilotHouse", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open Folder…", AddProject),
            MenuItem::action("New Workspace", NewWorkspace),
            MenuItem::action("Add Folder to Workspace…", AddFolderToWorkspace),
            MenuItem::action("New Chat", NewSession),
        ]),
        Menu::new("View").items([
            MenuItem::action("Command Palette", ToggleCommandPalette),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Toggle Panel", ToggleContextPanel),
            MenuItem::separator(),
            MenuItem::action("Files", ShowFiles),
            MenuItem::action("Changes", ShowChanges),
            MenuItem::action("Board", ShowBoard),
            MenuItem::action("Terminal", ShowTerminal),
        ]),
    ]);
}
