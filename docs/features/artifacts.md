# Artifacts

Crate: `ph-artifacts`. Shell: `ph_workspace::artifacts` (what the window lends), the *Artifacts* place of the left column. Behind the `artifacts` feature (`brand/brand.toml`).

An artifact is a web page an agent made for you that PilotHouse keeps and serves from this computer: a budget planner, a dashboard, a report, a small tool, a game. It has an address of its own to open in a browser, today and another day, and it remembers what you enter in it. Nothing is uploaded: the files stay in a folder of yours, and the address answers only on this computer, until you share it with the people on your network, who can then open it and use it with you, live.

They are PilotHouse's own. Claude and ChatGPT have artifacts of theirs, kept on their servers; an agent is told to keep the two apart, so ask for "a PilotHouse artifact".

## Making one

In any chat, of a workspace or of *Chats*:

- **From nothing**: "Make me a monthly budget planner as a PilotHouse artifact." The agent builds the page in the workspace, then publishes it.
- **From what is there**: build and check a page as usual, then "publish this as a PilotHouse artifact".

Publishing copies the page into the artifacts' folder and shows it in the workspace's built-in browser, its tab in front, beside the chat. From there it is the loop people know: point at things with the browser's tools (pick an element, mark, capture), say what to change, and the agent edits the files in the workspace and publishes again. A page that is open, in the pane or in your own browser, reloads by itself.

Publishing again from the same folder makes a new version of the same artifact, never a second one. The site it replaces is kept (the last five).

## The Artifacts place

*Artifacts* in the left column lists everything made, the one changed last first: a picture of each page (taken when it was shown in the built-in browser), its name, when it changed and the workspace it came from. A search box appears from nine artifacts on. The folder button at the top right shows where they are kept.

- **A click** opens the artifact in the system's browser, where it is used.
- **⋯** (or a right click): *Open in the browser*; *Open beside the chat in …* (the workspace it was published from, to go on changing it there); *Copy its address*; *Share on your network…* (below); *Reveal in Finder*; *Rename…*; *Go back to* (an earlier version: the live one takes its place among the earlier ones, so going back can be undone); *Delete…* (to the Trash, after a yes).
- An artifact that is shared says so under its name, in green, with how many have it open; and the *Artifacts* icon in the column carries a green dot for as long as any is.

The command palette has *Show Artifacts*.

## On disk

`Documents/PilotHouse Artifacts/` (the home folder without a Documents folder), one folder each, named after the artifact:

| | |
|---|---|
| `artifact.json` | What it is: its id, name, description, first page, port, version, when it was made and changed, where it was published from, and once it was shared the key of its link and the port it is shared on. Fields a build does not know are kept |
| `site/` | The published files, as they are served |
| `data/` | What the page stored for its owner: one JSON file per key (`todos.json`). `data/shared/`: what everyone with the page open reads and writes |
| `.versions/` | The sites earlier publishes put there, by their number (the last five) |
| `.thumbnail.png` | The picture for the list (640 pixels wide at most) |

`PILOTHOUSE_ARTIFACTS` names another folder. A run on a data folder of its own (`PILOTHOUSE_DATA_DIR`: tests, scratch profiles) keeps its artifacts inside that folder, so it never writes among your documents, nor serves your artifacts a second time.

A folder copied by hand is told apart from its original when the list is read: the copy gets an id and a port of its own.

**What a publish copies**: a folder with the page in it, or one `.html` file (which becomes the site's `index.html`). Hidden files (`.env`, `.git`), `node_modules` and links to other folders are left out; 5,000 files and 512 MB at most, so a whole repository given by mistake stops there. The first page is the folder's `index.html`, else its only page, else the one `entry` names. The name is the page's `<title>` unless one is given. The copy is made beside the live site and swapped in, so a reader sees the old site or the new one, never half of each.

## How one is served

Each artifact has an address of its own, `http://127.0.0.1:<port>/` (`server.rs`), on the loopback interface only. The port (23100 to 23999) is the artifact's for good, so its address never changes.

- **A port each**, not one server with a path each. The artifact is then the root of its address, so a built site's `/assets/app.js` loads as it did where it was made, and a browser keeps each artifact's own storage and permissions apart from the others'.
- **Static files**: GET and HEAD, the type by the file's name, parts of a file for a film's seeking, `ETag` with `Cache-Control: no-cache` (asked again each time, answered *not changed*), so a new publish shows at once. A folder answers with its `index.html`. A path that names no file and has no extension answers with the first page, for client-side routes. Nothing above the site, and nothing hidden, is ever served; a link put into the site by hand cannot lead out of it.
- **Only its own address is answered** (`Host` must be `127.0.0.1:<port>` or `localhost:<port>`): a page elsewhere whose name was pointed at this computer is refused.
- **While PilotHouse runs.** The twelve changed last are served from the start, so a page left open yesterday loads today; the others when they are opened. At most 32 are served at once (the one asked for longest ago makes room).
- **The page runs as it was built.** It may load what it loads (a CDN script, a font): what worked before publishing works after. The guide asks agents to put what a page needs in its own files, so the artifact works without a network.

Plain threads over blocking sockets, written here (`http.rs`): a handful of requests from one person's browser need no web framework and no async runtime, and nothing wakes while nobody asks.

### What the app adds to a page

Under `/_artifact/` at the artifact's own address:

| | |
|---|---|
| `client.js` | Put into every HTML page as the first thing in its head: it defines `window.artifact` and reloads the page on a new publish |
| `storage` | What the page stores (below) |
| `shared` | What everyone with the page open reads and writes |
| `events` | The page's stream: which publish is live (when it connects and whenever that changes, also after the app was started again), who is here, what the others say, what changed in the shared storage |
| `room` | What the page says to the others |

## What a page stores

A browser's own storage belongs to that browser: what a page keeps in the built-in browser is not there in your Chrome, nor in another workspace's pane. So a page stores what you would miss with the app:

```js
await artifact.storage.set("todos", [{ text: "Milk", done: false }]);
const todos = (await artifact.storage.get("todos")) ?? [];   // null when nothing is kept
await artifact.storage.remove("todos");
const keys = await artifact.storage.keys();
```

Values are JSON, 5 MB each at most, 100 MB and 10,000 keys per artifact. They are kept in the artifact's `data/` folder, one file per key, so they are the same wherever the page is opened, survive a new publish, go with the folder, and read well by hand. Only the artifact's own page reads and writes them: a request the browser marks as another site's is refused.

An agent reads them with `artifact_data`, to answer questions about what you entered ("how much did I spend on food?"). What it gets is marked as data, never instructions.

## Sharing on your network

*Share on your network…* in an artifact's menu opens a dialog that says what sharing is before anything is opened: people on the same network who have the link can open the artifact while PilotHouse is running on this computer; it is not put on the internet. Nothing is shared until **Share** is pressed.

Shared, the dialog shows:

- **The link**, with **Copy link**: `http://<this computer's address>:<port>/?invite=<key>`. The address is the one of the network your colleagues are on, read from the network cards themselves (a VPN's tunnel, a container's bridge and the like are passed over; `sharing.rs`). With a VPN on, the dialog says that it may keep people from reaching this computer: many keep a computer off its own office network while they are up.
- **How many have it open** now, live.
- **They can take part** (on by default): guests play, vote, write what the page shares. Off, they only look. A change holds at once, also for those who are here.
- **New link**: another key; the link given before stops working, and those who came by it are let go.
- **Stop sharing**: the address closes and the guests are let go.

Sharing ends when you stop it or quit PilotHouse; nothing is shared again by itself at the next start. Shared again, an artifact has the same link as before (until *New link*), so a link a team keeps goes on working.

### What a guest gets

Guests come in by a second door (`server.rs`): the owner's address stays bound to this computer alone, and a second address opens on the network, on a port of its own, only while the artifact is shared. Which door a connection came through says who it is; nothing a request claims about itself does.

- **Needs the key.** The first visit with the link leaves the key in a cookie of that artifact (not readable by scripts, not sent along from other sites) and moves on to the plain address. Without the key: a page that says to ask for the link. Keys are compared in the same time whatever is wrong with them; a key is 130 random bits.
- **Gets**: the page's files, the shared storage and the room.
- **Never gets** what you stored in the artifact for yourself (`artifact.storage`): a guest's own storage is kept in the guest's browser. So sharing a budget planner does not give your budget away.
- A guest of an artifact shared to look at only sends nothing and writes nothing.

### Using it together, live

A page is built for several people with three things, which work the same between two windows on this computer:

```js
artifact.me                                   // { id, name, host }
artifact.room.on("people", (people) => …)     // who has the page open now
artifact.room.on("message", ({ from, data }) => …)
await artifact.room.send({ move: 3 })         // to everyone else here; { to: id } for one
await artifact.room.setName("Maya")
await artifact.shared.set("scores", { maya: 3 })           // kept with the artifact, for everyone
artifact.shared.on("change", ({ key, value }) => …)        // someone else changed it
```

- **The room** (`room.rs`) only passes messages on, to the pages open at that moment; nothing is kept. Each open page holds one stream; messages go out in the order sent, in batches. A page that closes is gone from the others' list within two seconds. Up to 100 open pages; a message is JSON up to 64 KB. A page that reads slower than the others write is let go and connects again.
- **The shared storage** is for what must outlast the moment (the board, the scores, whose turn it is): the same limits as an artifact's own storage, and every other open page is told of a change.
- **Who is who**: an id each browser keeps for the artifact. It names, it proves nothing: fine for a game among colleagues, nothing to guard a secret with. `host` is true on the computer that keeps the artifact. `?artifact-who=second` in the address is someone else for once, to try a game alone.
- The app writes no game logic and runs none of a page's code: an agent builds the game as a page, and these three are all the server it needs.

### What it cannot do

- **Only while PilotHouse runs on this computer**, and while it is awake: the computer is the server.
- **The same network only.** Guest and some office Wi-Fi networks keep devices apart; a VPN may; the internet is not reached (that would take someone else's server in between).
- **The firewall may ask** the first time whether PilotHouse may accept connections.
- **The link carries this computer's address**, which changes with the network: the dialog shows the one that holds now.
- **Plain HTTP for guests**, where browsers switch off what needs a secure page: camera and microphone, the clipboard API, service workers, notifications. Games, forms, boards and dashboards are not affected.

## With an agent

Given to every agent through the PilotHouse MCP server. The server's instructions say when to use them and that these artifacts are the app's own.

| Tool | Does |
|---|---|
| `artifact_guide` | What an artifact is, how to build a page for one, how it is served, `artifact.storage` with a fallback for before publishing, building for several people (`artifact.room`, `artifact.shared`, `artifact.me`), the limits. Read once before the first publish (`guide.rs`) |
| `artifact_publish` | `source` (a folder or one `.html` file, inside the workspace), `title`, `description`, `entry`, `artifact` (an id or name to replace), `open` (default true). Copies, serves, shows the page in the built-in browser and keeps a picture of it. The same `source` again is a new version |
| `artifact_list` | Every artifact: name, id, version, address, where it was published from |
| `artifact_data` | What an artifact stored: its keys, or one value |

They are the `artifacts.*` control methods (`control.rs`).

- **An agent publishes only from its own workspace**: the source is resolved with every link followed and checked against the workspace's folders (its scratch folder, for *Chats*; the copies its tasks run in) before a byte is copied. Where the copy goes is never the agent's to say.
- A relative `source` starts where the agent works: the bridge makes it a whole path, since it runs in the agent's folder (also in a task's own copy of the project).
- Agents do not rename, delete or share artifacts: those are yours, in the list. Opening a port to the network is never an agent's doing.

## Code map

| File | What |
|---|---|
| `artifacts/src/library.rs` | The artifacts on disk: the folder, `artifact.json`, publishing (plan, copy, swap), versions, rename, the picture |
| `artifacts/src/server.rs`, `http.rs`, `client.js` | An address per artifact, and a second one for guests while it is shared: files, the app's calls, the script every page gets |
| `artifacts/src/storage.rs` | What a page stores: keys as file names, the limits |
| `artifacts/src/sharing.rs` | The link: its key, the cookie it is kept in, this computer's address on the network |
| `artifacts/src/room.rs` | Who has a page open, and what is passed on to them |
| `artifacts/src/share_view.rs` | The share dialog |
| `artifacts/src/hub.rs` | The running app's artifacts (`Artifacts`): the list, every change followed by a fresh read; what the shell lends (`Host`) |
| `artifacts/src/control.rs`, `guide.rs` | `artifacts.*` for agents; what they are told |
| `artifacts/src/view.rs` | The list (`ArtifactsView`) and each artifact's menu |
| `mcp/src/tools/artifacts.rs` | The `artifact_` tools |
| `workspace/src/artifacts.rs` | A workspace's folders, showing a page in its built-in browser, bringing it in front |

## Not yet

- **Beyond the network**: sharing reaches the people on the same network, not the internet.
- A name for this computer in the link (`name.local`) instead of its address, which changes with the network.
- **More kinds made from the list**: a sheet, a document, a board. Today the list makes [slides](slides.md): the page starts with a row of cards, one for each kind that can be made there (smaller than the artifacts' cards under it; each kind draws its own picture, `NewKind::art`, from the theme's colours), and the *Slides* card opens a new deck in a window of its own, whose page the app serves through `ph_artifacts::Kind`. Every other artifact is a web page made in a chat.
- A picture in the list only for artifacts shown in the built-in browser (a build without it, or a publish with `open: false`, shows a placeholder).
- A build step: publish what a browser loads as it is.
