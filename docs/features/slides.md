# Slides

Crates: `ph-slides` (the deck, its page, comments, exports, PowerPoint in and out) and `ph-webview` (the system's web view inside a window). Window: `ph_workspace::slides_window`. Agent tools: `crates/mcp/src/tools/decks.rs`.

Slides makes presentations beside a chat: the agent drafts and changes the deck, the user changes it by hand, points at things and comments on them, presents it, shares it and exports it. A deck is an [artifact](artifacts.md) of the kind `slides`: a folder of web pages, kept and served like any other artifact.

## Finding it

- The **Slides** card at the head of the Artifacts page makes a deck and opens its window. Decks are listed with the other artifacts (their card says *Slides*) and open in their window; the palette has **New slides**.
- **In a workspace's chat**, ask for a presentation of what is there ("make a deck about how sign-in works here"): the agent makes the deck, reads the code, and writes the slides. See [From a workspace](#from-a-workspace).
- A PowerPoint file (`.pptx`, `.potx`) in Files has **Open as Slides**: a deck is made from it and opened. The file itself is not changed.

The feature is the brand's `presentations` key and needs `artifacts`.

## The window

The chat is on the left (its width is kept; at least 340 points), the deck on the right. The deck is a web page shown in the system's web view (WKWebView on macOS, WebView2 on Windows) with no browser controls, so it reads as part of the window; its colours and sizes follow the app's theme and interface size. On Linux there is no web view to put in the window: the deck opens in the user's browser and the window keeps the chat.

The system draws a web view over everything the app draws, and gives it the keyboard when it is clicked. The window deals with both:

- **Keys.** A click in the app's part of the window takes the keyboard back from the web view (`WebView::release_keys`), so what is typed in the chat never acts on the deck. When the page takes the keyboard it says so, and the app's focus moves to the window itself, so ⌘Z, ⌘V or ⌘A meant for the deck never act on the chat's box. On macOS the *Edit* menu (Cut, Copy, Paste, Select All) is what sends those commands to a web view.
- **What the app draws over the deck.** While a dialog, a sheet, a note at the window's edge, or a menu or pop-up of the chat is open, the web view steps aside and a picture of it stands in its place (`DeckView::cover`), so the deck seems to stay and the app's thing shows over it. A tooltip of a button at the chat's right edge can still be cut by the deck.

## From a workspace

A deck about a project is made where the project is: in the workspace's chat, whose agent has the code. The deck then *belongs* to that workspace (`artifact.json`: `source`), and the workspace's chats are the deck's.

1. The user asks in the workspace's chat. The agent calls `deck_create` (a title, a look), which makes the deck with the artifacts (not in the workspace) and says where its files are.
2. The deck shows beside the chat at once, in the result pane's browser: the slides alone (the deck's page with `?preview`), the one written last in front, updating as the agent writes. Nothing else opens by itself.
3. **Open in Slides**, on that preview, opens the deck's window: the deck on the right, and on the left *the same conversation*, since the chat of a workspace's deck is the workspace's (same agent, same folders, same history; the two windows show one chat). There the user changes the deck by hand, comments, presents and exports, and the agent still knows the code.
4. Later, from Artifacts, the deck opens in that window with the workspace's chat it was last worked on in (`slides.chat.<deck>` in the settings).

Details:

- **Which workspace** a deck works from is said at the right of the deck window's title bar, and changed there: any workspace, or *No workspace: this deck only* (the deck's own chat, working in the deck's folder). A deck made under Artifacts can be given a workspace this way. When a deck's workspace is gone, the deck opens with its own chat.
- **Permissions.** Making a deck (`deck_create`) and exporting one ask as the chat's mode says. In a workspace that has a deck, its chats read and change the deck's files and use `deck_guide`, `deck_look`, `deck_comments`, `deck_import` and `deck_list` without being asked, as a deck's own chat does (`ph_agent::own_files`, kept up by the window shell as decks are made, renamed, given away and deleted).
- **Several decks.** The deck tools work on the workspace's deck changed last, unless `deck` names one (its id or title); `deck_list` shows them. Messages sent from a deck's window name that deck.
- **Without the deck's window.** What does not fit, pictures of slides and exports need the deck drawn somewhere. A deck whose window is not open is drawn out of sight in another window of the app (`Decks::drawer`: a web view beyond the window's edge, kept for the two decks drawn last), so `deck_look` and `deck_export` work from the workspace's chat before the user has opened anything. Its picture in the Artifacts list is taken there too.
- **What the user changed by hand** since the agent last looked is said by `deck_look` (and with each message sent from the deck's window).
- In a build without the built-in browser, the deck's window opens in place of the preview.

## The deck's files

```
<artifact>/
  artifact.json        kind: "slides"
  site/                what is served
    deck.json          title, size, transition, the slides in order
    deck.css           the deck's look: variables and rules for every slide
    slides/<name>.html one slide each: a <section class="slide">
    media/             pictures and videos, named by what they hold
  references/          files to work from (never served)
  exports/             what Export writes
  comments.json        comments (never served as a file)
```

`deck.json` lists each slide's file with its speaker notes, whether it is skipped, its section and transition; fields a build does not know are kept. A slide is 1920 × 1080 unless the deck says otherwise.

The files are the truth. The agent edits them with its own tools; a watcher sees the change (140 ms after the last write) and every open page draws it. A change made in the page is sent with the marks of the files it was made on, and is refused if one changed meanwhile (the page reads again and tries once more). Every change is one step of undo, whoever made it: typing within 2 seconds is one step, an agent's burst of writes within 6 seconds is one step.

## Changing a deck by hand

- **Insert**: text (`T`), a picture, a video, a table, a chart, shapes. A picture or video can be dropped or pasted in; a chart keeps its numbers (`data-chart`) and is changed in a small sheet.
- **Select, move, resize**: with snapping and guides; arrows nudge (Shift: ten at a time); pictures crop; ⌘D duplicates; ⌘C, ⌘X and ⌘V go through the system's clipboard, so a thing copied from one deck pastes into another.
- **Type in place**: double click or Enter. The bar over the selection has the text style, colour, bold, italic, underline, link, lists and alignment; the panel on the right has text, layout, appearance, and the deck's and slide's own settings.
- **Slides**: the strip under the stage, or the grid (with sections and notes); add one from ten layouts, duplicate, move, skip, delete; speaker notes under the slide.
- **Looks**: six to start from (Paper, Midnight, Clean, Forest, Bold, Slate), chosen in an empty deck or the panel; `deck.css` is the deck's own and can say anything.
- **Zoom**: `+`, `−` and `0` (fit), or the menu.

## Comments

A comment is on an element or on a slide: a pin on the stage, a count on the slide's tile, a thread with replies, resolved when done. **Send to the agent** puts the comment in the chat as a message that names the slide's file and the element; the agent makes the change and answers in the thread (`deck_comments`).

## Presenting and sharing

- **Present** fills the screen with the slides. **Present with notes** keeps the presenter's view in the window (the slide, the next one, the notes, a clock) and opens the slides for the audience in a second window, on another screen when there is one.
- A deck is shared like any artifact (*Share on your network*): people open its link in their browsers and see the slides, and follow the presenter while the deck is presented.

## Export

To the deck's `exports/` folder, shown in its folder when written:

| | |
|---|---|
| PDF | One page a slide, text as text, as the page draws it |
| Web page | One `.html` file with the slides, the look, the pictures and the fonts in it |
| PNG | The slide on the stage |
| PowerPoint, these fonts | Real text boxes, shapes, pictures, tables and charts (with their numbers), notes and transitions, each where the page draws it |
| PowerPoint, basic fonts | The same, in Arial, Georgia and Courier New, for computers without the deck's fonts |

A PDF, a picture and a PowerPoint file are made from the deck's page, in a second web view that is never shown, so the page the user works in is not disturbed.

## PowerPoint files in

**Start from a PowerPoint file** in an empty deck, a `.pptx` dropped on the page, *Open as Slides* in Files, or the agent's `deck_import`: the file's slides become slides of the deck, each element where it was (`section.slide.imported`), with the file's colours and fonts, its pictures (in `media/`), tables, charts with their numbers, and notes. An empty deck takes the file's size and look. The file is read within limits (5,000 parts, 200 MB a part, 1 GB in all, 500 slides).

## The agent

Each message in a deck's chat tells the agent where the deck's files are, what is selected on the stage, and what the user changed by hand since its last turn. It changes the deck by editing the files, which it may do in `site/` without being asked (`ChatContext::own_files`), and has these tools (in a workspace's chat they work on that workspace's decks):

| Tool | |
|---|---|
| `deck_create` | In a workspace's chat: makes a deck for the workspace (asks first) |
| `deck_list` | The workspace's decks |
| `deck_guide` | How a deck is written: the files, the rules, charts and shapes, what good looks like |
| `deck_look` | The deck as drawn: each slide's title, what does not fit or is hard to read, the selection, open comments, what the user changed by hand; with `render`, pictures of up to 8 slides |
| `deck_comments` | Reads the comments; replies to one and resolves it |
| `deck_import` | Makes slides from a PowerPoint file |
| `deck_export` | Writes a PDF, web page, picture or PowerPoint file (asks first) |

## Not there

- On Linux the deck is in the browser, not in the window; exports that need the page (PDF, PNG, PowerPoint) need macOS or Windows.
- Presentations made with the earlier editor (folders in `Documents/<name> Presentations`) are not opened. A `.pptx` exported from one opens as Slides.

## Working on it

The page is plain modules in `crates/slides/src/page/`, built into the app; a debug build reads them from the folder named by `PILOTHOUSE_SLIDES_PAGE`, so a change shows on reload. A development build (`automation`) has three methods for tests of the deck's window: `deck.screen` (a picture of the page as the window shows it), `deck.script` (a script in it) and `deck.keys` (who has the keyboard; a key pressed as the system presses it, on macOS). See [testing.md](../testing.md).
