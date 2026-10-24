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

## Agent boundaries

- **One workspace's tools.** The MCP bridge resolves the workspace from the agent's folder (any of its folders) and adds it to every call, overriding anything the agent passes. Board calls that take a card id check the card belongs to it; to-dos and tests are its own.
- **Mentions stay in the workspace.** `@path` and `@path:line` attach only files inside the workspace's folders (symlinks resolved), and `#12` only its cards.
- **Utilities.** An agent's `util_run sqlite` opens only SQLite files inside its workspace's folders, read-only; the Utilities dialog (you) can open any.
- **Text from others is data.** A pull request's title, author and description reach agents (reviews, `@pr:`, schedules started by a pull request or failing checks) marked as material to work on, never instructions; reviews report such text as a finding.
- **Done needs tests** is checked for every way a card moves (drag, editor, agent), so an agent cannot mark its own card done while its tests fail.
- **No silent confirmations.** Web pages' `confirm()` and `prompt()` dialogs are shown to you in the pane. With nobody watching they are cancelled, so an agent driving the browser can never confirm "Delete…?" by itself. Alerts are acknowledged, and every dialog is logged for the agent.
- **Links in agent output** open only for `http`, `https` and `mailto`. Agent text can echo pages it read, and a `file://` or custom-scheme link could start a program.
- Agents ask permission for tools through their own prompts (the approval inbox). PilotHouse advertises no file or terminal capabilities over ACP, and refuses those requests.

## Untrusted repositories

Opening a project should not run its code:

- `git` runs with `core.fsmonitor` off and without external diff or text conversion drivers (`--no-ext-diff`, `--no-textconv` on every diff, stash preview and blame), which a repository's config could otherwise point at programs. A test (`reading_a_repository_runs_none_of_its_programs`) sets them all and checks none runs.
- Checkpoints store and restore files' exact bytes, with line-ending conversion and attributes off.
- Detected run configurations quote script names, since names come from the repository and are typed into a shell. Running a configuration still runs that project's scripts, as it would in a terminal.
- Nothing is executed during detection; files are only read.

## Housekeeping

Copies PilotHouse made for agents (review checkouts, parallel-run lanes) are removed when unused, at startup, only inside `<data dir>/worktrees`, and only when the database in use is that data folder's own (so tests on a temporary database never touch it). Open lanes stay until you decide.

## On-device

Chrome starts with sync, component updates, metrics and other Google services off, and the address bar never searches. The headless profile uses Chrome's basic password store (no keychain prompts), so its folder being private is what protects saved logins.

## Known limits

- Any process running as **you** can use the control socket, as it can read your files. That is the same trust boundary as your shell.
- Git **filter** programs (`filter.<name>.clean` in a repository's own `.git/config`) still run when Git compares or stages files, as in every Git tool: turning them off would break Git LFS and git-crypt. A repository you clone has no such config; a folder copied with its `.git` from someone else can. Clone repositories you do not trust instead of copying them.
- The browser pane is macOS and Linux only for now; Windows needs the handle-based pipe variant.
