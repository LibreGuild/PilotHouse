# Terminal

Crate: `ph-terminal`.

- Each tab is a shell in the workspace's primary folder (its scratch folder when it has none), on a real PTY, with `TERM=xterm-256color` and truecolor. **+** lists the shells installed here, your default first (`ph_env::shells`): on macOS and Linux those in `/etc/shells` plus fish, nu and pwsh when on `PATH`, each as a login shell; on Windows PowerShell 7, Windows PowerShell, Command Prompt, Git Bash and WSL, as found. With several folders, each shell has a submenu of them. A folder on a network drive always gets the server's shell.
- Emulation (escape codes, grid, 10k lines of scrollback) is `alacritty_terminal`. PilotHouse draws the visible grid as styled text and maps keys to bytes (`keys.rs`).
- The 16 ANSI colors come from the theme (`palette.rs`), so terminals follow light and dark mode.
- Output bursts are coalesced into one redraw. Wheel scrolls the history, and typing jumps back to the live screen.
- **Select** by dragging; double-click selects a word, triple-click a line. ⌘C copies (Ctrl+Shift+C on Windows and Linux, where Ctrl+C stays the interrupt). Typing clears the selection.
- **⌘L** sends the selection (or, with none, what is on screen) to the chat as an attachment, for "why does this fail?"; the composer waits for your question.
- ⌘V pastes (bracketed paste when the program asks for it). ⌘ shortcuts otherwise go to the app.
- Tab names are short: a prompt title (`ada@host:~/code/app`) shows as the shell and folder (`zsh · app`), a running program's title stays as it is, cut to 24 characters, and a run configuration's terminal keeps its name.
- Shells start only when the Terminal tab is first opened, and they keep running while the app runs. Closing a tab ends its shell.
- **Shells belong to the workspace, not the tab.** A `Terminal` (`terminal.rs`) is the shell and its screen; `TerminalView` only draws one; `WorkspaceTerminals`, held by the `TerminalHub` global, is a workspace's list. So a shell keeps running when the tab's panel is rebuilt (folders added, the workspace moved to another window, an idle screen closed), and a new view picks it up as it is.

Not yet: mouse reporting to programs (e.g. clicking in `htop`).
