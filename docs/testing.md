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

## Driving the running app

`cargo dev` builds with the `automation` feature. The app then answers `ui.*` methods on its [control socket](control-socket.md). Input is injected straight into GPUI, so your real mouse and keyboard are never touched, and the window can stay in the background.

```sh
export PILOTHOUSE_DATA_DIR=/tmp/ph-profile        # same value as the running app
pilothouse ctl app.add_project '{"path":"/path/to/repo"}'
pilothouse ctl ui.click '{"label":"Board"}'        # by accessible label
pilothouse ctl ui.click '{"id":"send-message"}'    # by element id
pilothouse ctl ui.dispatch '{"action":"files::QuickOpen"}'
pilothouse ctl ui.input '{"text":"main.rs"}'
pilothouse ctl ui.press '{"key":"enter"}'
pilothouse ctl ui.screenshot '{"path":"/tmp/shot.png"}'
pilothouse ctl ui.elements                          # everything clickable, with ids/labels/bounds
pilothouse ctl app.state                            # projects, active tab, sessions, theme
```

Screenshots render the window offscreen (`Window::render_to_image`), so they need no screen-recording permission. Release builds do not include the `ui.*` methods.

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
