# Architecture

PilotHouse is one Rust binary built on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) through [GPUI Kit](https://gpui-kit.com) (`gpui-kit` 0.8, Apache-2.0). Everything runs on the user's machine; there is no backend.

## Crates

Code is split by **capability**, one crate per feature. The app shell composes features; features never depend on the shell.

```
crates/
  app/        pilothouse binary: startup, menus, CLI subcommands, control methods
  workspace/  window shell: workspaces (any number of folders), sidebar, editor | chat | tools,
              palette, settings, schedules and triggers, inbox and to-dos, utilities dialog
  agent/      chat with coding agents over ACP, approval inbox
  board/      Kanban board + its control methods (used by MCP)
  files/      file tree, editor, ⌘P, search, project watcher
  changes/    Git tab, branch picker, blame data: commit, history, stashes, conflicts, review
  github/     GitHub REST client, accounts, secrets in the OS keychain
  pulls/      PRs tab, PR view, Settings → GitHub
  review/     agent PR reviews: template, parsing, re-review merge, posting
  git/        git operations over the CLI (status, branches, log graph, blame, conflicts, worktrees,
              checkpoints, parallel-run patches)
  terminal/   PTY terminals (alacritty_terminal)
  browser/    per-project headless Chrome over DevTools + its control methods
  devices/    Android over adb and the emulator's gRPC controller + control methods
  media/      live frames: surface view, annotation, capture, recording, diff
  runs/       run configurations and the ports panel
  uitest/     UI tests: recorded from agents' tool calls, replayed on sizes/devices, visual
              baselines, the Done-needs-tests gate, repair
  utils/      offline utilities (formats, encodings, hashes, ids, times, regex, SQLite)
  lsp/        language servers: transport, client, diagnostics, providers
  mcp/        `pilothouse mcp`: stdio MCP server for agents
  agy/        `pilothouse agy-acp`: Google Antigravity (`agy`) as an ACP agent
  control/    local control socket (+ dev UI automation)
  store/      SQLite persistence (workspaces, chats, board, schedules, to-dos, usage…)
  theme/      light/dark themes, user preferences, layout + motion tokens
  ui/         icon assets, small shared components
  env/        the login shell's environment (PATH for agents/terminals)
```

Dependency direction (arrows point at dependencies):

```
app ──► workspace ──► agent, board, files, changes, terminal, browser, devices, runs ──► store, ui, theme, env
 │                                        browser, devices ──► media
 │                          board, browser, devices, runs ──► control
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
│                    │ │ │   └ BoardPanel ◄ BoardHub ◄ control ┼◄ pilothouse mcp
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

Agents run as child processes speaking the [Agent Client Protocol](https://agentclientprotocol.com) on stdio. `ph-agent` uses the official `agent-client-protocol-schema` types for exact message shapes, with a small hand-written JSON-RPC transport (`agent/src/rpc.rs`) on GPUI's executor, so there is no second async runtime. Gemini CLI speaks ACP natively; Claude Code and Codex use their official adapters via `npx`; Antigravity (`agy`) goes through PilotHouse's own bridge, `ph-agy`, run as `pilothouse agy-acp`. See [features/chat.md](features/chat.md).

## Agents → PilotHouse (MCP)

Every new agent session is given one MCP server: this same binary in `pilothouse mcp` mode. It speaks MCP on stdio and forwards tool calls to the running app over the [control socket](control-socket.md). The app owns all state; the bridge holds none. Tools: board, browser, devices and ports (see each feature page). Screenshots come back as MCP image content.

Slow tools (a page load, an adb call) use async control handlers, so the UI never waits on them.

## Names

The app says **workspace**; the code says it two ways. The window and screens use workspace names (`Workspace` is a window's shell, `WorkspaceScreen` one workspace's screen). Stored data and the protocols agents and tools speak keep the older name, **project**: the `projects` table, `Project`, `ProjectId`, and `project_id` in control-socket and MCP messages. Renaming those would break existing databases and clients for no gain.

## Persistence

One SQLite file (`~/Library/Application Support/PilotHouse/pilothouse.db` on macOS, or `$PILOTHOUSE_DATA_DIR`). Migrations are append-only (`store/src/migrations.rs`). Per-project layout is a JSON blob, so new layout fields need no migration.

## Feature loading

Nothing is on by default except what the project needs:

1. Features cost nothing until opened (lazy panels).
2. **Detection**: `workspace/src/detect.rs` reads root markers (`Cargo.toml`, `build.gradle`, `*.xcodeproj`, …) and shows the stack in the status bar. `runs/src/detect.rs` finds run configurations.
3. **Heavy helpers start on use**: Chrome when the Browser tab or a browser tool is first used, the device screen and logcat only while the Devices tab is visible, and an emulator only when asked.
4. **Settings → Features** hides tools the user does not want.

## Decisions

| Decision | Why |
|---|---|
| GPUI via gpui-kit, Apache-2.0 only | Native GPU UI for the performance budget; permissive license for an Apache-2.0 project. Zed's GPL crates are not used. |
| Bundled art, permissive only | File icons from Material Icon Theme (MIT, `crates/ui/assets/file-icons`), agent marks from Lobe Icons (MIT, `crates/ui/assets/agents`), UI icons from Lucide (ISC, via gpui-kit). The app icon (`crates/app/assets/icon`) was made for PilotHouse. Nothing is fetched at run time. |
| `git` CLI, not libgit2 | No license question, on every dev machine, simpler |
| syntect highlighting (not tree-sitter) for now | ~200 languages in a few MB vs tens of MB; tree-sitter returns with LSP work in Phase 3 |
| Hand-written MCP server and ACP transport | Tiny protocol surface; avoids tokio and keeps the binary small |
| Exact pin `gpui-kit = "=0.8.0"` | GPUI's API changes between snapshots; upgrades are deliberate |
| The user's Chrome over CDP, not an embedded web engine | No second engine in the binary; it renders what users' browsers render, and agents share the session |
| CDP over a pipe, not a port | No localhost endpoint other users could drive; Chrome exits with PilotHouse |
| Emulator gRPC with a hand-written HTTP/2 client | ~35–40 fps and real-time touch (adb screencap gives ~8 fps); no tokio, tonic or protoc |
