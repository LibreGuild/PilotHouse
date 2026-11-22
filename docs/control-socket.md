# Control socket

The running app listens on a Unix socket in its data directory (`control.sock`). If that path is too long for a socket, it uses a private per-user folder in the temp directory (or `$XDG_RUNTIME_DIR`), keyed by the data directory. The socket has owner-only permissions, clients refuse a socket in a folder other users could write, and nothing listens on the network. See [security.md](security.md).

Protocol: one JSON object per line.

```
→ {"id":1,"method":"test.list","params":{"project_id":3}}
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
| `test.expect` | `project_id`, `kind` (`text`, `no_text`, `url`, `look`), `value`, `device?` | waits for the check to hold; recorded as a step while a UI test records (`look` records a check of how the screen looks, and needs no value) |
| `test.list`, `test.run` | `project_id`, `name` | the UI tests with how each last went, and the commands of the project's own tests; a replay of one UI test |
| `test.record`, `test.save` | `project_id`, `checks`, `url` or `package`, `name?` / `project_id` | makes a UI test and starts recording it; ends the recording and writes its steps down |
| `browser.open` | `project_id`, `url` | `{url, title}` after the load event |
| `browser.snapshot` | `project_id` | `{title, url, elements, text}` |
| `browser.click` | `project_id`, `selector`, `x`+`y`, or `target` (`role`, `name`, `within?`, `nth?`, `selector?`: an element as a UI test names it); `wait_ms?` | `{url, title, element}`: `element` is what was clicked, as a test would name it |
| `browser.type` | `project_id`, `text`, `selector?` or `target?`, `submit?`, `wait_ms?` | `{url, title, element?, secret?}` (`secret` for a password field) |
| `browser.text` | `project_id` | `{url, text}`: all the page says |
| `browser.where` | `project_id` | `{url}`, without starting the browser |
| `browser.clear_sites` | `project_id`, `urls` | erases what those sites keep (sign-ins, storage) and cached pages |
| `browser.press`, `browser.scroll` | `project_id`, `key` / `dy` | `{url, title}` |
| `browser.screenshot` | `project_id`, `full_page?` | `{url, title, images: [{data, mimeType}]}` |
| `browser.evaluate` | `project_id`, `expression` | `{value}` |
| `browser.emulate` | `project_id`, `width`, `height`, `mobile?`; neither size to follow the pane again | `{}` |
| `browser.clear_cache` | `project_id` | `{}` (cookies and logins stay) |
| `browser.logs` | `project_id`, `kind?` (`console`/`network`), `errors_only?`, `limit?` | `{text, url}` |
| `device.list` | – | `{devices, avds}` |
| `device.start_emulator` | `avd` | `{serial}` once booted |
| `device.screenshot` | `serial?` | `{width, height, note, images}` |
| `device.ui` | `serial?` | `{text, texts}`: elements with bounds, and the words on the screen alone |
| `device.tap` | `serial?`, `x`+`y`, or the element's `text`, `id` or `desc` with `wait_ms?`; `screen?` (the size the point was recorded on) | where it tapped; while a UI test records, the `element` under it |
| `device.swipe`, `device.type`, `device.key` | `serial?`, coordinates (`screen?`) / `text` (`wait_ms?`: until a field has the focus) / `key` | – |
| `device.front` | `serial?` | `{package, serial}`: the app in front, and the device asked |
| `device.logcat` | `serial?`, `lines?`, `filter?`, `errors_only?` | `{text}` |
| `device.install`, `device.launch` | `serial?`, `apk` / `package` (`fresh?`: stopped, from its first screen; `clear?`: its data erased too) | – |
| `runs.ports` | – | `{ports: [{port, address, pid, process, cwd}]}` |
| `runs.configs` | `project_id` | `{configs: [{name, command, added, folder}]}`: those found in the project and those added |
| `runs.add`, `runs.remove` | `project_id`, `name`, `command`, `folder?` / `name` | adds (or changes, by name) or removes an added run configuration; open Run tabs follow |
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
| `ui.scroll` | `at: [x, y]`, `dy` (pixels; positive scrolls down), `dx` (positive scrolls right) |
| `ui.pinch` | `at: [x, y]`, `delta` (a trackpad pinch: 0.1 zooms in by a tenth, negative zooms out) |
| `ui.input` | `text` (typed at the current focus) |
| `ui.press` | `key`, e.g. `enter`, `secondary-k` |
| `ui.dispatch` | `action`, e.g. `workspace::NewSession` |
| `ui.screenshot` | `path` (PNG) |
| `deck.screen` | `project_id` (a deck's chat) | a picture of the deck's page as its window shows it (`images`); the app's own screenshots do not show a web view |
| `deck.script` | `project_id`, `script` | runs the script in the deck's page |
| `deck.keys` | `project_id`, `take?`, `press?` (what the key types), `plain?`, `code?`, `command?`, `shift?`, `function?`, `front?` | `{web}`: whether the deck's page has the keyboard (macOS). `take` gives it the keyboard, as a click in it does; `press` presses a key as the system does (macOS), `front` putting the window in front first; then also `{front}` |

Handlers start on the UI thread. Quick ones use `ControlServer::handle(name, handler, cx)` and answer at once. Slow ones use `ControlServer::handle_async`, which returns a task; the socket waits for it without blocking the window.

Windows: the socket is Unix-only for now; a named-pipe transport is planned.
