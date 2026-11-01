# Studio

Crate: `ph-studio`. Window: `ph_workspace::studio_window`.

Studio is a window of its own for editing a picture, a video or a presentation, by hand or by describing the edit to an agent in the chat beside it. Presentations are in [slides.md](slides.md).

**Edit** on an image or a video (in the editor's bar, or **Edit in Studio** in the Files tree's menu) opens it. Opening the same file again brings its window forward. A file opened from outside every workspace opens in Studio without a chat.

## The window

```
┌ title: Studio · name ●                         ↶ ↷ 👁 ⋯ [Export…] [Agent] ┐
│ tools │ the picture                                            │ chat      │
│       │                                                        │ (toggle)  │
│       ├────────────────────────────────────────────────────────┤           │
│       │ the bar: only what the tool (or the mark selected) needs│           │
│       │ status: original size, format, weight → result · pixel  │           │
└───────────────────────────────────────────────────────────────────────────┘
```

- **Tools** (left): Select (V); Crop and turn (C), Adjust colors, Resize, Frame; Arrow (A), Rectangle (R), Ellipse (O), Line (L), Pen (P), Text (T), Numbered step (N), Highlight (H); Blur (B), Pixelate (X), Spotlight (S). Letters work while the picture has the keyboard.
- **The bar** under the picture shows one thing at a time: the selected mark's color, line width, text, size, box or fill; the crop's proportions (Free, 1:1, 4:3, 3:2, 16:9, 9:16) and turn and flip; the color sliders; width and height (proportions locked or free, 25/50/75%); the frame's background, padding, corners and shadow. With nothing to show it gives a hint.
- **Marks**: drag to draw; click to place text (then type in the bar; left, centered or right lines) or numbered steps (1, 2, 3…). With Select, click a mark to change it, drag it to move it, drag its corners or ends to resize it; Delete removes it, Esc lets go. A text left empty goes when let go of.
- **Title bar**: undo and redo (⌘Z, ⇧⌘Z), Compare (hold to see the original), ⋯ (copy the picture, make app icons, start over, replace the original), **Export…** (⌘S), and **Agent**, which slides the chat in or out. Drag the chat's left edge to make it wider or narrower (the picture keeps at least 420 pixels); both are remembered for the next window while the app runs.
- **Status line**: the original's size, format and weight, the result's size, and under the pointer its pixel position and color.
- **Zoom**: pinch with two fingers on a trackpad, or ⌘ (Ctrl) and the wheel, zooms around the pointer; with two fingers (or the wheel) the picture moves when zoomed in, never all the way out of sight. The buttons at the bottom right zoom, show actual size, and fit. When the canvas changes size (the window, the chat) a fitted picture fits again at once.
- Closing with changes not saved asks first. The file itself is never changed until it is saved over (Replace the original, which asks).

## Edits are descriptions

An edit (`edit.rs`) never touches pixels: it says how to turn the original into the result, so any step can be changed or undone, by hand or by an agent, and the result is made again from the original. In order:

1. **Turn**: quarter turns and flips. Turning keeps the crop and the marks on what they were on.
2. **Crop**, in pixels of the turned original.
3. **Adjust**: brightness, contrast, saturation, grayscale, invert, blur, sharpen.
4. **Hide**: blur and pixelate marks, at the picture's own pixels.
5. **Resize** the result (one side keeps proportions).
6. **Marks** drawn at the result's size, so lines and text stay sharp: rectangles, ellipses, arrows, lines, pen, text (optionally on a box), numbered steps, highlights and spotlights, with a soft shadow so they read on anything.
7. **Frame**: space around the picture, a solid or gradient background (or none), round corners and a shadow.

Positions (the crop, the marks) are pixels of the turned original, whatever the crop, size or frame. Photos are turned as their camera recorded (EXIF orientation) when read.

`render.rs` makes the result. The same steps make the preview, from a copy of the original at most 2,048 pixels a side, and the saved file, from the original. Marks are drawn as SVG by resvg (already part of GPUI), with the system's fonts; text is escaped and the SVG links nothing. Shadows are blurred at a quarter of the size. Previews are made off the UI thread, one at a time, and changes made meanwhile go into the next.

Undo keeps 200 steps. A slider dragged, or a mark moved, is one step.

## Saving

**Export…** chooses the format and quality, then **Save a copy…** asks where (`name-edited.png` beside the original by default):

- PNG (optionally 256 colors: often a third of the size for screenshots), JPEG (quality), WebP (lossless), GIF, BMP, TIFF, ICO (16 to 256 pixels in one file).
- **At most … KB**: JPEG lowers its quality, then PNG drops to 256 colors, then the picture gets smaller, until the file fits; Export says what gave way.
- Export shows what the file would weigh. It works this out from the full picture, only while Export is open.
- Formats without transparency put see-through parts on white. Saved pictures carry no metadata (camera, place).
- Files are written beside their place first and then moved there, so a failed save never leaves half a picture.

**⋯ → Copy the picture** (⇧⌘C) puts it on the clipboard as PNG. **Make app icons…** writes a folder `AppIcons` with Android launcher icons (`mipmap-*`, Play Store 512), an iOS `AppIcon.appiconset` (1024, opaque), a macOS `AppIcon.iconset` (`iconutil -c icns` makes the `.icns`) and website icons (`favicon.ico`, `apple-touch-icon.png`, 192 and 512).

## With an agent

The chat beside the picture is the workspace's (its agents and models). Its starters: ready for docs, hide private details, point out what matters, make it smaller. Each message tells the agent which picture is open and to change it only with the Studio tools. In this chat the tools that look at and change the picture run without asking; saving asks.

| Tool | Does |
|---|---|
| `studio_get` | An image of the result (or, `view: "original"`, of the turned original to place marks on, with its scale), the edit so far with mark ids, the export settings, and a guide |
| `studio_edit` | `set` (crop, adjust, resize, frame), `transform`, `add`, `update` and `remove` marks, `export` settings, `reset`. Each call is one step the user can undo; a call that fails changes nothing |
| `studio_undo` | Undo, or redo |
| `studio_list` | The pictures open in Studio for the workspace |
| `studio_export` | Saves a new file (relative to the picture's folder, never over a file unless asked), or replaces the original when the user asked |

They are the `studio.*` control methods (`control.rs`). With several pictures open, `file` names one; otherwise the one used last.

## Video

The same window, for a video. Nothing is written until **Export…**, always to a new file.

```
┌ title: Studio · name ●                              ↶ ↷ ⋯ [Export…] [Agent] ┐
│ tools │ the video, with its marks                                │ chat      │
│       ├──────────────────────────────────────────────────────────┤ (toggle)  │
│       │ ◀◀ ◀ ▶ ▶  0:12.345 / 1:05.000  frame 371   A B Δ  ✂ 🔊 │           │
│       │ timeline: seconds, pictures, kept parts (2×), mark rows   │           │
│       │ the bar: the part, mark or tool selected                  │           │
│       │ status: source size, fps, length, reader → result         │           │
└────────────────────────────────────────────────────────────────────────────────┘
```

- **Play and step**: Space plays and pauses; ← and → step one frame (⇧ a second), by the frames' real times (screen recordings vary their frame rate); Home goes to the start. The time shown is the result's, with the source frame's number.
- **A and B** mark two moments and show the time and frames between them: for measuring latency (a tap to the screen changing).
- **The timeline** shows the result: the kept parts over pictures of them, with the seconds above. Click to go there; drag the playhead. **S** (or ✂) splits at the playhead. Click a part to select it: the bar offers its speed (0.5× to 16×) and **Cut this part**. Drag a part's edge to trim it, or the line between two parts to move the split.
- **Marks** are the picture's marks (arrow, rectangle, ellipse, text, numbered step, highlight, blur, pixelate, spotlight), each shown for a while: three seconds from the playhead when drawn. Their rows under the timeline show when (three at a time; more scroll, and selecting a mark scrolls to its row); drag a row's bar to move it in time, or its ends to change them. The bar has **Start here**, **End here** and **Fade**. Times are the source's, so a mark stays on what it points at when earlier parts are cut.
- **Crop** (Free, 16:9, 9:16, 1:1, 4:3) and **Resize** (Original, 1080p, 720p, 480p).
- **Sound**: 🔊 (or **Sound** in Export) turns it off in the result, for a smaller file. Parts sped up or slowed down are silent.
- **⋯**: copy or save the frame shown (as made for the result), keep all of it again, start over.
- **Export…**: MP4 (H.264, plays everywhere), MP4 with HEVC (about half the size), GIF (no sound; 480 to 1280 wide, 8 to 24 frames a second), WebM (VP9 and Opus: GStreamer on Linux, else ffmpeg). Only what this system can write is offered (HEVC needs an HEVC encoder: on Windows the HEVC Video Extensions or a graphics card that has one). **At most … MB** (10, 25, 50 for Confluence, 100) sets the bitrate so the file fits; a GIF over its limit is made again smaller. Frames a second as recorded or 60, 30, 24. The dialog shows the result's size, length and about how much it will weigh. Then a save panel; the export runs in the background (its progress and **Stop** in the title bar) and says when it is done.

**Reading and writing** (`video/media.rs`): the system's own framework first, so no codec ships with PilotHouse; `ffmpeg` and `ffprobe` when installed (on the PATH, `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`), never bundled, for what the system cannot do (kinds it cannot read, WebM on macOS and Windows, a missing encoder). A video the system reads is written by it too (it reads the sound); one ffmpeg reads, by ffmpeg.

- **macOS** (`video/av.rs`): AVFoundation reads and writes (H.264 and HEVC, hardware encoders, AAC sound).
- **Windows** (`video/mf.rs`): Media Foundation. A source reader decodes (its video processor converts to RGB32; frames are cropped to the display area, since decoders pad them to their block size, and turned upright as `MF_MT_VIDEO_ROTATION` says); a sink writer writes H.264 or HEVC from NV12 frames (converted in `video/yuv.rs`, BT.709 for HD, BT.601 below) with AAC sound into MP4, the index first. A graphics card's encoder is used where it takes the job, Windows' own where not (also when the card's fails on the first frame). Media Foundation is loaded when first used, so Windows N and KN start without it.
- **Linux** (`video/gst.rs`): GStreamer, loaded when first used (not linked). `parsebin` splits the file without decoding and only the stream wanted is decoded (`decodebin`), so pictures are never decoded for the sound or the other way round; `videoflip` stands phone recordings upright. Times are taken in the file's own timeline (an MP4's edit list applied). Writing goes through `appsrc` to the first encoder that passes a short trial run (x264, openh264, then VA-API and NVENC; x265 and its hardware kin; libvpx for WebM) and AAC (libav, fdk-aac, VisualOn, FAAC) or Opus. Without one, the export says what to install (`gstreamer1.0-plugins-ugly` for x264, `gstreamer1.0-libav` for AAC) or uses ffmpeg.

**The sound** (`video/sound.rs`): for the native writers, the result's sound is made sample for sample from the source's: each part kept at its own speed read in turn and placed at its time, silence between them and to the end, in stereo (more channels mixed down), resampled where the writer needs another rate. It is fed a little ahead of the pictures so the writer can interleave them. GStreamer's MP4 muxer keeps the AAC encoder's start-up silence (no edit list), so as much is dropped from the sound's start there; the sound then starts with the picture, to the sample.

**How it is made** (`video/export.rs`, `video/compose.rs`): each kept part is decoded in turn, upright; each frame of the result is the source frame showing at that moment, cropped, its hidden areas blurred or pixelated, resized, and its marks drawn by the same renderer as pictures' (the marks layer is kept between frames until the marks change). Frames go to the encoder at a steady rate; the sound of parts kept at their own speed goes with them. The file is written beside its place and moved there when done.

**While editing**: playing goes through the system player, which follows the timeline (cut parts jumped over, sped-up parts faster and muted); marks are drawn over it from small pictures made once each; hidden areas show as frosted boxes. Paused, an exact still of the result shows. Timeline pictures are made in the background and shown in batches, so the window stays idle.

**Agents**: the same tools. `studio_get` with `at` (source seconds) returns frames of the result at those times, the kept parts and where they land, and the marks with their times; `studio_edit` takes `cut`, `trim`, `speed`, `split`, `reset_timeline`, marks with `start`, `end` and `fade`, `set` (crop, resize, mute) and `export` (format, max_mb, fps, gif_width); `studio_export` writes a new file (never over the source). Starters: tighten it, explain the steps, hide private details, fit an upload limit.

## Code map

| File | What |
|---|---|
| `edit.rs` | The edit: turn, crop, adjust, marks, resize, frame; agents' changes (merge patches), checks and limits |
| `render.rs` | Making the result: adjust, hide, resize, marks through resvg, frame and shadows |
| `encode.rs` | Formats, quality, the size limit, 256-color PNG, ICO |
| `icons.rs` | App icon sets |
| `document.rs` | One picture: the original, the edit and its history, previews, the export estimate, saving |
| `editor.rs`, `editor/bar.rs`, `editor/export.rs` | The window's content: tools, the picture and what is drawn over it, the bar, Export |
| `history.rs` | Undo and redo, shared by pictures and videos |
| `video/edit.rs` | A video's edit: kept parts and speed, crop, size, marks over time, sound; export settings and bitrate |
| `video/media.rs` | Reading and writing: which system framework or ffmpeg, and falling back |
| `video/av.rs`, `video/mf.rs`, `video/gst.rs`, `video/ffmpeg.rs` | AVFoundation (macOS), Media Foundation (Windows), GStreamer (Linux), ffmpeg |
| `video/frames.rs`, `video/sound.rs`, `video/yuv.rs` | Frames after a seek, the result's sound, NV12 for Windows' encoders |
| `video/compose.rs`, `video/export.rs` | Each frame of the result; writing the file (MP4, HEVC, WebM, GIF) |
| `video/document.rs` | One video: what it is, its frames' times, the edit, timeline pictures, exact stills, exports |
| `video/editor.rs`, `video/editor/bar.rs` | The window's content: tools, the video, play and step, the timeline, the bar, Export |
| `control.rs` | `studio.*` for agents, pictures and videos |
