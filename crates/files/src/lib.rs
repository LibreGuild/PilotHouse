//! Project files for PilotHouse.
//!
//! [`FilesPanel`] is the Files tab of the context panel: a lazily loaded file
//! tree, project text search, and open files in a code editor. The panel
//! watches the project folder, so edits made by agents or other tools show up
//! without a manual refresh (unsaved edits in the editor are never clobbered).
//!
//! The pure pieces are public for other features: [`fuzzy`] scores paths,
//! [`index_files`] lists a project's files (for `@file` mentions).

mod blame_view;
mod code_intel;
mod dir_tree;
mod file_index;
mod file_view;
mod files_panel;
pub mod fuzzy;
pub mod large;
mod quick_open;
pub mod roots;
mod search;
pub mod symbols;
mod watcher;

pub use file_index::{FileList, index_files};
pub use file_view::{Ask, AskIntent, FileView};
pub use files_panel::{FileTree, FilesPanel, FilesPanelEvent};
pub use watcher::{FilesChanged, ProjectWatcher, WatcherHub};

use gpui_kit::{App, KeyBinding, actions};

actions!(
    files,
    [
        /// Fuzzy-find a file and open it (⌘P).
        QuickOpen,
        /// Search the whole project for text (⌘⇧F).
        FindInProject,
        /// Save the file in the focused editor (⌘S).
        SaveFile,
        /// Close the active file tab (⌘W).
        CloseFile,
        /// Every use of the symbol at the cursor (⇧F12).
        FindReferences,
        /// Rename the symbol at the cursor across the project (F2).
        RenameSymbol,
        /// The file's symbols (⌘⇧O).
        ShowOutline,
        /// Send the selected code to the chat with a question (⌘L).
        AskAboutSelection,
        /// Ask the agent to explain the selected code.
        ExplainSelection,
        /// Ask the agent to look for bugs in the selected code.
        FindBugsInSelection,
        /// Ask the agent to write tests for the selected code.
        WriteTestsForSelection,
    ]
);

/// Registers the file actions' key bindings. `QuickOpen` and `FindInProject`
/// are bound in the workspace context so they work from anywhere.
pub fn init(cx: &mut App, workspace_context: &'static str) {
    cx.bind_keys([
        KeyBinding::new("secondary-p", QuickOpen, Some(workspace_context)),
        KeyBinding::new("secondary-shift-f", FindInProject, Some(workspace_context)),
        KeyBinding::new("secondary-s", SaveFile, Some(file_view::FILE_VIEW_CONTEXT)),
        KeyBinding::new(
            "shift-f12",
            FindReferences,
            Some(file_view::FILE_VIEW_CONTEXT),
        ),
        KeyBinding::new("f2", RenameSymbol, Some(file_view::FILE_VIEW_CONTEXT)),
        KeyBinding::new(
            "secondary-shift-o",
            ShowOutline,
            Some(file_view::FILE_VIEW_CONTEXT),
        ),
        KeyBinding::new(
            "secondary-l",
            AskAboutSelection,
            Some(file_view::FILE_VIEW_CONTEXT),
        ),
        KeyBinding::new(
            "secondary-w",
            CloseFile,
            Some(files_panel::FILES_PANEL_CONTEXT),
        ),
    ]);
}
