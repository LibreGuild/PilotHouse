# Testing

Test at the lowest level that proves the behavior.

| Level | Where | Run |
|---|---|---|
| Pure logic (store, fuzzy match, diff parsing, key encoding, MCP protocol) | `#[cfg(test)]` modules next to the code | `cargo test --workspace` |
| UI integration (real views in a headless window, real clicks and keys) | `crates/workspace/tests/ui.rs` | `cargo test -p ph-workspace` |
| End to end on the running app | dev automation over the control socket | `pilothouse ctl …` (below) |
| Performance budget | `scripts/perf.sh` | after a release build |

Some tests touch the real machine: `ph-changes` runs `git` in a temp repo, `ph-agent` spawns `/bin/sh` as a fake agent. They clean up after themselves.

## UI integration tests

Use `gpui_kit::test::TestWindowExt` (`click`, `press`, `input`, `find`) and `TestQueryExt` (`find_by_label`). Open the production `Workspace` on `Store::open_in_memory()`. Assert on the outcome the user cares about: state, what is saved, what is visible. Check [the gpui-kit testing guide](https://gpui-kit.com/docs/test).

A kit control (a button, a tab) is found by its id as it is. A row made of a plain `div` with an id and `on_click` is found, by a test and by `ui.click`, only when it also calls `.test_support()` (`gpui_kit::TestSupportExt`); without it the test says the id is missing though the row is drawn.

## Driving the running app

A start on a profile that has not seen the [welcome tour](features/tour.md) opens it over the window, where nothing else can be clicked until its last card. `PILOTHOUSE_NO_TOUR=1` starts without it.

`cargo dev` builds with the `automation` feature. The app then answers `ui.*` methods on its [control socket](control-socket.md). Input is injected straight into GPUI, so your real mouse and keyboard are never touched, and the window can stay in the background.

```sh
export PILOTHOUSE_DATA_DIR=/tmp/ph-profile        # same value as the running app
pilothouse ctl app.add_project '{"path":"/path/to/repo"}'
pilothouse ctl ui.click '{"label":"Terminal"}'     # by accessible label
pilothouse ctl ui.click '{"id":"send-message"}'    # by element id
pilothouse ctl ui.dispatch '{"action":"files::QuickOpen"}'
pilothouse ctl ui.input '{"text":"main.rs"}'
pilothouse ctl ui.press '{"key":"enter"}'
pilothouse ctl ui.screenshot '{"path":"/tmp/shot.png"}'
pilothouse ctl ui.elements                          # everything clickable, with ids/labels/bounds
pilothouse ctl app.state                            # projects, active tab, sessions, theme
```

Screenshots render the window offscreen (`Window::render_to_image`), so they need no screen-recording permission. Release builds do not include the `ui.*` methods.

### A deck's window

A deck of [slides](features/slides.md) is a web page in the system's web view, which the app's own screenshots and injected input do not reach. Three more methods (development builds only) do:

```sh
pilothouse ctl deck.screen '{"project_id":10}'                       # a picture of the page, as the window shows it
pilothouse ctl deck.script '{"project_id":10,"script":"window.deckEditor.store.go(2)"}'
pilothouse ctl deck.keys   '{"project_id":10,"take":true}'           # gives the page the keyboard; answers who has it
pilothouse ctl deck.keys   '{"project_id":10,"press":"t","code":17}' # a key, pressed as the system presses it (macOS)
```

`project_id` is the deck's chat space (`app.state`). `deck.keys` with `press` posts a real key event to the app, so it goes through the window's key equivalents and the menus to whatever has the keyboard, as a person's key does: this is how to check that a key typed in the chat does not act on the deck, and the other way round. It needs the window in front (`"front": true` puts it there). The page exposes `window.deckEditor` (`state`, `store`, `editing`, `view`, `comments`, `checks`) for scripts. With `PILOTHOUSE_SLIDES_PAGE=crates/slides/src/page` a debug build reads the page's files from that folder, so a change to them shows on reload.

### A board's window

A [whiteboard](features/whiteboard.md) is shown the same way, and has the same three methods: `board.screen`, `board.script` and `board.keys` (with `take`, `press`, `code`, `front` as above). They act on the board whose window is open; `"id"` (the artifact's id) names one when several are. `ui.window '{"index":1}'` points the `ui.*` methods at the board's window, for its chat and the app's dialogs over the board.

## The explanation prompt

What an agent is asked when it explains a change (`crates/review/src/explanation/prompt.rs`, see [explanations](features/explanations.md)) is tried on real agents and real changes, without the app:

```sh
# The prompt for the work not committed in a folder, or for a commit.
cargo run -p ph-review --example explanation_prompt -- /path/to/repo > /tmp/prompt.txt
cargo run -p ph-review --example explanation_prompt -- /path/to/repo --commit abc1234
# With what the chats said (a list of `Asked`: chat, agent, request, outcome, commands).
cargo run -p ph-review --example explanation_prompt -- /path/to/repo --asked record.json

# Give it to an agent in that folder, read-only, and read what comes back.
cd /path/to/repo
claude -p "$(cat /tmp/prompt.txt)" --allowedTools Read Grep Glob "Bash(git diff:*)" "Bash(git show:*)" "Bash(git status:*)"
codex exec -s read-only "$(cat /tmp/prompt.txt)"
```

Try a small change, an ordinary one, a large one, and a commit with no record. Read each reply as the person it is for: would you read it to the end, does it say how this part worked before, is anything in it not true of the code, is it within its words (the prompt names the limit), and is the explanation in the reply between its two marker lines. Agents differ: one wrote a file instead of replying, and one ran the project's tests, until the prompt said not to. Change the prompt, not the reply.

## Network drives (SSH)

Unit tests cover paths both ways (Windows spellings included), quoting, failures and parsing. Git against a real server is `crates/git/tests/network_drive.rs`, skipped unless pointed at one. A container with SSH and Git sharing a folder with this computer stands in for a Samba server:

```sh
mkdir -p /tmp/ph-share && ssh-keygen -t ed25519 -N '' -f /tmp/ph-key
# A Dockerfile: alpine + openssh-server git bash iproute2, a user `dev` with
# /tmp/ph-key.pub in ~/.ssh/authorized_keys, sshd on port 2222.
docker run -d --name ph-sshd -p 127.0.0.1:2222:2222 -v /tmp/ph-share:/srv/share ph-sshd
mkdir -p /tmp/ph-ssh && ssh-keyscan -p 2222 127.0.0.1 > /tmp/ph-ssh/known_hosts
eval "$(ssh-agent)" && ssh-add /tmp/ph-key
PH_SSH_TEST=1 PH_SSH_HOST=dev@127.0.0.1 PH_SSH_PORT=2222 PH_SSH_LOCAL=/tmp/ph-share \
  PH_SSH_REMOTE=/srv/share PH_SSH_STATE=/tmp/ph-ssh cargo test -p ph-git --test network_drive
```

In the app, add the same mapping in Settings → Network drives and open a folder under `/tmp/ph-share`.

## Visual regression

Keep reference screenshots of key screens (welcome, chat, each tab, light and dark). Compare new captures after UI changes. Automating the comparison is planned; for now, review the images.

## Coverage

Which lines and functions the tests run, with [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) (once: `cargo install cargo-llvm-cov --locked` and `rustup component add llvm-tools-preview`):

```sh
cargo coverage-summary -p ph-store   # a table: per file, regions, functions and lines covered
cargo coverage -p ph-store           # the same as pages, in target/llvm-cov/html
cargo coverage --workspace           # everything: builds it all again, instrumented (tens of GB)
```

A run builds what it covers a second time, instrumented, in `target/llvm-cov-target`, so one crate at a time is the usual way to look. A function no test runs is not thereby unused: the compiler (with `-D warnings`) says what nothing in a crate calls, and for `pub` items another crate might call, count their uses across `crates/`.
