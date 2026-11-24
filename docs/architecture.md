# Architecture

PilotHouse is one Rust binary built on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) through [GPUI Kit](https://gpui-kit.com) (`gpui-kit` 0.8, Apache-2.0). Everything runs on the user's machine; there is no backend.

## Crates

Code is split by **capability**, one crate per feature. The app shell composes features; features never depend on the shell.

```
crates/
  app/        pilothouse binary: startup, menus, CLI subcommands, control methods
  workspace/  window shell: the Bridge (every task by state), Reviews, a workspace's screen
              (chat | result pane), palette, settings, schedules and triggers, inbox and
              to-dos
  agent/      chat with coding agents over ACP, approval inbox
  files/      file tree and its menu (file operations), editor, ⌘P, search, project watcher
  preview/    viewers: rich Markdown editor, Mermaid, images, data trees, tables, notebooks,
              audio, video (the system's media framework), PDF, SQLite, spreadsheets, archives,
              fonts, hex
  changes/    Git tab, branch picker, blame data: commit, history, stashes, conflicts, review
  github/     GitHub client (REST, GraphQL search), accounts, pull requests, issues
  pulls/      PR and issue lists and pages, finishing a review, Settings → GitHub, the Reviews view's managed clones
  review/     agent PR reviews: template, parsing, re-review merge, posting; explanations of
              changes: the prompt, the shared file, finding one by the content it explains
  explain/    explanations of changes where they are read: beside the changes in the Git tab,
              on a commit in History, Why Is This Here? in the editor
  git/        git operations over the CLI (status, branches, log graph, blame, conflicts, worktrees,
              checkpoints, parallel-run patches)
  terminal/   PTY terminals (alacritty_terminal)
  browser/    per-project headless Chrome over DevTools + its control methods
  devices/    Android over adb and the emulator's gRPC controller + control methods
  media/      live frames: surface view, annotation, capture, recording, diff
  runs/       run configurations, the ports panel, tunnels to a server's ports
  remote/     network drives: Git, shells and runs on the server over SSH (mappings, routing,
              host keys, checks)
  artifacts/  web pages agents make: their folder, a loopback address each, what a page
              stores, the list, and the agents' control methods
  slides/     decks of slides, an artifact of a kind: the deck's files and their undo, its
              page (editor, viewer), comments, exports, PowerPoint files in and out, and the
              agents' control methods
  boards/     whiteboards, an artifact of a kind: the board's file, its page (the board and
              its tools), and the agents' control methods
  webview/    the system's web view inside a window (WKWebView, WebView2): placing it, the
              keyboard, a picture or a PDF of the page
  uitest/     The Tests tab: the project's own suites (run and read back per test), and UI
              tests recorded from agents' tool calls, replayed from a clean start on sizes
              and devices, with look checks, repair and export
  utils/      offline utilities (formats, encodings, certificates, hashes, ids, times,
              regex, colors, numbers, networks, SQLite) and the Utilities dialog
  lsp/        language servers: transport, client, diagnostics, providers
  mcp/        `pilothouse mcp`: stdio MCP server for agents
  agy/        `pilothouse agy-acp`: Google Antigravity (`agy`) as an ACP agent
  control/    local control socket (+ dev UI automation)
  store/      SQLite persistence (workspaces, chats, schedules, to-dos, usage…)
  theme/      light, dark and warm-dark themes, user preferences, layout tokens
  ui/         icon assets, small shared components
  brand/      the app's name, ids, icons, features on or off and update sources, read from
              brand/brand.toml at build time (see customizing.md)
  custom/     an organisation's own views, commands and startup code (empty in PilotHouse;
              see customizing.md)
  env/        the login shell's environment (PATH for agents/terminals)
```

Dependency direction (arrows point at dependencies):

```
app ──► workspace ──► agent, files, changes, terminal, browser, devices, runs ──► store, ui, theme, env
 │        workspace ──► pulls, explain ──► agent ──► review ──► git, store
 │                                        browser, devices ──► media
 │                                         files, changes ──► preview
 │                          browser, devices, runs ──► control
 │        workspace ──► artifacts ──► control, ui
 │        workspace ──► slides ──► artifacts, webview, control, ui, theme
 │        workspace ──► boards ──► artifacts, control, ui
 ├──► mcp ──► control
 └──► agy ──► env
```

## Runtime picture

```
┌──────────────────── PilotHouse process ────────────────────┐
│ Windows: Workspace ─ WorkspaceScreen (a workspace's screen)   │
│   ├ ChatPanel ─────────┐   ├ FilesPanel / FileTree          │
│   ├ TerminalPanel ───┐ │   ├ ChangesPanel ─ Repo            │
│   ├ TestsPanel ────┐ │ │   ├ BrowserPanel / DevicesPanel    │
│                    │ │ │   └ their control methods ◄ control ┼◄ pilothouse mcp
│ Per workspace, no window (globals):                        │
│   ChatHub ─ WorkspaceChats ─ ChatThread ─ AgentConnection ─┼──► agent (ACP, stdio)
│   TerminalHub ─ WorkspaceTerminals ─ Terminal ─ PTY ───────┼──► login shell
│   TestRunHub ─ WorkspaceTestRuns                           │
│   BrowserHub ─ Browser ─ CDP ──────────────────────────────┼──► headless Chrome
│   WatcherHub ─ ProjectWatcher (per folder)                 │
│ App-wide: Automation (schedules, triggers, reminders)      │
│ Store (SQLite, WAL)                                        │
└────────────────────────────────────────────────────────────┘
```

- **Running work is not on the screen.** Chats, shells, test runs, browsers and folder watchers live in globals keyed by workspace; panels only show them. A workspace's screen can be closed and rebuilt (its folders changed, it moved to another window, it sat idle) without stopping anything. Schedules run app-wide, whatever window is open.
- **Panels are lazy.** A `WorkspaceScreen` creates a panel the first time its tool is shown, so an unused feature never starts a process, a watcher or a shell.
- **One window per workspace.** A workspace shows in one window at a time (`windows.rs`); idle hidden screens are closed after 30 minutes.
- **One agent process per (workspace, agent)**, shared by that workspace's chats.

## Agents (ACP)

Agents run as child processes speaking the [Agent Client Protocol](https://agentclientprotocol.com) on stdio. `ph-agent` uses the official `agent-client-protocol-schema` types for exact message shapes, with a small hand-written JSON-RPC transport (`agent/src/rpc.rs`) on GPUI's executor, so there is no second async runtime. Gemini CLI and Cline speak ACP natively (Cline with workarounds for its gaps, `ph_agent::cline`); Claude Code and Codex use their official adapters via `npx`; Antigravity (`agy`) goes through PilotHouse's own bridge, `ph-agy`, run as `pilothouse agy-acp`. See [features/chat.md](features/chat.md).

A session with an agent has a second form: the agent's own CLI in a terminal, in the chat's place. The chat hands the agent's session over (`ChatThread::hand_over`), `ph_workspace::terminal_agents::launch` starts the CLI resuming it among the workspace's agent terminals (`TerminalHub::agent_terminals`), and coming back the chat takes in what the CLI's session file added (`cli_transcript::take_in`) and loads the session again. `workspace_screen/cli.rs` holds the rules that keep it one conversation. How each agent is run (its command, environment and accounts) is `ph_agent::profiles`.

## Agents → PilotHouse (MCP)

Every new agent session is given one MCP server: this same binary in `pilothouse mcp` mode. The helper processes the app starts (this one, and the `agy-acp` bridge) run under a name of their own, `pilothouse-mcp` and `pilothouse-agy-acp`, so Task Manager and Activity Monitor tell them from the app: a hard link to the program in `<data dir>/helpers/<build>/` (`ph_store::paths::helper_program`), made by the app only and tried once (`--version`), or the program itself where a link cannot be made or does not start (another disk, a policy on where programs may run). It speaks MCP on stdio and forwards tool calls to the running app over the [control socket](control-socket.md). The app owns all state; the bridge holds none. Tools: the browser, the user's Chrome, devices, ports and UI tests, among others (see [chat.md](features/chat.md#pilothouses-own-tools) and each feature page). Screenshots come back as MCP image content.

Slow tools (a page load, an adb call) use async control handlers, so the UI never waits on them.

## Names

The app says **workspace**; the code says it two ways. The window and screens use workspace names (`Workspace` is a window's shell, `WorkspaceScreen` one workspace's screen). Stored data and the protocols agents and tools speak keep the older name, **project**: the `projects` table, `Project`, `ProjectId`, and `project_id` in control-socket and MCP messages. Renaming those would break existing databases and clients for no gain.

## Persistence

One SQLite file (`~/Library/Application Support/PilotHouse/pilothouse.db` on macOS, or `$PILOTHOUSE_DATA_DIR`; the folder's name is the brand's `data_folder`). Migrations are append-only (`store/src/migrations.rs`); an organisation's build keeps its own tables on its own track (`Store::apply_track`, see customizing.md). Per-project layout is a JSON blob, so new layout fields need no migration.

## Feature loading

Nothing is on by default except what the project needs:

1. Features cost nothing until opened (lazy panels).
2. **Detection**: `workspace/src/detect.rs` reads root markers (`Cargo.toml`, `build.gradle`, `*.xcodeproj`, …) and shows the stack in the status bar (with a workspace in front). `runs/src/detect.rs` finds run configurations.
3. **Heavy helpers start on use**: Chrome when the Browser tab or a browser tool is first used, the device screen only while the Devices tab is visible (and 20 seconds after), logcat once it is opened or an app is run, and an emulator only when asked.
4. **Settings → Features** hides tools the user does not want.

## Decisions

| Decision | Why |
|---|---|
| GPUI via gpui-kit, Apache-2.0 only | Native GPU UI for the performance budget; permissive license for an Apache-2.0 project. Zed's GPL crates are not used. |
| Bundled art, permissive only | File icons from Material Icon Theme (MIT, `crates/ui/assets/file-icons`), agent marks from Lobe Icons (MIT, `crates/ui/assets/agents`), UI icons from Lucide (ISC, via gpui-kit). The fonts are Inter and JetBrains Mono (SIL Open Font License 1.1, `crates/theme/fonts`): the interface looks the same on every system with them. The app icon (`brand/icons`) was made for PilotHouse. Nothing is fetched at run time. |
| `git` CLI, not libgit2 | No license question, on every dev machine, simpler |
| syntect highlighting (not tree-sitter) for now | ~200 languages in a few MB vs tens of MB; tree-sitter returns with LSP work in Phase 3 |
| Hand-written MCP server and ACP transport | Tiny protocol surface; avoids tokio and keeps the binary small |
| Exact pin `gpui-kit = "=0.8.1"` | GPUI's API changes between snapshots; upgrades are deliberate |
| The user's Chrome over CDP, not an embedded web engine | No second engine in the binary; it renders what users' browsers render, and agents share the session |
| CDP over a pipe, not a port | No localhost endpoint other users could drive; Chrome exits with PilotHouse |
| Emulator gRPC with a hand-written HTTP/2 client | ~35–40 fps and real-time touch (adb screencap gives ~8 fps); no tokio, tonic or protoc |
