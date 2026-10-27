# Terminal

Crate: `ph-terminal`.

- Each tab is a shell in the workspace's primary folder (its scratch folder when it has none), on a real PTY, with `TERM=xterm-256color` and truecolor. **+** lists the shells installed here, your default first (`ph_env::shells`): on macOS and Linux those in `/etc/shells` plus fish, nu and pwsh when on `PATH`, each as a login shell; on Windows PowerShell 7, Windows PowerShell, Command Prompt, Git Bash and WSL, as found. With several folders, each shell has a submenu of them. A folder on a network drive always gets the server's shell.
- Emulation (escape codes, grid, 10k lines of scrollback) is `alacritty_terminal`. PilotHouse draws the visible grid as styled text and maps keys to bytes (`keys.rs`).
- The 16 ANSI colors are a palette tuned for terminals (`palette.rs`), one for dark mode and one for light; background, text, cursor and selection follow the theme. `CLICOLOR=1` is set (unless you set it), so macOS `ls` and other BSD tools colour their output.
- Output bursts are coalesced into one redraw. Wheel scrolls the history, and typing jumps back to the live screen.
- **Select** by dragging; double-click selects a word, triple-click a line. ⌘C copies (Ctrl+Shift+C on Windows and Linux, where Ctrl+C stays the interrupt). Typing clears the selection.
- **⌘-click** (Ctrl-click on Windows and Linux) on a link in the output opens it (`links.rs`): a web address on this computer (`localhost`, `127.0.0.1`) in the Browser tool, any other in the system browser; a file path, with its line when printed as `path:line:column` like compiler and test output, in the editor when it is in one of the workspace's folders.
- **⌘F** (Ctrl+Shift+F on Windows and Linux) finds text in the history and screen (`search.rs`): the bar at the top right selects each match and scrolls to it; Enter goes to the next older match, Shift+Enter to the newer one, Esc closes it. The text is matched as written, ignoring case.
- **⌘L** sends the selection (or, with none, what is on screen) to the chat as an attachment, for "why does this fail?"; the composer waits for your question.
- ⌘V pastes (bracketed paste when the program asks for it). ⌘ shortcuts otherwise go to the app.
- Tab names are short: a prompt title (`ada@host:~/code/app`) shows as the shell and folder (`zsh · app`), a running program's title stays as it is, cut to 24 characters, and a run configuration's terminal keeps its name.
- Shells start only when the Terminal tab is first opened, and they keep running while the app runs. Closing a tab ends its shell.
- **Kept between runs.** When PilotHouse quits, each workspace's terminals (up to 8, the last 2,000 lines of each) are written as plain text to `<data dir>/terminals/<workspace id>.json`. The next time the workspace's terminals are wanted (its Terminal tab, a run, an agent), each comes back as a new shell with that text above it, dimmed, under a "from the last run of PilotHouse" line. The file is read once and removed. Programs do not come back (a shell cannot be resumed); colors and escapes are left out.
- **Shells belong to the workspace, not the tab.** A `Terminal` (`terminal.rs`) is the shell and its screen; `TerminalView` only draws one; `WorkspaceTerminals`, held by the `TerminalHub` global, is a workspace's list. So a shell keeps running when the tab's panel is rebuilt (folders added, the workspace moved to another window, an idle screen closed), and a new view picks it up as it is.

## Agents in a terminal

**+ → Claude Code / Codex / Gemini CLI / Antigravity** starts that agent's CLI in a new tab of the folder, and PilotHouse follows it (`ph_workspace::terminal_agents`): the tab shows a spinner while it works, an amber dot when it waits for you (a permission, a question, a sign-in, a folder to trust), a green dot when it finished a turn you have not looked at (looking at the tab clears it); it gets a card on the Agents board (with what it runs, asks or replied last), and notifications and the Dock badge count it like a chat. How PilotHouse knows, best first:

- **Hooks** (Claude Code started from **+**): `claude --settings <file>`, a settings file of PilotHouse's own (in the data folder's `terminal-agents/`, cleared at start) with hooks on `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop` and `SessionEnd`. Nothing is written to `~/.claude`. Each hook runs `pilothouse agent-hook --socket <control socket> --pane <id>`, which passes the event (stdin) to the app as `agent.event` and always succeeds silently, so the agent is never held up. They give the prompt (the card's name), each tool, and the last reply.
- **Program status** (OSC 7501, `ph_terminal::program_status`): Codex, Antigravity and Claude Code write their own state into their output once the terminal says it understands. The PTY's output is read through a tap that passes every byte on to the emulator and picks these reports out, and answers the probe. This works for an agent started by hand in any tab, too.
- **The title** (Gemini CLI, which has neither): it puts ◇ ready, ✦ working or ✋ action required at the start of the terminal's title.

Programs that ask the terminal for its colours (OSC 10/11) get the ones they are drawn with.

**+ → Resume a session… → agent** lists that agent's past sessions in the folder, from its own files, only read (`agent_sessions.rs`): Claude Code's `<CLAUDE_CONFIG_DIR or ~/.claude>/projects/<folder>/*.jsonl` (named by Claude's titles), Codex's `<CODEX_HOME or ~/.codex>/sessions/…/rollout-*.jsonl`, Gemini CLI's `~/.gemini/tmp/<project>/chats/`, Antigravity's `~/.gemini/antigravity-cli/conversation_summaries.db`. Picking one resumes it in a terminal (`claude --resume`, `codex resume`, `gemini --resume`, `agy --conversation`); **Open in chat** (on hover) continues it as a PilotHouse chat instead: the agent loads the session (ACP `session/load`) and plays it back, and a chat with no transcript of its own takes that as its history.

**Transcript** (the speech-bubble button by **+**, on an agent's terminal) shows its session beside it, as a read-only chat: what was asked, what it said, and the tools it ran (`ph_agent::cli_transcript`). It reads the agent's own session file (from its last 4 MB), the one its hooks name for Claude Code, else the one it resumed or the newest in the folder since it started, then only what is added as it grows, off the UI thread. Claude Code, Codex and Gemini CLI keep one; Antigravity does not. The divider between the two drags.

Not on network drives (the hooks call this computer's PilotHouse). Codex, Gemini CLI and Antigravity ask once to trust a folder; that is their own question and answer.

Not yet: mouse reporting to programs (e.g. clicking in `htop`).
