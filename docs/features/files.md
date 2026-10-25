# Files

Crate: `ph-files`.

- **Where**: the tree is the Files tool on the right (⌘1, `FileTree`); open files show in the editor column beside the chat (`FilesPanel`). One model serves both.
- **Tree** (`dir_tree.rs`, `files_panel.rs`): directories are listed on first expand, with ignored paths (`.gitignore`) and `.git` hidden. An unlisted folder carries a placeholder child so it stays expandable. Rows have chevrons and indent guides, an icon per file and folder type (`ph_ui::file_icon_element`: Material Icon Theme SVGs, MIT, bundled in `crates/ui/assets/file-icons`), and Git marks from each folder's status (**M**, **U**, **A**, **D**, **R**, **!**; a dot on folders holding changes).
- **Editor** (`file_view.rs`): the gpui-component code editor with syntect highlighting (about 200 languages), chosen by file extension. ⌘S saves, ⌘F finds and replaces in the file. Unsaved files show `●` in their tab. Binary files open as a notice.
- **Large files** (`large/`): text files over 4 MB open in a read-only viewer that handles many gigabytes. It reads with positioned reads (no memory map, so a file truncated underneath cannot crash the app) and keeps only where every 1024th line starts, built in the background (a 1 GB, 10-million-line log indexes in a few seconds; scrolling works while it counts). Only the lines on screen are read. Find (smart case, both ways, across chunk seams), go to line, click/⇧-click to select lines and ⌘C to copy them (up to 32 MB), sideways scroll for long lines (cut at 4 KB), and *Follow* to stay at the end of a growing log.
- **⌘P** (`quick_open.rs`, `fuzzy.rs`): fuzzy file finder over a background file index (`file_index.rs`, capped at 200k files). Scoring favours consecutive matches, word starts and the file name.
- **⌘⇧F** (`search.rs`): plain-text, smart-case project search with ripgrep's crates. Stops at 500 hits.
- **Watching** (`watcher.rs`): one `ProjectWatcher` per folder, shared through `WatcherHub` by every view and the schedules, debounced to 150 ms. The tree refreshes the folders it shows. Open files reload when changed on disk, unless they have unsaved edits (yours always win).

- **Blame** (`blame_view.rs`): the toolbar's Blame toggle shows who last changed each line, with an age bar; the footer shows the cursor line's commit.
- **Ask about code** (`file_view.rs`, `Ask`): the toolbar's **Ask** menu, the editor's right-click menu, or ⌘L sends the selection (or the cursor's line) to the chat as an attachment. *Explain*, *Find bugs* and *Write tests* send a prompt that asks the agent to look up callers and callees first, and that turn may read and search without asking, so answers come fast. *Ask…* (⌘L) attaches the code and focuses the composer. A busy chat hands the question to a new chat.

Open tabs and the active file are restored per workspace.

## Language servers

Crate: `ph-lsp`. PilotHouse starts a server when a file it handles is opened, if one is installed on `PATH` (`servers.rs`: rust-analyzer, typescript-language-server, Pyright or pylsp, gopls, clangd, and others). Nothing is downloaded. One server per language and project, kept while the project is open (a restart would re-index), then stopped.

- **Transport** (`transport.rs`, `client.rs`): JSON-RPC over stdio with `Content-Length` framing, one ordered outbox per server. After `initialized` the client sends `workspace/didChangeConfiguration` (Pyright waits for it before checking files). Edits are sent as full-document `didChange`, debounced.
- **In the editor** (`providers.rs`, `file_view.rs`): wavy underlines for errors, warnings and hints; hover; go to definition (F12 or ⌘-click, across files); completion. The footer shows error and warning counts and the server's state.
- **Code intelligence** (`code_intel.rs`): Find references (⇧F12, toolbar), Rename symbol across the project (F2, edits applied to open and closed files), and the file Outline (⌘⇧O, toolbar).
