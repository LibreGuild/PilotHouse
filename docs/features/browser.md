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

Given to every agent through the PilotHouse MCP server. Agents use the same browser the user sees: the first time an agent (or a UI test run) acts in a workspace's built-in browser since PilotHouse started, the Browser tab comes to the front of that workspace's tools (not when you switched the Browser off in Settings → Features; while no window shows the workspace, the next use tries again), so you see that a test in the browser is going on (`BrowserHub::on_agent_use`).

Which browser first: with the Chrome extension connected (the brand's `chrome_extension` feature on), the server's instructions and the tools' descriptions tell agents to use the `chrome_*` tools for any page, `localhost` included, and the built-in `browser_*` tools only when the extension is not connected.

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

## Agents' Chrome (your Chrome, through the extension)

The pane is for the project's own pages. For real websites (web apps, dashboards, accounts, documentation) agents use **agents' Chrome** (`agent_chrome.rs`): your own Chrome (or Edge, Brave, Chromium), with your profile and your sign-ins, reached through **PilotHouse's Chrome extension**. PilotHouse no longer starts a Chrome of its own for this: sites such as Google refused to sign in to a browser driven that way.

### Setting it up

**Settings → Chrome extension** (also the command palette's *Let agents use your Chrome*, and the pane's **⋯** menu):

1. **Set up** registers PilotHouse's native messaging host (`com.libreguild.pilothouse`) with Chrome, Chromium, Edge and Brave for your user, only on this click. On macOS and Linux it writes `com.libreguild.pilothouse.json` into each browser's `NativeMessagingHosts` folder (browsers never run here are skipped); on Windows it sets the browsers' `HKCU\Software\…\NativeMessagingHosts\com.libreguild.pilothouse` keys to a manifest in `<data dir>/chrome-host`. The manifest's `allowed_origins` names only PilotHouse's extension (`chrome-extension://ldjgjondfmfmfdkbbbghciidipnanjgn/`), and its `path` is a launcher script in `<data dir>/chrome-host` that runs `pilothouse chrome-host` for this data folder (so a test profile reaches its own app). **Remove** takes back exactly what Set up wrote (manifests that point at another data folder's launcher are left alone).
2. **Load the extension**: open `chrome://extensions`, turn on **Developer mode**, **Load unpacked**, and choose `<data dir>/chrome-extension` (Settings shows the path, with Copy and Reveal). The app writes the extension there at each start from the files built into it (`crates/browser/extension/`), so every package carries it. Its manifest has a fixed public `key`, so its id is the same wherever it is loaded from. After PilotHouse updates, reload it in `chrome://extensions`.
3. **Pin** it: its toolbar button opens the **PilotHouse panel** (Chrome's side panel, below), where **Let agents use this tab** shares the tab in front (it joins the PilotHouse group) or stops sharing it.

Settings shows *Connected to Chrome 131* once the extension reached the app. One browser at a time: a second browser's extension is told to wait.

### How it works

- **Only the PilotHouse tab group.** Agents open their tabs in a tab group named *PilotHouse* (purple; made in the window you were last in). You share a tab by putting it in that group (drag it in, or use the toolbar button). The extension never lists, reads or attaches to any other tab; agents' tab lists show only the group's tabs. A tab taken out of the group (or a group renamed away from *PilotHouse*) is let go at once, and its agent is told. The extension never moves a tab into the group itself: a tab you open (Ctrl+T) stays yours unless Chrome puts it in the group (as it does for links opened from a group tab, and for a tab you open inside the group). A popup window a page an agent is driving opens (a sign-in window, which cannot be grouped) counts as the group's.
- **A tab per agent.** Each agent session (each `pilothouse mcp` bridge, named `<agent>-<pid>`) works in tabs of its own. A tab its own page opens (a link to a new tab, `window.open`: DevTools' `Page.windowOpen`) becomes that agent's current tab; any other new tab in the group is only a shared one; when it closes, the one before it is current again. Tabs you close are forgotten, and the agent is told. When an agent's chat ends, its tabs stay open for you but are no longer watched.
- **Your tabs, when you share them.** You can start something in a tab (a form half filled), share it, and ask the agent to finish it "in the active tab": it takes that tab over (`chrome_tabs` `take`), keeps what you typed, and works in it with the glow and **Stop** like in its own. Closing a taken tab from the agent's side gives it back without closing it. One agent at a time: another cannot take a tab an agent works in.
- **Driven with Chrome's debugger.** The extension attaches `chrome.debugger` to a tab when an agent acts in it, and PilotHouse lets go once the tab has been idle 30 seconds (not while its agent is stopped), so Chrome's *"PilotHouse" started debugging this browser* bar shows only while agents work. Closing that bar cancels the control of the tab; the agent is told to ask you before going on. The extension passes on only the DevTools methods PilotHouse uses (a fixed list in `background.js`: page navigation and screenshots, input, `Runtime.evaluate`, network logs and bodies, dialogs, file fields), only to tabs it attached.
- **You see what it does.** While an agent acts, its tab has a violet glow, a pointer glides to where it clicks (with a ripple), and a bar at the bottom says *Claude Code is using this tab* with **Stop**. They stay on while it thinks between steps and fade about 20 seconds after its last action, and are left out of its screenshots. **Stop** pauses that agent until you press **Let it continue**; its browser actions fail meanwhile with a message to ask you. The overlay lives in an isolated world of the page (`agent_scripts.rs`), so the site's scripts can neither see nor break it, and it is built without `innerHTML` or inline style tags, so pages with a strict Content Security Policy or Trusted Types show it too.
- **Signed in as you.** Agents use your sign-ins, but never type into password fields: they ask you to sign in in that tab. **Checks that a person is there** (CAPTCHAs, Cloudflare's "Just a moment…") are yours too: agents never try to solve them.
- **Dialogs.** A dialog the page opens during an action ends the action at once with what it asks. The agent answers it with `chrome_dialog` (its description asks it to check with you before confirming anything that deletes, sends, pays or publishes); you can also answer it in the tab.
- **Screenshots** of a tab Chrome does not draw (behind others) bring it to the front of its window after 12 s and try again.
- **Downloads** an agent's page starts are saved by Chrome where it saves yours; `chrome_downloads` lists them. **Uploads** take only files of the workspace's folders, or files the agent's pages downloaded. Chrome lets an extension fill file fields only with **Allow access to file URLs** turned on in its details.
- **Logs per tab**, while the debugger is attached: console messages and uncaught errors with `file:line:column` and their stacks, a line marking each move to another page, and every request with method, status, type, size, time, headers (`Authorization`, `Cookie`, `Set-Cookie` and any whose name says token, secret, key or session shown as `••••`), what was sent (8 KB) and, on request, the response body (64 KB of text). In what was sent and received, the values of fields named like passwords, tokens and keys are hidden too. 1000 of each are kept per tab, and each action's result says when there are new errors or failed requests.
- **The channel.** Extension ⇄ Chrome's native messaging (stdio: a 32-bit length, then JSON) ⇄ `pilothouse chrome-host` ⇄ a local endpoint in the data folder (`chrome-bridge.sock`, owner-only; a named pipe only you can open on Windows) ⇄ the app. No network port is opened. The host serves only the extension's origin, checks that every message is a JSON object within size limits (64 MB from Chrome, 32 MB from the app), and cuts the app's messages over Chrome's 1 MB limit into chunks the extension joins. If PilotHouse is not running, the host says so and exits; the extension tries again later (when a tab changes, or after up to a minute).
- **Not connected?** Every `chrome_*` tool answers that the user should set up the extension in Settings → Chrome extension (and keep Chrome open), and points the agent at the built-in `browser_*` tools meanwhile (where the build has the built-in browser).

| Tool | Does |
|---|---|
| `chrome_open` | Open a URL in the agent's tab (or a new one) and wait for it to load |
| `chrome_snapshot` | Title, address, scroll position, numbered interactive elements with their state, and the text |
| `chrome_click`, `chrome_hover`, `chrome_drag` | By element number, CSS selector, text on it, or a point; right and double clicks, modifier keys |
| `chrome_type`, `chrome_select`, `chrome_press` | Type (replace, a key at a time, Enter after), choose list options, keys and shortcuts (`Control+A`) |
| `chrome_scroll`, `chrome_wait` | Wheel scrolling; wait for an element, its going, an address, a quiet network or the load |
| `chrome_navigate`, `chrome_tabs` | Back, forward, reload; list, open, switch to and close the agent's tabs; list and take over tabs the user shared |
| `chrome_screenshot` | The tab, the whole page, or one element |
| `chrome_evaluate` | JavaScript in the page's own world |
| `chrome_console`, `chrome_network`, `chrome_request` | Logs as above, filtered; only what is new since the agent last looked; one request in full |
| `chrome_dialog`, `chrome_upload`, `chrome_downloads` | Answer a dialog, fill a file field, list downloads |
| `chrome_performance` | Core Web Vitals (LCP with its element, CLS with what shifted, INP, FCP, TTFB), long animation frames with their scripts, render-blocking resources, the slowest and heaviest requests, page weight and main-thread time; reloads to measure unless `reload: false` |
| `chrome_audit` | Accessibility (alt text, names, labels, contrast, headings, touch targets, zoom), SEO (title, description, h1, canonical, indexing) and best practices (HTTPS, mixed content, oversized or unsized images, quirks mode, console errors), each with the element's selector |
| `chrome_emulate` | A device (phone, tablet, laptop, desktop, or a size), a network (offline, slow-3g, fast-3g, 4g), a slower CPU, dark or light, reduced motion, a location, a time zone, a locale; `reset` |
| `chrome_mock` | Answer requests matching a pattern with a status, body, headers and delay, or make them fail; list and clear the rules |
| `chrome_memory` | JavaScript heap, DOM nodes (and about how many are detached), event listeners, documents, frames; `compare` shows what grew since the last look, for leaks |
| `chrome_errors` | The page's errors with their stacks mapped through its source maps to the original files (the workspace's when there) |
| `chrome_site_tools` | The tools a website offers agents itself (WebMCP, in Chrome's early preview): list them, call one |
| `chrome_workflows`, `chrome_run_workflow` | The workflows the user recorded; run one in the agent's tab, with `values` for what to type and `from_step` to go on after a stop |

**Emulation and mocks change your own tab**, so they last only while the agent works there: they end when the debugger lets go of the idle tab (two minutes after the agent's last emulation or mock at the least), when you press **Stop** (your tab is back to normal at once), or on `reset`/`clear`; the agent is told when they ended. Heap snapshots are not among what Chrome lets extensions do (DevTools' Memory panel takes them), so `chrome_memory` measures and compares instead.

**Workflows** (`workflows.rs`) are steps you recorded in Chrome (below), kept in PilotHouse's settings (`chrome.workflows`). An agent runs one with its own actions, so you see them with the glow and **Stop**: it opens the first page, clicks (by selector, then by the words on the element when the page changed), types, chooses and presses as recorded. It stops where a person is needed (a password: agents never type one; files to choose) or a step fails, and says which `from_step` to go on with. **Settings → Chrome extension → Workflows** lists them: their steps, **Make a test** (a chat drafts a Playwright test of it, for you to send), **Schedule…** (the Schedules dialog opens with a schedule that runs it filled in, for you to choose when and save), rename, and delete (after a yes).

## From Chrome to PilotHouse

The extension also brings things from your Chrome to PilotHouse, **always on your own action**, and what you send waits in a chat's message box until you send it: nothing reaches an agent by itself.

- **Send to PilotHouse**, from the right-click menu (*PilotHouse*), a shortcut, or the panel: the **selection** or the **page** as Markdown (its main text, without menus and footers), a **link**, an **image**, a **screenshot of an area** you drag over (a click takes the visible page), or an **element** you click (its selector, text, styles, the CSS that shapes it, the component and source file a dev build names (React, Vue, Svelte, `data-source`), its HTML, and a picture of it). The page's own scripts never see the reading: it runs in the extension's isolated world, and the element picker's click does not reach the page.
- **Where it goes**: the chat on screen in PilotHouse's front window (a new chat there when the chat on screen is a closed terminal session, which takes no messages), a new chat, or quick chat: the panel's **Send to**, also in the right-click menu. **Bring PilotHouse forward** (on by default) brings its window (or quick chat's) to the front.
- **Page text is marked**: whatever text comes from a page (what you send, `@tabs`, a site button's chat or card) reaches agents inside a `<web-page source="…">` tag with a line telling them to treat it as data, not as instructions (`browser_inbox::from_web`), since a page can carry text written to steer an agent.
- **Shortcuts**: ⌥⇧P (Alt+Shift+P) sends the selection, or the page when nothing is selected; ⌥⇧S an area; ⌥⇧E an element; ⌥⇧O opens the panel. They are Chrome's to change, at `chrome://extensions/shortcuts`.
- **Record a bug report**: a bar in the page says it is recording (with **Stop and send** and **Cancel**) while you do what goes wrong; it follows the tab from page to page. Stopping sends the steps you took (what you clicked, typed and chose; never passwords or card numbers), a video of the tab with four still frames, and the page's console errors and failed requests (read with Chrome's debugger, whose bar shows meanwhile; secret-looking address parameters are hidden). Three minutes at most.
- **Record a workflow**: the same bar and steps, saved in PilotHouse's workflows (above) for agents to run again. Ten minutes at most.
- **`@tabs`** in a chat (typed or picked, or *Tabs you shared in Chrome* in the composer's **+** menu; the message waits until they are read) attaches the text of the tabs you put in the PilotHouse group that are not an agent's, each read in an isolated world with the debugger attached only for the read.
- **Buttons on sites**: on GitHub, Jira and Linear, once you turn the site on in the panel (Chrome asks for access to it then; turning it off gives the access back), a small pill at the page's bottom right offers what fits: **Review in PilotHouse** (a pull request opens in the GitHub view), **Ask about it** and **Start a chat** (a new chat with the page attached and a prompt drafted, for you to send), **Fix this failure** (a failed Actions run), **Add to board** (an issue becomes a card on the board of the workspace on screen). × hides it on the page.
- **Send a summary**: where Chrome has its built-in summarizer (Gemini Nano), the panel offers a summary of the page made on your computer, sent instead of the page.

### The PilotHouse panel

The toolbar button opens it, beside the page: whether PilotHouse is connected; **Waiting for you** (chats that need an answer: a click opens the chat in PilotHouse); **This tab** (share it with agents; send from it; record); **Send to**; **Agents in Chrome** (each agent's tabs, a click brings one forward, and **Stop** or **Continue**, as the bar in its tab); and **Options**: the count of waiting chats on the toolbar icon (on), Chrome notifications when chats finish or need you (off: PilotHouse already posts system notifications), a notification when an agent needs you in a tab (on), buttons on sites, the sites the panel may read (its buttons ask once per site; the right-click menu and shortcuts need no such access: Chrome gives it for the one click), and the shortcuts.

When an agent needs you in one of its tabs (to sign in: agents never type passwords; or a check that a person is there), the tab comes to the front of its window and the window asks for attention without taking the keyboard, with a notification unless you turned it off.

### What the extension may do

It asks Chrome for `debugger`, `nativeMessaging`, `tabs` and `tabGroups` (agents' tabs), `activeTab`, `scripting` and `contextMenus` (reading the tab you point at, when you do), `sidePanel`, `notifications`, `storage` (its own choices, kept in Chrome), and `offscreen` and `tabCapture` (recording a tab's video for a bug report). It has no host permissions of its own and no content scripts by default: sites are reached only for your click, or once you allow them in the panel (*sites the panel may read*, *buttons on sites*), each with Chrome's own prompt.

## Code map

| File | Role |
|---|---|
| `chrome.rs` | Find and launch the pane's headless Chrome with its pipe; stop a leftover Chrome holding the profile |
| `cdp.rs` | DevTools client over a sink (Chrome's pipe, or the extension's bridge): replies by id, sessions |
| `browser.rs` | `Browser` entity: lifecycle, events, input, agent actions |
| `logs.rs` | Console and network records (1000 of each): sources, stacks, headers with credentials hidden, timing |
| `agent_chrome.rs`, `agent_scripts.rs`, `agent_control.rs` | Agents' Chrome: tabs per agent session in the PilotHouse group, overlay and page helpers in an isolated world, `chrome.*` control methods |
| `extension/` | The Chrome extension. `background.js`: the worker (agents' tabs with an allowlist of DevTools methods, the right-click menu, shortcuts, the panel's requests); `lib/` (the link to the app, the tab group, its settings, reading pages); `features/` (send, status and handoffs, recording, site buttons); `page/` (scripts run in pages: clip, area, picker, recorder, site buttons); `sidepanel.*`; `offscreen.*` (a tab's video) |
| `agent_chrome/inbox.rs` | What the extension brings (sends, site actions, saved workflows, Stop and Continue) and what it is told (chats waiting and at work, news, handoffs); the workspace shell says where things go (`BrowserHost`) |
| `agent_chrome/devtools.rs`, `sourcemap.rs` | Performance, audits, emulation, mocks, memory and mapped errors |
| `agent_chrome/shared_text.rs`, `site_tools.rs`, `workflow_run.rs`, `workflows.rs` | `@tabs`; WebMCP; running and keeping workflows |
| `extension.rs`, `bridge.rs`, `extension_view.rs` | The extension's files and id; Set up / Remove of the native messaging host; `pilothouse chrome-host`; the app's end of the channel; Settings → Chrome extension (with its workflows) |
| `input.rs`, `scripts.rs` | Key mapping, and the page-side scripts (picker, snapshot, single pane) |
| `browser_panel.rs` | The pane |
| `control.rs`, `hub.rs` | `browser.*` control methods; one browser per project |

Not yet: the pane on Windows (Chrome's pipe there uses handles, `--remote-debugging-io-pipes`). Agents' Chrome works there through the extension (a named pipe for the bridge, registry keys for the host), but has not been tried on Windows yet.

UI test runs can fix the page at a screen size (`browser.emulate`, phone and tablet sizes laid out as on mobile; the pane shows it letterboxed) and clear the cache first (`browser.clear_cache`); the page follows the pane again afterwards.
