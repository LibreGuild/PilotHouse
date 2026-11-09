# Changelog

What changed in each release, newest first. The full notes are on the [releases page](https://github.com/LibreGuild/PilotHouse/releases).

Each release also has **Merge notes** for organisations that build their own copy of PilotHouse ([docs/customizing.md](docs/customizing.md)): what to look at when merging it. These cover new `[features]` keys (new features arrive switched on), new or renamed keys in `brand/brand.toml`, changes to `crates/custom` or `Store::apply_track`, and moved files.

## 0.6.2

- **Files: drag and drop, as in VS Code.** Drag rows (or all the selected ones) onto a folder to move them, after a question you can turn off; hold ⌥ (Ctrl on Windows and Linux) to copy. A closed folder opens when the drag rests on it. Files from Finder or Explorer are copied in where they are dropped; a row dropped on the editor opens, and on a chat it is attached.
- **Chat: paste images.** ⌘V with a screenshot or a copied image on the clipboard attaches it to the message; files copied or dropped from the system attach too.
- **Terminal: selection and right-click menu.** A drag selection follows the pointer anywhere in the window, ends where the button is let go, and scrolls the history when held above or below the terminal. Right-click offers Copy, Paste, Select All, Find, Ask in Chat, Split Terminal and Clear.
- **Agents use your Chrome first** for web pages, `localhost` included, and the built-in browser only when the extension is not connected. The first time an agent or a UI test run uses a workspace's built-in browser since PilotHouse started, its Browser tab comes to the front, so you see the test going on.
- **Chrome extension: from Chrome to PilotHouse.** Always on your own action, and into a chat's message box (the chat on screen, a new chat or quick chat) for you to send:
  - **Send** the selection, the page as Markdown, a link, an image, a screenshot of an area, or an element you click (selector, styles, the source file a dev build names, and a picture), from the right-click menu, shortcuts (⌥⇧P, ⌥⇧S, ⌥⇧E) or the panel.
  - **Record a bug report** (the steps, a video with still frames, the page's console errors and failed requests) or **a workflow** (steps that agents can run again), with a bar in the page that says so.
  - **`@tabs`** in a chat attaches the text of the tabs you shared with agents.
  - **Buttons on GitHub, Jira and Linear** (off until you turn a site on): review a pull request in PilotHouse, start a chat from an issue or a failed run, add an issue to the board.
  - **Send a summary** made on your computer, where Chrome has its built-in summarizer.
- **The PilotHouse panel** in Chrome's side panel (the toolbar button): chats waiting for you, agents in Chrome with Stop and Continue, sharing the tab, sending, recording and the options. The toolbar icon counts chats waiting for you; Chrome notifications when you choose; a tab comes forward when an agent needs you to sign in or prove you are a person.
- **Debugging tools for agents in your Chrome:** `chrome_performance` (Core Web Vitals and what slows the page), `chrome_audit` (accessibility, SEO, best practices), `chrome_emulate` (device, network, CPU, colour scheme, place, time zone), `chrome_mock` (answer requests to test empty, failing and slow states), `chrome_memory` (heap, detached DOM nodes and listeners, compared for leaks), `chrome_errors` (stacks mapped through source maps to the workspace's files), `chrome_site_tools` (sites' own tools, WebMCP, in Chrome's early preview). Emulation and mocks end by themselves once the agent leaves the tab idle, or when you press Stop.
- **Workflows:** agents run them (`chrome_workflows`, `chrome_run_workflow`), stopping where a password or a person is needed. Settings → Chrome extension lists them, makes a Playwright test or a schedule of one, renames and deletes them.
- A managed Chrome whose policy keeps extensions' debugger out now says so clearly, and what to do instead.
- **Fixes from a review of all of this:** recordings keep no secrets (shown passwords, card numbers, tokens in addresses); page text reaches agents marked as untrusted; sends go to the window in front; dropped and pasted images go to agents in a format they take; moving files never takes a workspace folder along and leaves no tab on an old path; mocks, emulation and site tools clean up after themselves; the panel reconnects by itself and says *Connected* only once PilotHouse answers.
- **The built-in browser** no longer drops an agent's first page while it starts.
- **After updating**, reload the extension in `chrome://extensions` (it asks Chrome for more permissions now; nothing reaches a site without your click or your yes). It needs Chrome 118 or later.

### Merge notes

- `ph_agent::LiveSources` has a new field, `tabs` (`None` where there is no `@tabs`): a copy that builds `LiveSources` itself adds `tabs: None`.
- The extension's files are listed in `crates/browser/src/extension.rs` (`FILES`); a test fails when a file in `crates/browser/extension/` is missing there.

## 0.6.1

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
- **Lighter while agents work.** An agent working in a terminal no longer redraws every window four times a second, nor writes to the database each second; the sidebar shares its chat lists instead of copying them on each draw, and the Chats list shows its 50 newest (*Show all* for the rest). A closed terminal's session file is not looked for again and again, a quick chat window saves where it is once it stops moving, and Cline's settings are read once per change, without waiting for the login shell.
- **Fixed: a database backup of one kind deleting the other.** Backups made before a migration and those made before an organisation's own migrations are now kept apart, three of each.
- **Fixed: Cline's provider changed in one workspace** now starts Cline again in every workspace, so none keeps running on the old one.
- **Fixed: agent processes left running.** A new chat that was replaced before its first message, or switched to another agent, left its agent session open until PilotHouse quit. The same happened to deleted chats and to chats let go of as idle. For Claude Code that is a process of its own with its MCP servers, hundreds of MB each. Sessions now close with their chat (`session/close`, where the agent offers it), and *New chat* on a still untouched chat keeps it.

### Merge notes

- First release with `brand/` and `crates/custom/`. If your copy already renamed PilotHouse in the code, move the name, ids and icons to `brand/brand.toml` and `brand/icons/` and take PilotHouse's code as it is.
- The icons moved to `brand/icons/` (`app.icns`, `app.ico`, `app-1024.png`, `app-256.png`, `title-bar.png`). The update key moved to `brand/update_key.pub`.
- `AgentSpec` has two new fields, `env` and `quirks`: an agent your copy added to `AGENTS` needs `env: &[]` and `quirks: Quirks::NONE`.
- `[features]` keys: `files`, `changes`, `pulls`, `board`, `browser`, `devices`, `runs`, `tests`, `terminal`, `logcat`, `github`, `schedules`, `utilities`, `usage`, `network_drives`, `parallel_runs`, `presentations`, `chrome_extension`, `quick_chat` (new in this release).
- Installer file names now start with the brand's name without spaces. They are unchanged for PilotHouse: `PilotHouse-macOS-universal.dmg` and the rest.
