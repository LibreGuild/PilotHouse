# Agent chat

Crate: `ph-agent`. PilotHouse chats with the coding agents you already have, using your installs and your logins. It never sees credentials.

| Agent | How it starts | Needs |
|---|---|---|
| Claude Code | `npx -y @agentclientprotocol/claude-agent-acp@0.95.0` | `claude` logged in, Node.js |
| Codex | `npx -y @agentclientprotocol/codex-acp@2.2.2` | `codex login`, Node.js |
| Gemini CLI | `gemini --acp` | `gemini` signed in |
| Antigravity | `pilothouse agy-acp` (bridge, crate `ph-agy`) | `agy` signed in |

Adapter versions are pinned in `agents.rs` so an update on the npm registry never runs unannounced; bump them on purpose.

Agents and terminals use your **login shell's** environment (`ph-env`), so tools on your shell PATH are found even when PilotHouse is started from the Dock.

## Flow

1. **First message** creates a session (named after the message) and starts the agent process for this project. The process is reused by later chats with the same agent.
2. `initialize`, then `session/new` with the project folder as cwd, plus the [PilotHouse MCP server](board.md#agent-tools).
3. `session/prompt`. Streamed `session/update`s become transcript entries: replies (markdown), thoughts, tool calls and plans. Thoughts and tool calls read as one line of muted text each (a spinner while running, red when failed, the size of an edit), and open on click to show diffs and output; steps of one run sit close together, a new message stands apart.
   - **Subagents.** Claude Code marks what its subagents say and do (`_meta.claudeCode.parentToolUseId`). Those steps go under the tool call that started the subagent (shown with a bot and "N steps") instead of between the main agent's own.
   - **Working.** Under the newest entry, a line says the agent is working, for how long, and the step under way (the running tool, or the subagent's). Claude Code also carries on by itself after background commands or subagents finish, without a prompt: such updates show the line too ("carrying on by itself") until it has been quiet for a few seconds with no tool running.
   - **Runs.** In the *Normal* view, two or more tool calls with no words between fold into one line saying what they did ("Read 3 files, ran 2 commands · 1 failed"; while one runs, "Running `cargo test`"). Click it to open the steps under it (`transcript::run_of`, `run_summary`).
   - **Turn summary.** Under a turn that changed files: how many, the lines added and removed, **Review changes** (the turn's diffs, merged per file) and **Undo turn** (the checkpoint before its message, after asking).
   - **View** (the layers button in the header, for every chat, kept in `chat.view_mode`): *Detailed* shows every step, *Normal* folds runs, *Summary* shows only messages, replies, questions and the turn summaries.
4. **Permission requests** appear inline and in the title-bar **Approvals** inbox (across all projects). Answering either one answers the agent once. Stop, or an agent exit, cancels them. Once answered, a request shrinks to a muted line.
   - **Plans.** When the agent asks to leave plan mode (ACP `switch_mode`), the card shows the plan, as Markdown, above the choices.
   - **Questions.** PilotHouse accepts ACP form elicitation (`elicitation/create`, `form.rs`): Claude Code's *AskUserQuestion* and MCP servers asking for values arrive as a question card. Pick one (radio) or several (checks) of the offered answers, or type your own (*Other…*); **Answer** sends, **Skip** declines. A single one-answer question is answered by the pick. Questions wait in the inbox like permissions, and make the chat *Needs you*.
   - **Subagents.** While subagents work, a line above the composer says how many ("2 subagents running"); it opens to each one's state, latest step, steps and time, and a row jumps to it in the transcript. Claude starts subagents in the background, so one counts as working while a step of it runs or for a few seconds after its last. Codex's `spawnAgent` calls show as subagents too, but its adapter keeps their own steps to itself.

## The composer

- **Model picker.** The model button shows the agent's mark and the current model. It opens one compact list with a search on top: each agent's name (with *This chat*, *New chat* or *Not set up*) over its models, each with its mark and a line about it. Typing filters by model or agent, ↑/↓ and Enter pick, Tab jumps to the next agent. It stays open while you look around; click outside or press Esc to close it. Models come from each agent's own ACP config options, or for agents on the older protocol (Gemini CLI) from its `models` and `modes` lists, changed with `session/set_model` and `session/set_mode` (`options.rs`): live for this chat's agent, cached from the last run for the others. Picking another agent's model switches a fresh chat to it, or opens a new chat if this one already started. Choices are remembered per agent and applied to new chats.
- **Effort and mode** are their own buttons beside the model (🧠 *Medium*, 🛡 *Manual*), each a short menu of the agent's levels or modes.
- **The composer grows** with what you type, from two lines to six, then scrolls.
- **Jump to latest.** Scrolled up, a ↓ button brings the newest message back. Within about two lines of the end counts as the end: the button hides, and new text keeps the view on the newest reply, unless you are scrolling (`transcript_scroll.rs`). The transcript keeps some room under the last message.
- **Who wrote what.** Each reply starts with the agent's mark and name (Claude's orange burst, Codex, Gemini, Antigravity). The chat header shows the agent's mark with a dot for its state (the words are in its tooltip), and the picker shows each agent's mark.
- **Where it works.** Before the title, the chat header names the place the agent works in: the workspace with its colour (and the subfolder, `name › sub`, when the chat runs in one), **Chats** for a chat outside any workspace (the agent gets a folder of its own), or the repository for a GitHub copy.
- **Diagrams.** A ```` ```mermaid ```` block in a reply is drawn as a picture once its closing fence arrives (flowcharts, sequence, class, state and ER diagrams, timelines, Gantt and more; `reply.rs`, drawn to SVG without a browser by `mermaid-rs-renderer`, in the theme's light or dark). One that does not parse stays code, with the reason; hovering a diagram offers to copy its source. The marks are bundled from Lobe Icons (MIT); see `crates/ui/assets/agents/`.
- **Mode** (permission mode) has its own pill; anything else the agent offers is under **⋯**. ⇧Tab in the composer switches to the next mode (Manual, Accept edits, Plan, Auto), as in Claude Code, but never to one that skips permissions.
- **Warm start.** Focusing the composer starts the agent and its session in the background, so the pickers fill in and the first reply comes sooner. Nothing is stored until the first message.
- **Attachments** show as chips (images with thumbnails) and are sent as ACP content: files as resource links, images as image content when the agent accepts images, text as text. Resting on a chip for two seconds (in the composer or a sent message) shows a large preview; clicking the chip or the preview opens the whole image, text or file (first 256 KB) in a dialog with Copy (`attachment_view.rs`).
- **Agent setup.** When an agent cannot start, the empty chat shows a setup card instead: CLI installed (with the install command to copy), Node.js for adapters (download link), adapter already in npm's cache, sign-in hint, and **Check again** (`setup.rs`, `setup_view.rs`). The picker lists such agents as **Set up** with the real reason. Settings → Agents shows every agent's card.
- An empty chat suggests starter prompts.
- **While the agent works**, Enter queues the message instead of sending it. Queued messages wait above the composer and go one at a time when a turn ends; each can be edited or removed. **Send now** adds the first to the turn under way when the agent supports steering (Claude Code and Codex: `_session/steering`, advertised in the handshake's `_meta.steering`), and otherwise (Gemini CLI, Antigravity) stops the agent and sends it.
- **⌘Enter** (Ctrl+Enter elsewhere) sends and opens a new chat at once: the sent one carries on in the sidebar, where its card shows how it does.
- **Commands.** `/` at the start of a message offers the agent's own commands (ACP `available_commands_update`, e.g. Claude Code's `/review` or `/compact`) with their descriptions.

## Mentions

- `@src/main.rs` attaches that file as a resource link the agent can read.
- `@src/` (a folder, offered with a trailing slash) attaches what the folder holds, folders first, up to 200 entries, so the agent sees its shape and reads what it needs.
- `#12` attaches board card 12 (title, description, acceptance criteria).
- `@screenshot` and `@log` attach the browser or device screen and recent logs ([live-panes.md](live-panes.md)); `@test-run` (or `@tests`) attaches how each UI test last went, with the first failing step's screen ([tests.md](tests.md)).
- Typing `@` and a name offers definitions (functions, types, classes in Rust, TypeScript/JavaScript, Python, Go, Kotlin/Java/Swift/C#, C/C++; found by reading the files, `ph_files::symbols`); one inserts `@path:line`, which attaches the code from that line to the end of the definition. Any `@path:line` works the same.
- `@pr:12` attaches a pull request of the workspace's GitHub repository (title, author, branches, description, and how to get its diff); typing `@pr` lists the open ones.

Typing `@` or `#` opens suggestions above the composer (`mention_complete.rs`): files of the project (fuzzy, listed in the background and refreshed every minute) and the live sources for `@`, cards by number or title for `#`. ↑↓ pick, Enter or Tab inserts the mention, Esc closes; Enter does not send while suggestions show.

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

The ↗ button in the chat header (*Continue with…*) moves the work to another agent: a new chat opens with that agent, says where it came from, and offers a first message to send or edit. With that message the new agent gets a recap of the old chat (its recent messages and tool calls, up to 8,000 characters) and is told to check the files' current state first. The old chat stays as it was.

## Side chat

**⌘;** (*Side chat about this one* in ⌘K) steps aside from the chat in front: a new chat with the same agent opens, titled *Aside: …*, and its first message carries a recap of the chat it came from. Nothing goes back to that chat. The ← in its header returns to it.

## Usage

When an agent reports usage (ACP `usage_update`: context used and window, running cost; the prompt reply's `usage`: running token totals), the chat header shows a small bar of how full the context is, its share, and the cost so far; the tooltip has the tokens in and out. Reports are running totals per agent session, so the store keeps the last one and adds only the difference: a restarted agent adds on, a resumed one is not counted twice (`ph_store::usage`, migration 5). *Usage…* in the palette totals tokens and cost by agent or project over 7 days, 30 days or all time. The costs are the agents' own estimates.

## Agent instructions

*Agent instructions…* in the palette shows the project's `AGENTS.md` (Codex, Antigravity and most agents), `CLAUDE.md` (Claude Code) and `GEMINI.md` (Gemini CLI), and **Sync** makes `AGENTS.md` the one place to write instructions: Claude Code and Gemini follow an `@AGENTS.md` line as an import, so their files start with one, and a project without `AGENTS.md` gets one made from its existing file. Nothing is deleted; each file's own lines stay below the import (`instructions.rs`).

## MCP servers and skills

*Settings → MCP & skills*:

- **MCP servers for every agent**: local (stdio) servers PilotHouse passes to each new agent session, next to its own `pilothouse` server (board, browser, devices). Set one up once instead of in each agent; switch it off or remove it here. The command line splits like a shell's; environment is one `KEY=value` per line. Stored in PilotHouse's database (`mcp_config.rs`).
- **In your agents' own settings**: the local servers found in `~/.claude.json` (global and this project), the project's `.mcp.json`, `~/.gemini/settings.json` and `.gemini/settings.json`, and `~/.codex/config.toml`. *Use for every agent* copies one over. Remote (URL) servers are not listed.
- **Skills**: `SKILL.md` folders for Claude Code (the project's `.claude/skills`, `~/.claude/skills`) and Codex (`~/.codex/skills`), with their name and description; *Open* edits one, *New project skill* starts one from a template (`skills.rs`).

## Several agents at once

*Run with several agents…* in the composer's **+** menu gives one prompt to two or more installed agents. PilotHouse takes a checkpoint of the project (uncommitted and new files included), makes a worktree per agent from it in its data folder (the dialog says how much space each copy takes), and starts a chat per agent working there. *Parallel runs…* in the palette compares them: files changed and lines added and removed per agent (counting new files), a link to each chat, **Keep** (applies that agent's changes to the project after a checkpoint, three-way when the project moved on) and **Discard** (removes the copy). A kept lane's copy stays for its chat until *Remove copy* (`ph_git::parallel`, `ph_store::parallel`, migration 6, `workspace/src/parallel.rs`).

## Search

⌘⇧K (or *Search chats* in the sidebar, the palette, or the chat's history menu when the sidebar is hidden) searches the words of every message in the project's chats, or in all projects. Results come as you type (the last word matches as a prefix), best first, with the matched words highlighted; picking one opens that chat scrolled to the message. The index is SQLite FTS5 (`message_search`, migration 4), kept in step with the messages by triggers, so it needs no upkeep.

## Persistence and restarts

Transcripts are stored per session, with the agent's ACP session id. After a restart, agents that support `session/load` (Claude Code does) resume their own session. For the others, PilotHouse starts a new session and sends a short recap of the earlier conversation with the first new message.

## Code map

| File | Role |
|---|---|
| `rpc.rs` | Child process and JSON-RPC: request/response matching, incoming requests, stderr tail |
| `connection.rs` | One agent process: handshake, routing updates and permission requests to threads |
| `thread.rs` | One session: prompt turns, checkpoints and restores, applying updates, persistence |
| `transcript.rs` | Entry model, stored form, diff summaries |
| `permission.rs` | Permission requests and the approval inbox |
| `options.rs` | Agent config options (model, mode, effort…), remembered choices |
| `attachments.rs` | Attachments, mentions and live sources (`@screenshot`, `@log`, `@test-run`) |
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
- **Limits.** agy has no per-session MCP setting, so Antigravity chats do not get the PilotHouse board, browser and device tools. Images are not sent.
