# Workspace shell

Crate: `ph-workspace`. Code: `crates/workspace/src/`.

```
┌ title bar: ◆ · sidebar · [Workspaces | GitHub] · ⌘K search · inbox · theme · tools · settings ┐
│ sidebar          │ editor (while a file  │ chat              │ tool panel       │ icon │
│  New chat   ⌘N   │  is open)             │                   │ (Files, Git, …)  │ bar  │
│  CHATS           │                       │                   │                  │      │
│  WORKSPACES      │                       │                   │                  │      │
│   ▾ ● app  ⟳     │                       │                   │                  │      │
│       chats…     │                       │                   │                  │      │
└ status bar: folder · branch                                     detected stack · version ┘
```

- **`Workspace`** (`workspace.rs`) is a window's root: the title bar, the sidebar, the workspace in front and the status bar. It keeps one `WorkspaceScreen` per workspace opened in the window, and the window's [GitHub view](pulls.md#the-github-view) once shown.
- **Two views**, picked in the title bar: **Workspaces** (⌘⇧E) and **GitHub** (⌘⇧G). The GitHub view shows pull requests across repositories and their agent reviews, apart from any workspace; it has its own sidebar.
- **`WorkspaceScreen`** (`workspace_screen.rs`) is a workspace's screen: the editor column (only while a file is open), the chat, the tool panel and the icon bar on the right that picks the tool. Panels are made the first time they are shown. Its layout is saved per workspace: tool, panel open, chat, open files, sidebar folds.
- **Running work outlives the screen.** Chats (`ph_agent::chats`), shells (`ph_terminal::TerminalHub`), test runs (`ph_uitest::runner`) and schedules (`schedules::Automation`) belong to the workspace or the app, not to a window. A screen can be closed and rebuilt (another window, folders changed, idle) without stopping anything.
- **Actions** (`actions.rs`) are the single source for every command. Shortcuts, the palette, menus and buttons all dispatch them.

## Title bar

- The app icon sits beside the window buttons (off macOS, it opens the app menu).
- The **view toggle**: *Workspaces* carries a dot when a workspace other than the one in front wants you (a question, news not seen); *GitHub* shows how many pull requests wait for your review (once the view has been opened).
- In the Workspaces view the bar wears a faint wash of the workspace's colour (`ph_ui::WORKSPACE_COLORS`; a chosen colour is saved in `projects.color`).

## Sidebar

Everything at once, so nothing needs switching to be seen (`sidebar.rs`):

- **New chat** (⌘N) for the workspace in front, and a **Search chats** field (⌘⇧K), each a full-width row with its shortcut.
- **Pinned**: chats pinned from any workspace (each chat's ⋯ menu: *Pin to top*), with the workspace each belongs to. Kept in `workspace.sidebar_pinned`.
- **Chats**: the Chats space, for chats that need no folder (thinking something through, the browser, the board). A first start opens straight into a chat there. Its chats are grouped *Today*, *Yesterday*, *This week*, *Older*; **+** starts one.
- **Workspaces**, newest first (an order that stays put as they are used): each with its colour dot and what it is doing (a spinner while working, amber for a question, red for failing tests, green for something finished and not seen; the tooltip says it in words, with the ⌃1–⌃9 shortcut). The chevron unfolds a workspace to list its five newest chats (*Show all* for the rest); opening a workspace unfolds it. Picking a chat opens its workspace at that chat. On hover, a row offers a new chat there and its menu: *Add a folder…*, *Rename…*, *Open in new window*, *Colour*, *Remove from PilotHouse*. The section's **+** opens a folder as a workspace or makes one without folders.
- In every list a working chat has a spinner and one waiting for an answer an amber dot. Chats PilotHouse started (schedules, test recording and repair) fold into *Runs*.
- Each chat's ⋯ menu (on hover) pins or unpins it, renames it, or deletes it with its transcript after asking (not while its agent works or waits for an answer).
- Every section (*Pinned*, *Chats*, *Workspaces*) folds from its header; a folded one shows how many it hides. Folded sections are kept (`workspace.sidebar_folded`).
- Which workspaces are unfolded is kept (`workspace.sidebar_expanded`). The chats of a workspace whose screen is closed are read from the store each second, so a schedule's new chat shows up.
- ⌘B hides the sidebar, and the chat header then offers the chat list.

## The Chats space

A row of kind `chats` in `projects` (`ph_store::projects::ProjectKind`): no folders, agents working in a private scratch folder, made the first time the app starts. It has a screen like a workspace's with the tools that need no folder (board, terminal, browser, devices, tests), but it is not a workspace: it is not listed under *Workspaces*, has no colour or menu, cannot move to another window, and *Add a folder…* from it opens the folder as a new workspace instead.

## Editor, chat and tools

- **Files** is the first tool (⌘1): the folder tree, each folder a root when there are several (the primary one says so), with an icon per file type, chevrons, indent guides and Git marks (**M** changed, **U** new, **D** deleted; a dot on a folder holding changes). Its header's search button (⌘⇧F) turns the tree into workspace search, and **+** adds a folder. Right-click a folder root to make it primary or take it out.
- Picking a file opens it in the **editor column** beside the chat; closing the last file closes the column.
- The **icon bar** on the right shows the tools this workspace uses (Files in a workspace, Git with a repository, PRs with a hosted remote, Browser for web code, Devices for Android, Run with something to run, Tests with tests or an app); the rest are under **⋯**. Names show in tooltips, or under the icons with *Settings → Appearance → Names under tool icons*.
- On a narrow window, a file and a tool do not fit beside the chat at once: the one opened last shows, and the other folds to the side (the editor to a strip, the tool to its icon).
- ⌘J hides or shows the tool panel, ⌘⇧J lets it fill the window.

## Several workspaces

- Switch from the sidebar, with ⌃1–⌃9 (macOS) or Alt+1–9 (elsewhere) for the sidebar's rows, or Ctrl+Tab for the one before.
- Coming back to a workspace shows **While you were away**: the chats that finished and a test run that ended, one click each.
- Clicking a schedule's or reminder's system notification opens its workspace (and the chat it ran).
- **Open in new window** moves a workspace to its own window (`windows.rs`). A workspace shows in one window at a time: picking it in another window brings its window forward. Files with unsaved changes must be saved first.
- A workspace hidden for 30 minutes with nothing going on and nothing unsaved has its screen closed to keep memory flat; opening it rebuilds the screen, and its chats and shells were running all along.

## Workspaces and folders

A **workspace** (a `Project` in the code, table `projects`) is a name and any number of folders, from none up.

- *New workspace* makes an empty one; add folders whenever you like (the Files tool, the workspace's menu in the sidebar, the palette). *Open folder…* (⌘O) is a shortcut that makes a workspace with that folder, or reopens the one that has it.
- The first folder is the **primary** one: agents work there, and the terminal and run tools start there. Every folder:
  - is a root of Files, its paths named with the folder first (`server/src/main.js`), in ⌘P, search and `@` mentions too (`ph_files::roots`);
  - is given to agents as an extra root (ACP `additionalDirectories`), and a chat's first message tells the agent the folders and how paths name them;
  - has its own Git and PRs tools: a **Folder** picker on top of those tools switches between them, and the status bar's branch names the folder (`server · main`).
- **No folders**: for planning on the board or driving the browser and devices with a workspace of its own (plain chats go in the Chats space). Agents work in a private scratch folder (`<data dir>/workspaces/…`); Files, Git and PRs say a folder is needed and offer to add one. The first folder added becomes the primary one, and a workspace still called *Workspace N* takes its name.
- The **Run** tool finds each folder's run configurations and names them after their folder (`server: dev`). The Terminal's **+** asks which folder a new shell starts in.

Removing a folder leaves it on disk; removing the primary folder makes the next one primary, and removing the last leaves the workspace without folders. Folders change live: Files, Git and the tools that read folders start again; chats and shells keep going, and new agent sessions get the new folders. *Open with PilotHouse* on a folder opens the workspace that has it among its folders; a file opens in the workspace whose folder holds it (the deepest one).

## Shortcuts

| Keys | Action |
|---|---|
| ⌘K / ⌘⇧P | Command palette |
| ⌘O | Open a folder as a workspace |
| ⌘N | New chat |
| ⌘⇧K | Search chats |
| ⌘B | Toggle sidebar |
| ⌘J / ⌘⇧J | Toggle tools / let them fill the window |
| ⌘⇧E / ⌘⇧G | The Workspaces view / the GitHub view |
| ⌘1 | Files |
| ⌘2–9 | Git, Board, Terminal, Browser, Devices, Run, PRs, Tests |
| ⌃1–⌃9 (Alt+1–9 off macOS) | Workspace 1–9 in the sidebar |
| Ctrl+Tab | The workspace shown before |
| ⌘P / ⌘⇧F | Go to file / search in workspace |
| ⌘, | Settings |

## Settings

Stored in the `settings` table:
- **Appearance**: System, Light or Dark (`appearance`); names under tool icons.
- **Features**: turn tools off (`features.disabled`); *System* holds the file manager menu switch.

## Architecture map

*Architecture map…* in the palette (`arch_map/`) draws the project's parts in layers, foundations at the bottom: the members of a Cargo or npm/yarn/pnpm workspace with their internal dependencies, or the top-level folders otherwise. Each box shows lines of code and main languages (manifests and docs are not counted); a line goes from a part to what it uses directly, transitive ones left out so the picture stays readable. Read from the manifests on this computer, nothing run. *Explain with agent* sends the map to the chat and asks for a short explanation of each part, the main flows and where to start.

## Opening from outside

`open.rs`, `file_window.rs`, `shell_integration.rs`.

- **`pilothouse <path>...`**: a folder opens as a workspace (the listed one if it is already there), a file in a workspace's folder opens in its editor, and any other file opens in a **window of its own**: just the file in the editor (or the large-file viewer), like a notepad, with **Open folder** to make its folder a workspace. Opening the same file again focuses its window.
- **One app at a time**: a second launch with paths sends them to the running app over the control socket (`app.open`) and exits, so there is never a second copy of the window or the agents.
- **File manager**: *Settings → Features → System* adds "Open with PilotHouse" for the current user only. Windows: Explorer verbs under `HKCU\Software\Classes` for folders, folder backgrounds and files (on Windows 11 under *Show more options*). Linux: a `.desktop` entry in `~/.local/share/applications` (folders and text files). macOS: Finder's *Open With* and `open -a PilotHouse <path>` come from the app bundle's document types; the app receives them as file URLs (`on_open_urls`).

## Adding a tool

1. Create a feature crate with a view entity. Anything that must keep running when the panel closes (a process, a task) goes in a model held by a global keyed by workspace, like `TerminalHub`.
2. Add a `ContextTab` variant (`context_panel.rs`) with a label, icon, key and action, and say when it is relevant in `WorkspaceScreen::is_relevant`.
3. Create the panel in `WorkspaceScreen::ensure_panel` and render it in `render_tab_content`.
