# PilotHouse docs

Short, practical notes on how PilotHouse is built. Start with **architecture**, then read the page for the area you are changing.

| Page | What it covers |
|---|---|
| [architecture.md](architecture.md) | Crates, how they depend on each other, the main data flows, key decisions |
| [user-guide.md](user-guide.md) | Using PilotHouse: first steps, the screen, shortcuts |
| [development.md](development.md) | Build, run, disk-friendly setup, code conventions |
| [packaging.md](packaging.md) | Installers for macOS, Windows and Linux, and releases |
| [testing.md](testing.md) | Test levels and the UI automation harness (`pilothouse ctl`) |
| [performance.md](performance.md) | Budgets, how they are measured, current numbers |
| [control-socket.md](control-socket.md) | The local socket and every method on it |
| [security.md](security.md) | Local surfaces, agent boundaries, untrusted repositories, and their protections |
| **Features** | |
| [features/workspace.md](features/workspace.md) | Window shell: the two views, workspaces and their folders, the sidebar and the Chats space, windows, command palette, settings, opening from outside |
| [features/chat.md](features/chat.md) | Agent chat over ACP, permissions, the approval inbox, mentions |
| [features/board.md](features/board.md) | Kanban board and the MCP tools agents use |
| [features/files.md](features/files.md) | File tree, editor, ⌘P finder, project search |
| [features/pulls.md](features/pulls.md) | GitHub PRs, accounts, agent reviews with findings and re-review |
| [features/tests.md](features/tests.md) | UI tests: recorded from an agent's browser or device actions, replayed without AI, repaired by an agent |
| [features/schedules.md](features/schedules.md) | Agent prompts and UI test runs at set times or on events (files, commits, pull requests, checks, chains) |
| [features/inbox.md](features/inbox.md) | The title-bar inbox (approvals, reminders, finished runs) and to-dos |
| [features/utilities.md](features/utilities.md) | Offline utilities: formats, encodings, hashes, ids, times, regex, diff, SQLite |
| [features/changes.md](features/changes.md) | Git: branch picker, commit, history graph, stashes, conflicts, blame, review per hunk |
| [features/terminal.md](features/terminal.md) | Embedded terminals |
| [features/browser.md](features/browser.md) | The per-project browser pane over Chrome DevTools, and its agent tools |
| [features/devices.md](features/devices.md) | Android emulators and phones: live screen, input, logcat, UI tree, files, agent tools |
| [features/live-panes.md](features/live-panes.md) | Annotate, pick elements, capture, record, before/after, `@screenshot`, `@log` |
| [features/runs.md](features/runs.md) | Run configurations and the ports panel |
| [features/network-drives.md](features/network-drives.md) | Network drives: Git, terminals and runs on the server over SSH |

