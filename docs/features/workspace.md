# Workspace shell

Crate: `ph-workspace`. Code: `crates/workspace/src/`.

```
┌ title bar: ◆ · sidebar · [Workspaces | Agents | GitHub] · ⌘K search · inbox · theme · tools · settings ┐
│ sidebar          │ editor (while a file  │ chat              │ right dock       │ icon │
│  New chat   ⌘N   │  is open)             │                   │ (Files, Git, …)  │ bar  │
│  CHATS           │                       │                   │                  │      │
│  WORKSPACES      │                       │                   │                  │      │
│   ▾ ● app  ⟳     │                       │                   │                  │      │
│       chats…     │                       │                   │                  │      │
└ status bar: branch · CI · folder · ports     agent limits · detected stack · version ┘
```

- **`Workspace`** (`workspace.rs`) is a window's root: the title bar, the sidebar, the workspace in front and the status bar. It keeps one `WorkspaceScreen` per workspace opened in the window, and the window's [GitHub view](pulls.md#the-github-view) once shown.
- **Three views**, picked in the title bar: **Workspaces** (⌘⇧E), **Agents** (⌘⇧A) and **GitHub** (⌘⇧G). The Agents view is a board of the chats at work across every workspace (below). The GitHub view shows pull requests across repositories and their agent reviews, apart from any workspace; it has its own sidebar.
- The look follows [the style guide](../styleguide.md): quiet graphite chrome, colour only for state, focus and selection.
- **`WorkspaceScreen`** (`workspace_screen.rs`) is a workspace's screen: the chat, the editor (only while a file is open) and the tools, each a pane of a dockable layout (`workspace_screen/dock.rs`, on gpui-component's `DockArea`), and the icon bar on the right that shows a tool. The drawing above is the layout a workspace starts with. Panels are made the first time they are shown. Its state is saved per workspace: the dock layout (`workspace.dock.<id>`), the tool shown last, chat, open files, sidebar folds.
- **Running work outlives the screen.** Chats (`ph_agent::chats`), shells (`ph_terminal::TerminalHub`), test runs (`ph_uitest::runner`) and schedules (`schedules::Automation`) belong to the workspace or the app, not to a window. A screen can be closed and rebuilt (another window, folders changed, idle) without stopping anything.
- **Actions** (`actions.rs`) are the single source for every command. Shortcuts, the palette, menus and buttons all dispatch them.

## Title bar

- The app icon sits beside the window buttons (off macOS, it opens the app menu).
- The **view toggle**: *Workspaces* carries a dot when a workspace other than the one in front wants you (amber: an agent waits for an answer; green: news not seen); *Agents* shows how many chats wait for an answer (amber), else how many work; *GitHub* shows how many pull requests wait for your review (once the view has been opened).
- The bar is plain chrome; a workspace's colour (`ph_ui::WORKSPACE_COLORS`, a chosen one saved in `projects.color`) marks its rows, not the bar.

## Sidebar

Everything at once, so nothing needs switching to be seen (`sidebar.rs`):

- **New chat** (⌘N) for the workspace in front, and **Search chats** (⌘⇧K), each a plain row with its shortcut.
- **Pinned**: chats pinned from any workspace (each chat's ⋯ menu: *Pin to top*), with the workspace each belongs to. Kept in `workspace.sidebar_pinned`.
- **Chats**: the Chats space, for chats that need no folder (thinking something through, the browser, the board). A first start opens straight into a chat there. Its chats are grouped *Today*, *Yesterday*, *This week*, *Older*; **+** starts one.
- **Workspaces**, newest first (an order that stays put as they are used): each with its colour dot and, in a word, what it is doing (*needs you* in amber, *failing* in red, *2 working*; a green dot for something finished and not seen; the tooltip says more, with the ⌃1–⌃9 shortcut). The chevron unfolds a workspace to list its five newest chats (*Show all* for the rest); opening a workspace unfolds it. Picking a chat opens its workspace at that chat. On hover, a row offers a new chat there and its menu: *Add a folder…*, *Rename…*, *Open in new window*, *Colour*, *Remove from PilotHouse*. The section's **+** opens a folder as a workspace or makes one without folders.
- **Chat rows are small cards**: a state glyph (a spinner while working, an amber **?** when it waits for an answer, red when its last turn failed, green when it finished and you have not looked yet), the title and how long ago (*now*, *5m*, *3h*, *Tue*); under it, for chats loaded in this run, the agent's mark and what it is doing, the question it asks, or the start of its last reply. A finished turn counts as read once the chat is on screen (`WorkspaceChats::summary`, `set_viewing`). Chats PilotHouse started (schedules, test recording and repair) fold into *Runs*.
- Each chat's ⋯ menu (on hover) pins or unpins it, **marks it done**, renames it, or deletes it with its transcript after asking (not while its agent works or waits for an answer). Chats marked done (`workspace.sidebar_settled`) fold into *Done · N* under their workspace and leave the Agents board; one that works or asks again comes back on its own.
- Every section (*Pinned*, *Chats*, *Workspaces*) folds from its header; a folded one shows how many it hides. Folded sections are kept (`workspace.sidebar_folded`).
- Which workspaces are unfolded is kept (`workspace.sidebar_expanded`). The chats of a workspace whose screen is closed are read from the store each second, so a schedule's new chat shows up.
- ⌘B hides the sidebar, and the chat header then offers the chat list.

## Agents view

`agents_board.rs`: every chat loaded in this run of PilotHouse, from every workspace and the Chats space, in four columns: **Needs you**, **Working**, **Finished** (news not seen, or a failed turn) and **Recent** (the newest eight others). A card shows the state glyph, title and age; the workspace (its colour dot) and the agent; and what the agent is doing, asks, or said last. Waiting cards have an amber edge. A card opens its chat in its workspace.

## Notifications

`attention.rs`, one watcher for the app: when a chat starts waiting for an answer, finishes, or stops with an error while it is not the chat on screen, PilotHouse posts a system notification (clicking it opens that chat). On macOS the Dock icon shows how many chats wait for an answer. The ⌘K palette starts with **Chats**: those waiting, working or finished first, then every chat, newest first.

## Tasks

A **task** is a branch in its own worktree, opened as a workspace, so an agent can work on it while the main checkout stays as it is (`tasks.rs`). *New task…* (a workspace's menu, or ⌘K *New task in a worktree…*) asks for a name; PilotHouse makes branch `task/<name>` from the folder's HEAD in a worktree under the data folder's `tasks/`, opens it as *Repo · Name*, and runs its setup in a terminal named *Setup*. The repository can say how in a checked-in `pilothouse.toml`:

```toml
[worktree]
setup = "npm install"          # run in the task's Setup terminal
copy = [".env", ".env.local"]  # files copied from the main checkout
share = ["node_modules"]       # folders linked to the main checkout's
```

The setup sees `PILOTHOUSE_PORT`, a port of the task's own (4100, 4110, …) for its dev server. *Finish task…* (the workspace's menu, ⌘K) removes the worktree and the workspace after asking: files PilotHouse copied in go if unchanged and links go; anything else not committed makes Git refuse, and the task stays. Its branch is deleted too once merged (`git branch -d`), else kept. When the branch has been merged into its base, the status bar offers **Merged · Finish task**. Which workspaces are tasks is kept in the `workspace.tasks` setting.

## Status bar

Left: the branch (ahead/behind, an operation under way; opens the branch picker), **CI** for the commit checked out (a check when every GitHub check passed, a spinner while some run, red with the count when one failed; the tooltip names them, a click opens the failed or running one), the folder, the network drive, and the **ports** the workspace's processes listen on (opens the Run tool). `signals.rs` reads ports every 10 seconds and CI every minute or when HEAD moves, only for the workspace on screen and not on network drives; the Android bridge and emulator are left out (the Devices tool has them). Right: agent limits, the detected stack and the version.

## Agent limits

When an agent reports how much of its subscription window is used, the status bar shows a small meter per window, amber near the limit and red at it, with the reset time in the tooltip (`ph_agent::rate_limits`). Claude Code reports its 5-hour and weekly windows in chat as a limit comes close; Codex writes its windows into its own session files (`$CODEX_HOME/sessions`), which PilotHouse reads once a minute, whatever ran Codex. Nothing is fetched for this. Gemini CLI and Antigravity report none.

## The Chats space

A row of kind `chats` in `projects` (`ph_store::projects::ProjectKind`): no folders, agents working in a private scratch folder, made the first time the app starts. It has a screen like a workspace's with the tools that need no folder (board, terminal, browser, devices, tests), but it is not a workspace: it is not listed under *Workspaces*, has no colour or menu, cannot move to another window, and *Add a folder…* from it opens the folder as a new workspace instead.

## Editor, chat and tools

- **Files** is the first tool (⌘1): the folder tree, each folder a root when there are several (the primary one says so), with an icon per file type, chevrons, indent guides and Git marks (**M** changed, **U** new, **D** deleted; a dot on a folder holding changes). Its header's search button (⌘⇧F) turns the tree into workspace search; the others make a file or a folder, fold every folder and add a folder to the workspace. Right-click any row for the file menu ([files](files.md)); a folder root's also makes it primary or takes it out.
- The **chat and the editor** are two tabs of one group in the layout a workspace starts with: picking a file (from the tree, ⌘P, search, a link) brings the Editor tab to the front, and anything sent to the chat, or ⌘L from the editor, brings the Chat tab back. The Editor tab shows only while a file is open. The panes sit on a rounded card between the sidebar and the icon bar.
- The **icon bar** on the right shows the tools this workspace uses (Files in a workspace, Git with a repository, PRs with a hosted remote, Browser for web code, Devices for Android, Run with something to run, Tests with tests or an app); the rest are under **⋯**. Names show in tooltips, or under the icons with *Settings → Appearance → Names under tool icons*.
- **Docking.** Every pane has a tab. Drag a tab into another group to share it, onto a group's edge to split beside or below it, or to the window's left, right or bottom edge to dock it there. A pane's ⋯ menu does the same (*Move into the chat's tabs*, *Move beside the chat*, *Move below the chat*, *Move to the left side*, *to the right side*, *to the bottom*), and also works where a drag cannot: the last pane of a dock does not drag out of it. A dock left empty goes away. Tools close with their tab's ×; the chat and the editor stay. The layout is kept per workspace and comes back with it (version `dock::LAYOUT_VERSION`; a layout this build cannot read falls back to the default). *Reset layout* (the ⋯ menu, or the palette) goes back to the default.
- A tool shown from the icon bar is selected where it is docked (its dock opens if closed), or docked on the right when it is not. The icon bar highlights every tool on screen.
- ⌘J opens or closes the dock holding the tool shown last; ⌘⇧J lets that tool fill the window (each pane's ⋯ menu does the same for it).

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
- **Appearance**: System, Light or Dark (`appearance`); the dark theme, *Graphite* or *Warm*, a soft dark brown (`Preferences::dark_theme`, `PilotHouse Warm` in `themes/pilothouse.json`); names under tool icons.
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
