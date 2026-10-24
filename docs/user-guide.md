# User guide

A short tour of PilotHouse for people using it. For how it is built, see the [docs index](README.md).

## First steps

1. **Open a folder** (⌘O, or *Open a folder* on the welcome screen). It becomes a **workspace**. A workspace can hold more folders (*Add a folder…* in the Files section or the workspace menu), or none, just to chat or use the browser.
2. **Pick an agent.** The model button in the chat composer opens the picker: the agents on the left, their models on the right. An agent that is not installed or signed in says what is missing; the empty chat shows a setup card with the steps.
3. **Ask.** Type in the composer and press Enter. Mention things with `@`: a file (`@src/main.rs`), a line (`@src/main.rs:42`), a function by name, `@screenshot` (the browser or device on screen), `@log`, `@pr:12`, `@test-run`, or a board card with `#12`.
4. **Approve.** When the agent wants to run a command or edit files, it asks in the chat (and in the title bar's inbox). The **Mode** button next to the model sets how much it may do without asking.

## The screen

- **Title bar:** the workspace menu (switch, make, rename, colour, open in a new window), search and commands (⌘K), the inbox, light or dark, the tool panel, settings.
- **Left sidebar:** the workspace's files on top, its chats below (grouped by day; chats PilotHouse started are under *Runs*). ⌘B hides it.
- **Editor:** opens beside the chat when you pick a file.
- **Chat** in the middle.
- **Tools** on the right, chosen from the icon bar: Git, pull requests, the board, terminals, the browser, devices, run configurations, UI tests. Only the tools a workspace uses show; the rest are under ⋯.

A workspace keeps its chats, shells and test runs going while you look at another one. The workspace menu shows what each is doing, and coming back shows what finished while you were away.

## Shortcuts

| Keys (Ctrl on Windows and Linux) | Does |
|---|---|
| ⌘K | Search or run a command |
| ⌘O | Open a folder as a workspace |
| ⌘N | New chat |
| ⌘⇧K | Search chats |
| ⌘P / ⌘⇧F | Go to a file / search the workspace |
| ⌘B | Show or hide the sidebar |
| ⌘J / ⌘⇧J | Show or hide the tools / let them fill the window |
| ⌘2…⌘9 | Git, Board, Terminal, Browser, Devices, Run, PRs, Tests |
| ⌃1…⌃9 (Alt+1…9 on Windows and Linux) | Workspace 1…9 |
| Ctrl+Tab | The workspace shown before |
| ⌘⇧S | Send the browser or device screen (with your marks) to the chat |
| ⌘L | Ask about the selected code or terminal output |
| ⌘, | Settings |

## Your data

Everything is stored on your computer, in PilotHouse's data folder (on macOS `~/Library/Application Support/PilotHouse`, on Windows `%APPDATA%\PilotHouse`, on Linux `~/.local/share/PilotHouse`): one SQLite database, browser profiles, captures, and test reports. Tokens for GitHub are kept in the system keychain (Keychain, Windows Credential Manager, or the Secret Service on Linux). Nothing is sent anywhere except to the agents you run, Git remotes, and the GitHub hosts you add.
