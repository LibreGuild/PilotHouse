# Terminal

Crate: `ph-terminal`.

- Each tab is your **login shell** (`$SHELL -l`) in the workspace's primary folder (its scratch folder when it has none; with several folders, **+** asks which), on a real PTY, with `TERM=xterm-256color` and truecolor.
- Emulation (escape codes, grid, 10k lines of scrollback) is `alacritty_terminal`. PilotHouse draws the visible grid as styled text and maps keys to bytes (`keys.rs`).
- The 16 ANSI colors come from the theme (`palette.rs`), so terminals follow light and dark mode.
- Output bursts are coalesced into one redraw. Wheel scrolls the history, and typing jumps back to the live screen.
- **Select** by dragging; double-click selects a word, triple-click a line. ⌘C copies (Ctrl+Shift+C on Windows and Linux, where Ctrl+C stays the interrupt). Typing clears the selection.
- **⌘L** sends the selection (or, with none, what is on screen) to the chat as an attachment, for "why does this fail?"; the composer waits for your question.
- ⌘V pastes (bracketed paste when the program asks for it). ⌘ shortcuts otherwise go to the app.
- Tab titles keep the end of long shell titles (`…/multi/client`), so the tab bar never pushes its buttons away.
- Shells start only when the Terminal tab is first opened, and they keep running while the app runs. Closing a tab ends its shell.
- **Shells belong to the workspace, not the tab.** A `Terminal` (`terminal.rs`) is the shell and its screen; `TerminalView` only draws one; `WorkspaceTerminals`, held by the `TerminalHub` global, is a workspace's list. So a shell keeps running when the tab's panel is rebuilt (folders added, the workspace moved to another window, an idle screen closed), and a new view picks it up as it is.

## Agent CLIs without a chat

Coding-agent CLIs PilotHouse has no ACP chat for (Aider, Amp, Crush, Cursor Agent, GitHub Copilot CLI, Goose, OpenCode, Qwen Code; `cli_agents.rs`) start from the tab's agent button (or *Start … in a terminal* in the palette), each in its own named terminal. Only the CLIs found on the shell's `PATH` are listed; with none, the menu says which programs it looked for.

Not yet: mouse reporting to programs (e.g. clicking in `htop`).
