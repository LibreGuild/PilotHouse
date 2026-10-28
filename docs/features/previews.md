# Previews

Crate: `ph-preview`. A file with more to show than its lines opens with a viewer; text kinds can still be edited. PilotHouse draws everything itself: no web view, and no codec or browser engine ships with it.

## Text kinds

A text file of one of these kinds has a switch in its toolbar between its **Source** (the editor), its **Preview** and **Split** (both, side by side). The view picked last for a kind is used for the next file of that kind, for the rest of the run (`FileView::set_view`). The preview follows typing a moment after it pauses.

| Kind | Files | Opens as | The preview |
|---|---|---|---|
| Markdown | `.md`, `.markdown`, `.mdx` | Rich | GitHub-flavored Markdown with Mermaid diagrams drawn in place, images (beside the document, or on the web over HTTPS), tables, tasks; links to files open them in the editor, web and mail links in the browser, and other links nothing (see the rich editor below) |
| Mermaid | `.mmd`, `.mermaid` | Split | The diagram, zoomable (`mermaid-rs-renderer`, pure Rust, 23 diagram types) |
| SVG | `.svg` | Split | The drawing, zoomable |
| JSON | `.json`, `.jsonc`, `.json5`, `.geojson`… | Source | A tree: fold levels, pick a value to see its path (`$.a.b[0]`), copy the path or the value. **Format** and **Minify** reflow strict JSON without touching keys' order or numbers' spelling |
| JSON Lines | `.jsonl`, `.ndjson` | Source | One tree row per line; a bad line says why |
| YAML, TOML | `.yaml`, `.yml`, `.toml` | Source | The same tree |
| Tables | `.csv`, `.tsv` | Preview | A grid with a sticky header, row numbers, sorting by a column (numbers as numbers) and a filter; a table over 5,000 rows is filtered and sorted in the background |
| `.env` | `.env`, `.env.*`, `*.env` | Preview | Names and values, the values hidden until shown (a screenshot or a shared screen does not show secrets), copy per value |
| Logs | `.log` | Preview | Terminal colours instead of escape codes (16, 256 and true colour), and *Follow the end* |
| Patches | `.diff`, `.patch` | Source | Each file's changes, tinted (shared with the Git tab, `ph_preview::diff`) |
| Notebooks | `.ipynb` | Preview | Markdown cells rendered, code highlighted, saved outputs: text, images, errors |
| HTML | `.html`, `.htm` | Source | **Open in Browser** shows it in the workspace's browser |

## Shown only

Opened with their viewer; **Open in Default App** in the toolbar hands the file to the system.

- **Images** (PNG, JPEG, GIF with animation, WebP, BMP, ICO, TIFF, PNM): fitted to the pane, zoom with the buttons or ⌘/Ctrl and the wheel, actual size; size, format and file size underneath (`image_view.rs`). Images over 256 MB, 16,384 pixels a side or 100 megapixels say so instead (Open in Default App shows them).
- **Audio** (MP3, WAV, FLAC, Ogg Vorbis, AAC/M4A): a waveform to click and seek in, play and pause, the time, speed (`audio.rs`). Decoded by `rodio`/Symphonia in the app; the waveform keeps only the loudest sample of each stretch, so an hour-long recording takes kilobytes, not gigabytes. It plays through Core Audio or WASAPI on macOS and Windows, and on Linux through the GStreamer player the video view uses, so PilotHouse needs no ALSA library to start.
- **Video** (MP4, MOV, M4V; WebM, MKV and others where the system decodes them): play and pause (Space, K), a scrubber to click or drag, ±5 s (←/→, J/L), speed, mute (M). The system's own media framework decodes and plays, sound included, and PilotHouse draws the frames with its own controls (`video/`):
  - macOS: AVFoundation (`AVPlayer` with an `AVPlayerItemVideoOutput`); frames go to the GPU as they are (`gpui::surface`), with no copy.
  - Windows: the Media Foundation media engine; each new frame is copied into a WIC bitmap.
  - Linux: GStreamer's `playbin` with an `appsink`, loaded with `libloading` when the first video opens (not linked), so the app starts without it. Without it, or without a codec, the view says what to install (`gstreamer1.0-plugins-good`, `gstreamer1.0-libav`).
  A tab that goes to the back pauses.
- **PDF**: every page in a column, drawn as it scrolls into view at the screen's pixel density, fit to width or zoomed, the page number underneath (`pdf.rs`, `hayro`, pure Rust). The file is parsed once and pages are drawn from it in the background, at most 8,192 pixels a side. At most a dozen drawn pages, and 512 MB of them, stay; the textures of the rest are released. Text cannot be selected; encrypted files cannot be opened (the view says so).
- **SQLite** (`.db`, `.sqlite`, `.sqlite3`…): opened read-only; tables and views down the side, the first 5,000 rows of the one picked in the table grid (`sqlite.rs`). A view that takes over 10 seconds is stopped, and the database's own functions are not trusted (`trusted_schema` off).
- **Spreadsheets** (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods`): a tab per sheet, cells under the sheet's column letters, the first 20,000 rows and 1,000 columns (`spreadsheet.rs`, `calamine`). Excel files are read cell by cell, stopping at the last row shown.
- **Archives** (ZIP and its kin: JAR, APK, wheel, VSIX…; `.tar`, `.tar.gz`, `.tgz`, `.gz`): what is inside, with sizes, as a table. Nothing is extracted (`archive.rs`); a compressed tar is read at most 2 GB unpacked, and a single `.gz` shows the size its last bytes record.
- **Fonts** (`.ttf`, `.otf`, `.ttc`): the family at six sizes, its characters, and what it says about itself (`font.rs`). A family the system already has is shown in the installed font; the file is added to the app's fonts only otherwise.
- **Anything else binary**: a hex view, sixteen bytes a row, and what the first bytes say it is (ELF, Mach-O, PE, ZIP, SQLite…; `hex.rs`). Files over 32 MB show their start.

## The rich Markdown editor

Markdown opens in **Rich**: the document as formatted text, edited a block at a time, like Notion (`rich/`). Every block shows formatted until clicked; the block being edited shows its text in the block's own look (a heading stays large, code stays code), and a Mermaid block draws its diagram under the text as it is typed.

- **Enter** starts a new block (the next item, in a list; an empty item leaves the list); **Shift+Enter** a new line in the block. **Backspace** at the start turns a heading, item or quote back into text, or joins a paragraph to the one above. **↑/↓** at the edges move between blocks. **Esc** stops editing.
- **Markdown shortcuts** at the start of a paragraph make their block: `# `, `## `, `### `, `- `, `1. `, `[] `, `> `, ```` ``` ```` and `---`.
- **`/`** opens a menu of blocks (text, headings, lists, to-dos, quote, code, Mermaid diagram, table, divider), filtered as you type; ↑/↓ and Enter pick. The **+** beside a block adds one below with the menu open.
- **Tab** and **Shift+Tab** move a list item in and out. Tick boxes tick with a click (or ⌘Enter while editing the item).
- **⌘B**, **⌘I**, **⌘E** and **⌘K** wrap the selection in bold, italic, code or a link; the toolbar does the same, and turns the block into another kind.
- **Drag** a block by its handle to move it.
- **Paste an image** (⌘V) to save it beside the document, in `images/`, and show it in a block below.
- **Undo** (⌘Z) undoes typing in the block first, then changes to the blocks themselves (⌘⇧Z redoes).

The Markdown text stays the truth (`rich/blocks.rs`): a block keeps the exact text it was read from until it is edited, and the whitespace between blocks is kept too, so editing one paragraph changes only that paragraph in the file. Tables, HTML, front matter and other Markdown without a block of its own are edited as Markdown, in place. Edits reach the file's text a quarter of a second after typing pauses, and at once on ⌘S.

In the **Source** view, Enter at the end of a list item starts the next one (`- `, `3. `, `- [ ] `), and in an empty one ends the list.

## Size

Video uses the system's frameworks, so nothing is bundled for it. The viewers add about 6–9 MB to the binary (the PDF renderer and the audio decoders are the largest parts).
