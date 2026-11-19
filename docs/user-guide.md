# User guide

A short tour of PilotHouse for people using it. For how it is built, see the [docs index](README.md).

## First steps

1. **Chat right away**, or **open a folder** (⌘O, or **+** in the column of icons). PilotHouse starts in a chat that needs no folder (*Chats*, in the column of icons). A folder you open becomes a **workspace**; it can hold more folders (*Add a folder* in its Files tool or the menu on its badge), or none.
2. **Pick an agent.** The model button in the chat composer opens the picker: the agents on the left, their models on the right. An agent that is not installed or signed in says what is missing; the empty chat shows a setup card with the steps.
3. **Ask.** Type in the composer and press Enter. Mention things with `@`: a file (`@src/main.rs`), a line (`@src/main.rs:42`), a function by name, `@screenshot` (the browser or device on screen), `@log`, `@pr:12`, `@test-run`, `@tabs` (the Chrome tabs you shared, with PilotHouse's Chrome extension), or a board card with `#12`.
4. **Approve.** When the agent wants to run a command or edit files, it asks in the chat (and in the title bar's inbox). The **Mode** button next to the model sets how much it may do without asking.

## The screen

- **The frame:** the title bar, the column of icons at the left edge and the status bar. With a workspace in front the frame takes that workspace's colour, faintly, so you can tell at a glance where you are.
- **Title bar:** the name of what is in front, search and commands (⌘K) in the middle, then the inbox and, with a workspace in front, the two switches of its screen: the chat and the result pane.
- **The column of icons:** what the window shows. The **Bridge** (⌘⇧E), **Reviews** (⌘⇧G), a badge for each workspace (its letter on its colour, with a dot when something there wants you; right-click for its menu: add folders, rename, recolour, open in a new window, remove), *Chats* (⌘⇧D) for chats that need no folder, **+** for a new workspace, and Settings at the foot.
- **The Bridge** is where the window starts: every task of every workspace in four columns, *Needs you*, *Working*, *Ready for review* and *Done*. A card opens its chat; *Review changes* opens it with what the agent changed beside it; *Done* puts it away. *New task* starts a chat in the workspace you pick. The status bar counts the same from anywhere (*2 need you · 1 working*).
- **A workspace's screen** is two panes: the **chat**, and one **result pane** under the names *Changes* (what changed, to review and commit), *Preview* (the browser, or a phone), *Terminal* and *Files* (the folder tree and the editor); the board, run configurations, tests, pull requests and the phone's log are under ⋯. Picking a file shows it in Files. Markdown opens formatted and editable like a document; images, audio, video, PDF, tables, notebooks, databases and more open with a viewer. See [previews](features/previews.md).
- **Give either pane the window:** ⌘B folds the chat to a strip at the edge (click it, or send something, to bring it back), ⌥⌘B hides the result pane, and the gap between them drags. The chat's header lists the workspace's other chats.
- **Right-click** a file or folder in Files for new, rename, move to the trash, copy and paste, reveal, Git, compare and chat (the empty space under the files for new file, new folder and paste); an editor tab to close others; selected code to add it to the chat or copy it with where it is from. Ctrl+click (⌘ on a Mac) picks more files, Shift+click a range; the menu and keys then act on all of them.
- **Chat menu** (the arrow at the top right of a chat): *Compact chat* frees the agent's context (Claude Code's own `/compact`, otherwise a fresh chat that starts from a recap), or continue the chat with another agent. Code blocks in replies have a copy button.

A workspace keeps its chats, shells and test runs going while you look at another one. The Bridge and the dot on its badge show what each is doing, and coming back shows what finished while you were away.

**Quick chat:** ⌥Space (Alt+Shift+Space on Windows and Linux) opens a small chat window over every app, from anywhere, while PilotHouse runs; press it again to hide it. Its chats are under *Chats*. Set the shortcut in Settings → Chat, or give `pilothouse quick-chat` a shortcut in your system's keyboard settings ([quick-chat.md](features/quick-chat.md)).

**Agents in a terminal:** start one from the terminal's **+** (or resume one of its sessions there). Once you ask it something it is listed with the chats; after its terminal closes it stays, with its last answer, and opens read only, to go on in a terminal or in a new chat.

## Reviews

*Reviews* in the column of icons (⌘⇧G) is for other people's pull requests, apart from your workspaces: those waiting for your review, your own, those assigned to you, and the repositories you follow (your workspaces' and any you add). A list is a queue, grouped by what an agent has made of each pull request: *Findings to read*, *Agents at work*, *Not started*, *Reviewed, nothing found*. **Tick the ones nobody looked at and press *Review N with an agent*** to start a review of each at once; each runs on its own copy of the repository and only reads, and nothing is posted until you say so. Open a pull request to read it, comment, approve or merge; one with findings opens on them. The chat beside it is that PR's: ask about the change, or start with a template (*Review this PR*, *Security review*, *Explain this PR*, *Test plan*, or your own from Settings → Templates). PilotHouse keeps its own copy of the repository for this, so your checkouts are never touched, and these chats only read. A review's findings go to the pull request's *Findings* tab: post them (on the code, with one-click suggestions, and one summary comment the author ticks), read the author's answers back, check the fixes after a push, get a second opinion or a double-check, fix them in a task, or open issues. *Walkthrough* briefs you on a change before you review it (what changes and why, how that part of the system works today, what is different after, where to look closely), and *Watch* tells you when the author pushes or answers. **Review** in the Git tab reviews your own changes before you push. A repository can add its own review rules in `.pilothouse/review.md`. Add a GitHub account first in Settings → GitHub. See [pull requests](features/pulls.md).

## Code on a network drive

If your code lives on a server and you reach it as a mapped drive (`Z:\`) or a mounted share, open the folder as usual, then click **Network drive** in the status bar (or Settings → Network drives). PilotHouse checks the server with you, and from then on Git, terminals and run configurations work on the server over SSH; the status bar says **SSH · server**. Files are still read and saved through the drive, and agents run on this computer. See [network drives](features/network-drives.md).

## Shortcuts

**⌘/** shows every shortcut in the app.

| Keys (Ctrl on Windows and Linux) | Does |
|---|---|
| ⌘K | Search or run a command |
| ⌘O | Open a folder as a workspace |
| ⌘N | New chat |
| ⌘⇧K | Search chats |
| ⌘P / ⌘⇧F | Go to a file / search the workspace |
| ⌘B / ⌥⌘B | Fold the chat away / hide the result pane |
| ⌘J / ⌘⇧J | The terminal / the result pane fills the window |
| ⌘⇧E / ⌘⇧D / ⌘⇧G | The Bridge / Chats / Reviews |
| ⌘1…⌘9 | Files, Git, Board, Terminal, Browser, Devices, Run, PRs, Tests |
| ⌃1…⌃9 (Alt+1…9 on Windows and Linux) | Workspace 1…9 in the column of icons |
| Ctrl+Tab | The workspace shown before |
| ⌘⇧S | Send the browser or device screen (with your marks) to the chat |
| ⌘L | Ask about the selected code or terminal output |
| ⌘, | Settings |
| ⌥Space (Alt+Shift+Space) | Quick chat, from any app (set in Settings → Chat) |
| ⌘/ | All shortcuts, read from the keys in force |

## Your data

Everything is stored on your computer, in PilotHouse's data folder (on macOS `~/Library/Application Support/PilotHouse`, on Windows `%APPDATA%\PilotHouse`, on Linux `~/.local/share/PilotHouse`): one SQLite database, browser profiles, captures, and test reports. Settings → Storage shows what takes the space: GitHub clones and their pull request copies, tasks' worktrees, review copies, recordings, logs and the browser's data, each with its size and a way to remove it (a task's removal also takes its workspace out of PilotHouse). Clones, copies and worktrees can go to another folder, on a bigger disk, chosen there; those made before stay where they are until removed. Tokens for GitHub are kept in the system's credential store (Keychain on macOS, Credential Manager on Windows; on Linux, a file in the data folder only you can read). Nothing is sent anywhere except to the agents you run, Git remotes, and the GitHub hosts you add.
