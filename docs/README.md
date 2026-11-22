# PilotHouse docs

Short, practical notes on how PilotHouse is built. Start with **architecture**, then read the page for the area you are changing.

| Page | What it covers |
|---|---|
| [architecture.md](architecture.md) | Crates, how they depend on each other, the main data flows, key decisions |
| [user-guide.md](user-guide.md) | Using PilotHouse: first steps, the screen, shortcuts |
| [development.md](development.md) | Build, run, disk-friendly setup, code conventions |
| [styleguide.md](styleguide.md) | UI rules: colour as signal, type steps, density, controls, review checklist |
| [customizing.md](customizing.md) | For organisations building their own copy: name, icons, features on or off, updates, views and commands of their own, merging new releases |
| [packaging.md](packaging.md) | Installers for macOS, Windows and Linux, and releases |
| [testing.md](testing.md) | Test levels and the UI automation harness (`pilothouse ctl`) |
| [performance.md](performance.md) | Budgets, how they are measured, current numbers |
| [control-socket.md](control-socket.md) | The local socket and every method on it |
| [security.md](security.md) | Local surfaces, agent boundaries, untrusted repositories, and their protections |
| **Features** | |
| [features/workspace.md](features/workspace.md) | Window shell: the frame and its islands, the column of icons (Workspaces, Chats, GitHub), workspaces and their folders, the lists, windows, command palette, settings, opening from outside |
| [features/chat.md](features/chat.md) | Agent chat over ACP, permissions, the approval inbox, mentions |
| [features/files.md](features/files.md) | File tree, editor, ⌘P finder, project search |
| [features/previews.md](features/previews.md) | Viewers: rich Markdown and Mermaid, images, data trees, tables, notebooks, audio, video, PDF, databases, archives, fonts |
| [features/pulls.md](features/pulls.md) | GitHub PRs and issues, accounts, agent reviews with findings and re-review, finishing a review |
| [features/tests.md](features/tests.md) | UI tests: recorded from an agent's browser or device actions, replayed without AI, repaired by an agent |
| [features/schedules.md](features/schedules.md) | Agent prompts and UI test runs at set times or on events (files, commits, pull requests, checks, chains) |
| [features/inbox.md](features/inbox.md) | The title-bar inbox (approvals, reminders, finished runs) and to-dos |
| [features/studio.md](features/studio.md) | Studio: edit a picture or a video by hand or with an agent (crop, mark up, hide, resize, frame, compress; cut, speed up, step frames, export MP4 or GIF) |
| [features/tour.md](features/tour.md) | The welcome tour: a few cards that say what the app has, shown once after an install and after an update that added some |
| [features/slides.md](features/slides.md) | Slides: decks made beside a chat, as web pages in the system's web view (the agent edits files; edit by hand; comments; present; share; export PDF, web page, PNG, PowerPoint; start from a `.pptx`) |
| [features/artifacts.md](features/artifacts.md) | Artifacts: web pages agents make, kept in a folder of yours and served from this computer; the list, versions, what a page stores, sharing on your network and using one together, live |
| [features/themes.md](features/themes.md) | Eight themes, and designing your own with an agent, shown live as it changes |
| [features/utilities.md](features/utilities.md) | Offline utilities, each in a view of its own: formats, encodings, tokens, certificates, hashes, ids, times, regex, diff, colors, SQLite |
| [features/changes.md](features/changes.md) | Git: branch picker, commit, history graph, stashes, conflicts, blame, review per hunk |
| [features/explanations.md](features/explanations.md) | Explain: a change told in plain words before it is committed, kept with its commit, *Why Is This Here?* for a line, sharing one as a file |
| [features/terminal.md](features/terminal.md) | Embedded terminals |
| [features/browser.md](features/browser.md) | The per-project browser pane over Chrome DevTools, and its agent tools |
| [features/devices.md](features/devices.md) | Android emulators and phones: live screen, input, logcat, UI tree, files, agent tools |
| [features/live-panes.md](features/live-panes.md) | Annotate, pick elements, capture, record, before/after, `@screenshot`, `@log` |
| [features/runs.md](features/runs.md) | Run configurations and the ports panel |
| [features/network-drives.md](features/network-drives.md) | Network drives: Git, terminals and runs on the server over SSH |

