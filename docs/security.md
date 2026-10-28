# Security

PilotHouse runs on your machine with your permissions and has no server. The things worth protecting are your chats, the browser's logins, your code, and what agents can do. This page lists each way in and how it is closed.

## Local surfaces

| Surface | Exposure | Protection |
|---|---|---|
| Data folder (chats, browser profiles, captures) | Files on disk | Created `0700`, and tightened if an older version made it wider (`store::paths::ensure_private_dir`) |
| Control socket (MCP bridge, `pilothouse ctl`) | A Unix socket | Owner-only (`0600`), in the data folder or a private per-user temp folder (`$XDG_RUNTIME_DIR` on Linux). Clients refuse a socket whose folder is not the user's own (`verify_private_parent`), so another account cannot plant one |
| Browser DevTools | Full control of a logged-in browser | A private pipe (`--remote-debugging-pipe`), not a localhost port that every local user could reach. A port opens only if you turn on **Allow other DevTools clients** for that project |
| Emulator gRPC | The emulator's own endpoint | Uses the token the emulator writes in its user-only discovery file |
| Agents' child processes | Run with your permissions | Adapters are pinned versions (`agents.rs`), so a new npm release never runs unannounced |
| SSH to network drives' servers | Commands run as you on a server | The system `ssh` with your keys; never a stored password. Commands use `BatchMode` and `StrictHostKeyChecking=yes`, so they never prompt or accept an unknown or changed server. A new server is trusted only after its fingerprints are shown. Hosts that `ssh` could read as an option are refused, and every word sent to the server's shell is quoted. Only a validated public key line is ever installed on a server |
| Shared SSH connections (macOS, Linux) | A socket that runs commands on the server as you | In a private (`0700`) folder; closed at quit or after 10 idle minutes |
| Tunnels to a server's ports | A port on this computer | Bound to `127.0.0.1`; each ends when PilotHouse does |
| Terminal text kept between runs | Up to 2,000 lines of each terminal, which can include whatever a program printed | Plain text in the data folder (`terminals/`, so `0700`), read when that workspace's terminals are next wanted and then deleted (also when the workspace is removed); colours and escapes are dropped, so nothing replays as input |
| Agents in a terminal | Hooks call PilotHouse; their session files are read | The hook settings file is per launch in the data folder and removed at the next start; paths typed into the shell are single-quoted. Session files of Claude Code, Codex and Gemini CLI are only read |
| Updates | A new program replaces this one | Only `latest.json` and files under `https://github.com/LibreGuild/PilotHouse/releases/download/` (HTTPS, no `..` or escapes in the path) are fetched; nothing is sent. The version must be a plain one, since it names the download folder. Each file must match its SHA-256 before it is unpacked, and it is put in place only after PilotHouse quits. Turn checks off in *Settings → About* ([packaging.md](packaging.md#updates)) |

## Agent boundaries

- **One workspace's tools.** The MCP bridge PilotHouse starts is given its workspace (a review or a parallel lane works in a copy no workspace folder holds); one you set up yourself resolves it from the agent's folder (any of its folders). The bridge adds it to every call, overriding anything the agent passes. Board calls that take a card id check the card belongs to it; to-dos and tests are its own.
- **Mentions stay in the workspace.** `@path` and `@path:line` attach only files inside the workspace's folders (symlinks resolved), and `#12` only its cards.
- **Utilities.** An agent's `util_run sqlite` opens only SQLite files inside its workspace's folders, read-only; the Utilities dialog (you) can open any.
- **Text from others is data.** A pull request's title, author and description reach agents (reviews, `@pr:`, schedules started by a pull request or failing checks) marked as material to work on, never instructions; reviews report such text as a finding.
- **Done needs tests** is checked for every way a card moves (drag, editor, agent), so an agent cannot mark its own card done while its tests fail.
- **No silent confirmations.** Web pages' `confirm()` and `prompt()` dialogs are shown to you in the pane. With nobody watching they are cancelled, so an agent driving the browser can never confirm "Delete…?" by itself. Alerts are acknowledged, and every dialog is logged for the agent.
- **Links in agent output** open only for `http`, `https` and `mailto`. Agent text can echo pages it read, and a `file://` or custom-scheme link could start a program.
- Agents ask permission for tools through their own prompts (the approval inbox). PilotHouse advertises no file or terminal capabilities over ACP, and refuses those requests.
- **What is approved without you.** A review chat (and a quick question) gets reads, searches and a short list of read-only commands approved by itself: a system `git` with reading subcommands, `rg`, `grep`, `ls` and the like, without shell operators, without `git -c` or other options before the subcommand, and without options that run another program (`rg --pre`, `git grep -O`, `--ext-diff`, `--textconv`) or write a file. A program named by a path (`./git`, which a pull request can add) is never approved, except from the system's folders. While a UI test is recorded or repaired, PilotHouse's own browser, device and check tools are approved by the tool's name only; a shell command that mentions them still asks.

## Untrusted repositories

Opening a project should not run its code:

- `git` runs with `core.fsmonitor` off and without external diff or text conversion drivers (`--no-ext-diff`, `--no-textconv` on every diff, stash preview and blame), which a repository's config could otherwise point at programs. A test (`reading_a_repository_runs_none_of_its_programs`) sets them all and checks none runs.
- Checkpoints store and restore files' exact bytes, with line-ending conversion and attributes off.
- Detected run configurations offer only scripts whose names no shell reads anything into (letters, digits and `-_:./+`), since names come from the repository and are typed into a shell (PowerShell or cmd on Windows, a POSIX shell on a server). Running a configuration still runs that project's scripts, as it would in a terminal.
- Nothing is executed during detection; files are only read.
- Restoring a checkpoint removes files created since only when the checkpoint holds every untracked file; one that left some out (over 20,000 files, or a file over 20 MB) keeps files it does not know and says so.
- Viewing a file never runs it. Files are read by viewers in the app (`ph-preview`; the PDF renderer forbids `unsafe`), or by the system's media framework for audio and video. *Open in Default App* is not offered for programs, scripts, installers, launchers and shortcut files (`.fileloc`, `.url`, `.lnk`…), nor for any file with an executable bit (`ph_ui::platform::opens_safely`), since opening those runs them.
- A file can be made to look harmless and be built to hurt the viewer, so every viewer bounds its work: databases open read-only with `trusted_schema` off, and a table or view that takes over 10 s to read is stopped (a view is a query, and can run for ever); archives are listed, never extracted, a compressed tar is unpacked at most 2 GB to list it, and a single `.gz` is not unpacked at all (its size is in its last bytes); Excel sheets are read cell by cell up to the rows shown, so a sheet with one cell at its last row and column never lays out its whole grid; images over 256 MB, 16,384 pixels a side or 100 megapixels are not decoded; a PDF page is drawn at most 8,192 pixels a side and drawn pages keep at most 512 MB.
- Links in documents (Markdown, notebooks, the rich editor) open with one click only when they are web or mail links (`ph_ui::platform::is_web_link`, also used for agent output); a `file:` or custom-scheme link does nothing, since it could start a program. Links to files beside the document open them in the editor.
- Nothing a document names is read from another computer: Markdown images and links to `\\server\share` or `//server/share` paths are refused, and so is an SVG drawing that links such a file (`ph_ui::platform::is_network_path`), because on Windows reading one sends the user's sign-in to that computer. Web images load over HTTPS only; plain HTTP ones are not fetched.
- A font file opened to look at is added to the app's fonts only when its family is not one the app knows, so a file can never stand in for the interface's font.
- File operations from the Files tree stay inside the workspace's folders (names with `..` or a leading `/` are refused), never overwrite (a taken name is refused or numbered) and never delete: removing moves to the system trash (on macOS through the system call, which needs no permission to control Finder). Git commands that name one file (stage, unstage, discard, history, blame, compare) pass `--literal-pathspecs`, so a file named `*.rs` is only that file; *Add to .gitignore* escapes wildcards the same way.

## The GitHub view's clones

- Reviews run in PilotHouse's own clone of the repository (`<data dir>/github/…`, a private folder), never in the user's checkouts, and the pull request's head is a detached worktree beside it. Opening that clone as a workspace is refused.
- Git reaches the host over HTTPS with the account's token, passed per call in the environment as a header scoped to that host's URL (`http.https://HOST/.extraHeader` through `GIT_CONFIG_*`): never on a command line other processes can list, in the remote URL, or in a config file. Error messages name only the Git command, not its settings.
- Files a chat names open from the GitHub view only when their real path (links and `..` resolved) is inside those clones.
- Chats in the GitHub view only read: edits, deletes and moves are refused without asking, and no checkpoints are taken. Antigravity's "go on with every permission for the rest of the turn" is never given automatically, in these chats or in reviews.
- A repository's own review rules and its notes for agents (`AGENTS.md`, `CLAUDE.md`, Copilot's instructions) are read from its base branch, never from the pull request's head.
- What a pull request carries that would act on the agent reviewing it is taken out of the checkout first: agent settings with hooks (`.claude/settings*.json`, `.claude/hooks`), MCP servers (`.mcp.json`, `.vscode/mcp.json`), other agents' folders (`.gemini`, `.codex`, `.cursor`, `.agents`, `.clinerules`) and the root's `.env` files. A task fixing someone else's pull request takes out the tracked ones too and hides the removal from git (`--skip-worktree`), so it is never committed.
- A security finding in a public repository is not turned into an issue without a warning that suggests a private security advisory instead.

## Housekeeping

Copies PilotHouse made for agents (review checkouts, parallel-run lanes) are removed when unused, at startup, only inside its own copies folders (`<data dir>/worktrees`, and this install's folder on a network drive), and only when the database in use is that data folder's own (so tests on a temporary database never touch it). Open lanes stay until you decide. When the workspaces cannot be read, nothing is removed. A removed workspace's copies stay unused for three days too, in case their work is wanted.

A removed workspace's browser profile (its cookies and logins) is deleted with it, and its id is never given to a new workspace, so nothing kept by id passes on.

## On-device

Chrome starts with sync, component updates, metrics and other Google services off, and the address bar never searches. The headless profile uses Chrome's basic password store (no keychain prompts), so its folder being private is what protects saved logins.

## Known limits

- Any process running as **you** can use the control socket, as it can read your files. That is the same trust boundary as your shell. One PilotHouse runs per data folder: a second launch hands its folders to the running one and exits.
- **Language servers** start when you open a file in a workspace folder, as in most editors, and some run the project's code to understand it (rust-analyzer runs build scripts and procedural macros; TypeScript's may load the project's own `typescript`). Copies made for reviews get none. Open a repository you do not trust as a plain folder only once you have read it.
- Git **filter** programs (`filter.<name>.clean` in a repository's own `.git/config`) still run when Git compares or stages files, as in every Git tool: turning them off would break Git LFS and git-crypt. A repository you clone has no such config; a folder copied with its `.git` from someone else can. Clone repositories you do not trust instead of copying them.
- The browser pane is macOS and Linux only for now; Windows needs the handle-based pipe variant.
