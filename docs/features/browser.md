# Browser

Crate: `ph-browser`. One browser pane per workspace, backed by your installed Chrome (or Chromium, Edge, Brave). There are no tabs; links that would open a new tab open in the pane.

Which browser: the one chosen in the pane (setting `browser.chrome_path`), else `PILOTHOUSE_CHROME`, else the first one installed. With none found, the pane says so and offers **Choose a browser…** (any Chromium-family program, or a macOS `.app`) and **Get Chrome**, which opens Chrome's download page in your default browser. PilotHouse never downloads a browser itself, so it stays on-device. Agents asking for the browser get the same message.

## How it works

1. **Start on demand.** Chrome starts the first time the Browser tab is shown or an agent calls a browser tool. It runs headless with a per-project profile in `<data dir>/browser/<project id>` (private to you), so logins persist per project and never mix with your own Chrome.
2. **DevTools over a pipe.** PilotHouse talks to Chrome through `--remote-debugging-pipe` (file descriptors 3 and 4), attaches to the page with a flat session, and enables Page, Runtime, Log and Network. There is no localhost port that other processes could use, and Chrome exits on its own when PilotHouse does, even after a crash.
3. **Frames.** While the pane is visible, `Page.startScreencast` sends JPEG frames sized to the pane (at the screen's scale). Each frame is decoded off the UI thread and acked after decoding, so only one frame is ever in flight. Hidden panes and other projects stream nothing.
4. **Input.** Clicks, drags, the wheel and keys go through `Input.dispatch*Event`. ⌘V pastes, and ⌘C/⌘X copy the page selection through the system clipboard.
5. **Idle stop.** A browser that is not shown and not used by an agent for 10 minutes is stopped to free its memory. The next use starts it again with the same logins.
6. **One page.** Links and `window.open` stay in the pane, and any other page a site opens is closed.
7. **Dialogs.** `confirm()`, `prompt()` and "leave page" dialogs appear as a card in the pane for you to answer. With the pane hidden they are cancelled (alerts are acknowledged), so an agent never confirms one by itself. Every dialog is logged.

On-device only: Chrome's sync, component updates, metrics and other Google services are switched off with launch flags, and the address bar never searches. Text that is not a URL is opened as `https://…`, and local hosts as `http://…`.

## The pane

- **Address bar** with back, forward and reload. `localhost:3000`, `:5173` and `127.0.0.1:8080` work.
- **Live tools** (the floating toolbar): annotate, pick an element, capture, record, before/after. See [live-panes.md](live-panes.md).
- **Console and Network** drawer, with error counts on the tabs, **Errors only**, and **Send to chat**.
- **⋯ menu**: **Allow other DevTools clients** opens a localhost port (off by default; any local process could use it) and then **Copy DevTools address**, so another client such as a browser MCP server can attach to the same browser. Also stop or start the browser.

## Agent tools

Given to every agent through the PilotHouse MCP server. Agents use the same browser the user sees.

| Tool | Does |
|---|---|
| `browser_open` | Navigate and wait for the load event (20 s max) |
| `browser_snapshot` | Title, URL, visible text, and interactive elements with CSS selectors |
| `browser_click` | Click by selector (scrolled into view) or by CSS-pixel point |
| `browser_type` | Type, optionally into a selector, optionally pressing Enter |
| `browser_press`, `browser_scroll` | Keys and scrolling |
| `browser_screenshot` | PNG image (viewport or full page) |
| `browser_evaluate` | Run JavaScript and return its JSON value |
| `browser_logs` | Recent console output or network requests, optionally errors only |

## Code map

| File | Role |
|---|---|
| `chrome.rs` | Find and launch Chrome with its pipe; stop a leftover Chrome holding the profile |
| `cdp.rs` | DevTools client over the pipe: reader thread, writes under a mutex, replies by id, sessions |
| `browser.rs` | `Browser` entity: lifecycle, events, input, agent actions |
| `logs.rs` | Console and network records (500 of each) |
| `input.rs`, `scripts.rs` | Key mapping, and the page-side scripts (picker, snapshot, single pane) |
| `browser_panel.rs` | The pane |
| `control.rs`, `hub.rs` | `browser.*` control methods; one browser per project |

Not yet: Windows (Chrome's pipe there uses handles, `--remote-debugging-io-pipes`).

UI test runs can fix the page at a screen size (`browser.emulate`, phone and tablet sizes laid out as on mobile; the pane shows it letterboxed) and clear the cache first (`browser.clear_cache`); the page follows the pane again afterwards.
