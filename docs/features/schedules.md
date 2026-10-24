# Schedules

Code: `crates/workspace/src/schedules/` (runner and dialog), `crates/store/src/schedules.rs` (data, times). Open with **Schedules…** in the command palette.

A schedule does one thing for a workspace, at set times or when something happens:

- **Ask the agent**: starts a chat with a prompt for the default agent, in the background; the chat on screen stays. When an event started it, the prompt ends with what happened ("Why now: pull request #12 … was opened", the changed files, the failing tests). Presets: *Morning summary*, *Nightly check*, *Review a pull request*, *Fix failing tests*.
- **Run the UI tests**: replays every recorded test of the workspace (web tests at the desktop size) and says how many passed.

**When** (`When` in the store):

- **At times**: every 15 or 30 minutes, hourly, daily, weekdays or weekly at a time (local time, `jiff`, so daylight saving is handled).
- **On events** (`triggers.rs`):
  - *Files change*, optionally only paths containing some text; it starts once the files have been quiet for 5 seconds, and changes its own run makes do not start it again.
  - *New commit*: a folder's Git HEAD moved.
  - *PR opens* and *Checks fail*: PilotHouse asks GitHub every 5 minutes (only for workspaces with such a schedule), with the account in Settings → GitHub; failing checks count once per commit.
  - *After another* schedule, each time or only when it fails (a test run fails when a test does): **chains**, like "when the nightly tests fail, ask the agent to fix them".
  
  The first look only learns how things are, so opening PilotHouse does not count every open pull request as new. An event schedule starts at most every 2 minutes, and never while it is still running.

How it runs:

- Only while PilotHouse is open; it looks for due schedules every 30 seconds. One that came due while PilotHouse was closed runs **once** when it opens, then waits for its next time.
- When a run finishes, a notification says how it went, in the window and from the system. The schedule keeps the result (the first line of the agent's reply, or the test summary) and, for agent runs, **Open chat**.
- Each row has a switch to pause it, **Run now**, **Edit** and delete.
- An agent run is a whole new chat each time and costs what any chat costs; the form warns for frequent agent runs. Agents still ask before acting, so an unattended run may wait in the approval inbox.

Not yet: events from other hosts than GitHub, and a push made outside PilotHouse (it shows as a commit only when HEAD moves).
