# Workspace shell

Crate: `ph-workspace`. Code: `crates/workspace/src/`.

```
┌ title bar: ◆ ● name · ⌘K search · ⋯ · inbox · chat · result pane ┐
│ ⎈ ╭──────────────────────╮ ╭─────────────────────────────────────╮ │
│ ⌥ │ chat                 │ │ Changes · Preview · Terminal · Files │ │
│ ─ │                      │ │ ⋯                                    │ │
│ A │                      │ │ the result pane: one tool at a time  │ │
│ B │                      │ │                                      │ │
│ 💬 │                      │ │                                      │ │
│ + ╰──────────────────────╯ ╰─────────────────────────────────────╯ │
│ ⚙                                                                    │
└ status bar: branch · CI · folder · ports · tasks        agent limits ┘
```

- **Three places, one column of icons.** The window shows the **Bridge** (every task, by state), **Reviews** (other people's pull requests) or **a workspace's screen** (a task: its chat and what it produced). They are picked in the column of icons at the left edge; there is no list beside it and no column of tools on the right.
- **The status bar is one line of small text with no room of its own above it**: the islands end a gap (6 px) over it, and that gap is the room above. (It had 4 px more, which put its text under the strip's middle and cost the islands that height.)
- **The frame and its islands** (`frame.rs`). The title bar, the column of icons and the status bar are one piece, the window's frame, with no line between them: what stays the same whatever is shown. What changes sits in it as islands, rounded panels with a small gap between them: the chat and the result pane (in Reviews: the list, the page, the pull request's chat; the Bridge is one island). The frame is lit from above, a little lighter at its top than at its bottom, in the theme's own colour. **With a workspace in front the light has that workspace's colour**, strongest in the title bar and fading down the column (and the workspace's dot stands before its name in the title bar), so the window's edge says where a new chat would go; the Bridge and Reviews belong to no workspace and stay plain. The colour is faint on purpose: it says which workspace, and is not there to be looked at. A window whose frame shows the desktop or a picture (Settings → Appearance → Window) has that behind it instead of the light, the workspace's colour over it.
- **Rounded corners.** The UI framework clips children to a panel's rectangle, not to its rounding, so a tab bar or a terminal that fills an island would poke out of its corners. An island (`frame::island`, a `ph_ui::Surface` told what is behind it) paints its corners over with the frame's colour at that height, worked out from the same numbers the frame is painted with (`Paint::at`). No such fill where the frame is not a colour (the desktop, a picture).
- **`Workspace`** (`workspace.rs`) is a window's root: the title bar, the column of icons, what is in front (`View`: `Bridge`, `Workspaces` for a workspace's screen, `GitHub` for Reviews, `Custom` for an organisation's view) and the status bar. It keeps one `WorkspaceScreen` per workspace opened in the window, and the window's [Reviews view](pulls.md#reviews) once shown. Which of the Bridge or a workspace was in front is kept (`workspace.front_view`); a window opens on the Bridge when there are workspaces, and on the chat when there are none yet.
- The look follows [the style guide](../styleguide.md): quiet graphite chrome, colour only for state, focus and selection.
- **`WorkspaceScreen`** (`workspace_screen.rs`) is a workspace's screen: the chat and one result pane (`workspace_screen/layout.rs`). Panels are made the first time they are shown. Its state is saved per workspace (`ProjectUiState`): the tool shown last, whether the result pane is open, whether the chat is folded and how wide it is, whether the folder tree shows, what Preview showed last, the chat and the open files.
- **Running work outlives the screen.** Chats (`ph_agent::chats`), shells (`ph_terminal::TerminalHub`), test runs (`ph_uitest::runner`) and schedules (`schedules::Automation`) belong to the workspace or the app, not to a window. A screen can be closed and rebuilt (another window, folders changed, idle) without stopping anything.
- **Actions** (`actions.rs`) are the single source for every command. Shortcuts, the palette, menus and buttons all dispatch them.

## Title bar

- On the left, the app's mark (its icon in small) beside the window buttons (off macOS, where the window's buttons are on the right, it opens the app menu and stands centred over the column of icons under it) and the name of what is in front: *Bridge*, *Reviews* or the workspace (with its colour dot). The task in the chat is not named here: its title and state stood after the workspace's name for a while and crowded the corner; the chat's own header says both.
- In the middle, search and commands (⌘K).
- On the right: the next step of the task in front, when there is one (*Review changes* while its chat changed files and another tool is in front; else the pull request of its branch, `#42`), updates, the inbox (bell), and with a workspace in front the two switches of its screen: the chat (⌘B) and the result pane (⌥⌘B). In Reviews the one switch shows or hides its list.
- The bar has no fill or line of its own: it is part of the frame (above), which carries the colour of the workspace in front (`ph_ui::WORKSPACE_COLORS`, a chosen one saved in `projects.color`).

## The column of icons

`rail.rs`. From the top:

- **Bridge** (⌘⇧E, also ⌘⇧A), **Reviews** (⌘⇧G), which shows how many pull requests wait for your review, and **Chats** (⌘⇧D), the chats that need no folder (below); then an organisation's own views. These are always there, in these places. A line parts them from the user's workspaces.
- **One badge per workspace of the user's**, newest first (an order that stays put as they are used): its first letter on its colour, a ring when it is in front, and a dot for what goes on there (amber: an agent waits for an answer; red: a turn failed; green: news not seen; blue: working). The tooltip says the name, what it is doing and its shortcut (⌃1–⌃9, Alt+1–9 off macOS). A click opens the workspace; right-click is its menu: *New chat*, *New task*, *Add a folder*, *Rename*, *Open in new window*, *Colour*, *Finish task*, *Remove from PilotHouse*. The badges scroll when there are many.
- **+**: open a folder as a workspace, or make one without folders.
- **Settings**, at the foot.

Names show under the icons; *Settings → Appearance → Names under the icons* switches them off, and they show in tooltips then. (They were off unless switched on before 0.7. The setting is kept under a new key, `rail_names`, so everyone starts with the names after updating; the old `tool_labels` is no longer read.)

## The Bridge

`bridge.rs`. The window's home, an island of the frame like every other view (its cards a step off the island's own fill): **every task of every workspace, by what it needs from you**. A task is a chat (or an agent CLI in a terminal, or a review being readied). Four columns:

| Column | What is in it |
|---|---|
| **Needs you** | an agent waits for an answer, or its last turn failed (until it is marked done) |
| **Working** | an agent is at it, also a review whose repository is still being cloned |
| **Ready for review** | the agent finished, or the chat is idle and nobody marked it done |
| **Done** | marked done, or untouched for a week; and the runs PilotHouse started itself (a schedule's, a test's recording or repair, a review of your changes) once their news was read |

- A card shows the workspace (its colour dot) and the agent, the title, how long ago, and what the agent is doing, asks, or said last. A click opens the workspace at that chat. *Answer* does the same for one that waits; **Review changes** opens it with Git (the changes of its workspace) beside the chat; **Done** marks it done (`workspace.sidebar_settled`), its news seen. One that works or asks again comes back on its own. The ⋯ on hover pins it to the top of its column, renames it, or deletes it with its transcript after asking (not while its agent works or waits).
- **A session in its agent's CLI is a task like a chat.** Its card (*workspace · terminal*) has **Done** and the same ⋯: marked done it goes under *Done* while its CLI stays open, and comes back when it works again; renamed, its CLI's card and tab go by the new name. Deleting one the chat shows as a CLI closes that CLI with it; one running in a terminal of the Terminal tab is deleted once its terminal is closed (*Delete (close its terminal first)*), since it would be left running with nothing to keep what it says. An agent in a terminal that was never asked anything has no session yet, and so no menu.
- **What a task is besides its chat** shows in a row of its card (`task_facts.rs`, `ph_store::sessions::SessionFacts`): the branch its folder is on, what the chat changed (`+87 −12 6 files`), how it checked that (`14 tests passed`, `2 screenshots`), and the open pull request of the branch with a dot for its checks (green passed, red failed, amber running), which opens it in the browser. The chat's own numbers are read off its tool calls and kept with its stored row when a turn ends (`sessions.facts`), so no transcript is read for a card. Branch, pull request and checks are found in the background for the folders the window shows tasks of: the branch every 15 seconds, the pull request and checks every three minutes, and only where the repository is hosted and an account is connected.
- **A question answered by picking an option** (a permission: *Allow once*, *Always allow*, *Deny*) shows the options on its card as the agent gave them, with *Open* to read the chat first. A question with a form, or a plan to approve, opens the chat (*Answer*).
- **Chips** over the columns narrow the Bridge to one workspace. **New task** starts a chat: in a workspace (*In <name>*), without a folder (under Chats), or **on a branch of its own** (*From <name>…*, for a workspace that is a Git repository: the task's branch and worktree are made and opened as a workspace, see Tasks below); it also opens a folder or makes a workspace.
- *Ready for review* starts with a link to Reviews when pull requests wait for your review. Its ⋯ (with two or more waiting) marks them all done at once: someone who comes from a version without the Bridge finds their last week of chats there.
- A column lists twelve cards, then *Show N more*. In a narrow window the columns become two rows of two, then one column.
- With nothing under way the Bridge says how to begin: open a folder, or start a chat without one.
- The status bar's *N need you · N working · N to look at* counts the same cards from anywhere, and a click on it shows the Bridge.
- The chats of a workspace whose screen is closed are read from the store each second, so a schedule's new chat shows up.

- **A command for "this workspace"** (the palette's *Add a folder to this workspace*, *New task in a worktree*, a tool's key) given on the Bridge or in Reviews first shows the workspace it means, the one last in front. Removing or finishing that one leaves another ready behind the Bridge (the one shown before, else the Chats space).

The Bridge takes the place of the lists that stood beside the workspace (workspaces, their chats, the pinned ones) and of the *Agents* pop-up. Those listed chats by where they live; what one wants to know on opening the window is which of them need something.

## Notifications

`attention.rs`, one watcher for the app: when a chat starts waiting for an answer, finishes, or stops with an error while it is not the chat on screen, PilotHouse posts a system notification (clicking it opens that chat). On macOS the Dock icon shows how many chats wait for an answer. The ⌘K palette starts with **Chats**: those waiting, working or finished first, then every chat, newest first.

## Tasks

A **task** is a branch in its own worktree, opened as a workspace, so an agent can work on it while the main checkout stays as it is (`tasks.rs`). *New task* (a workspace's menu on its badge, or ⌘K *New task in a worktree*) asks for a name; PilotHouse makes branch `task/<name>` from the folder's HEAD in a worktree under `tasks/` in the folder for copies (the data folder, unless another was chosen in Settings → Storage), opens it as *Repo · Name*, and runs its setup in a terminal named *Setup*. The repository can say how in a checked-in `pilothouse.toml`:

```toml
[worktree]
setup = "npm install"          # run in the task's Setup terminal
copy = [".env", ".env.local"]  # files copied from the main checkout
share = ["node_modules"]       # folders linked to the main checkout's
```

The setup sees `PILOTHOUSE_PORT`, a port of the task's own (4100, 4110, …) for its dev server. Only your own repository's `pilothouse.toml` is followed: for a task made from a copy PilotHouse made (a GitHub clone, or a task's folder, which can hold a pull request author's files) nothing is copied, linked or run, and a notice shows the setup command to read and run by hand ([security.md](../security.md#reviews-clones)). *Finish task* (the workspace's menu, ⌘K) removes the worktree and the workspace after asking: files PilotHouse copied in go if unchanged and links go; anything else not committed makes Git refuse, and the task stays. Its branch is deleted too once merged (`git branch -d`), else kept. When the branch has been merged into its base, the status bar offers **Merged · Finish task**. Which workspaces are tasks is kept in the `workspace.tasks` setting.

## Status bar

Left: the branch (ahead/behind, an operation under way; opens the branch picker), **CI** for the commit checked out (a check when every GitHub check passed, a spinner while some run, red with the count when one failed; the tooltip names them, a click opens the failed or running one), the folder, the network drive, and the **ports** the workspace's processes listen on (opens the Run tool). `signals.rs` reads ports every 10 seconds and CI every minute or when HEAD moves (seen in the Git model, without running Git), only for the workspace on screen and not on network drives; while PilotHouse is behind another program both are read once a minute, and ports not at all on Windows, where the system does not say which folder a listener was started in; the Android bridge and emulator are left out (the Devices tool has them). Right: agent limits and the detected stack. (The version is in Settings → About.)

## Agent limits

When an agent reports how much of its subscription window is used, the status bar shows a small meter per window, amber near the limit and red at it, with the reset time in the tooltip (`ph_agent::rate_limits`). Claude Code reports its 5-hour and weekly windows in chat as a limit comes close; Codex writes its windows into its own session files (`$CODEX_HOME/sessions`), which PilotHouse reads once a minute, whatever ran Codex. Nothing is fetched for this. Gemini CLI and Antigravity report none.

## Chats: the chats that need no folder

A chat with no project of its own goes under **Chats**, and is found there: a quick chat, *New task → Without a folder*, ⌘⇧D (*Show Chats*), and the first chat of a first start.

Underneath it is a workspace without folders, where agents work in a private scratch folder, so its screen is a workspace's: tabs for its chats, the terminal, the browser, the Bridge's cards. `Store::scratch_workspace` returns the one noted in the setting `workspace.scratch`, making it when there is none; every window makes sure of it as it opens (`Workspace::chats_place`).

But it is not one of the user's projects, and does not look like one:

- **It has a place of its own**, at the top of the column of icons under the Bridge and Reviews, with an icon. It is not a letter on a colour among the workspaces, moving down as they are added, and it is not counted in their numbers (⌃1–9 are the user's workspaces, `Workspace::listed`).
- **It is not removed.** Its menu has no *Remove*, the command says why, and `Store::remove_project` refuses. Its chats are deleted one by one, from a card's or a tab's menu.
- **It takes no folder.** *Add a folder* while it is in front opens the folder as a workspace of its own (`Store::add_workspace_folder` refuses); its menu has no *Add a folder* or *New task in a worktree*.

It can be renamed and moved to a window of its own, from its right-click menu.

A chat made there away from the window (the quick chat's, as it is sent) opens as itself when asked for: the window reads the workspace's chats again for a chat it does not know (`WorkspaceScreen::select_session`), and its workspaces again for a workspace it does not know (`Workspace::open_chat`). A chat of a workspace shown in another window opens in that window.

Before 0.7 it was a space of a kind of its own (a row of kind `chats`), with exceptions wherever workspaces were handled and a lesser screen. Migration 17 turned that row into this workspace; its chats and its name stayed.

## A workspace's screen

`workspace_screen/layout.rs`, `workspace_screen/render.rs`. Two islands side by side: the **chat**, and one **result pane** for what the task produced, under a row of names.

- **Git**: the review of what changed, with the repository's branches, history and stashes; its list of changed files has one diff of them beside it ([Git](changes.md)). **Preview**: what the work looks like when it runs, the browser or a phone (a switch at the end of the row picks, and the name remembers which). **Terminal**. **Files**: the folder tree and the files open from it, side by side (the button at the end of the row hides the tree). The workspace's other tools (Run, Tests, Pull requests, Logcat) are under **⋯**, and the one in use shows as a name of its own beside the four. A name shows only for a tool the workspace has a use for (*Git* with a Git repository; Git is under ⋯ where none was seen) and that is switched on; *Preview* stands for whichever of the browser and the phone is.
- **One tool at a time**, with the whole pane: the browser and a phone need the room. Each tool was a tab in a dock before, which could be dragged anywhere; few arranged it, an arrangement saved could come back wrong, every tool was named twice (its icon in a column and its tab), and the wide tools sat in a narrow column.
- **Files** is the first tool (⌘1): the folder tree, each folder a root when there are several (the primary one says so), with an icon per file type, chevrons, indent guides and Git marks (**M** changed, **U** new, **D** deleted; a dot on a folder holding changes). Its header's search button (⌘⇧F) turns the tree into workspace search; the others make a file or a folder, fold every folder and add a folder to the workspace. Right-click any row for the file menu ([files](files.md)); a folder root's also makes it primary or takes it out. Picking a file (from the tree, ⌘P, search, a link in the chat) shows Files with that file in the editor.
- **Sizes.** The chat starts at two fifths of the window (never under its least width, 380 px, unless the window is too small for both panes); drag the gap between the two to change it. Where it is left is kept per workspace, as a share of the window's width (`chat_share`), so it holds in a window of another size.
- **Showing and hiding.** ⌘B (and the title bar's left switch) folds the chat to a strip at the pane's edge, so the result pane has the window; the strip says what the chat is doing and brings it back, as does anything sent to the chat. ⌥⌘B (the right switch) hides the result pane, and brings it back as it was. ⌘⇧J gives the result pane the window in one step. ⌘J and Ctrl+backtick show the terminal; pressed again they go back to what was there (the chat alone, when the result pane was shut). Something is always on screen: with the result pane hidden the chat cannot be folded. With the chat folded the keyboard goes to the file in front, else to the window (`WorkspaceScreen::focus_chat`), never to the composer that is not drawn: no shortcut would reach the window from there. A chat asked for (a card on the Bridge, ⌘N) unfolds it. *Reset the layout* (the palette) shows both again.
- **Several chats are tabs over the chat** (`workspace_screen/tabs.rs`, drawn in the chat panel's header): one tab for each chat of the workspace that is still open (what the Bridge has outside *Done*) and for the one in front, in the order they were started, each with its state (the working mark, or a dot: amber needs you, red stopped, green finished and not looked at). A click shows the chat; the cross on a tab (shown while the pointer is on it) closes the tab and leaves the chat as it is, in *Past chats* and on the Bridge, until it is opened again or needs you; right-click is the chat's menu (done, rename, delete), and a chat marked done leaves the row. At the end of the tabs are *Past chats* (the clock: the chats last used, where a chat is also marked done, renamed or deleted, and the way to all of them) and *New chat* (⌘N). In a narrow panel the tabs are a row of their own over the row of the chat in front; from 760 px the two are one row ([chat](chat.md)).
- **Layouts from before.** An arrangement of the dock saved by 0.6.4 or earlier (`workspace.dock.<id>`) is not read any more; the tool shown last, whether the pane was open, the chat and the open files come back as they were.

## Several workspaces

- Switch with the badges in the column of icons, with ⌃1–⌃9 (macOS) or Alt+1–9 (elsewhere) for the first nine, or Ctrl+Tab for the one before.
- Coming back to a workspace shows **While you were away**: the chats that finished and a test run that ended, one click each.
- Clicking a schedule's or reminder's system notification opens its workspace (and the chat it ran).
- **Removing a workspace** (*Remove from PilotHouse*, its menu) removes its chats with it and leaves its folders alone. With any chats it says how many and asks first.
- **Open in new window** moves a workspace to its own window (`windows.rs`). A workspace shows in one window at a time: picking it in another window brings its window forward. Files with unsaved changes must be saved first.
- A workspace hidden for 30 minutes with nothing going on and nothing unsaved has its screen closed to keep memory flat; opening it rebuilds the screen, and its chats and shells were running all along.

## Workspaces and folders

A **workspace** (a `Project` in the code, table `projects`) is a name and any number of folders, from none up.

- *New workspace* makes an empty one; add folders whenever you like (the Files tool, the workspace's menu on its badge, the palette). *Open folder* (⌘O) is a shortcut that makes a workspace with that folder, or reopens the one that has it.
- The first folder is the **primary** one: agents work there, and the terminal and run tools start there. Every folder:
  - is a root of Files, its paths named with the folder first (`server/src/main.js`), in ⌘P, search and `@` mentions too (`ph_files::roots`);
  - is given to agents as an extra root (ACP `additionalDirectories`), and a chat's first message tells the agent the folders and how paths name them;
  - has its own Git and PRs tools: a **Folder** picker on top of those tools switches between them, and the status bar's branch names the folder (`server · main`).
- **No folders**: for driving the browser and devices, or thinking something through, with a workspace of its own (plain chats go under Chats, which is one of these underneath). Agents work in a private scratch folder (`<data dir>/workspaces/…`); Files, Git and PRs say a folder is needed and offer to add one. The first folder added becomes the primary one, and a workspace still called *Workspace N* takes its name.
- The **Run** tool finds each folder's run configurations and names them after their folder (`server: dev`). The Terminal's **+** asks which folder a new shell starts in.

Removing a folder leaves it on disk; removing the primary folder makes the next one primary, and removing the last leaves the workspace without folders. Folders change live: Files, Git and the tools that read folders start again; chats and shells keep going, and new agent sessions get the new folders. *Open with PilotHouse* on a folder opens the workspace that has it among its folders; a file opens in the workspace whose folder holds it (the deepest one).

## Shortcuts

| Keys | Action |
|---|---|
| ⌘K / ⌘⇧P | Command palette |
| ⌘O | Open a folder as a workspace |
| ⌘N | New chat |
| ⌘⇧K | Search chats |
| ⌘B | Fold the chat away, or bring it back (in Reviews: its list) |
| ⌥⌘B | Show or hide the result pane |
| ⌘J / ⌃` | The terminal, and back to what was there |
| ⌘⇧J | The result pane fills the window |
| ⌘⇧E / ⌘⇧D / ⌘⇧G | The Bridge / Chats (the chats that need no folder) / Reviews |
| ⌘1 | Files |
| ⌘2, ⌘4–9 | Git, Terminal, Browser, Devices, Run, PRs, Tests (⌘3 was the board's, and is free) |
| ⌃1–⌃9 (Alt+1–9 off macOS) | Workspace 1–9 in the column of icons |
| Ctrl+Tab | The workspace shown before |
| ⌘P / ⌘⇧F | Go to file / search in workspace |
| ⌘, | Settings |

## Settings

Stored in the `settings` table:
- **Appearance**: the theme (`appearance`: the system's light or dark one, or a theme by name), accent, density; see [Themes](themes.md), with designing your own and importing; names under tool icons.
- **Features**: turn tools off (`features.disabled`); *System* holds the file manager menu switch.
- **Zoom**: ⌘= and ⌘- (Ctrl+= and Ctrl+- elsewhere) make the interface, code and terminal text half a point larger or smaller together, kept within each size's bounds; ⌘0 (Ctrl+0) goes back to the default sizes (`Preferences::zoom`). Saved with the other appearance preferences. Back in the editor keeps ⌃- on a Mac and Alt+← elsewhere, so the keys do not meet.

## Architecture map

*Architecture map* in the palette (`arch_map/`) draws the project's parts in layers, foundations at the bottom: the members of a Cargo or npm/yarn/pnpm workspace with their internal dependencies, or the top-level folders otherwise. Each box shows lines of code and main languages (manifests and docs are not counted); a line goes from a part to what it uses directly, transitive ones left out so the picture stays readable. Read from the manifests on this computer, nothing run. *Explain with agent* sends the map to the chat and asks for a short explanation of each part, the main flows and where to start.

## Opening from outside

`open.rs`, `file_window.rs`, `shell_integration.rs`.

- **`pilothouse <path>...`**: a folder opens as a workspace (the listed one if it is already there), a file in a workspace's folder opens in its editor, and any other file opens in a **window of its own**: just the file in the editor (or the large-file viewer), like a notepad, with **Open folder** to make its folder a workspace. What the file's kind offers is in a bar under the title: **Edit** for a picture, a video or a PowerPoint deck (Studio, with no workspace needed), the Markdown views, *Format* for JSON, *Open in Default App*. Opening the same file again focuses its window.
- **One app at a time**: a second launch with paths sends them to the running app over the control socket (`app.open`) and exits, so there is never a second copy of the window or the agents. The place of the one app is taken first of all, with a lock file beside the database (`pilothouse.lock`), so two launches at the same moment (several files opened at once from Explorer, a double click) cannot both become the app: the later one waits for the first to answer, hands its paths over and exits.
- **File manager**: *Settings → Features → System* adds "Open with PilotHouse" for the current user only. Windows: Explorer verbs under `HKCU\Software\Classes` for folders, folder backgrounds and files (on Windows 11 under *Show more options*). Linux: a `.desktop` entry in `~/.local/share/applications` (folders and text files). macOS: Finder's *Open With* and `open -a PilotHouse <path>` come from the app bundle's document types; the app receives them as file URLs (`on_open_urls`).

## Adding a tool

1. Create a feature crate with a view entity. Anything that must keep running when the panel closes (a process, a task) goes in a model held by a global keyed by workspace, like `TerminalHub`.
2. Add a `ContextTab` variant (`context_panel.rs`) with a label, icon, key and action, and say when it is relevant in `WorkspaceScreen::is_relevant`.
3. Create the panel in `WorkspaceScreen::ensure_panel` and render it in `render_tab_content`. It shows under **⋯** over the result pane, and as a name there once it is in use.
