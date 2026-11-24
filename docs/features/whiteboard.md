# Whiteboard

An endless board of notes, shapes, arrows and drawings, made under [Artifacts](artifacts.md) and worked on beside a chat: the agent draws a flow, a plan or the parts of a system; you move, type and draw by hand. Behind the `whiteboards` feature (which needs `artifacts`).

## Making one

- **Artifacts → Whiteboard** (the card at the head of the page), or *New whiteboard* in the palette. The board opens in a window of its own: the chat on the left, the board on the right.
- An empty board offers **templates** (a flow, a retrospective, a task board, a map of ideas, for and against, two by two, a timeline, a system), and the chat offers four starts (*Draw a flow*, *Map a system*, *Think it through with me*, *Tidy the board*).
- **From a workspace's chat**: "draw how sign-in works", "map this project". The agent makes a board (`board_create`), reads the code and draws. The board shows beside the chat as it is drawn, to look at (it keeps what is drawn in sight until you scroll or zoom yourself); **Open in Whiteboard** opens its own window, with the same conversation on the left. Such a board belongs to the workspace: its chats change the board's file without asking, as they do a deck's.
- **The workspace a board works from** is in its title bar, and changed there: a board made under Artifacts can be given a workspace (its agent then has the workspace's files to draw from), and one made from a workspace can be let go of (*No workspace: this board only*).

## Where things are

- **The tools, down the left edge**: select (`V`), move the board (`H`, or hold Space), note (`S`), text (`T`), shape (`R`: fourteen shapes), arrow (`L`), pen (`P`; highlighter `M`, eraser `E`), section (`F`), table, sticker (`K`), picture. A tool with a mark in its corner opens its choices on a second press.
- **The bar on top**: undo, redo and Templates on the left; the timer, the walk through the sections, pinned notes, share and export on the right.
- **The bar over what is selected**: what that thing can be given (colour, shape, outline, text size, bold, italic, list, alignment; an arrow's ends, route and dash; a table's rows and columns; votes on a note; lining up and tidying several), the lock and a menu with the rest.
- **The corner, bottom right**: the small map of the board, zoom and fit.

## On a board

- **Notes** in nine colours; a note grows with its words. Double-click the empty board for one. **Text** alone, in five sizes. **Shapes** with words in them, a fill and an outline. **Stickers**: any emoji, found by a word.
- **Arrows** tie to what they join and follow it when it moves: drag from one thing to another with the arrow tool, or from one of the four dots around a selected thing. A click on a dot makes the same thing again on that side, joined by an arrow and ready to be typed in: how a flow or a map grows. An arrow is straight, curved or at right angles, has a label, and ends in nothing, an arrow or a dot.
- **Sections** hold what lies in them: moved, their contents move. They are taken hold of by the edge or the title.
- **Tables**: type in the cells (Tab goes on), add and remove rows and columns.
- **The pen** draws freehand in four widths; the highlighter marks; the eraser removes strokes.
- **Pictures** are dropped, pasted or chosen, and kept with the board (`media/`).
- **Several things**: drag over them or Shift-click; line them up, space them evenly, **Tidy up** (rows in reading order), group (⌘G), lock (⇧⌘L), bring to the front (`]`), send to the back (`[`), duplicate (⌘D, or drag with Alt), copy and paste (also from one board to another). Things snap to each other's edges and middles while moved (hold ⌘ or Ctrl to move freely).
- **Pasted words** become notes, one a line: a list from anywhere is a wall of notes.
- **Votes**: a dot on a note per click of *Vote* (Shift-click takes one off); the outline lists them.
- **Zoom**: pinch or ⌘-scroll, `+`, `−`, `0` (100%), ⇧1 (fit), ⇧2 (what is selected).

## Working with others and with the agent

- **Pinned notes** (`C`): click a thing or a spot and write. *Send to the agent* puts the note in the chat with what it is about; the agent makes the change, answers in the note and resolves it. The list of notes is in the top bar.
- **Select, then say "this"**: the agent is told what is selected with each message, and whether the board was changed by hand since it last looked.
- **The timer**: a round of work counted down in the top bar, with a chime.
- **The walk**: the sections one at a time, each filling the window (arrows to go on, Esc to end): a board talked through like slides.
- **Share on your network** like any artifact: people who open it see the board as it changes; only this computer changes it.
- **Export**: a picture of the whole board (PNG, in the board's `exports` folder), or the board as an outline of words (sections with their notes, what leads to what) on the clipboard.

## How it is kept

```
<artifacts>/<board>/
  artifact.json      the artifact: kind "whiteboard"
  site/
    board.json       everything on the board
    media/           pictures, named by what they hold
  exports/           what Export writes
```

`board.json` is `{ version, title, elements: [...] }`; an element has an `id`, a `type` (`sticky`, `shape`, `text`, `frame`, `line`, `table`, `sticker`, `image`, `draw`, `comment`) and its fields, and fields a build does not know are kept. An arrow's ends name the elements they are tied to (`{ "id": … }`), so it has no place of its own.

The file is the truth, and it is written by hand as often as by the page, so the page takes it as it comes: an element with no size gets its kind's, one with no id gets one, what is no element is left out, and one that cannot be drawn is skipped while the rest is. A picture is shown only from the board's own `media/` (an address elsewhere is not fetched), and a colour that is no name of the board's only as `#rrggbb`.

The file is the truth. The agent edits it with its own file tools (`board_guide` says how a board is written, `board_look` what is on one; `board_create` and `board_list` make and list a workspace's boards); a watcher sees the change and every open page draws it, as one step of undo. A change made in the page is written a moment later, with the mark of the file it was made on. If the file changed meanwhile (the agent wrote while you moved something), the page takes the file and lays over it what was changed here and not yet written, thing by thing; where both changed the same thing the file's wins, and what was here is a step back. What is being typed stays open through it, and a drag is finished first. Undo and redo are the page's (200 steps).

The picture in the Artifacts list is drawn by the page from the board itself (all of it, without the controls), a few seconds after the board was last changed.

## Code

| Where | What |
|---|---|
| `crates/boards/src/board.rs` | A board's file: an empty one, its mark, its outline for an agent |
| `crates/boards/src/kind.rs` | What a board's address answers (`/_artifact/whiteboard/…`): the page, `board`, `save`, `media`, `picture`, `tell`; the watcher |
| `crates/boards/src/hub.rs` | `Boards`: making a board (for a workspace too), what a page tells the app, the picture for the list, export |
| `crates/boards/src/guide.rs`, `control.rs` | What agents are told (`board.guide`), what is on a board (`board.look`), a board for a workspace (`board.create`, `board.list`) |
| `crates/boards/src/page/` | The page: `model.js` (state, undo, the file), `shapes.js` (colours, outlines, where arrows meet things), `draw.js`, `tools.js` (pointer and keys), `bars.js` (controls, pinned notes), `extras.js` (templates, timer, map, walk, the picture), `board.css` |
| `crates/artifacts/src/kit/` | What the pages of Slides and Whiteboard share: `ui.js` (buttons, menus, dialogs, the icons), `kit.css`, the fonts (`/_artifact/kit/…`) |
| `crates/workspace/src/board_window.rs` | The window: the chat beside the board (its own, or its workspace's), what the page asks of the app |
| `crates/workspace/src/artifact_window.rs` | What the windows of a deck and a board share: the workspace they work from, stepping aside for the app's dialogs |
| `crates/workspace/src/artifacts.rs` | A board (or deck) made from a workspace's chat: shown beside it, its file trusted to that workspace's chats |
| `crates/mcp/src/tools/boards.rs` | The agents' `board_create`, `board_list`, `board_guide` and `board_look` |

The page is plain modules, built into the app; a debug build reads them from the folder named by `PILOTHOUSE_BOARD_PAGE` (and the kit from `PILOTHOUSE_KIT_PAGE`), so a change shows on reload. A development build has `board.screen`, `board.script` and `board.keys` for tests of the board's window ([testing.md](../testing.md)).

## Not yet

- People a board is shared with look, and do not change it; nobody's pointer shows on another's board.
- Stickers are emoji: none are drawn or made by an agent.
- On Linux the board opens in the browser, not in the window.
