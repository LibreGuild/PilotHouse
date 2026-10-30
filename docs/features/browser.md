# Browser

Crate: `ph-browser`. One browser pane per workspace, backed by your installed Chrome (or Chromium, Edge, Brave). There are no tabs; links that would open a new tab open in the pane.

Which browser: the one chosen in the pane (setting `browser.chrome_path`), else `PILOTHOUSE_CHROME`, else the first one installed. With none found, the pane says so and offers **Choose a browser…** (any Chromium-family program, or a macOS `.app`) and **Get Chrome**, which opens Chrome's download page in your default browser. PilotHouse never downloads a browser itself, so it stays on-device. Agents asking for the browser get the same message.

## How it works

1. **Start on demand.** Chrome starts the first time the Browser tab is shown or an agent calls a browser tool. It runs headless with a per-project profile in `<data dir>/browser/<project id>` (private to you), so logins persist per project and never mix with your own Chrome. Sites see the Chrome it is: the user agent and client hints say Chrome (headless Chrome otherwise names itself `HeadlessChrome`), and `navigator.webdriver` is off (`--disable-blink-features=AutomationControlled`), so ordinary sites do not answer with a CAPTCHA.
2. **DevTools over a pipe.** PilotHouse talks to Chrome through `--remote-debugging-pipe` (file descriptors 3 and 4), attaches to the page with a flat session, and enables Page, Runtime, Log and Network. There is no localhost port that other processes could use, and Chrome exits on its own when PilotHouse does, even after a crash.
3. **Frames.** While the pane is visible, `Page.startScreencast` sends JPEG frames sized to the pane (at the screen's scale). Each frame is decoded off the UI thread and acked after decoding, so only one frame is ever in flight. Hidden panes and other projects stream nothing.
4. **Input.** Clicks, drags, the wheel and keys go through `Input.dispatch*Event`. ⌘V pastes, and ⌘C/⌘X copy the page selection through the system clipboard.
5. **Idle stop.** A browser that is not shown and not used by an agent for 10 minutes is stopped to free its memory. The next use starts it again with the same logins.
6. **One page.** Links and `window.open` stay in the pane, and any other page a site opens is closed.
7. **Dialogs.** `confirm()`, `prompt()` and "leave page" dialogs appear as a card in the pane for you to answer. With the pane hidden they are cancelled (alerts are acknowledged), so an agent never confirms one by itself. Every dialog is logged.

Chrome's sync, component updates, metrics and other Google services are switched off with launch flags. A new pane opens Google's home page. In the address bar, an address (a dotted host such as `example.com`, or a local one) opens as `https://…`, local hosts as `http://…`; anything else (words, a single name such as `gpui`) is searched for on Google.

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

## Agents' Chrome (real websites)

The pane is for the project's own pages. For real websites (web apps, dashboards, accounts, documentation) agents use **agents' Chrome** (`agent_chrome.rs`): your installed Chrome (or Edge, Brave, Chromium, as chosen for the pane), started visible with a profile of its own in `<data dir>/browser/agents`. No extension is needed, nothing comes from a store, and your everyday Chrome and its profile are untouched.

- **Easy to tell apart.** Its windows take PilotHouse's violet (set in its profile on first start; a color you pick there yourself is kept), and its first tab is a PilotHouse page saying what the window is for, rather than a blank one.
- **Signed in once, for every agent.** One profile serves every workspace and chat, and it is kept across restarts, so a site you sign in to there stays signed in. **Open PilotHouse Chrome** (the command palette, or the pane's **⋯** menu) brings it up for you to sign in or watch. Agents never type into password fields: they ask you to sign in in that window.
- **A tab per agent.** Each agent session (each `pilothouse mcp` bridge, named `<agent>-<pid>`) works in its own tab: the first takes the window's first tab, others open windows of their own, so agents never act in each other's pages. A tab a page opens (a link to a new tab, a sign-in popup) becomes that agent's current tab; when it closes, the one before it is current again. Tabs you close are forgotten, and the agent is told. When an agent's chat ends, its tabs stay open for you but are no longer watched, so a page left running costs PilotHouse nothing.
- **Your tabs, when you ask.** Tabs you open yourself are yours: no agent acts in them, and the first agent only takes the window's first tab while it is still blank. An agent's tab list shows your tabs too, with the one on screen marked, so you can start something (a form half filled) and ask the agent to finish it "in the active tab": it takes that tab over (`chrome_tabs` `take`), keeps what you typed, and works in it with the glow and **Stop** like in its own. Closing a taken tab from the agent's side gives it back to you without closing it. One agent at a time: another cannot take a tab an agent works in.
- **You see what it does.** While an agent acts, its tab has a violet glow, a pointer glides to where it clicks (with a ripple), and a bar at the bottom says *Claude Code is using this tab* with **Stop**. They stay on while it thinks between steps and fade about 20 seconds after its last action, and are left out of its screenshots. **Stop** pauses that agent in this browser until you press **Let it continue**; its browser actions fail meanwhile with a message to ask you. The overlay lives in an isolated world of the page (`agent_scripts.rs`), so the site's scripts can neither see nor break it, and it is built without `innerHTML` or inline style tags, so pages with a strict Content Security Policy or Trusted Types show it too. Agent clicks pass through the bar; yours reach Stop.
- **Full speed in the background.** Chrome starts with background throttling off (`--disable-background-timer-throttling`, `--disable-backgrounding-occluded-windows`, `--disable-renderer-backgrounding`), so agents in tabs behind others, or windows under others, keep working. A screenshot that Chrome does not draw within 12 s brings the tab forward and tries again.
- **Dialogs.** A dialog the page opens during an action ends the action at once with what it asks, rather than waiting on a page that cannot go on. The agent answers it with `chrome_dialog` (its description asks it to check with you before confirming anything that deletes, sends, pays or publishes); you can also answer it in the window.
- **Checks that a person is there** (CAPTCHAs, Cloudflare's "Just a moment…") are yours: agents never try to solve them, and when a page shows one, the agent's result tells it to ask you to complete it in that window. A new profile is checked more often; signing in to your Google account there makes it rarer.
- **Downloads** go to `PilotHouse` in your Downloads folder (`<data dir>/downloads` without one), named as the site names them (`report (2).pdf` when taken). **Uploads** take only files of the workspace's folders or that downloads folder.
- **Logs per tab**, kept for every agent to debug with: console messages and uncaught errors with `file:line:column` and their stacks, a line marking each move to another page, and every request with method, status, type, size, time, headers (`Authorization`, `Cookie`, `Set-Cookie` and any whose name says token, secret, key or session shown as `••••`), what was sent (8 KB) and, on request, the response body (64 KB of text). In what was sent and received, the values of fields named like passwords, tokens and keys are hidden too, so signing in to a site in an agent's tab never shows the agent your password. 1000 of each are kept per tab, and each action's result says when there are new errors or failed requests.
- **Over the pipe.** Like the pane, it is driven over `--remote-debugging-pipe`: no port another program could use, and Chrome ends with PilotHouse. If you quit it, the next agent action starts it again (your sign-ins are kept) and tells the agent its tab is gone.

| Tool | Does |
|---|---|
| `chrome_open` | Open a URL in the agent's tab (or a new one) and wait for it to load |
| `chrome_snapshot` | Title, address, scroll position, numbered interactive elements with their state, and the text |
| `chrome_click`, `chrome_hover`, `chrome_drag` | By element number, CSS selector, text on it, or a point; right and double clicks, modifier keys |
| `chrome_type`, `chrome_select`, `chrome_press` | Type (replace, a key at a time, Enter after), choose list options, keys and shortcuts (`Control+A`) |
| `chrome_scroll`, `chrome_wait` | Wheel scrolling; wait for an element, its going, an address, a quiet network or the load |
| `chrome_navigate`, `chrome_tabs` | Back, forward, reload; list, open, switch to and close the agent's tabs; list and take over the user's tabs |
| `chrome_screenshot` | The tab, the whole page, or one element |
| `chrome_evaluate` | JavaScript in the page's own world |
| `chrome_console`, `chrome_network`, `chrome_request` | Logs as above, filtered; only what is new since the agent last looked; one request in full |
| `chrome_dialog`, `chrome_upload`, `chrome_downloads` | Answer a dialog, fill a file field, list downloads |

## Code map

| File | Role |
|---|---|
| `chrome.rs` | Find and launch Chrome with its pipe; stop a leftover Chrome holding the profile |
| `cdp.rs` | DevTools client over the pipe: reader thread, writes under a mutex, replies by id, sessions |
| `browser.rs` | `Browser` entity: lifecycle, events, input, agent actions |
| `logs.rs` | Console and network records (1000 of each): sources, stacks, headers with credentials hidden, timing |
| `agent_chrome.rs`, `agent_scripts.rs`, `agent_control.rs`, `agent_start.html` | Agents' Chrome: tabs per agent session, overlay and page helpers in an isolated world, `chrome.*` control methods |
| `input.rs`, `scripts.rs` | Key mapping, and the page-side scripts (picker, snapshot, single pane) |
| `browser_panel.rs` | The pane |
| `control.rs`, `hub.rs` | `browser.*` control methods; one browser per project |

Not yet: Windows (Chrome's pipe there uses handles, `--remote-debugging-io-pipes`).

UI test runs can fix the page at a screen size (`browser.emulate`, phone and tablet sizes laid out as on mobile; the pane shows it letterboxed) and clear the cache first (`browser.clear_cache`); the page follows the pane again afterwards.
