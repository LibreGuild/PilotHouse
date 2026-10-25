# User guide

A short tour of PilotHouse for people using it. For how it is built, see the [docs index](README.md).

## First steps

1. **Chat right away**, or **open a folder** (⌘O, or *Open a folder…* in the sidebar). PilotHouse starts in a chat that needs no folder (the sidebar's *Chats*). A folder you open becomes a **workspace**; it can hold more folders (*Add a folder…* in its Files tool or its menu in the sidebar), or none.
2. **Pick an agent.** The model button in the chat composer opens the picker: the agents on the left, their models on the right. An agent that is not installed or signed in says what is missing; the empty chat shows a setup card with the steps.
3. **Ask.** Type in the composer and press Enter. Mention things with `@`: a file (`@src/main.rs`), a line (`@src/main.rs:42`), a function by name, `@screenshot` (the browser or device on screen), `@log`, `@pr:12`, `@test-run`, or a board card with `#12`.
4. **Approve.** When the agent wants to run a command or edit files, it asks in the chat (and in the title bar's inbox). The **Mode** button next to the model sets how much it may do without asking.

## The screen

- **Title bar:** the two views (*Workspaces* and *GitHub*), search and commands (⌘K), the inbox, light or dark, the tool panel, settings.
- **Left sidebar:** *New chat*, then *Chats* (chats outside any workspace) and every workspace with what it is doing; unfold one to see its chats and pick one to open it there. A workspace's menu (⋯ on hover) adds folders, renames it, recolours it, opens it in a new window or removes it. Chats PilotHouse started are under *Runs*. ⌘B hides the sidebar.
- **Editor:** opens beside the chat when you pick a file.
- **Chat** in the middle.
- **Tools** on the right, chosen from the icon bar: Files, Git, pull requests, the board, terminals, the browser, devices, run configurations, UI tests. Only the tools a workspace uses show; the rest are under ⋯.

A workspace keeps its chats, shells and test runs going while you look at another one. The sidebar shows what each is doing, and coming back shows what finished while you were away.

## The GitHub view

*GitHub* in the title bar (⌘⇧G) shows what you would see on GitHub, apart from your workspaces: the pull requests waiting for your review, your own, those assigned to you, and the repositories you follow (your workspaces' and any you add). Open a pull request to read it, comment, approve or merge. The chat beside it is that PR's: ask about the change, or start with a template (*Review this PR*, *Security review*, *Explain this PR*, *Test plan*, or your own from Settings → Templates). PilotHouse keeps its own copy of the repository for this, so your checkouts are never touched, and these chats only read. Reviews list each finding with its evidence and fix, ready to post as line comments. A repository can add its own review rules in `.pilothouse/review.md`. Add a GitHub account first in Settings → GitHub. See [pull requests](features/pulls.md).

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
| ⌘B | Show or hide the sidebar |
| ⌘J / ⌘⇧J | Show or hide the tools / let them fill the window |
| ⌘⇧E / ⌘⇧G | The Workspaces view / the GitHub view |
| ⌘1…⌘9 | Files, Git, Board, Terminal, Browser, Devices, Run, PRs, Tests |
| ⌃1…⌃9 (Alt+1…9 on Windows and Linux) | Workspace 1…9 in the sidebar |
| Ctrl+Tab | The workspace shown before |
| ⌘⇧S | Send the browser or device screen (with your marks) to the chat |
| ⌘L | Ask about the selected code or terminal output |
| ⌘, | Settings |
| ⌘/ | All shortcuts, read from the keys in force |

## Your data

Everything is stored on your computer, in PilotHouse's data folder (on macOS `~/Library/Application Support/PilotHouse`, on Windows `%APPDATA%\PilotHouse`, on Linux `~/.local/share/PilotHouse`): one SQLite database, browser profiles, captures, and test reports. Tokens for GitHub are kept in the system's credential store (Keychain on macOS, Credential Manager on Windows; on Linux, a file in the data folder only you can read). Nothing is sent anywhere except to the agents you run, Git remotes, and the GitHub hosts you add.
