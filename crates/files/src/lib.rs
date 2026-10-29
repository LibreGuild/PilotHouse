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
pub mod diff_tab;
mod dir_tree;
mod file_index;
pub mod file_ops;
mod file_view;
mod files_panel;
pub mod fuzzy;
pub mod large;
mod name_dialog;
mod quick_open;
pub mod roots;
mod search;
pub mod symbols;
mod tree_menu;
pub mod unsaved;
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
        /// Open the file selected in the tree (Enter; arrows move and fold).
        OpenSelected,
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
        /// A new file in the selected folder of the tree.
        NewFile,
        /// A new folder in the selected folder of the tree.
        NewFolder,
        /// Rename the selected file or folder (F2).
        RenameEntry,
        /// Move the selected file or folder to the trash.
        TrashEntry,
        /// Pick the selected file or folder to paste elsewhere.
        CopyEntry,
        /// Pick the selected file or folder to move elsewhere.
        CutEntry,
        /// Paste what was copied or cut into the selected folder.
        PasteEntry,
        /// Copy the selected file or folder beside itself.
        DuplicateEntry,
        /// Show the selected file or folder in the system's file manager.
        RevealEntry,
        /// Copy the selected file's full path.
        CopyEntryPath,
        /// Copy the selected file's path within its folder.
        CopyRelativePath,
        /// Copy the selected code fenced as Markdown, with where it is from.
        CopyAsMarkdown,
        /// Copy where the selected lines are (`src/app.rs:10-14`).
        CopyReference,
        /// Copy a link to the selected lines on the repository's host.
        CopyLinkToLines,
        /// Copy the open file's full path.
        CopyFilePath,
        /// Copy the open file's path within its folder.
        CopyFileRelativePath,
        /// Show the open file in the system's file manager.
        RevealFile,
        /// Select the open file in the Files tree.
        RevealInFiles,
    ]
);

/// The key context of the Files tree, where its keys act on the selection.
pub const FILE_TREE_CONTEXT: &str = "FileTree";

/// Registers the file actions' key bindings. `QuickOpen` and `FindInProject`
/// are bound in the workspace context so they work from anywhere.
pub fn init(cx: &mut App, workspace_context: &'static str) {
    ph_preview::rich::init(cx);
    cx.bind_keys([
        KeyBinding::new("secondary-p", QuickOpen, Some(workspace_context)),
        KeyBinding::new("enter", OpenSelected, Some("Tree")),
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
    let tree = Some(FILE_TREE_CONTEXT);
    let trash = if cfg!(target_os = "macos") {
        "secondary-backspace"
    } else {
        "delete"
    };
    cx.bind_keys([
        KeyBinding::new("f2", RenameEntry, tree),
        KeyBinding::new(trash, TrashEntry, tree),
        KeyBinding::new("secondary-c", CopyEntry, tree),
        KeyBinding::new("secondary-x", CutEntry, tree),
        KeyBinding::new("secondary-v", PasteEntry, tree),
        KeyBinding::new("secondary-d", DuplicateEntry, tree),
        KeyBinding::new("alt-secondary-r", RevealEntry, tree),
        KeyBinding::new("alt-secondary-c", CopyEntryPath, tree),
        KeyBinding::new("alt-shift-secondary-c", CopyRelativePath, tree),
    ]);
}
