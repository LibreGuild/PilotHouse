# Live panes: pick, mark, capture, record

Crate: `ph-media`. The browser and device panes share one pattern: a **frame source**, an **input sink**, and a layer PilotHouse draws on top. Because PilotHouse draws every frame itself, marks sit directly on the live picture.

## The tools

They are in the pane's own toolbar (the browser's beside its address, the phone's beside Back, Home and Recents). Until 0.7.0 they floated over the picture, at its bottom until dragged away, and so always hid a part of what one was looking at; in the toolbar they hide nothing.

| Tool | What it does |
|---|---|
| Pick an element | The next click describes the element under it (web: selector, text, style, the CSS that shapes it — box model, flex, borders, type — the source file and line when a dev build tells (React before 19, Vue, Svelte, `data-source`), and HTML; Android: class, id, text, bounds), outlines it and attaches it to the chat. One pick, then the pointer is the app's again |
| Mark | Draws on the picture in red, the kind used last: a box, an arrow, freehand or a numbered pin (the ▾ beside it picks another). Pins are numbered (1, 2, 3…) so a message can refer to them. Undo and clear appear once there are marks. The button again, or Esc, ends marking; so does capturing |
| Capture (⌘⇧S) | The frame with the marks burned in, at full resolution, attached to the chat with a note explaining the marks |
| Record | Frames are written to a GIF as they arrive (downscaled to 720 px, at most 8 per second, up to 10 minutes), on a worker thread, so memory stays flat. Stopping attaches 4 evenly spaced frames and the GIF's path |

Without a mode chosen the pointer and the keys go to the page or device.

**The outline of a picked element** on a phone's screen goes with the eraser (which shows while there is an outline or a mark), with the next press on the screen (which it would no longer fit), with a capture, or when the same element is picked again.

**In a narrow pane** the tools fold: they are items of the pane's **⋯** menu (*Pick an element*, *Mark the picture* with its kinds, *Send a screenshot to chat*, *Record the screen*), and the row keeps only what is going on, a mode to leave, the eraser, a recording to stop. So the address of a page, and a phone's Back, Home and Recents, keep their room, and nothing is cut off at the pane's edge.

*Before/after* (keep a frame, compare it with a later one) was a fifth tool until 0.7.0. It is gone: two captures say the same to an agent, and nobody found a use for the diff. UI tests compare pictures their own way ([tests.md](tests.md)).

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
| `recorder.rs` | Streaming GIF recordings with a small keyframe sample |
| `live_tools.rs` | The tools and `ToChat`, the event a pane uses to hand things to the chat |
