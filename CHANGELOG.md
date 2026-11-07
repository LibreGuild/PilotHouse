# Changelog

What changed in each release, newest first. The full notes are on the [releases page](https://github.com/LibreGuild/PilotHouse/releases).

Each release also has **Merge notes** for organisations that build their own copy of PilotHouse ([docs/customizing.md](docs/customizing.md)): what to look at when merging it. These cover new `[features]` keys (new features arrive switched on), new or renamed keys in `brand/brand.toml`, changes to `crates/custom` or `Store::apply_track`, and moved files.

## Unreleased

- **Builds for organisations.** Everything an organisation changes now lives in a few places of its own. [docs/customizing.md](docs/customizing.md) explains each:
  - `brand/` holds the name, ids, icons, features on or off, and update sources, read by the app and by the packaging scripts.
  - `crates/custom` holds the organisation's own views beside Workspaces and GitHub, its commands and its startup code.
  - `CUSTOM.md` holds the organisation's rules for coding agents.
- **Coding agents get context.** [AGENTS.md](AGENTS.md) (and `CLAUDE.md`) covers how the code is laid out and its rules, and a `merge-upstream` skill takes in a new PilotHouse release.
- An organisation's tables go on a migration track of their own (`Store::apply_track`), so they never take PilotHouse's numbers.
- Appearance preferences that a build does not know (a newer one's, or an organisation's) are kept when it saves.
- `scripts/package/icon.py` now makes every icon from one picture (it used to draw an old version of the icon).
- **A new Walkthrough for pull requests.** For the tech lead or manager who reviews a change: a briefing in plain words, written by an agent that read the change and the code around it. It covers what changes and why, how that part of the system works today, what is different after (with diagrams), how it is done, what it affects, where to look closely, how it is tested and what to ask the author. Its text can be selected and copied, and references to code open in the Files tab or on the web. It replaces the gist, file groups and tour, and the tour strip in the Files tab; a walkthrough made by an earlier version is asked for again.
- **Cline.** Cline's CLI joins Claude Code, Codex, Gemini CLI and Antigravity, with your own install and sign-in (`cline --acp`): chats with its models and its Plan and Act modes, PilotHouse's MCP tools and yours, one-off jobs like commit messages, its sessions in the usage dashboard, and Cline in a terminal with its past sessions to resume. It runs as in your terminal: on a provider Cline uses there that needs no key (Vertex AI, Ollama), not only on a Cline account. Where Cline's ACP side falls short, PilotHouse works around it: attachments go as text, tool titles that are JSON read as words, a reopened chat is recapped to a new session, it neither updates itself nor leaves a background hub running, and an expired sign-in says so with a way to sign in again.
- **Quick chat.** A small chat window over every app, from anywhere: ⌥Space on macOS, Alt+Shift+Space on Windows and Linux (set it in Settings → Chat), the command palette, or `pilothouse quick-chat` for a shortcut in the system's own settings (the only way on Wayland). It moves, resizes and comes back where it was, with the chat it showed; its chats are under Chats. [docs/features/quick-chat.md](docs/features/quick-chat.md)
- **Agents in a terminal stay in the chat list.** A coding agent's CLI run in a workspace terminal is saved as a chat once it is asked something. With its terminal open, clicking it shows the terminal; once closed, it shows the agent's last answer under its title, its session read only in the chat column, with *Continue in terminal* (the CLI resumes it) and *Continue in a new chat*, and Search chats finds what was said in it. Closing a terminal whose agent is at work or waiting for you asks first. Cline in a terminal is followed too, its transcript readable. Cline's provider can be chosen in the chat (⋯ → Provider): its accounts, and the providers saved with `cline auth` that need no key.
- **Clearer agent errors.** When an agent fails a request with a reason (Cline's "Internal error" carries one), the chat shows the reason. A sign-in that expires mid-chat offers the agent's sign-in, and a chat whose agent could not resume its session (a failed load) now gives the new session the chat's history. A remembered model the agent no longer offers is not asked for again.
- **Diagrams zoom.** Hovering a Mermaid diagram, in a chat, a pull request's walkthrough or a Markdown document, shows buttons to zoom out, zoom in, fit, open it large in a dialog and copy its source; a pinch or ⌘/Ctrl and the wheel zoom too.
- **Fixed: agent processes left running.** A new chat that was replaced before its first message, or switched to another agent, left its agent session open until PilotHouse quit. The same happened to deleted chats and to chats let go of as idle. For Claude Code that is a process of its own with its MCP servers, hundreds of MB each. Sessions now close with their chat (`session/close`, where the agent offers it), and *New chat* on a still untouched chat keeps it.

### Merge notes

- First release with `brand/` and `crates/custom/`. If your copy already renamed PilotHouse in the code, move the name, ids and icons to `brand/brand.toml` and `brand/icons/` and take PilotHouse's code as it is.
- The icons moved to `brand/icons/` (`app.icns`, `app.ico`, `app-1024.png`, `app-256.png`, `title-bar.png`). The update key moved to `brand/update_key.pub`.
- `AgentSpec` has two new fields, `env` and `quirks`: an agent your copy added to `AGENTS` needs `env: &[]` and `quirks: Quirks::NONE`.
- `[features]` keys: `files`, `changes`, `pulls`, `board`, `browser`, `devices`, `runs`, `tests`, `terminal`, `logcat`, `github`, `schedules`, `utilities`, `usage`, `network_drives`, `parallel_runs`, `presentations`, `chrome_extension`, `quick_chat` (new in this release).
- Installer file names now start with the brand's name without spaces. They are unchanged for PilotHouse: `PilotHouse-macOS-universal.dmg` and the rest.
