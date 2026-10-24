# Control socket

The running app listens on a Unix socket in its data directory (`control.sock`). If that path is too long for a socket, it uses a private per-user folder in the temp directory (or `$XDG_RUNTIME_DIR`), keyed by the data directory. The socket has owner-only permissions, clients refuse a socket in a folder other users could write, and nothing listens on the network. See [security.md](security.md).

Protocol: one JSON object per line.

```
→ {"id":1,"method":"board.list","params":{"project_id":3}}
← {"id":1,"result":{"cards":[…]}}        or   {"id":1,"error":"…"}
```

Try it with `pilothouse ctl <method> '<json params>'`.

## Always available

| Method | Params | Result |
|---|---|---|
| `app.ping` | – | `"pong"` |
| `app.state` | – | projects, active project, tab, sessions, theme |
| `project.resolve` | `path` | the project containing `path`, or `null` |
| `app.open` | `paths: [..]` | opens folders as projects and files in their project or own window (what `pilothouse <path>` forwards) |
| `test.expect` | `project_id`, `kind` (`text`, `no_text`, `url`), `value`, `device?` | waits for the check to hold; recorded as a step while a UI test records |
| `board.list` | `project_id` | `{cards}` |
| `board.add_cards` | `project_id`, `cards: [{title, description?, acceptance?}]` | `{cards}` |
| `board.next_card` | `project_id`, `agent?` | claims the top To do card → `{card}` |
| `board.update_card` | `card_id`, `title? description? acceptance? status? assignee?` | `{card}` |
| `board.add_note` | `card_id`, `text`, `agent?` | `{ok}` |
| `browser.open` | `project_id`, `url` | `{url, title}` after the load event |
| `browser.snapshot` | `project_id` | `{title, url, elements, text}` |
| `browser.click` | `project_id`, `selector` or `x`+`y` | `{url, title}` |
| `browser.type` | `project_id`, `text`, `selector?`, `submit?` | `{url, title}` |
| `browser.press`, `browser.scroll` | `project_id`, `key` / `dy` | `{url, title}` |
| `browser.screenshot` | `project_id`, `full_page?` | `{url, title, images: [{data, mimeType}]}` |
| `browser.evaluate` | `project_id`, `expression` | `{value}` |
| `browser.emulate` | `project_id`, `width`, `height`, `mobile?`; neither size to follow the pane again | `{}` |
| `browser.clear_cache` | `project_id` | `{}` (cookies and logins stay) |
| `browser.logs` | `project_id`, `kind?` (`console`/`network`), `errors_only?`, `limit?` | `{text, url}` |
| `device.list` | – | `{devices, avds}` |
| `device.start_emulator` | `avd` | `{serial}` once booted |
| `device.screenshot` | `serial?` | `{width, height, note, images}` |
| `device.ui` | `serial?` | `{text}`: elements with bounds |
| `device.tap`, `device.swipe`, `device.type`, `device.key` | `serial?`, coordinates / `text` / `key` | – |
| `device.logcat` | `serial?`, `lines?`, `filter?`, `errors_only?` | `{text}` |
| `device.install`, `device.launch` | `serial?`, `apk` / `package` | – |
| `runs.ports` | – | `{ports: [{port, address, pid, process, cwd}]}` |
| `util.list` | – | `{utilities: [{id, name, input, second, modes}]}` |
| `util.run` | `id`, `text`, `second?`, `mode?` (index or name) | `{output}` |
| `todo.list` | `project_id?` | `{todos: [{id, text, done, remind_at, all_workspaces}]}` |
| `todo.add` | `project_id?`, `text`, `remind_in_minutes?`, `all_workspaces?` | `{id, text, remind_at}` |
| `test.list` | `project_id` | `{tests: [{name, description, steps, passed, status}]}` |
| `test.run` | `project_id`, `name` | `{test, passed, failure: {step, does, error}?, looks_different: [{step, changed_percent}]}` |
| `test.expect` | `project_id`, `kind?`, `value`, `device?` | `{ok, note}` once the check holds |

## Development builds only (`automation` feature)

| Method | Params |
|---|---|
| `app.add_project` | `path` (skips the native folder picker) |
| `app.add_folder` | `path`: adds a folder to the workspace in front |
| `ui.elements` | – |
| `ui.click`, `ui.right_click` | `id`, `label` or `path` (suffix of an element path from `ui.elements`), or `at: [x, y]` |
| `ui.window` | `index?` | window count; with `index`, later `ui.*` calls act on that window |
| `ui.hover` | same targets as `ui.click`; moves the pointer there (hover cards, tooltips) |
| `ui.drag` | `from: [x, y]`, `to: [x, y]` (drawing on, or swiping in, a live pane) |
| `ui.scroll` | `at: [x, y]`, `dy` (pixels; positive scrolls down) |
| `ui.input` | `text` (typed at the current focus) |
| `ui.press` | `key`, e.g. `enter`, `secondary-k` |
| `ui.dispatch` | `action`, e.g. `workspace::NewSession` |
| `ui.screenshot` | `path` (PNG) |

Handlers start on the UI thread. Quick ones use `ControlServer::handle(name, handler, cx)` and answer at once. Slow ones use `ControlServer::handle_async`, which returns a task; the socket waits for it without blocking the window.

Windows: the socket is Unix-only for now; a named-pipe transport is planned.
