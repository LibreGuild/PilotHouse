# Agent chat

Crate: `ph-agent`. PilotHouse chats with the coding agents you already have, using your installs and your logins. It never sees credentials.

| Agent | How it starts | Needs |
|---|---|---|
| Claude Code | `npx -y @agentclientprotocol/claude-agent-acp@0.95.0` | `claude` logged in, Node.js |
| Codex | `npx -y @agentclientprotocol/codex-acp@2.2.2` | `codex login`, Node.js |
| Gemini CLI | `gemini --acp` | `gemini` signed in |
| Antigravity | `pilothouse agy-acp` (bridge, crate `ph-agy`) | `agy` signed in |
| Cline | `cline --acp` | `cline auth`: a Cline account or ChatGPT subscription, or a provider that needs no key (Vertex AI, Ollama) |

Adapter versions are pinned in `agents.rs` so an update on the npm registry never runs unannounced; bump them on purpose.

Agents and terminals use your **login shell's** environment (`ph-env`), so tools on your shell PATH are found even when PilotHouse is started from the Dock.

## Flow

1. **First message** creates a session (named after the message) and starts the agent process for this project. The process is reused by later chats with the same agent.
2. `initialize`, then `session/new` with the project folder as cwd, plus the [PilotHouse MCP server](#pilothouses-own-tools).
3. `session/prompt`. Streamed `session/update`s become transcript entries: replies (markdown), thoughts, tool calls and plans. Thoughts and tool calls read as one line of muted text each (a spinner while running, red when failed, the size of an edit), and open on click to show diffs and output; steps of one run sit close together, a new message stands apart.
   - **Subagents.** Claude Code marks what its subagents say and do (`_meta.claudeCode.parentToolUseId`). Those steps go under the tool call that started the subagent (shown with a bot and "N steps") instead of between the main agent's own.
   - **Working.** Under the newest entry, a line says the agent is working, for how long, and the step under way (the running tool, or the subagent's). Claude Code also carries on by itself after background commands or subagents finish, without a prompt: such updates show the line too ("carrying on by itself") until it has been quiet for a few seconds with no tool running.
   - **Runs.** In the *Normal* view, two or more tool calls with no words between fold into one line saying what they did ("Read 3 files, ran 2 commands · 1 failed"; while one runs, "Running `cargo test`"). Click it to open the steps under it (`transcript::run_of`, `run_summary`).
   - **A command's output** opens at its end, in a box that scrolls: a test run or a build says what went wrong there. What is kept is the first 20 lines and the last 200 (32 KB at most), with a line saying how many were left out between; colour codes are taken out, and a line a progress bar rewrote shows as it ended. Codex sends what a command prints while it runs (`_meta.terminal_output_delta` on the call's updates), so its output grows as the command goes; Claude Code sends it when the command ends. Both say the exit code (`_meta.terminal_exit`), and a command that failed shows it on its line ("exit 101"). PilotHouse asks for both by declaring `terminal_output_delta` in its capabilities (`transcript::CommandRun`). A read or a search keeps its first 60 lines.
   - **A picture a tool gives back** (a screenshot tool's, the browser's) shows with the call: small, opening large on a click, and the call's line says "1 image". They are kept as files under the data folder's `captures/` (PNG, JPEG, WebP and GIF; eight a call at most), since an agent sends them inside its message (`ToolEntry::keep_images`).
   - **A changed file** is named from the workspace's folder, and a name too long for the row loses its folders before its own name.
   - **Turn summary.** Under a turn that changed files: how many, the lines added and removed, **Review changes** (the turn's diffs, merged per file) and **Undo turn** (the checkpoint before its message, after asking).
   - **View** (*Show* in the chat's ⋯ menu, for every chat, kept in `chat.view_mode`): *Detailed* shows every step, *Normal* folds runs, *Summary* shows only messages, replies, questions and the turn summaries.
4. **Permission requests** appear inline and in the title-bar **Approvals** inbox (across all projects). Answering either one answers the agent once. Stop, or an agent exit, cancels them. Once answered, a request shrinks to a muted line.
   - **What is being approved** shows under the card's title (`permission::Asked`): an edit as its diff, a command in full when the title does not already say all of it (a script of several lines, the script a shell is started with), and any other tool as what it is called with, a line per field. A title is one line, and "Edit files" is not enough to say yes to.
   - **What "always" leaves behind.** An approval answered *Always* is kept by the agent, not by PilotHouse: Claude Code writes a rule (`Bash(cargo test:*)`) to the folder's `.claude/settings.local.json` and never asks for that again. A Claude Code chat's ⋯ menu has **Always allowed here…**, which lists those rules and takes one back (`claude_rules.rs`; everything else in the file stays). The project's own rules (`.claude/settings.json`, shared through the repository) are listed too and left to be changed there. The other agents keep such answers for the session, or in files of their own that are not read.
   - **Plans.** When the agent asks to leave plan mode (ACP `switch_mode`), the card shows the plan, as Markdown, above the choices.
   - **Questions.** PilotHouse accepts ACP form elicitation (`elicitation/create`, `form.rs`): Claude Code's *AskUserQuestion* and MCP servers asking for values arrive as a question card. Pick one (radio) or several (checks) of the offered answers, or type your own (*Other*); **Answer** sends, **Skip** declines. A single one-answer question is answered by the pick. Questions wait in the inbox like permissions, and make the chat *Needs you*.
   - **Subagents.** While subagents work, a line above the composer says how many ("2 subagents running"); it opens to each one's state, latest step, steps and time, and a row jumps to it in the transcript. Claude starts subagents in the background, so one counts as working while a step of it runs or for a few seconds after its last. Codex's `spawnAgent` calls show as subagents too, but its adapter keeps their own steps to itself.

## A chat being readied

Some chats need something made before their agent can start: a review needs the repository cloned and the pull request checked out. That is part of the run to whoever asked for it, so it shows as one (`ChatPanel::set_preparing`, `render_preparing`): the chat opens at once with what was asked as its first message, and under the agent's name the step under way (*Cloning acme/shop…*) with PilotHouse's working mark and the time it has taken. The workspace counts it as work, and the Bridge lists it under **Working** (`WorkspaceChats::preparing`). When it is done the agent starts by itself; a failure stays over the composer in red. In a chat that has messages already, the step shows over the composer with its time.

## The composer

- **Model picker.** The model button shows the agent's mark and the current model. It opens one compact list with a search on top: each agent's name (with *This chat*, *New chat* or *Not set up*) over its models, each with its mark and a line about it. Typing filters by model or agent, ↑/↓ and Enter pick, Tab jumps to the next agent. It stays open while you look around; click outside or press Esc to close it. Models come from each agent's own ACP config options, or for agents on the older protocol (Gemini CLI) from its `models` and `modes` lists, changed with `session/set_model` and `session/set_mode` (`options.rs`): live for this chat's agent, cached from the last run (or the background load below) for the others; an agent whose models are still being loaded shows *Loading models…*, and one whose load failed offers *Load its models* with the reason. Picking another agent's model switches a fresh chat to it, or opens a new chat if this one already started. Choices are remembered per agent and applied to new chats.
- **Effort and mode** are their own buttons beside the model (🧠 *Medium*, 🛡 *Manual*), each a short menu of the agent's levels or modes.
- **The composer grows** with what you type, from two lines to six, then scrolls.
- **Jump to latest.** Scrolled up, a ↓ button brings the newest message back. Within about two lines of the end counts as the end: the button hides, and new text keeps the view on the newest reply, unless you are scrolling (`transcript_scroll.rs`). The transcript keeps some room under the last message.
- **Who wrote what.** Each reply starts with the agent's mark and name (Claude's orange burst, Codex, Gemini, Antigravity, Cline). The chat's header shows the agent's mark with a dot for its state (the words are in its tooltip), and the picker shows each agent's mark.
- **The top of the chat.** In a workspace it is the chats' tabs with *Past chats* and *New chat* at their end, and the chat in front: its agent's mark, its title and **Chat | CLI**. In a narrow panel these are two rows; from 760 px one, where the tab in front stands for the title ([workspace](workspace.md)).
- **A tab's cross closes the tab, nothing else.** It shows while the pointer is on the tab. The chat is not marked done and not deleted: it is in *Past chats* and on the Bridge as before, and has its tab again when it is opened again or needs you. Closing the tab in front shows the one beside it (a new chat when it was the last). Which tabs are closed is kept with the workspace's layout (`closed_tabs`). A tab's right-click menu marks the chat done, renames it and deletes it; pinning is the Bridge's, where cards have an order.
- **Past chats** (the clock) is one list, the last used first, whatever a chat's state: the ten last used, each with its agent's mark and, when something is going on in it, the working mark or a dot (*needs you*, *stopped*, *finished*). A row pointed at shows what can be done without opening the chat: **mark as done** (the tick; *not done* when it is), **rename** and **delete** (which asks first; not offered while the agent works in it). **Show all chats** at its end puts the whole list where the chat is: the transcript and the message box give way to it, with when each chat was last used, the search and *Started elsewhere* (an agent's own sessions in this folder that did not start here). Picking a chat shows it as any past chat; the cross goes back to the chat that was in front (`chat_panel/history.rs`). A panel without tabs (a review's chat, the quick chat) names the place the agent works in before the title: the repository for a GitHub copy, or the workspace with its colour (and the subfolder, `name › sub`, when the chat runs in one).
- **Restore files** shows under your message, at the right, while the pointer is on it: it puts the project's files back as they were before that message.
- **Copying a reply.** Text selected in a reply copies as plain text and as HTML (`ph_ui::rich_copy`), so a table, a list or a heading pastes as one into a wiki, a mail or a document.
- **Diagrams.** A ```` ```mermaid ```` block in a reply is drawn as a picture once its closing fence arrives (flowcharts, sequence, class, state and ER diagrams, timelines, Gantt and more; `reply.rs`, drawn to SVG without a browser by `mermaid-rs-renderer`, in the theme's light or dark). One that does not parse stays code, with the reason. Hovering a diagram shows buttons at its top right: zoom out, zoom in, fit, open it large in a dialog (where it fits the window and zooms too) and copy its source; a pinch, or ⌘/Ctrl and the wheel, also zooms. Zoomed in place, a diagram grows to 480 px tall, then scrolls (`ph_ui::DiagramCard`, also used for the diagrams in Markdown documents). The marks are bundled from Lobe Icons (MIT); see `crates/ui/assets/agents/`.
- **The header is the session's name and its form** (**Chat | CLI**), the same for a chat and for its CLI. What is the chat's alone sits with the message box, which only a chat has: under the message, the agent and model, the account, the effort and the mode, then **⋯** for the rest; at the right what the chat has used and Send.
- **The ⋯ menu has three groups, each under its name.** The agent's (named after it): its options that have no button of their own, each opening its choices beside the menu and saying the one in use (*Collaboration mode: Default*), and for Claude Code *Always allowed here…*. *This chat*: *Compact chat*, and *Continue with*, which opens the other agents. *View*: *Width* and *Show* (how much of an agent's work the transcript shows). What cannot be done yet (nothing said, the agent at work) stays in the menu, greyed, so the menu is the same every time.
- **How wide the chat gets** in a pane with room to spare is *Width* under **⋯ → View** (and *Reading width* in Settings → Chat; one setting, `chat_width` in the preferences, for every chat): *Normal* is a column in the pane's middle (760 px at the default interface size), *Wide* a wider one (1000 px), *Full* the whole pane. The messages and the message box follow it; an empty chat's welcome keeps to a column in all three. (A narrower column of 640 px that earlier builds offered reads as *Normal*.)
- **The row never cuts a button off.** In a pane too narrow for it, the buttons give up their words one at a time and stay as icons: first the usage bar (its number stays), then the effort, the account, the mode, and last the model's name (the agent's mark stays). An icon opens the same menu and says its choice when pointed at ("Mode: Auto review"); with the room back the words are back (`chat_panel/fit.rs` works the widths out).
- **Mode** (permission mode) has its own pill; anything else the agent offers is under **⋯**. ⇧Tab in the composer switches to the next mode (Manual, Accept edits, Plan, Auto), as in Claude Code, but never to one that skips permissions.
- **Whose choice it is.** A model, effort or mode picked in a chat nothing was said in yet is how chats with that agent start from then on (`options::remember`). One picked once something was said is that chat's own (`sessions.options`, with what the chat started in): putting one chat in plan mode does not put the next one there, and a chat picked up again is put back in what it was in, whatever new chats start in by then. A mode the agent changes itself (out of plan mode once the plan is approved) is kept the same way.
- **Its name.** A chat is named after its first message. When the agent names the session itself (ACP `session_info_update`; Claude Code and Codex do after a turn or two) that name takes its place, and may change again as the work goes on. A name you gave stays.
- **Warm start.** Focusing the composer starts the agent and its session in the background, so the pickers fill in and the first reply comes sooner. Nothing is stored until the first message. *New chat* on a chat that is still new and untouched keeps it, rather than starting another session.
- **Sessions end with their chat.** When a chat goes away (a new chat replaces an unsent one, another agent is picked for it, it is deleted, or it is let go of as idle), PilotHouse closes its agent session (`session/close`) if the agent can (Claude Code and Codex can), so the agent frees what it held for it: Claude Code a process of its own and its MCP servers. Agents that cannot keep their sessions until they exit.
- **Attachments** show as chips (images with thumbnails) and are sent as ACP content: files as resource links, images as image content when the agent accepts images, text as text. Resting on a chip for two seconds (in the composer or a sent message) shows a large preview; clicking the chip or the preview opens the whole image, text or file (first 256 KB) in a dialog with Copy (`attachment_view.rs`).
- **Paste or drop to attach** (`chat_panel/dropped.rs`): ⌘V with an image on the clipboard and no text (a screenshot, an image copied in a browser) attaches the image, saved in PilotHouse's `captures` folder (as PNG when it is not PNG, JPEG, GIF or WebP, such as the TIFF macOS often hands over; a pasted SVG is not attached); files copied in Finder or Explorer attach as if dropped. Rows dragged from the Files tree attach as the workspace's files. Files dropped from the system (20 at most) attach by kind, read off the UI thread: PNG, JPEG, GIF and WebP images as images, the workspace's files as its files, other text files up to 256 KB by their content, anything else by its path. An image goes to the agent as an image only when its bytes are PNG, JPEG, GIF or WebP and it is 5 MB at most; otherwise as a link to its file. The composer lights up while something can drop on it.
- **Agent setup.** When an agent cannot start, the empty chat shows a setup card instead: CLI installed (with the install command to copy), Node.js for adapters (download link), adapter already in npm's cache, sign-in hint, and **Check again** (`setup.rs`, `setup_view.rs`). The picker lists such agents as **Set up** with the real reason. Settings → Agents lists every agent: a line (its name, where it was found, *Ready*) for one that can start, the card for one that cannot (`setup_view::status`), with **Check again** beside the page's title.
- **Gemini asks for a login in a folder it does not trust.** Gemini CLI reads a key saved in `~/.gemini/.env` only in folders on its trusted list, so the same key that works in a terminal elsewhere is not read in a folder outside it (a new clone, a scratch folder). When Gemini asks for a login and that is why, the card says so in place of *Sign in* (`gemini_trust.rs` follows Gemini's own rule: its environment switches, `security.folderTrust.enabled`, then the longest rule of `trustedFolders.json`): switch the chat to **CLI** and answer Gemini's question about the folder, or give Gemini CLI `GEMINI_CLI_TRUST_WORKSPACE=true` in Settings → Agents. PilotHouse does not read the key or change the list.
- **Which agent new chats use.** At start PilotHouse looks, in the background, for the agents that can start here (each CLI, and Node.js for those run through an adapter, on the login shell's `PATH`; `agents::detect`, `installed.rs`). New chats use the agent last chosen in the picker while it is installed, else the first installed of Claude Code, Codex, Gemini CLI and Antigravity (setting `agent.default`; `agents::pick_default`). While that first look runs, an empty chat says *Looking for your coding agents…* rather than naming one. If the chosen agent is no longer found, new chats use the first installed one and the empty chat says so in a line; the choice is kept for when it is back.
- **First run, no agent installed.** The empty chat greets you instead of naming what is missing: every agent PilotHouse works with, whether it was found, its install command with a Copy button (Node.js with a download link where its adapter needs it), its page, and **Check again**. It also looks again every few seconds while shown, and a chat takes an agent as soon as one is installed. A chat whose agent is not installed while others are shows that agent's setup card with **Use …** buttons for the installed ones.
- **Models load in the background.** A few seconds after start (and after a later look finds a newly installed agent), for each installed agent whose models were never cached or were cached more than a day ago, one at a time, PilotHouse starts the agent in its own neutral folder (`<data dir>/agents`), asks for a new session (where ACP lists models, modes and efforts), caches what it offers and stops it: no chat is made and nothing is prompted. Each gets 90 seconds; one that fails (not signed in, offline) is not tried again in this run, and its picker row says why and still offers **Load its models** (`preload.rs`). Claude Code and Codex run through an npm adapter, so the first of these loads downloads it with `npx`, as the first chat would; Codex may note the session in its own history.
- An empty chat suggests starter prompts.
- **While the agent works**, Enter queues the message instead of sending it. Queued messages wait above the composer and go one at a time when a turn ends; each can be edited or removed. **Send now** adds the first to the turn under way when the agent supports steering (Claude Code and Codex: `_session/steering`, advertised in the handshake's `_meta.steering`), and otherwise (Gemini CLI, Antigravity) stops the agent and sends it.
- **⌘Enter** (Ctrl+Enter elsewhere) sends and opens a new chat at once: the sent one carries on behind it, and its card on the Bridge shows how it does.
- **Commands.** `/` at the start of a message offers the agent's own commands (ACP `available_commands_update`, e.g. Claude Code's `/review` or `/compact`) with their descriptions: all of them, eight at a time, the list moving with the arrow keys (it says "9 of 41").

## Mentions

- `@src/main.rs` attaches that file as a resource link the agent can read.
- `@src/` (a folder, offered with a trailing slash) attaches what the folder holds, folders first, up to 200 entries, so the agent sees its shape and reads what it needs.
- `@screenshot` and `@log` attach the browser or device screen and recent logs ([live-panes.md](live-panes.md)); `@test-run` (or `@tests`) attaches how each UI test last went, with the first failing step's screen ([tests.md](tests.md)). With PilotHouse's Chrome extension, `@tabs` (picked or typed, or *Tabs you shared in Chrome* in the **+** menu) attaches the text of the Chrome tabs you shared with agents, marked as page text; sending waits until they are read, and a typed `@tabs` sends once they are ([browser.md](browser.md)).
- Typing `@` and a name offers definitions (functions, types, classes in Rust, TypeScript/JavaScript, Python, Go, Kotlin/Java/Swift/C#, C/C++; found by reading the files, `ph_files::symbols`); one inserts `@path:line`, which attaches the code from that line to the end of the definition. Any `@path:line` works the same.
- `@pr:12` attaches a pull request of the workspace's GitHub repository (title, author, branches, description, and how to get its diff); typing `@pr` lists the open ones.

Typing `@` opens suggestions above the composer (`mention_complete.rs`): files of the project (fuzzy, listed in the background and refreshed every minute) and the live sources. ↑↓ pick, Enter or Tab inserts the mention, Esc closes; Enter does not send while suggestions show.

## Links

Links in agent replies open only when they are `http`, `https` or `mailto` (see [security.md](../security.md)).

## Sign-in

If an agent reports that it needs a login, the chat shows its sign-in methods as buttons (ACP `authenticate`). PilotHouse never sees the credentials.

## Checkpoints

Before each message reaches the agent, PilotHouse snapshots the project's files (`ph_git::checkpoint`), while the agent session starts, so it costs no waiting. The snapshot is a commit of every tracked and untracked file (ignored files, untracked files over 20 MB, and more than 20,000 untracked files are left out), built in a temporary index and kept under `refs/pilothouse/checkpoints/` (the newest 200). HEAD, the index, branches and the stash are never touched, and nothing shows in history. An unchanged tree reuses the last checkpoint.

Hover a message and pick **Restore files** to put the files back as they were before it: files changed since are written back, files created since are removed. A restore first takes a checkpoint of its own, so the note it leaves in the chat has **Undo**. The agent's next message starts with a note that the files were restored, so it reads them again. Review chats take no checkpoints (they only read), and nothing is restored while the agent is working.

## Ask about code

From the editor (see [files.md](files.md)): *Explain*, *Find bugs* and *Write tests* send the selection with a prompt that tells the agent to look at callers and callees first; that turn may read and search without asking (the same read-only policy review chats use).

## Handing off to another agent

*Continue with* in the chat's ⋯ menu moves the work to another agent: a new chat opens with that agent, says where it came from, and offers a first message to send or edit. With that message the new agent gets a recap of the old chat (its recent messages and tool calls, up to 8,000 characters) and is told to check the files' current state first. The old chat stays as it was.

## Side chat

**⌘;** (*Side chat about this one* in ⌘K) steps aside from the chat in front: a new chat with the same agent opens, titled *Aside: …*, and its first message carries a recap of the chat it came from. Nothing goes back to that chat. The ← in its header returns to it.

## Usage

When an agent reports usage (ACP `usage_update`: context used and window, running cost; the prompt reply's `usage`: running token totals), the message box shows, beside Send, a small bar of how full the context is and its share (in a narrow pane the share alone, in amber or red as it fills); pointing at it says the tokens in and out and the cost so far. Reports are running totals per agent session, so the store keeps the last one and adds only the difference: a restarted agent adds on, a resumed one is not counted twice (`ph_store::usage`, migration 5). What each report adds is also kept by local day and model (`usage_days`, migration 14), and chats from before that keep their totals on the day they were last used.

**Usage** (the palette, the app menu) opens a compact dialog (`usage_view`), one page read top to bottom:

- **Period and grouping**: a menu for the last 7, 30 or 90 days or 12 months, and whether the list is by **Agents**, **Workspaces** (where the work happened: the session's folder, matched to the workspace whose folder holds it, else the folder's name) or **Models**. *Refresh* reads again.
- **Headline**: tokens processed (input, cached or not, and output) and **≈ what they would cost at API prices**, then sessions, the share of input read from a cache, output, and any cost agents reported.
- **Bars**: a day, week (90 days) or month (12 months) each, stacked by agent colour; pointing at one reads it out underneath.
- **A warning** when an agent has used 70% or more of a limit window, with when it resets.
- **The list**, busiest first, in columns: tokens, sessions and the estimate; a workspace or model with its share. An agent's line shows its limit windows (or why there are none) and opens to its details: each window with its reset time, its models, the estimate and reported cost, its busiest day and active days, where its numbers come from, and *Plan and limits* (Claude, Codex).
- **How this is counted**, folded away.

Where the numbers come from (`ph_agent::usage_logs`): where an agent keeps its own session files on this computer, they are read, so sessions it ran in a terminal or an editor count too. Claude Code's `<CLAUDE_CONFIG_DIR or ~/.claude>/projects/**/*.jsonl` (a reply copied into a resumed session counts once, by its message and request ids), Codex's `<CODEX_HOME or ~/.codex>/sessions` (`token_usage_record`, or the growth of `token_count` totals), Gemini CLI's `~/.gemini/tmp/*/chats` (the last write of each reply), Cline's `<CLINE_DIR or ~/.cline>/data/sessions/*/*.messages.json` (each reply's `metrics`, the folder from the session's `<id>.json`). For such an agent PilotHouse's own records add only the cost it reported; for the others (Antigravity) the tokens are what it reported in PilotHouse chats. Gigabytes of files are read quickly: lines are matched as bytes before any is parsed, only the needed fields are parsed, files are read in parallel, and what each held (token counts only) is kept in `<data dir>/usage-files.json`, so a later read parses only files that changed, and of a Claude Code file only what was added. The first read of a large history takes seconds (a year of 15 GB took about 4.5 s in a release build); the dialog shows how many files are read.

**The estimate** prices each reply at its model's API list price, cache reads and writes at theirs, from LiteLLM's public price list as shipped with PilotHouse (Claude, GPT and Gemini models). PilotHouse never downloads it by itself: **Update prices** (under *How this is counted*) fetches the latest list from GitHub when you click it and keeps it as `<data dir>/model-prices.json`, used while it is newer than the shipped one. It is what the tokens would cost without a subscription, not a bill; models without a listed price are left out and counted. A cost an agent reported (Claude Code's own estimate in PilotHouse chats) is shown apart. Limits are what the agents report while they work (`ph_agent::rate_limits`).

## Agent instructions

*Agent instructions* in the palette shows the project's `AGENTS.md` (Codex, Antigravity, Cline and most agents), `CLAUDE.md` (Claude Code) and `GEMINI.md` (Gemini CLI), and **Sync** makes `AGENTS.md` the one place to write instructions: Claude Code and Gemini follow an `@AGENTS.md` line as an import, so their files start with one, and a project without `AGENTS.md` gets one made from its existing file. Nothing is deleted; each file's own lines stay below the import (`instructions.rs`).

## MCP servers and skills

*Settings → MCP & skills*:

- **MCP servers for every agent**: local (stdio) servers PilotHouse passes to each new agent session, next to its own `pilothouse` server (browser, devices, tests). Set one up once instead of in each agent; switch it off or remove it here. The command line splits like a shell's; environment is one `KEY=value` per line. Stored in PilotHouse's database (`mcp_config.rs`).
- **In your agents' own settings**: the local servers found in `~/.claude.json` (global and this project), the project's `.mcp.json`, `~/.gemini/settings.json` and `.gemini/settings.json`, and `~/.codex/config.toml`. *Use for every agent* copies one over. Remote (URL) servers are not listed.
- **Skills**: `SKILL.md` folders for Claude Code (the project's `.claude/skills`, `~/.claude/skills`) and Codex (`~/.codex/skills`), with their name and description; *Open* edits one, *New project skill* starts one from a template (`skills.rs`).

### PilotHouse's own tools

Every agent session gets the PilotHouse MCP server (`pilothouse mcp`, crate `ph-mcp`): this same program, which passes each tool call to the running app over the [control socket](../control-socket.md) and keeps no state of its own. Its tools are those of the features a build has:

| Tools | For |
|---|---|
| `browser_*` | The built-in browser ([browser.md](browser.md)) |
| `chrome_*` | The user's own Chrome, through the extension ([browser.md](browser.md)) |
| `device_*` | Android emulators and phones ([devices.md](devices.md)) |
| `test_list`, `test_run`, `test_expect` | The workspace's UI tests ([tests.md](tests.md)) |
| `ports_list` | The ports the project's programs listen on ([runs.md](runs.md)) |
| `todo_*` | Reminders for the user |
| `theme_*`, `slides_*`, `studio_*`, `util_*` | Designing a theme, slide decks, pictures and video, and the small utilities |

Which browser comes first is the user's choice (*Settings → Agents → Preferred browser*); the server says so in its instructions as a chat starts.

To use them from an agent started outside PilotHouse (PilotHouse must be running):

```sh
claude mcp add pilothouse -- /path/to/pilothouse mcp --agent claude
```

The project is the agent's working directory, or `--project PATH`.

(It also had tools for a board of cards in each workspace, `board_*`, until 0.7, when the board was removed.)

## Several agents at once

*Run with several agents* in the composer's **+** menu gives one prompt to two or more installed agents. PilotHouse takes a checkpoint of the project (uncommitted and new files included), makes a worktree per agent from it in its data folder (the dialog says how much space each copy takes), and starts a chat per agent working there. *Parallel runs* in the palette compares them: files changed and lines added and removed per agent (counting new files), a link to each chat, **Keep** (applies that agent's changes to the project after a checkpoint, three-way when the project moved on) and **Discard** (removes the copy). A kept lane's copy stays for its chat until *Remove copy* (`ph_git::parallel`, `ph_store::parallel`, migration 6, `workspace/src/parallel.rs`).

## Search

⌘⇧K (or *Search chats* in the palette or the chat's history menu) searches the words of every message in the project's chats, or in all projects. Results come as you type (the last word matches as a prefix), best first, with the matched words highlighted; picking one opens that chat scrolled to the message. The index is SQLite FTS5 (`message_search`, migration 4), kept in step with the messages by triggers, so it needs no upkeep.

## Persistence and restarts

Transcripts are stored per session, with the agent's ACP session id. After a restart, agents that support `session/load` (Claude Code does) resume their own session. For the others, PilotHouse starts a new session and sends a short recap of the earlier conversation with the first new message. The same happens when an agent that can resume does not find the session any more. Either way the chat says so ("… starts fresh here. It is given a summary of what was said, not all it knew"): the agent then knows less than the transcript shows, and that is not left to be found out.

## Chat or CLI: one session, two forms

A session with an agent is one conversation, the agent's own session, and it has two forms: the chat, and the agent's own program (its CLI) in a terminal. **Chat | CLI** in the chat's header goes from one to the other, in the same place: the CLI shows where the transcript and the message box were. Nothing but what you type into changes. The switch is in a workspace's chat, for the agents that have a CLI (all five), where the build has terminals.

What keeps that true (`workspace_screen/cli.rs`, `ChatThread::hand_over`, `terminal_agents::launch`):

- **One program at a time.** A chat hands its conversation over at rest: while the agent works, waits for an answer or has messages queued, the switch says so and changes nothing. The agent's side then lets go, and the CLI starts only once it has (`ChatThread::hand_over` gives back what to wait for). An agent keeps a hold on a conversation it has open: started too soon, Antigravity's CLI says "already open in another CLI instance" and Codex's takes no input. Where no other chat of the agent's process is at work, the process is ended, the one sure way that nothing holds the conversation (its other chats, at rest, load their sessions again with their next message). Where one is, only this session is closed (`session/close`, which PilotHouse's Antigravity bridge has too) and that is waited for; an agent that cannot close one session keeps the chat where it is, and says why. Coming back, the CLI's terminal is closed before the chat loads the session again; a CLI at work is not closed (wait for it, or stop it there).
- **One conversation, one CLI.** A session whose CLI runs has it: asking for the CLI again, also while the first is still opening (the agent's side takes a few seconds to let go), starts nothing more.
- **Never on a guess.** The CLI goes on with the session the chat had, by its id (`claude --resume`, `codex resume`, `gemini --resume`, `agy --conversation`, `cline --id`). Coming back uses the id the CLI is on: Claude Code says it through its hooks, the others' is the one they resumed or is found in their files. The others' files are looked at two seconds after they are asked something, then at growing intervals (`terminal_agents::find_sessions`). When you switch back before that look came by, the id is looked for at once, a few times over three seconds (`find_session_now`), and the switch goes on by itself once it is known; only when it still is not (the agent wrote nothing down) does the switch say so and stay in the CLI.
- **Nothing said is lost.** As a chat goes to the CLI it keeps how many steps the agent's session file holds (a mark, in the setting `chat.cli_marks`). Coming back, what the file holds after the mark was said in the CLI and is added to the chat's transcript (`cli_transcript::take_in`). These entries read plainer than a chat's own (a tool is its name, without its output or diff): the file says less than the agent tells a chat. Claude Code, Codex, Gemini CLI and Cline keep a session as a file of JSON lines. Antigravity keeps a database a conversation, in a layout of its own that is written down nowhere (`antigravity_store.rs`): what you typed and what it replied are read from it, its tool steps are not. If an agent's record cannot be read, or holds nothing new, the chat says which of the two, and whether the agent still knows what was said; nothing is guessed.
- **The session keeps its model and mode.** The CLI starts with the chat's model (`--model`) and, for Claude Code, its permission mode (`--permission-mode`), and with PilotHouse's own tools (the browser, devices, tests: `--mcp-config` for Claude Code, `-c mcp_servers.pilothouse…` for Codex, for this run only). Codex's mode and the other CLIs' tools are their own settings.

The CLI is the terminal's own program: the user's shell starts it (`-l -i -c`, so `PATH` and aliases are theirs) and ends with it, so leaving the CLI leaves no prompt behind. Its terminal belongs to the chat: it is not among the Terminal tab's, and the session keeps its tab over the chat and its card on the Bridge, with the state the CLI reports ([terminal.md](terminal.md#agents-in-a-terminal)).

**What the app sends to a chat goes to the CLI too.** Selected code to ask about, a file from the tree, a capture of the browser or a phone, lines of a log: with the session in its CLI form these are typed into the CLI (pasted, not entered), for you to send there. A file or an image goes as its path, which an agent's CLI reads; text as it is. With the CLI not running, the app says so instead of putting it in a message box that is not on screen (`ChatPanel::type_in_cli`).

A session in its CLI form whose program is not running (after a restart, or once you left the CLI) shows what its session file says, read only, with **Continue**: nothing starts by itself. A chat nothing was sent in has no conversation yet: its CLI starts a session of its own, named by the first thing asked there. A session keeps its name in either form: the CLI's terminal does not name it (its shell would call every one "zsh"). *Past chats → Started outside this app* lists an agent's own sessions in the folder to go on with here (a chat that takes the session back; for Cline, which cannot, its CLI).

**Nobody's child.** Claude Code tells the programs it starts that they run inside its session (`CLAUDE_CODE_CHILD_SESSION` and others), and a `claude` that sees that keeps no record of what is said ("Transcript saving is off"). So an app started from a terminal in a Claude Code session would hand that on to every agent it starts, and a session's CLI part would be in no file to take back. The app forgets those marks as it starts (`ph_env::forget_parent_session`); what your shell profile sets for an agent stays. Should a CLI keep no record all the same, the chat says so when you come back, instead of looking as if nothing was said.

What does not carry over: a turn under way; work an agent left running in its process when you switch; and for Cline the way back to the chat is a summary, not its memory (it cannot load a session), which the chat says.

## Settings → Agents: how each agent is run

`agents_view.rs`, `profiles.rs` (one JSON value, `agents.profiles`; what a build does not know in it is kept).

- **Open agents as** *Chat* or *CLI*: how a session opens when you ask for a new one (*New chat*, ⌘N). Each agent can say otherwise (*As all agents*, *Chat*, *CLI* on its row). A chat made for something (a message to finish, a review, a schedule's run) is always a chat.
- **A new CLI session asks which agent first** (`chat_panel/cli_pick.rs`). Where new sessions open as a CLI, *New chat* shows the installed agents that have one, the agent last used leading and each of an agent's accounts on a row of its own, in place of the chat. Picking one starts its CLI; nothing starts before, and no session is made for asking. **Chat** in the header's switch shows the new chat instead. With one agent and one account there is nothing to ask, and its CLI starts.
- **Installed** lists the agents found on this computer (their CLI is on your `PATH`), each with the command it is started by and whether it is ready; **Not installed** lists the others with the command that installs each, **Copy**, and **Install**, which runs it in a terminal of the workspace in front, where you see it run and fail. *Check again* looks anew.
- An installed agent's row opens to what can be set for it: its **Command** (its CLI, when not the usual one: a wrapper, another build), **Arguments** added when its CLI starts, and its **Environment**, set for it as a chat and as its CLI. An agent that is not ready shows what it needs first.
- **Accounts.** A second sign-in of an agent is a set of environment variables it is started with, usually the folder it keeps its sign-in and its sessions in (`CLAUDE_CONFIG_DIR=~/.claude-work`, `CODEX_HOME=…`): what a shell alias for a second account does. It is kept as variables, not as that alias, because an alias exists only in an interactive shell and would hold for the CLI and not for a chat. With an account added, the message box shows which account a new session runs in, to pick before something is said; after that it only says which, since the agent keeps the conversation in that account's files. To sign an account in, open a session in it as its CLI and sign in there.
  - One agent process serves one account (`WorkspaceChats::connection` keeps them by agent and account), and a session's account is stored with it (`sessions.account`).
  - What reads the agent's own files follows the session's account (`cli_sessions::sessions_in`): going back to the chat from the CLI, a terminal session's transcript, the last reply kept with a chat. Usage counts every account's files. The limit meter in the status bar is the agent's own account's.

## Newer versions of the agents' CLIs

`ph_workspace::agent_updates`. A minute after the app starts and once a day, each installed CLI is asked its version (`claude --version`) and the npm registry the newest version of the package that carries its releases (`@anthropic-ai/claude-code`, `@openai/codex`, `@google/gemini-cli`, `cline`): one small request each, naming only the package. Nothing about you or this computer is sent. A CLI that is behind shows at the right of the status bar (*2 agent updates*); its pop-up lists each with its version and the newest, and **Update** runs the agent's own updater in a terminal of the workspace in front (`claude update`, `codex update`, `cline update`; for Gemini CLI, `npm install -g @google/gemini-cli@latest`), where it is seen to run. Antigravity publishes nowhere a version can be read from, so it is not looked at.

An updater does not say when it is done, so from **Update** on that CLI is asked its version every four seconds, for up to five minutes: its row says *Updating…*, shows the version it is at, and leaves the list once it is the newest, and the status bar's count follows. Only the CLI is asked for this (the newest version is known), nothing goes over the network. **Check again** (the turning arrow at the pop-up's top) looks at everything anew, CLIs and registry, whenever you want it to.

Switch it off in *Settings → Agents → Updates*. A build whose brand turns updates off (`[updates] enabled = false`) never looks.

This is about the CLI on your `PATH`, which a session's CLI form runs. A chat runs on the adapter this version of the app was built with (`agents.rs`), which a CLI's update does not change.

## Stopping

**Stop** asks the agent to end its turn (ACP `session/cancel`); the agent answers when it has. An agent that does not (a tool that hangs, an agent that no longer reads) leaves the turn running: after three seconds the working line says so and Stop turns red, and pressing it again ends the agent's process (`ChatThread::force_stop`). One process serves every chat of that agent in the workspace, so the others at work lose their turn too and are told why. Each chat keeps its session and picks it up with its next message.

## Code map

| File | Role |
|---|---|
| `rpc.rs` | Child process and JSON-RPC: request/response matching, incoming requests, stderr tail |
| `connection.rs` | One agent process: handshake, routing updates and permission requests to threads |
| `thread.rs` | One session: prompt turns, checkpoints and restores, applying updates, persistence |
| `transcript.rs` | Entry model, stored form, diff summaries |
| `permission.rs` | Permission requests, what a card shows of the action (`Asked`), and the approval inbox |
| `claude_rules.rs` | What Claude Code may do in a folder without asking: read, and taken back |
| `profiles.rs` | How each agent is run: open as chat or CLI, command, arguments, environment, accounts |
| `agents_view.rs` | Settings → Agents |
| `cli_transcript.rs` | An agent's own session file as a transcript; what was said in its CLI taken into a chat (`Record`) |
| `antigravity_store.rs` | What was said in an Antigravity conversation, read from its database |
| `cli_sessions.rs` | An agent's own sessions in a folder, from its files (per account) |
| `options.rs` | Agent config options (model, mode, effort…), remembered choices |
| `installed.rs` | Which agents are installed, looked for at start and on request |
| `preload.rs` | Agents' models, loaded in the background after start |
| `attachments.rs` | Attachments, mentions and live sources (`@screenshot`, `@log`, `@test-run`, `@tabs`) |
| `attachment_view.rs` | Chips with hover preview and a full-view dialog |
| `model_picker.rs` | The model picker popover |
| `setup.rs`, `setup_view.rs` | Per-agent setup checks and the setup card |
| `chat_panel.rs`, `entry_view.rs`, `inbox_button.rs` | UI |

## Antigravity (`ph-agy`)

`agy` has no ACP mode, so PilotHouse bridges its headless mode: `agy -p= --input-format=stream-json --output-format=stream-json` reads one user message per line and prints one event per line. `pilothouse agy-acp` serves ACP and runs one agy process per chat.

- **Sessions** are agy conversations: the ACP session id is agy's conversation id, so reopening a chat resumes it with `--conversation`.
- **Model, effort, mode** are agy flags. `agy models` lists each model once per effort; the picker shows each model once and effort separately. Changing one restarts agy on the same conversation before the next turn. Modes: Ask, Accept edits, Auto, Plan, Full access (`--dangerously-skip-permissions`).
- **Permissions.** Headless agy cannot ask; it refuses the action and reports it. In Ask, Accept edits and Auto the bridge then asks you, and if you allow it continues the conversation once with permissions skipped.
- **Cost.** An agy process holds about 200 MB, so it stops after 5 idle minutes and comes back on the next turn. agy takes about 5 s to start.
- **Limits.** agy has no per-session MCP setting, so Antigravity chats do not get the PilotHouse browser, device and test tools. Images are not sent.

## Cline

Cline's CLI speaks ACP itself (`cline --acp`), with your own install. Its ACP side differs from what its handshake says in a few ways; `AgentSpec::quirks` and `cline.rs` work around them:

- **Started quietly.** `CLINE_NO_AUTO_UPDATE=1`, since Cline otherwise updates itself (a global npm install) on every start, and `CLINE_SESSION_BACKEND_MODE=local`, since it otherwise runs sessions through a background hub daemon that outlives it. Both only for the processes PilotHouse starts; Cline in a terminal is as you run it.
- **Runs as in your terminal.** Over ACP Cline on its own only uses a Cline account, ClinePass or a ChatGPT subscription saved by `cline auth`. When the provider Cline last used in a terminal is another one that needs no key (Vertex AI on Google Cloud's own sign-in, a local Ollama), PilotHouse starts Cline on it: `CLINE_PROVIDER`, its model as `CLINE_MODEL`, and a stand-in `CLINE_API_KEY`, which Cline asks for though the provider's settings sign it in (`cline::launch_env`). PilotHouse reads no keys, so a provider set up with an API key (Anthropic, OpenRouter…) works only with an account or with `CLINE_PROVIDER` and `CLINE_API_KEY` in your shell; the setup card says so. Your own `CLINE_PROVIDER` or `CLINE_API_KEY` always wins.
- **Choosing the provider.** The chat's **⋯ → Provider** lists Cline's accounts and the saved providers that need no key (`cline::adjust_options`). Between accounts Cline switches over ACP. To or from a saved provider, which Cline takes only when it starts, PilotHouse starts Cline again (`AgentConnection::restart`, not while one of its chats works): its chats go on in new sessions, given their history, and the models come in again. The choice is PilotHouse's own; your terminal keeps `cline auth`'s. An expired sign-in ("cline requires re-authentication") shows a sign-in card with Cline's own methods (they open the browser).
- **A Cline without `--acp`** (an organisation's own build with editor mode left out, an old version): the first chat fails on that flag, which PilotHouse recognises in what the CLI printed (`AgentSpec::refused_flag`: "unknown option" and the like, naming the flag). The chat then says so in words and offers **Run Cline in a terminal** over the composer: the CLI starts in a terminal of the workspace, where PilotHouse follows it like a chat ([terminal](terminal.md)). Remembered until PilotHouse quits (`agents::chat_unsupported`); the same holds for any agent whose CLI speaks ACP itself.
- **Text only.** Cline reads only a prompt's text: attached files go as a line naming the file (it reads it with its tools), and images as their file's path.
- **No resume.** Its `session/load` is missing before 3.0.74 and answers the first prompt after it with nothing in 3.0.74, so a reopened chat starts a new Cline session that is given the chat's recent history. Its own sessions still resume in a terminal (`cline --id`).
- **MCP.** Cline ignores the servers a session is given and reads its own settings file, so its process is pointed (`CLINE_MCP_SETTINGS_PATH`) at a copy PilotHouse writes in `<data dir>/agents/cline/`: your servers from `~/.cline/data/settings/cline_mcp_settings.json` with PilotHouse's and those from Settings → MCP & skills (one file per workspace, private, written when the agent starts).
- **Options.** Plan and Act are its modes; its models come from its account's catalogue. Before 3.0.74 it also files its providers as a model list; only the list named `model` is the model picker. Reasoning effort is not offered over ACP.
- **One-off jobs** (commit messages and the like) run `cline --plan --json --auto-approve false`, the prompt on stdin, and take the text of its last `run_result`. They use the provider Cline uses in a terminal.
- **Tool titles.** Cline titles a call with its input as JSON (`editor: {"path": …}`); such titles are shown as what it does and to what ("Edit tide.py", `transcript::readable_title`).
- **Known in Cline itself:** edits come without a diff (see them in Changes); *Always allow* is not remembered (it asks again); a mode change after the first message may not reach the running session (Cline issue #13040); usage is not reported over ACP (the dashboard reads its session files).
