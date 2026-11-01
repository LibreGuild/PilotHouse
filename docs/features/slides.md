# Slides

Crate: `ph-studio` (`slides/`). Window: `ph_workspace::studio_window` (`open_presentation`). Dialogs: `ph_workspace::presentations`.

Slides is Studio's presentation editor: decks made by hand or by describing them to an agent in the chat beside them, in a model of PowerPoint's own kind, drawn by PilotHouse and exported as editable PPTX, PDF or pictures.

## Finding it

The **More** button (⋯) right of the search in the title bar has **New presentation…**, **Open presentation…** and the five latest presentations, then the tools only the palette reached before (Utilities, Architecture map, Parallel runs, Schedules, Usage, Network drives, Agent instructions, Search chats) and Help (Keyboard shortcuts, Settings). The command palette has New and Open presentation too.

- **New presentation**: a title, a size (16:9, 16:10, 4:3, A4 and US Letter either way, 1:1, 4:5, 9:16) and a theme, picked from a title slide drawn in each (the built-in ones and the user's own). An optional brief is sent to the agent when the window opens, to draft the deck.
- **Open presentation**: every presentation, newest first, with a picture of its first slide, searchable. **Open a PowerPoint file…** opens any `.pptx` or `.potx` (see [PowerPoint files](#powerpoint-files)). In the Files tree, a PowerPoint file's menu has **Open in Studio Slides**.

## Spaces

Each presentation is a folder in `Documents/PilotHouse Presentations/` (the home folder without a Documents folder; `PILOTHOUSE_PRESENTATIONS` overrides it, for scratch profiles and tests):

| | |
|---|---|
| `deck.json` | The deck |
| `media/` | Its pictures, named by what they hold (the same picture added twice is kept once); pictures over 3,840 pixels a side are made smaller when added, and a copy at most 960 pixels a side (`media/.small/`) draws the small pictures quickly |
| `references/` | Files to work from (the References panel), for the agent and for slides |
| `source.pptx` | The PowerPoint file the deck was opened from, when it was (its masters and layouts are written back from it) |
| `exports/` | What Export writes |
| `.versions/` | The deck as it was, kept every five minutes while it changes (the last 20) |
| `.thumbnail.png` | Its first slide, for the Open dialog |

The deck saves itself 0.7 seconds after the last change, written beside the file and moved over it, and once more when the window closes. Themes the user saves are in `Themes/*.json` there.

The space is also a project in the store (`ProjectKind::Presentation`, found or made by `presentation_space`): its chats, agents, checkpoints and usage stay with it, and it is not shown as a workspace. The agent works in the presentation's folder.

## The window

```
┌ title: Studio · title                  ↶ ↷ ⋯ ▶ Present ▾  [Export ▾] [Agent] ┐
│ + Slide ▾ │ Text Shape▾ Picture Table▾ Chart▾ Icon Code SVG │ Layout Theme Size −%+ 💬 📁 ✎│
├──────────┬─────────────────────────────────────────┬─────────────┬──────────┤
│ 1 ▭      │                                         │ Comments or │ chat     │
│ 2 ▭      │                the slide                │ References  │ (toggle) │
│ 3 ▭      │                                         │ (toggle)    │          │
│          ├─────────────────────────────────────────┤             │          │
│ + New ▾  │ the bar: what the selection takes       │             │          │
│          │ speaker notes                           │             │          │
└──────────┴─────────────────────────────────────────┴─────────────┴──────────┘
```

- **The rail**: the slides, a picture each (made only for those in view and near it). Click to go to one; drag to move it; right-click for a new slide after it, duplicate, hide in presenting, start a section (named, shown on the slide that starts it), present from here, move up or down, delete.
- **The toolbar**: a new slide in a layout, a text box (typed in at once), shapes (lines and arrows, rectangles, ovals, arrows, stars, callouts and more), pictures (from files, or pasted), tables, charts (column, stacked column, bar, line, area, pie, doughnut, scatter; the data dialog opens), icons (a searchable library of about 230), code (coloured as in an editor), and SVG drawings (pasted or from a file). Then the slide's layout, the theme (built in, the user's, colours and fonts, save this theme), the size (or a custom one in inches, centimetres, pixels or points; what is on the slides is scaled), the zoom and the notes. A narrow window (the chat open) shows icons only, their names in tooltips.
- **The slide**: click to select (⇧ or ⌘ to add), drag in an empty place to select what is inside the box. Drag to move; ⇧ keeps to one axis; edges and middles snap to the slide's and to other elements' (guides show), ⌘ while dragging stops snapping. The eight handles resize (⇧ keeps proportions; pictures and icons keep them from a corner unless ⇧; ⌥ from the middle); the round handle above turns it (in 15° steps with ⇧, a full turn snapping back to none). Turned elements are picked, resized and snapped by their own outline.
- **Typing in place**: double-click (or Enter) a text box, shape or code. The text shows as Markdown (`**bold**`, `*italic*`, `~~struck~~`, `` `code` ``, `[link](url)`, `- ` bullets, `1. ` numbers, two spaces more a level) in the element's own size, colour and font; ⌘B, ⌘I, ⇧⌘X and ⌘E mark the selected words. Escape or a click elsewhere ends it. A changed line becomes what was typed, keeping its colour, size and font; lines left as they were keep every style. An empty new text box goes away. Double-click a table or a chart for its cells or data, an icon to change it, a picture to replace it, an SVG drawing for its source.
- **Lines and connectors**: a selected line shows its two ends, not a box. Drag an end onto the side of another element (its middles show as dots while dragging) and the end is fixed there, shown filled; the line then follows that element wherever it moves or resizes (`connectors.rs`, on every change). Dropping an end elsewhere, or moving the line on its own, lets go.
- **Cropping**: a picture's **Crop** turns its handles into crop marks; dragging a side or corner cuts the picture (or uncovers it again, up to all of it) while the picture stays where it is. Esc, Done cropping or a click elsewhere ends it. A picture that filled or fitted its frame is cropped to what it showed, then stretched to the frame from there.
- **The bar** shows what the selection takes. Text: font (the theme's heading, body or code font, or a family the system has), size, bold, italic, underline, strike, colour, alignment, bullets, numbers, indent, line spacing, where the text sits in its box, and what happens when there is too much (shrinks to fit, the box grows, runs over). Shapes: fill, outline colour, width and dashes, arrowheads on lines, shadow. Pictures: crop, fill or fit the frame, corners, shadow, replace. Tables: edit cells, header row, banded rows, header colour, text size. Charts: edit data, kind, legend, values. Code: language, dark, line numbers, size. Icons: colour, weight, change. Several: line them up, space them out, group. Each: how it comes in when presenting (**Animate**: appear, fade, fly in, zoom or wipe; on a click, with the one before or after it; from which side; how long; earlier or later in the slide's order), order (front, back), on the slide (centred, left …), opacity, duplicate, delete. Nothing selected: the slide's background, its transition (fade, push, wipe, cover, zoom or dissolve, from which side, how long) and hiding it in presenting.
- **Comments** (💬, with the number open on this slide): the slide's comments, each with its replies; Reply, Resolve (resolved ones go below, dimmed, and can be reopened), Delete. A comment written with one element selected is about that element; its numbered marker sits on the element's corner and follows it. Clicking a marker opens the panel to reply to it. The agent reads comments with the slides and can answer and resolve them.
- **References** (📁): the files in the space's `references/` folder, with their pages or slides counted. **Add files…** or dropping files copies them in. Each one's menu puts it on the slide: a picture as itself; a PDF page or a PowerPoint slide as a picture (any of the first 40); Markdown or text as a text box; a CSV or TSV as a table, or as a column chart (the first column names the categories, each column of numbers a series). **Ask the agent to use it** starts a message about it in the chat; Open and Show in Finder too.
- **Find and replace** (⌘F): text in every slide's boxes, shapes and tables; how many times it is found as it is typed; **Find next** goes to the slide and selects the element; **Replace all** is one step to undo.
- **Colours** are offered as the theme's (text, background, accents) with lighter and darker steps of each, then standard colours. A colour picked from the theme stays the theme's: another theme recolours it.
- **Speaker notes** under the bar, per slide.
- **Keys**: ⌘Z, ⇧⌘Z; ⌘F find and replace; Delete; ⌘D duplicates (the slide when nothing is selected); ⌘C, ⌘X, ⌘V (elements, between decks too; text pastes as a text box, a picture as a picture); ⌘G and ⇧⌘G group and ungroup; ⌘A; arrows nudge a point (⇧ ten), or with nothing selected go through the slides; Page Up and Down; ⌘M a new slide; ⌘] ⌘[ forward and backward, ⇧ to the front and back; ⌘↩ presents from this slide, ⇧⌘↩ or F5 from the start.
- **Title bar**: undo and redo; ⋯: rename (the window's title and the chat's heading follow), save the theme for other decks, earlier versions (go back to one, which can be undone), show in Finder; **Present ▾** (from this slide, from the start, rehearse); **Export**.

## Model

`model.rs`: lengths are points (16:9 is 960 × 540; the PPTX writes EMU, 12,700 a point).

- **Deck**: title, size, theme, slides, sections. Slides are shared (`Arc`), so a copy of the deck (an undo step, a picture drawn off the UI thread) shares the slides it did not change.
- **Slide**: its layout, background, elements, notes, hidden, transition (kind, the side it comes from, seconds), animations (in order: the element, effect, side, start, seconds, delay) and comments (author, text, where, the element, resolved, replies).
- **Element**: an id that stays its own (`e12`), a name, a frame (x, y, w, h, rotation, flips), the placeholder it fills, opacity, and its kind: shape (24 outlines or a path, fill, line with arrowheads, shadow, text; a line may be fixed to elements' sides, and a PowerPoint outline name kept), text box, picture (crop, fit, corners), table (cells with fills and spans, header, banded rows), chart, code, icon, SVG, group, or a PowerPoint object kept as it was (a video, an equation, ink …: its XML and files, drawn from its picture, moved and resized only).
- **Text**: paragraphs (alignment, level, bullet or number, spacing) of runs (font, size, bold, italic, underline, strike, colour, highlight, link, baseline). Unset properties come from the placeholder, then the theme. Text given as a string is read as Markdown.
- **Colours**: a theme slot (`accent1`, `accent1+40` lighter, `accent1-25` darker) or `#RRGGBB[AA]`.
- **Theme**: twelve colours, heading, body and code fonts, a background, decorations on every slide (a logo, a bar), title and body styles, and twelve layouts (title, title and content, section, two content, comparison, title only, statement, quote, picture and caption, agenda, closing, blank), made for the deck's size (portrait sizes get layouts of their own). Twelve built in (`theme.rs`): Clean, Harbor, Midnight, Slate, Paper, Bold, Corporate, Sunset, Forest, Mono, Pastel, Ocean.
- **Operations** (`ops.rs`): every change, for the editor and for agents alike: add, duplicate, remove, move and update slides; add, update (a JSON merge), remove and order elements; set and style text; replace text; group, ungroup, align, distribute; animate and unanimate; add, reply to, resolve and remove comments; set the theme, a saved theme, its colours and fonts; set the size; sections; the title. A list applies as one step, all or none. SVG that does not parse and unknown icons are refused.

## Drawing

One renderer, so the editor, the rail, presenting, PNG, PDF and what the agent sees always match: a slide becomes SVG (`svg.rs`), which resvg draws (`raster.rs`) or svg2pdf writes as a PDF page (`pdf.rs`). Text is laid out by cosmic-text with the system's fonts (`text.rs`: wrapping, alignment, bullets and numbers, spacing, shrinking to fit) and drawn as SVG text at the widths laid out. A family missing here is drawn in a look-alike (Liberation, DejaVu …; `fonts.rs`), and its weight is the nearest the family has, so layout and drawing use the same face. Charts (`charts.rs`), code highlighted by syntect (`code.rs`), shapes (`shapes.rs`) and icons (`icons.rs`, Lucide) are drawn the same way.

Pictures of slides are made off the UI thread (`document.rs`), the one being edited first, then those in the rail in view, a few at a time. Each is kept until its slide or the theme changes; a change shows the last picture until the new one is ready. Nothing is drawn while nothing changes.

## Presenting

**Present** (⌘↩ from this slide; ⇧⌘↩ or F5 from the start) shows the slides full screen, hidden ones left out, on another screen when there is one. →, ↓, Space, Page Down, Enter, N or a click go on; ←, ↑, Page Up, Backspace, P or a right-click go back; Home and End; B or . blanks the screen; Esc ends.

- **Transitions**: fade, push, wipe, cover, zoom and dissolve (drawn as a fade), from the side set, taking the time set.
- **Animations**: going on plays the slide's next build (the elements that come in on one click, with those set to go with or after them) before the next slide; a first build not on a click plays by itself once the slide is in. Each element comes in its own way: appear, fade, fly in from a side, zoom, wipe from a side. A click during a build finishes it; going back undoes a build, then shows the slide before as it ended. Each element coming in is drawn alone (`svg::element_layer`), over the slide drawn without the elements still to come; nothing is drawn between builds.
- **The presenter's view**: with a second screen, this one shows the slide (as it will be after the build playing), the next, the notes, the time, the slide and click it is at, and Back, Next and End. **Rehearse in the presenter's view** (Present ▾) opens that view in a window on this screen, to go through the talk with its notes and builds (transitions and animations are not played there).

The slide shown (its builds either side) and the next are drawn at the screen's size ahead, so going on is instant; the others are let go, and each window frees the pictures it drew.

## Export

**Export ▾**: for a deck opened from a `.pptx`, **Save to *name*.pptx** first (it asks before writing over that file); then PowerPoint (.pptx), PDF, PDF with speaker notes, pictures of every slide (PNG), a picture of this slide; **Export as…** asks where; **More options…** chooses the slides (all, this one, or `1-3, 5`), notes pages, and the pictures' width (1280, 1920, 3840). Files go to the presentation's `exports/` folder, named after its title, which opens when they are written.

- **PPTX** (`pptx.rs`, own writer over `zip`): the theme's colours and fonts as a PowerPoint theme; its layouts as slide layouts (PowerPoint's New Slide offers them) with their placeholders; shapes as PowerPoint shapes (preset outlines, or paths as custom geometry) with real, editable text (sizes, styles, bullets, numbers, spacing, line breaks, the shrink worked out here, links); lines fixed to elements as PowerPoint connectors; pictures with their crop (fit by cropping or by the frame); tables with fills, borders and merged cells; charts as native charts with their data and an embedded workbook (`xlsx.rs`), so PowerPoint's Edit Data works; code as coloured text; icons and SVG as SVG pictures with a PNG beside for older readers; groups; notes; sections; hidden slides; transitions; animations as PowerPoint's timing (entrance effects, on click, with or after the previous); comments with their replies (as PowerPoint 2013's threaded comments; a resolved one is marked "[Resolved]"); PowerPoint objects kept as they were. Theme colours are written as theme colours, so another theme in PowerPoint recolours them. The deck's own JSON goes in as `pilothouse/deck.json`, with a fingerprint of the slides as written, so it opens here again exactly; a file changed since in another program is read from its slides instead.
- **PDF**: a page a slide (with notes, a notes page after each), vector, text kept as text with its fonts in the file.
- **PNG**: one picture, or a folder of them for several slides.

## PowerPoint files

`pptx_read.rs` reads any PowerPoint file into a presentation space of its own (best effort for files made elsewhere; a deck made here comes back exactly). Opening the same file again, unchanged since, opens that presentation again; a file changed since is read anew.

- **Read**: the slide size; the theme (colours, fonts, the colour map); masters and layouts (backgrounds, decorations, placeholders matched to ours with PowerPoint's type and index kept); slides with backgrounds, shapes (outlines matched to ours, the preset kept), text boxes and placeholders (styles from the layout, master and theme, worked into the text), pictures with crops, tables, groups, connectors (fixed to their shapes), charts (from their cached data), SmartArt (from its drawing), sections, notes, hidden slides, transitions, entrance animations and comments (legacy, threaded and modern, with replies and resolution). What has no counterpart here (video, audio, equations, ink, OLE objects …) is kept as a PowerPoint object: drawn from its picture and written back as it was. What could not be read is listed when the file opens.
- **Written back**: while the theme is the one read, an export reuses the file's own masters, layouts and theme verbatim (`source.pptx`), so a corporate template keeps its look exactly; a theme changed here is written as ours. **Save to *name*.pptx** writes over the original (after asking), and opening that file again opens this presentation.

## With an agent

The chat beside the deck is the presentation's own. Its starters: draft the deck, tighten the words, make it look better, an executive summary. Each message tells the agent which deck is open, its folder and `references/`, and what the user has selected (the slide, the elements, the words selected while typing), so "make this bigger" or "rewrite this paragraph" work. In this chat the tools that look at and change the deck run without asking; exporting asks.

| Tool | Does |
|---|---|
| `slides_get` | The deck in outline (size, theme, every slide's elements with ids, frames and text, the selection) and a guide to the model and every operation; `slide` shows one in full; `render` attaches pictures of slides |
| `slides_edit` | `ops`: operations applied in order as one step the user can undo (all or none); `import` copies pictures into the deck's media first |
| `slides_undo` | Undo, or redo |
| `slides_themes` | The built-in themes with their colours and fonts, the saved ones, the current layouts and their placeholders' places, the sizes, and icon names |
| `slides_export` | PPTX, PDF (notes) or PNG into `exports/` (or a path), never over a file unless asked |

They are the `slides.*` control methods (`control.rs`).

## Code map

| File | What |
|---|---|
| `slides/model.rs` | The deck: slides, elements, text, colours, fills, themes and layouts |
| `slides/ops.rs` | Every change, for the editor and for agents |
| `slides/theme.rs` | The built-in themes and their layouts, for each size |
| `slides/markdown.rs` | Markdown to slide text and back |
| `slides/text.rs`, `slides/fonts.rs` | Styles resolved, text laid out; the system's fonts and look-alikes |
| `slides/svg.rs`, `slides/raster.rs`, `slides/charts.rs`, `slides/code.rs`, `slides/shapes.rs`, `slides/icons.rs` | Drawing a slide |
| `slides/pptx.rs`, `slides/xlsx.rs`, `slides/pdf.rs` | Export; charts' workbooks |
| `slides/pptx_read.rs` | Reading any PowerPoint file |
| `slides/connectors.rs` | Lines whose ends follow elements |
| `slides/space.rs` | A presentation's folder: saving, versions, media, the list, saved themes, PowerPoint files opened (and opened again) |
| `slides/document.rs` | One deck being edited: history, saving, pictures of slides, export |
| `slides/editor/` | The window's content: `mod.rs` (commands, keys, title bar), `canvas.rs` (the slide, selecting, moving, typing), `geometry.rs` (hits, handles, resizing, snapping), `rail.rs`, `toolbar.rs`, `bar.rs`, `dialogs.rs` (also find and replace), `side.rs` (comments and references), `present.rs` (presenting, builds, transitions, rehearsing) |
| `slides/control.rs` | `slides.*` for agents |
| `workspace/presentations.rs`, `workspace/title_bar.rs` | New and Open presentation; the More menu |

## Not yet

Exit and emphasis animations and motion paths (read as entrances, or left out), PowerPoint's modern comments written as such (they are written as threaded legacy comments, which PowerPoint reads), transitions beyond the six (each is read as the nearest), editable slides pulled in from a reference deck (they come in as pictures; the agent can rebuild them), and the presenter's view showing animations as they play.
