# Live panes: annotate, capture, record, compare

Crate: `ph-media`. The browser and device panes share one pattern: a **frame source**, an **input sink**, and a layer PilotHouse draws on top. Because PilotHouse draws every frame itself, marks sit directly on the live picture.

## The floating toolbar

| Tool | What it does |
|---|---|
| Use the app | Pointer and keys go to the page or device |
| Pick an element | The next click describes the element under it (web: selector, text, style, HTML; Android: class, id, text, bounds), outlines it and attaches it to the chat |
| Box, arrow, pen, pin | Marks in red. Pins are numbered (1, 2, 3…) so a message can refer to them. Undo and clear appear once there are marks |
| Capture (⌘⇧S) | The frame with the marks burned in, at full resolution, attached to the chat with a note explaining the marks |
| Record | Frames are written to a GIF as they arrive (downscaled to 720 px, at most 8 per second, up to 10 minutes), on a worker thread, so memory stays flat. Stopping attaches 4 evenly spaced frames and the GIF's path |
| Before/after | The first press keeps the current frame. The second compares it with the frame now and attaches before, after and a diff, with the changed share and area |

⌘⇧S works from anywhere in the project. It captures the visible browser or device, or the one that last showed something.

## Mentions

In the chat composer, `@screenshot` attaches the visible pane's frame (with marks), and `@log` attaches recent browser console errors, failed requests and logcat errors. Both are also in the composer's **+** menu.

## Where files go

Captures and recordings are saved in `<data dir>/captures` and `<data dir>/recordings`, never in the project folder.

## Code map

| File | Role |
|---|---|
| `frame.rs` | Frames, stored in BGRA inside the `RenderImage` GPUI draws: converted once off the UI thread, shown without a copy. RGBA only for PNG, marks and GIFs |
| `surface.rs` | `SurfaceView`: draws the latest frame (contain-fit), maps input to frame pixels, the annotation overlay. Replaced GPU textures are freed on the next render |
| `annotate.rs` | Shapes and burning them into a frame |
| `recorder.rs`, `compare.rs` | Streaming GIF recordings with a small keyframe sample; visual diff |
| `live_tools.rs` | The toolbar and `ToChat`, the event a pane uses to hand things to the chat |
