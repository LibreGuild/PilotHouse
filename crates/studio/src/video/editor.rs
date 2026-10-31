//! The video editor: tools on the left, the video in the middle with its
//! marks, under it the controls to play and step, the timeline of kept
//! parts and marks, and the bar for what is selected.
//!
//! Playing goes through the system's player, which follows the timeline:
//! a cut part is jumped over, a sped-up part plays faster (silently, as it
//! will be written). Marks are drawn over the picture from small pictures
//! made once by the same renderer the export uses; paused, an exact still
//! of the result is shown, hidden areas and all.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, Entity, FocusHandle,
    Focusable, Hsla, Image, ImageFormat, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement as _,
    Pixels, Point, Render, RenderImage, ScrollHandle, SharedString, Styled as _, StyledImage as _,
    Subscription, Task, Window, canvas, div, img, point, px,
};
use ph_media::Frame;
use ph_preview::video::{Engine, Frame as PlayerFrame, Status, picture};
use ph_ui::{Icons, MenuButton as _};

use super::document::VideoDocument;
use super::edit::{Overlay, VideoEdit};
use crate::edit::{Color, Mark, MarkKind, MarkType, Rect};
use crate::editor::{CORNERS, EDGES, Handle, PALETTE, near_segment, notice, to_hsla};

mod bar;

/// How long a new mark shows.
const MARK_SECONDS: f64 = 3.;

/// When a mark made at `position` shows: from there for a few seconds,
/// or, near the end, the last few seconds (never no time at all).
fn mark_time(position: f64, duration: f64) -> (f64, f64) {
    let length = MARK_SECONDS.min(duration.max(0.));
    let start = position.clamp(0., (duration - length).max(0.));
    (start, (start + length).min(duration.max(start)))
}
/// How near (screen pixels) the pointer must be to pick something.
const REACH: f32 = 8.;
/// Space kept around the picture.
const MARGIN: f32 = 20.;
/// Marks' pictures are made at most this many pixels a side.
const SPRITE_SIDE: f32 = 1280.;
/// How often the player is looked at while playing (each frame) and not.
const PLAYING_TICK: Duration = Duration::from_millis(16);
const IDLE_TICK: Duration = Duration::from_millis(120);
/// Closer than this to the end, Play starts again from the beginning.
const END_SLACK: f64 = 0.05;
/// How long a seek may take before the player's own place is believed.
const SEEK_PATIENCE: Duration = Duration::from_millis(1500);

/// What the pointer does on the video, and what the bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Crop,
    Resize,
    Mark(MarkType),
}

impl Tool {
    fn picks(self) -> bool {
        matches!(self, Tool::Select | Tool::Resize)
    }
}

const TOOLS: &[Option<(Tool, Icons, &str, &str)>] = &[
    Some((Tool::Select, Icons::MousePointer2, "Select and move", "V")),
    None,
    Some((Tool::Crop, Icons::Crop, "Crop", "C")),
    Some((Tool::Resize, Icons::Scaling, "Resize", "")),
    None,
    Some((
        Tool::Mark(MarkType::Arrow),
        Icons::MoveUpRight,
        "Arrow",
        "A",
    )),
    Some((Tool::Mark(MarkType::Rect), Icons::Square, "Rectangle", "R")),
    Some((Tool::Mark(MarkType::Ellipse), Icons::Circle, "Ellipse", "O")),
    Some((Tool::Mark(MarkType::Text), Icons::Type, "Text", "T")),
    Some((
        Tool::Mark(MarkType::Step),
        Icons::Hash,
        "Numbered step",
        "N",
    )),
    Some((
        Tool::Mark(MarkType::Highlight),
        Icons::Highlighter,
        "Highlight",
        "H",
    )),
    None,
    Some((
        Tool::Mark(MarkType::Blur),
        Icons::Droplet,
        "Blur (hide an area)",
        "B",
    )),
    Some((
        Tool::Mark(MarkType::Pixelate),
        Icons::Grid3x3,
        "Pixelate (hide an area)",
        "X",
    )),
    Some((
        Tool::Mark(MarkType::Spotlight),
        Icons::Focus,
        "Spotlight (dim the rest)",
        "",
    )),
];

/// What is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selection {
    None,
    Mark(u32),
    /// A kept part, by its place.
    Part(usize),
}

/// Where the picture is on screen.
#[derive(Debug, Clone, Copy)]
struct Placement {
    origin: Point<Pixels>,
    /// Screen pixels per source pixel.
    scale: f32,
    /// The part of the source shown (all of it while cropping).
    shown: Rect,
}

impl Placement {
    fn to_source(self, position: Point<Pixels>) -> [f32; 2] {
        [
            f32::from(position.x - self.origin.x) / self.scale + self.shown.x,
            f32::from(position.y - self.origin.y) / self.scale + self.shown.y,
        ]
    }

    fn on_screen(self, p: [f32; 2]) -> Point<Pixels> {
        point(
            self.origin.x + px((p[0] - self.shown.x) * self.scale),
            self.origin.y + px((p[1] - self.shown.y) * self.scale),
        )
    }
}

/// A drag under way on the picture or the timeline.
#[derive(Debug, Clone)]
enum Drag {
    Create {
        kind: MarkType,
        from: [f32; 2],
        to: [f32; 2],
        points: Vec<[f32; 2]>,
    },
    Move {
        id: u32,
        from: [f32; 2],
        to: [f32; 2],
    },
    Resize {
        id: u32,
        handle: Handle,
        to: [f32; 2],
    },
    Crop {
        start: Rect,
        handle: Option<Handle>,
        from: [f32; 2],
        to: [f32; 2],
        new: bool,
    },
    /// The playhead, on the timeline.
    Scrub,
    /// A kept part's start or end, by its place; the pointer's result time.
    Edge { part: usize, end: bool, at: f64 },
    /// A mark's time on the timeline: what moves, and how far the pointer
    /// was from its start when grabbed (result seconds).
    MarkTime {
        id: u32,
        what: TimeGrab,
        grab: f64,
        at: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeGrab {
    Whole,
    Start,
    End,
}

/// A mark's picture: what it was made from, and where it goes (source
/// pixels).
struct Sprite {
    made_from: String,
    frame: Frame,
    at: Rect,
}

pub struct VideoEditor {
    pub(crate) doc: Entity<VideoDocument>,
    engine: Option<Result<Engine, SharedString>>,
    picture: Option<PlayerFrame>,
    old_images: Vec<Arc<RenderImage>>,
    /// The source time shown (the player's while playing).
    position: f64,
    rate: f32,
    focus: FocusHandle,
    pub(crate) tool: Tool,
    pub(crate) color: Color,
    pub(crate) selection: Selection,
    drag: Option<Drag>,
    canvas: Rc<Cell<Bounds<Pixels>>>,
    timeline: Rc<Cell<Bounds<Pixels>>>,
    /// The marks' rows under the timeline, when more overlap than fit.
    mark_rows: ScrollHandle,
    placement: Option<Placement>,
    pub(crate) crop_aspect: Option<f32>,
    /// Two moments to measure between (result seconds).
    pub(crate) marks_ab: (Option<f64>, Option<f64>),
    sprites: HashMap<u32, Sprite>,
    _sprites: HashMap<u32, Task<()>>,
    pub(crate) controls: bar::Controls,
    _tick: Option<Task<()>>,
    /// Where the player was last sent and when: seeking takes a moment,
    /// and until the player is there its old place is not where it is.
    seeking: Option<(f64, std::time::Instant)>,
    _subscriptions: Vec<Subscription>,
}

impl VideoEditor {
    pub fn new(doc: Entity<VideoDocument>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (controls, mut subscriptions) = bar::Controls::new(window, cx);
        subscriptions.push(cx.observe_in(&doc, window, |this, doc, window, cx| {
            this.sync_controls(window, cx);
            // An export finished: say how it went, once.
            if let Some(outcome) = doc.update(cx, |doc, _| doc.finished.take()) {
                let notification = match outcome {
                    Ok(done) => Notification::success(format!(
                        "Saved {} ({}).",
                        done.path.file_name().unwrap_or_default().to_string_lossy(),
                        crate::control::kilobytes(done.bytes as usize)
                    )),
                    Err(err) if err == "Stopped." => Notification::info("Export stopped."),
                    Err(err) => Notification::error(format!("Not exported: {err}")),
                };
                window.push_notification(notification, cx);
            }
            cx.notify();
        }));
        let path = doc.read(cx).path().to_path_buf();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut this = Self {
            doc,
            engine: Some(Engine::open(&path).map_err(Into::into)),
            picture: None,
            old_images: Vec::new(),
            position: 0.,
            rate: 1.,
            focus,
            tool: Tool::Select,
            color: PALETTE[0],
            selection: Selection::None,
            drag: None,
            canvas: Rc::default(),
            timeline: Rc::default(),
            mark_rows: ScrollHandle::new(),
            placement: None,
            crop_aspect: None,
            marks_ab: (None, None),
            sprites: HashMap::new(),
            _sprites: HashMap::new(),
            controls,
            _tick: None,
            seeking: None,
            _subscriptions: subscriptions,
        };
        this.watch(cx);
        cx.on_release(|this, cx| {
            if let Some(engine) = this.engine() {
                engine.pause();
            }
            let current = match this.picture.take() {
                Some(PlayerFrame::Image(image)) => Some(image),
                _ => None,
            };
            for image in this.old_images.drain(..).chain(current) {
                cx.drop_image(image, None);
            }
        })
        .detach();
        this
    }

    pub fn document(&self) -> &Entity<VideoDocument> {
        &self.doc
    }

    fn engine(&self) -> Option<&Engine> {
        self.engine.as_ref().and_then(|engine| engine.as_ref().ok())
    }

    fn playing(&self) -> bool {
        self.engine().is_some_and(Engine::is_playing)
    }

    // Playing, along the timeline.

    /// Looks at the player often while it plays, now and then otherwise.
    fn watch(&mut self, cx: &mut Context<Self>) {
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                let playing = this
                    .update(cx, |this, cx| {
                        this.tick(cx);
                        this.playing()
                    })
                    .unwrap_or(false);
                let wait = if playing { PLAYING_TICK } else { IDLE_TICK };
                cx.background_executor().timer(wait).await;
                if this.upgrade().is_none() {
                    break;
                }
            }
        }));
    }

    /// Follows the timeline: past a kept part's end the player jumps to the
    /// next, at its speed; after the last it stops.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        let duration = doc.duration();
        let pieces = doc.edit().pieces(duration);
        let mute = doc.edit().mute;
        let exporting = doc.job().is_some();
        // The field alone, so the rate and position can change beside it.
        let Some(Ok(engine)) = &self.engine else {
            return;
        };
        // While exporting, often enough for its bar to move.
        let mut changed = exporting;
        // A seek not done yet: the player is where it was sent.
        let reported = engine.position();
        let at = match self.seeking {
            Some((target, since))
                if (reported - target).abs() > 0.05 && since.elapsed() < SEEK_PATIENCE =>
            {
                Some(target)
            }
            _ => {
                self.seeking = None;
                None
            }
        };
        if engine.is_playing() && at.is_none() {
            let at = reported;
            match pieces.iter().find(|piece| piece.contains(at)) {
                Some(piece) => {
                    if piece.speed != self.rate {
                        self.rate = piece.speed;
                        engine.set_rate(piece.speed);
                        engine.set_muted(mute || piece.speed != 1.);
                    }
                }
                None => match pieces.iter().find(|piece| piece.start > at - 0.001) {
                    Some(next) => {
                        engine.seek(next.start);
                        self.seeking = Some((next.start, std::time::Instant::now()));
                        self.rate = next.speed;
                        engine.set_rate(next.speed);
                        engine.set_muted(mute || next.speed != 1.);
                    }
                    None => {
                        engine.pause();
                        if let Some(last) = pieces.last() {
                            let end = (last.end - 0.001).max(last.start);
                            engine.seek(end);
                            self.seeking = Some((end, std::time::Instant::now()));
                        }
                    }
                },
            }
            let at = match self.seeking {
                Some((target, _)) => target,
                None => engine.position(),
            };
            if (at - self.position).abs() > 1e-4 {
                self.position = at;
                changed = true;
            }
        }
        if let Some(frame) = engine.frame() {
            if let Some(PlayerFrame::Image(old)) = self.picture.replace(frame) {
                self.old_images.push(old);
            }
            if self.old_images.len() > 1 {
                for image in self.old_images.drain(..self.old_images.len() - 1) {
                    cx.drop_image(image, None);
                }
            }
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        let (duration, mute) = (doc.duration(), doc.edit().mute);
        let pieces = doc.edit().pieces(duration);
        let Some(Ok(engine)) = &self.engine else {
            return;
        };
        if engine.is_playing() {
            engine.pause();
            self.position = engine.position();
        } else {
            // From a kept part: the next one, or the first after the end
            // (at the end, within a frame or so of it, too: playing on
            // from there would end at once). Where it is: where it was
            // sent, while a seek is under way.
            let at = self
                .seeking
                .map_or_else(|| engine.position(), |(target, _)| target);
            let at_end = pieces.last().is_some_and(|last| at >= last.end - END_SLACK);
            let piece = pieces
                .iter()
                .find(|piece| !at_end && piece.contains(at))
                .or_else(|| pieces.iter().find(|piece| !at_end && piece.start > at))
                .or(pieces.first())
                .copied();
            if let Some(piece) = piece {
                if at_end || !piece.contains(at) {
                    engine.seek(piece.start);
                    self.seeking = Some((piece.start, std::time::Instant::now()));
                    self.position = piece.start;
                }
                self.rate = piece.speed;
                engine.set_muted(mute || piece.speed != 1.);
                engine.play(piece.speed);
            }
        }
        cx.notify();
    }

    /// Shows source time `t` (paused).
    pub(crate) fn seek(&mut self, t: f64, cx: &mut Context<Self>) {
        let duration = self.doc.read(cx).duration();
        let t = t.clamp(0., (duration - 0.0005).max(0.));
        if let Some(engine) = self.engine() {
            engine.seek(t);
        }
        self.seeking = Some((t, std::time::Instant::now()));
        self.position = t;
        cx.notify();
    }

    /// Shows result time `t`.
    pub(crate) fn seek_result(&mut self, t: f64, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        let (source, _) = doc.edit().to_source(doc.duration(), t.max(0.));
        self.seek(source, cx);
    }

    /// The result time shown.
    pub(crate) fn result_time(&self, cx: &App) -> f64 {
        let doc = self.doc.read(cx);
        let duration = doc.duration();
        let edit = doc.edit();
        edit.to_output(duration, self.position).unwrap_or_else(|| {
            // In a cut part: where the next kept part starts.
            let mut before = 0.;
            for piece in edit.pieces(duration) {
                if piece.start >= self.position {
                    return before;
                }
                before += piece.length();
            }
            before
        })
    }

    /// Steps `frames` frames (negative: back), over cut parts.
    pub(crate) fn step(&mut self, frames: i32, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine() {
            engine.pause();
        }
        let doc = self.doc.read(cx);
        let edit = doc.edit().clone();
        let duration = doc.duration();
        let starts = doc.frames().to_vec();
        let fps = doc.info().map_or(30., |info| info.fps);
        let mut at = self.position;
        for _ in 0..frames.unsigned_abs() {
            let next = if frames > 0 {
                starts
                    .iter()
                    .find(|start| **start > at + 1e-4)
                    .copied()
                    .unwrap_or(at + 1. / fps)
            } else {
                starts
                    .iter()
                    .rev()
                    .find(|start| **start < at - 1e-4)
                    .copied()
                    .unwrap_or((at - 1. / fps).max(0.))
            };
            at = next;
            // Over a cut, to the nearest kept frame that way.
            if edit.to_output(duration, at).is_none() {
                let pieces = edit.pieces(duration);
                at = if frames > 0 {
                    pieces
                        .iter()
                        .find(|piece| piece.start > at)
                        .map_or(at, |piece| piece.start)
                } else {
                    pieces
                        .iter()
                        .rev()
                        .find(|piece| piece.end <= at)
                        .map_or(at, |piece| (piece.end - 1. / fps).max(piece.start))
                };
            }
        }
        self.seek(at, cx);
    }

    // Selecting and changing.

    pub(crate) fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if tool == Tool::Crop {
            self.selection = Selection::None;
        }
        self.tool = tool;
        self.drag = None;
        cx.notify();
    }

    pub(crate) fn select(
        &mut self,
        selection: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A text left empty goes when it is let go of.
        if let Selection::Mark(old) = self.selection
            && selection != self.selection
        {
            let empty = self.doc.read(cx).edit().overlay(old).is_some_and(
                |overlay| matches!(&overlay.kind, MarkKind::Text { text, .. } if text.trim().is_empty()),
            );
            if empty {
                self.doc.update(cx, |doc, cx| {
                    doc.update(
                        "remove empty text",
                        |edit, _, _| edit.overlays.retain(|o| o.id != old),
                        cx,
                    )
                });
            }
        }
        self.selection = selection;
        if let Selection::Mark(id) = selection {
            self.reveal_mark_row(id, cx);
        }
        self.doc.update(cx, |doc, _| doc.settle());
        self.sync_controls(window, cx);
        cx.notify();
    }

    /// Changes the selected mark.
    pub(crate) fn update_mark(
        &mut self,
        what: &'static str,
        change: impl FnOnce(&mut Overlay),
        cx: &mut Context<Self>,
    ) {
        let Selection::Mark(id) = self.selection else {
            return;
        };
        self.doc.update(cx, |doc, cx| {
            doc.update(
                what,
                |edit, _, _| {
                    if let Some(overlay) = edit.overlay_mut(id) {
                        change(overlay);
                    }
                },
                cx,
            )
        });
    }

    /// Changes the timeline (one step to undo).
    pub(crate) fn update_timeline(
        &mut self,
        what: &'static str,
        change: impl FnOnce(&mut VideoEdit, f64),
        cx: &mut Context<Self>,
    ) {
        self.doc.update(cx, |doc, cx| {
            doc.settle();
            doc.update(what, |edit, duration, _| change(edit, duration), cx);
            doc.settle();
        });
    }

    pub(crate) fn split_here(&mut self, cx: &mut Context<Self>) {
        let at = self.position;
        self.update_timeline("split", |edit, duration| edit.split(duration, at), cx);
    }

    pub(crate) fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.selection {
            Selection::Mark(id) => {
                self.doc.update(cx, |doc, cx| {
                    doc.settle();
                    doc.update(
                        "delete",
                        |edit, _, _| edit.overlays.retain(|o| o.id != id),
                        cx,
                    )
                });
                self.select(Selection::None, window, cx);
            }
            Selection::Part(ix) => {
                self.update_timeline(
                    "delete part",
                    |edit, duration| edit.delete_piece(duration, ix),
                    cx,
                );
                self.select(Selection::None, window, cx);
            }
            Selection::None => {}
        }
    }

    fn add_overlay(&mut self, kind: MarkKind, window: &mut Window, cx: &mut Context<Self>) {
        let position = self.position;
        let mut added = None;
        self.doc.update(cx, |doc, cx| {
            doc.settle();
            doc.update(
                "add mark",
                |edit, duration, size| {
                    let (start, end) = mark_time(position, duration);
                    let overlay = Overlay {
                        id: 0,
                        start,
                        end,
                        // Seen at once where it is made; Fade adds one.
                        fade: 0.,
                        kind,
                    };
                    added = edit.add_overlay(overlay, size).ok();
                },
                cx,
            );
            doc.settle();
        });
        if let Some(id) = added {
            self.select(Selection::Mark(id), window, cx);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.secondary() || modifiers.alt || modifiers.control {
            return;
        }
        let handled = match keystroke.key.as_str() {
            "space" => {
                self.toggle_play(cx);
                true
            }
            "left" => {
                if modifiers.shift {
                    let to = self.result_time(cx) - 1.;
                    self.seek_result(to, cx);
                } else {
                    self.step(-1, cx);
                }
                true
            }
            "right" => {
                if modifiers.shift {
                    let to = self.result_time(cx) + 1.;
                    self.seek_result(to, cx);
                } else {
                    self.step(1, cx);
                }
                true
            }
            "home" => {
                self.seek_result(0., cx);
                true
            }
            "s" => {
                self.split_here(cx);
                true
            }
            "backspace" | "delete" => {
                self.delete_selected(window, cx);
                true
            }
            "escape" => {
                if self.drag.take().is_none() {
                    if self.tool != Tool::Select {
                        self.set_tool(Tool::Select, cx);
                    } else {
                        self.select(Selection::None, window, cx);
                    }
                }
                true
            }
            key => {
                let key = key.to_ascii_uppercase();
                match TOOLS
                    .iter()
                    .flatten()
                    .find(|(_, _, _, shortcut)| !shortcut.is_empty() && *shortcut == key)
                {
                    Some((tool, ..)) => {
                        self.set_tool(*tool, cx);
                        true
                    }
                    None => false,
                }
            }
        };
        if handled {
            cx.stop_propagation();
        }
    }

    // The picture.

    /// Where the picture goes in `bounds`.
    fn place(&self, bounds: Bounds<Pixels>, cx: &App) -> Option<Placement> {
        let doc = self.doc.read(cx);
        let size = doc.info()?.size;
        let shown = if self.tool == Tool::Crop {
            Rect::new(0., 0., size.0 as f32, size.1 as f32)
        } else {
            doc.edit().crop_rect(size)
        };
        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if width <= 0. || height <= 0. {
            return None;
        }
        let scale = ((width - 2. * MARGIN) / shown.width)
            .min((height - 2. * MARGIN) / shown.height)
            .max(0.01);
        let origin = point(
            bounds.origin.x + px((width - shown.width * scale) / 2.),
            bounds.origin.y + px((height - shown.height * scale) / 2.),
        );
        Some(Placement {
            origin,
            scale,
            shown,
        })
    }

    fn reach(&self, placement: &Placement) -> f32 {
        REACH / placement.scale
    }

    /// The marks showing now, topmost last.
    fn showing<'a>(&self, edit: &'a VideoEdit) -> Vec<&'a Overlay> {
        edit.overlays
            .iter()
            .filter(|overlay| {
                overlay.alpha(self.position) > 0. || self.selection == Selection::Mark(overlay.id)
            })
            .collect()
    }

    fn mark_at(&self, p: [f32; 2], reach: f32, cx: &App) -> Option<u32> {
        let edit = self.doc.read(cx).edit();
        self.showing(edit).into_iter().rev().find_map(|overlay| {
            let hit = match &overlay.kind {
                MarkKind::Arrow {
                    from, to, stroke, ..
                }
                | MarkKind::Line {
                    from, to, stroke, ..
                } => near_segment(p, *from, *to) <= reach + stroke / 2.,
                MarkKind::Pen { points, stroke, .. } => points
                    .windows(2)
                    .any(|pair| near_segment(p, pair[0], pair[1]) <= reach + stroke / 2.),
                MarkKind::Rect {
                    fill: None, stroke, ..
                }
                | MarkKind::Ellipse {
                    fill: None, stroke, ..
                } => {
                    let bounds = overlay.kind.bounds();
                    bounds.grow(reach + stroke).contains(p)
                        && !bounds.grow(-(reach + stroke)).contains(p)
                }
                kind => kind.bounds().grow(reach).contains(p),
            };
            hit.then_some(overlay.id)
        })
    }

    fn handle_at(&self, p: [f32; 2], reach: f32, cx: &App) -> Option<Handle> {
        let Selection::Mark(id) = self.selection else {
            return None;
        };
        let overlay = self.doc.read(cx).edit().overlay(id)?;
        let near =
            |q: [f32; 2]| (q[0] - p[0]).abs() <= reach * 1.5 && (q[1] - p[1]).abs() <= reach * 1.5;
        match &overlay.kind {
            MarkKind::Arrow { from, to, .. } | MarkKind::Line { from, to, .. } => {
                if near(*to) {
                    Some(Handle::To)
                } else if near(*from) {
                    Some(Handle::From)
                } else {
                    None
                }
            }
            kind => {
                let area = kind.area()?;
                CORNERS.into_iter().find(|handle| near(handle.at(area)))
            }
        }
    }

    fn canvas_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let Some(placement) = self.placement else {
            return;
        };
        let p = placement.to_source(event.position);
        let reach = self.reach(&placement);
        let size = self.doc.read(cx).size();
        let outside = p[0] < 0. || p[1] < 0. || p[0] > size.0 as f32 || p[1] > size.1 as f32;
        match self.tool {
            Tool::Select | Tool::Resize => {
                if let Selection::Mark(id) = self.selection
                    && let Some(handle) = self.handle_at(p, reach, cx)
                {
                    self.drag = Some(Drag::Resize { id, handle, to: p });
                } else if let Some(id) = self.mark_at(p, reach, cx) {
                    if self.selection != Selection::Mark(id) {
                        self.select(Selection::Mark(id), window, cx);
                    }
                    self.drag = Some(Drag::Move { id, from: p, to: p });
                } else {
                    self.select(Selection::None, window, cx);
                }
            }
            Tool::Crop => {
                let crop = self.doc.read(cx).edit().crop_rect(size);
                let handles: &[Handle] = if self.crop_aspect.is_some() {
                    &CORNERS
                } else {
                    &EDGES
                };
                let near = |q: [f32; 2]| {
                    (q[0] - p[0]).abs() <= reach * 1.5 && (q[1] - p[1]).abs() <= reach * 1.5
                };
                let handle = handles.iter().copied().find(|handle| near(handle.at(crop)));
                let new = handle.is_none() && !crop.contains(p);
                self.drag = Some(Drag::Crop {
                    start: crop,
                    handle,
                    from: p,
                    to: p,
                    new,
                });
            }
            Tool::Mark(_) if outside => {}
            Tool::Mark(MarkType::Text) => {
                let kind = MarkKind::Text {
                    x: p[0],
                    y: p[1],
                    text: String::new(),
                    size: 0.,
                    color: self.color,
                    background: Some(bar::contrasting(self.color)),
                    bold: true,
                    align: crate::edit::Align::Left,
                };
                self.add_overlay(kind, window, cx);
                self.set_tool(Tool::Select, cx);
                cx.on_next_frame(window, |this, window, cx| {
                    this.controls.focus_text(window, cx)
                });
            }
            Tool::Mark(MarkType::Step) => {
                self.add_overlay(MarkType::Step.mark(p, p, self.color), window, cx);
            }
            Tool::Mark(kind) => {
                self.drag = Some(Drag::Create {
                    kind,
                    from: p,
                    to: p,
                    points: vec![p],
                });
            }
        }
        cx.notify();
    }

    fn canvas_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(placement) = self.placement else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let p = placement.to_source(event.position);
        let reach = self.reach(&placement);
        match &mut self.drag {
            Some(Drag::Create { to, points, .. }) => {
                *to = p;
                if points
                    .last()
                    .is_none_or(|last| (last[0] - p[0]).hypot(last[1] - p[1]) > reach / 3.)
                {
                    points.push(p);
                }
            }
            Some(Drag::Move { to, .. } | Drag::Resize { to, .. } | Drag::Crop { to, .. }) => {
                *to = p
            }
            _ => return,
        }
        cx.notify();
    }

    fn canvas_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(placement) = self.placement else {
            return;
        };
        let reach = self.reach(&placement);
        // Only the picture's own drags: one on the timeline ends there.
        if matches!(
            self.drag,
            Some(Drag::Scrub | Drag::Edge { .. } | Drag::MarkTime { .. }) | None
        ) {
            return;
        }
        match self.drag.take() {
            Some(Drag::Create {
                kind,
                from,
                to,
                points,
            }) => {
                if (to[0] - from[0]).hypot(to[1] - from[1]) >= reach {
                    let mark = match kind {
                        MarkType::Pen => MarkKind::Pen {
                            points,
                            color: self.color,
                            stroke: 0.,
                        },
                        kind => kind.mark(from, to, self.color),
                    };
                    self.add_overlay(mark, window, cx);
                }
            }
            Some(Drag::Move { id, from, to }) => {
                let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
                if dx != 0. || dy != 0. {
                    self.doc.update(cx, |doc, cx| {
                        doc.settle();
                        doc.update(
                            "move",
                            |edit, _, _| {
                                if let Some(overlay) = edit.overlay_mut(id) {
                                    overlay.kind.translate(dx, dy);
                                }
                            },
                            cx,
                        );
                        doc.settle();
                    });
                }
            }
            Some(Drag::Resize { id, handle, to }) => {
                self.doc.update(cx, |doc, cx| {
                    doc.settle();
                    doc.update(
                        "resize mark",
                        |edit, _, _| {
                            if let Some(overlay) = edit.overlay_mut(id) {
                                resize_kind(&mut overlay.kind, handle, to);
                            }
                        },
                        cx,
                    );
                    doc.settle();
                });
            }
            Some(drag @ Drag::Crop { .. }) => {
                if let Some(crop) = self
                    .dragged_crop(&drag, cx)
                    .filter(|crop| crop.width >= 16. && crop.height >= 16.)
                {
                    self.doc.update(cx, |doc, cx| {
                        doc.settle();
                        doc.update("crop", |edit, _, _| edit.crop = Some(crop), cx);
                        doc.settle();
                    });
                }
            }
            other => self.drag = other,
        }
        cx.notify();
    }

    fn dragged_crop(&self, drag: &Drag, cx: &App) -> Option<Rect> {
        let Drag::Crop {
            start,
            handle,
            from,
            to,
            new,
        } = drag
        else {
            return None;
        };
        let size = self.doc.read(cx).size();
        let rect = if *new {
            let mut to = *to;
            if let Some(aspect) = self.crop_aspect {
                let height = (to[0] - from[0]).abs() / aspect;
                to[1] = from[1] + height * if to[1] < from[1] { -1. } else { 1. };
            }
            Rect::from_corners(*from, to)
        } else if let Some(handle) = handle {
            handle.drag(*start, *to, self.crop_aspect)
        } else {
            let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
            Rect::new(
                (start.x + dx).clamp(0., size.0 as f32 - start.width),
                (start.y + dy).clamp(0., size.1 as f32 - start.height),
                start.width,
                start.height,
            )
        };
        Some(rect.clamped(size.0 as f32, size.1 as f32))
    }

    /// Makes the pictures of marks not made yet (or changed), off the UI
    /// thread.
    fn want_sprites(&mut self, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        let size = doc.size();
        if size.0 == 0 {
            return;
        }
        let scale = (SPRITE_SIDE / size.0.max(size.1) as f32).min(1.);
        let wanted: Vec<(u32, MarkKind, String)> = doc
            .edit()
            .overlays
            .iter()
            .filter(|overlay| {
                !matches!(
                    overlay.kind,
                    MarkKind::Blur { .. } | MarkKind::Pixelate { .. }
                )
            })
            .map(|overlay| {
                (
                    overlay.id,
                    overlay.kind.clone(),
                    format!("{:?}", overlay.kind),
                )
            })
            .filter(|(id, _, made_from)| {
                self.sprites
                    .get(id)
                    .is_none_or(|sprite| sprite.made_from != *made_from)
            })
            .collect();
        let alive: Vec<u32> = doc.edit().overlays.iter().map(|o| o.id).collect();
        self.sprites.retain(|id, _| alive.contains(id));
        for (id, kind, made_from) in wanted {
            // One at a time per mark: a change meanwhile is made after.
            if self._sprites.contains_key(&id) {
                continue;
            }
            let task = cx.background_spawn(async move { sprite(&kind, size, scale) });
            self._sprites.insert(
                id,
                cx.spawn(async move |this, cx| {
                    let made = task.await;
                    _ = this.update(cx, |this, cx| {
                        this._sprites.remove(&id);
                        if let Some((frame, at)) = made {
                            if let Some(old) = this.sprites.insert(
                                id,
                                Sprite {
                                    made_from,
                                    frame,
                                    at,
                                },
                            ) {
                                this.old_images.push(old.frame.render_image());
                            }
                            cx.notify();
                        }
                    });
                }),
            );
        }
    }

    fn render_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let cell = self.canvas.clone();
        let measure = canvas(
            move |bounds, window, _| {
                if cell.get() != bounds {
                    cell.set(bounds);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let bounds = self.canvas.get();
        self.placement = self.place(bounds, cx);
        self.want_sprites(cx);
        let playing = self.playing();
        let doc = self.doc.read(cx);
        let paused_still = (!playing && self.tool != Tool::Crop && self.drag.is_none())
            .then(|| doc.still(self.position).cloned())
            .flatten();
        let body: AnyElement = match (doc.error(), &self.engine, self.placement) {
            (Some(error), _, _) => notice(error.clone(), cx),
            (None, Some(Err(error)), _) => notice(error.clone(), cx),
            (None, _, Some(placement)) => {
                let size = doc.info().map_or((1, 1), |info| info.size);
                let local = |p: Point<Pixels>| p - bounds.origin;
                let at = local(placement.origin);
                let (shown_w, shown_h) = (
                    placement.shown.width * placement.scale,
                    placement.shown.height * placement.scale,
                );
                // The player's frame, the whole of it, moved so the crop
                // shows in the window cut to it.
                let frame = self.picture.as_ref().map(|frame| {
                    div()
                        .absolute()
                        .left(px(-placement.shown.x * placement.scale))
                        .top(px(-placement.shown.y * placement.scale))
                        .w(px(size.0 as f32 * placement.scale))
                        .h(px(size.1 as f32 * placement.scale))
                        .child(picture(frame))
                });
                let still = paused_still.map(|still| {
                    img(still.render_image())
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .object_fit(ObjectFit::Fill)
                });
                let showing_still = still.is_some();
                // Over an exact still, only a selected mark not showing at
                // this moment (faded, or outside its time), to edit it.
                let marks = self.render_marks(&placement, bounds, showing_still, cx);
                div()
                    .size_full()
                    .child(
                        div()
                            .absolute()
                            .left(at.x)
                            .top(at.y)
                            .w(px(shown_w))
                            .h(px(shown_h))
                            .overflow_hidden()
                            .bg(gpui_kit::black())
                            .shadow_md()
                            .children(frame)
                            .children(still),
                    )
                    .children(marks)
                    .children(self.render_selection(&placement, bounds, cx))
                    .into_any_element()
            }
            _ => notice("Opening…", cx),
        };
        // Paused: an exact still of the result, when it is made.
        if !playing && self.tool != Tool::Crop {
            let position = self.position;
            self.doc.update(cx, |doc, cx| doc.want_still(position, cx));
        }
        // Pictures replaced (player frames on Windows and Linux, marks'
        // pictures) are freed on the GPU, not only let go; the newest is
        // kept until a newer frame has been drawn, as the player does.
        let stale = self.doc.update(cx, |doc, _| doc.take_stale());
        let keep = self.old_images.len().saturating_sub(1);
        for image in stale.into_iter().chain(self.old_images.drain(..keep)) {
            let _ = window.drop_image(image);
        }
        div()
            .id("video-canvas")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .bg(theme.muted.opacity(0.5))
            .track_focus(&self.focus)
            .cursor(match self.tool {
                Tool::Select | Tool::Resize => gpui_kit::CursorStyle::Arrow,
                Tool::Mark(MarkType::Text) => gpui_kit::CursorStyle::IBeam,
                _ => gpui_kit::CursorStyle::Crosshair,
            })
            .child(measure)
            .child(body)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::canvas_down))
            .on_mouse_move(cx.listener(Self::canvas_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::canvas_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::canvas_up))
            .into_any_element()
    }

    /// The marks showing now, from their pictures, faded as they come and
    /// go; hidden areas as frosted boxes until paused.
    fn render_marks(
        &self,
        placement: &Placement,
        bounds: Bounds<Pixels>,
        over_still: bool,
        cx: &App,
    ) -> Vec<AnyElement> {
        let theme = cx.theme();
        let edit = self.doc.read(cx).edit();
        let mut out = Vec::new();
        for overlay in self.showing(edit) {
            let shown = overlay.alpha(self.position);
            let selected = self.selection == Selection::Mark(overlay.id);
            if over_still && !(selected && shown < 0.99) {
                continue;
            }
            let alpha = if selected { shown.max(0.6) } else { shown };
            if alpha <= 0. {
                continue;
            }
            let (rect, element) = match &overlay.kind {
                MarkKind::Blur { .. } | MarkKind::Pixelate { .. } => {
                    let area = overlay.kind.area().expect("an area");
                    (
                        area,
                        div()
                            .size_full()
                            .bg(theme.muted.opacity(0.92))
                            .border_1()
                            .border_color(theme.border)
                            .into_any_element(),
                    )
                }
                _ => match self.sprites.get(&overlay.id) {
                    Some(sprite) => (
                        sprite.at,
                        img(sprite.frame.render_image())
                            .size_full()
                            .object_fit(ObjectFit::Fill)
                            .into_any_element(),
                    ),
                    None => continue,
                },
            };
            let a = placement.on_screen([rect.x, rect.y]) - bounds.origin;
            out.push(
                div()
                    .absolute()
                    .left(a.x)
                    .top(a.y)
                    .w(px(rect.width * placement.scale))
                    .h(px(rect.height * placement.scale))
                    .opacity(alpha)
                    .child(element)
                    .into_any_element(),
            );
        }
        out
    }

    /// The crop, the selection and what is being drawn.
    fn render_selection(
        &self,
        placement: &Placement,
        bounds: Bounds<Pixels>,
        cx: &App,
    ) -> Vec<AnyElement> {
        let theme = cx.theme();
        let ring: Hsla = theme.ring;
        let screen = |p: [f32; 2]| placement.on_screen(p) - bounds.origin;
        let screen_rect = |r: Rect| {
            let a = screen([r.x, r.y]);
            let b = screen([r.right(), r.bottom()]);
            (
                a.x.min(b.x),
                a.y.min(b.y),
                (b.x - a.x).abs(),
                (b.y - a.y).abs(),
            )
        };
        let handle = |p: Point<Pixels>| {
            div()
                .absolute()
                .left(p.x - px(4.))
                .top(p.y - px(4.))
                .size(px(8.))
                .rounded(px(2.))
                .bg(gpui_kit::white())
                .border_1()
                .border_color(ring)
                .into_any_element()
        };
        let outline = |r: Rect, color: Hsla| {
            let (x, y, w, h) = screen_rect(r);
            div()
                .absolute()
                .left(x - px(1.))
                .top(y - px(1.))
                .w(w + px(2.))
                .h(h + px(2.))
                .border_1()
                .border_color(color)
                .into_any_element()
        };
        let mut out = Vec::new();
        if self.tool == Tool::Crop {
            let crop = self
                .drag
                .as_ref()
                .and_then(|drag| self.dragged_crop(drag, cx))
                .unwrap_or_else(|| self.doc.read(cx).edit().crop_rect(self.doc.read(cx).size()));
            let (x, y, w, h) = screen_rect(crop);
            let (ix, iy) = (
                placement.origin.x - bounds.origin.x,
                placement.origin.y - bounds.origin.y,
            );
            let (iw, ih) = (
                px(placement.shown.width * placement.scale),
                px(placement.shown.height * placement.scale),
            );
            let shade = gpui_kit::black().opacity(0.55);
            for (sx, sy, sw, sh) in [
                (ix, iy, iw, y - iy),
                (ix, y + h, iw, iy + ih - (y + h)),
                (ix, y, x - ix, h),
                (x + w, y, ix + iw - (x + w), h),
            ] {
                if sw > px(0.) && sh > px(0.) {
                    out.push(
                        div()
                            .absolute()
                            .left(sx)
                            .top(sy)
                            .w(sw)
                            .h(sh)
                            .bg(shade)
                            .into_any_element(),
                    );
                }
            }
            out.push(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(w)
                    .h(h)
                    .border_1()
                    .border_color(gpui_kit::white())
                    .into_any_element(),
            );
            let handles: &[Handle] = if self.crop_aspect.is_some() {
                &CORNERS
            } else {
                &EDGES
            };
            for h in handles {
                out.push(handle(screen(h.at(crop))));
            }
            return out;
        }
        match &self.drag {
            Some(Drag::Create { kind, from, to, .. }) => {
                let rect = Rect::from_corners(*from, *to);
                let color = to_hsla(self.color);
                let (x, y, w, h) = screen_rect(rect);
                let line = matches!(kind, MarkType::Arrow | MarkType::Line | MarkType::Pen);
                out.push(
                    div()
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(w.max(px(2.)))
                        .h(h.max(px(2.)))
                        .border_1()
                        .border_color(if line { ring.opacity(0.6) } else { color })
                        .when(*kind == MarkType::Ellipse, |this| this.rounded_full())
                        .into_any_element(),
                );
            }
            Some(Drag::Move { id, from, to }) => {
                if let Some(overlay) = self.doc.read(cx).edit().overlay(*id) {
                    let mut moved = overlay.kind.clone();
                    moved.translate(to[0] - from[0], to[1] - from[1]);
                    out.push(outline(moved.bounds().grow(1.), ring));
                }
            }
            Some(Drag::Resize { id, handle: h, to }) => {
                if let Some(overlay) = self.doc.read(cx).edit().overlay(*id) {
                    let mut kind = overlay.kind.clone();
                    resize_kind(&mut kind, *h, *to);
                    out.push(outline(kind.bounds(), ring));
                }
            }
            _ => {}
        }
        if let Selection::Mark(id) = self.selection
            && !matches!(self.drag, Some(Drag::Move { .. } | Drag::Resize { .. }))
            && let Some(overlay) = self.doc.read(cx).edit().overlay(id)
        {
            match &overlay.kind {
                MarkKind::Arrow { from, to, .. } | MarkKind::Line { from, to, .. } => {
                    out.push(handle(screen(*from)));
                    out.push(handle(screen(*to)));
                }
                kind => {
                    let mut bounds_ = kind.bounds();
                    if let MarkKind::Text { size, text, .. } = kind
                        && text.is_empty()
                    {
                        bounds_.width = size * 3.;
                        bounds_.height = size * 1.25;
                    }
                    out.push(outline(bounds_.grow(2.), ring));
                    if let Some(area) = kind.area() {
                        for h in CORNERS {
                            out.push(handle(screen(h.at(area))));
                        }
                    }
                }
            }
        }
        out
    }

    // Saving and copying.

    /// Asks where, and exports there.
    pub(crate) fn export_to(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        if doc.info().is_none() || doc.job().is_some() {
            return;
        }
        let format = doc.export().format;
        let source = doc.path().to_path_buf();
        let folder = source.parent().map(PathBuf::from).unwrap_or_default();
        let stem = source
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "video".into());
        let name = format!("{stem}-edited.{}", format.extension());
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        let doc = self.doc.clone();
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(mut path))) = chosen.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension(format.extension());
            }
            _ = doc.update(cx, |doc, cx| doc.start_export(path, cx));
        })
        .detach();
    }

    /// The frame shown, as made for the result, onto the clipboard.
    pub(crate) fn copy_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shot = self.doc.read(cx).snapshot(self.position, false, 4096, cx);
        cx.spawn_in(window, async move |_, cx| {
            let result = shot.await;
            _ = cx.update(|window, cx| match result {
                Ok(png) => {
                    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                        ImageFormat::Png,
                        png,
                    )));
                    window.push_notification(Notification::success("Copied the frame."), cx);
                }
                Err(err) => window
                    .push_notification(Notification::error(format!("Not copied: {err:#}")), cx),
            });
        })
        .detach();
    }

    /// Asks where, and saves the frame shown as a PNG.
    pub(crate) fn save_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        let source = doc.path().to_path_buf();
        let shot = doc.snapshot(self.position, false, 8192, cx);
        let folder = source.parent().map(PathBuf::from).unwrap_or_default();
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = format!("{stem}-{:.2}s.png", self.position);
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let result = match shot.await {
                Ok(png) => std::fs::write(&path, png).map_err(anyhow::Error::from),
                Err(err) => Err(err),
            };
            _ = cx.update(|window, cx| {
                let notification = match result {
                    Ok(()) => Notification::success(format!(
                        "Saved {}.",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )),
                    Err(err) => Notification::error(format!("Not saved: {err:#}")),
                };
                window.push_notification(notification, cx);
            });
        })
        .detach();
    }

    /// The title bar's buttons: undo, redo, more, export (or how far an
    /// export has got).
    pub fn title_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let doc = self.doc.read(cx);
        let ready = doc.info().is_some();
        let (can_undo, can_redo) = (doc.can_undo(), doc.can_redo());
        let undo_key = if cfg!(target_os = "macos") {
            "⌘Z"
        } else {
            "Ctrl+Z"
        };
        let editor = cx.weak_entity();
        let more = Button::new("video-more")
            .ghost()
            .small()
            .icon(Icons::Ellipsis)
            .disabled(!ready)
            .tooltip("More")
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                let (copy, save, timeline, reset) = (
                    editor.clone(),
                    editor.clone(),
                    editor.clone(),
                    editor.clone(),
                );
                menu.item(
                    PopupMenuItem::new("Copy this frame").on_click(move |_, window, cx| {
                        _ = copy.update(cx, |this, cx| this.copy_frame(window, cx));
                    }),
                )
                .item(
                    PopupMenuItem::new("Save this frame…").on_click(move |_, window, cx| {
                        _ = save.update(cx, |this, cx| this.save_frame(window, cx));
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Keep all of it again").on_click(move |_, _, cx| {
                        _ = timeline.update(cx, |this, cx| {
                            this.update_timeline("timeline", |edit, _| edit.segments.clear(), cx)
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("Start over").on_click(move |_, window, cx| {
                        _ = reset.update(cx, |this, cx| {
                            this.select(Selection::None, window, cx);
                            this.doc.update(cx, |doc, cx| doc.reset(cx));
                        });
                    }),
                )
            });
        let export: AnyElement = match doc.job() {
            Some(job) => {
                let percent = (job.progress.fraction() * 100.).round() as u32;
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .w(px(120.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(cx.theme().muted)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(cx.theme().primary)
                                    .w(px(120. * job.progress.fraction())),
                            ),
                    )
                    .child(div().text_xs().w(px(34.)).child(format!("{percent}%")))
                    .child(
                        Button::new("video-export-cancel")
                            .ghost()
                            .xsmall()
                            .label("Stop")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.doc.update(cx, |doc, _| doc.cancel_export())
                            })),
                    )
                    .into_any_element()
            }
            None => Button::new("video-export")
                .primary()
                .small()
                .icon(Icons::Download)
                .label("Export…")
                .disabled(!ready)
                .tooltip(if cfg!(target_os = "macos") {
                    "Write the result as a new file (⌘S)"
                } else {
                    "Write the result as a new file (Ctrl+S)"
                })
                .on_click(cx.listener(|this, _, window, cx| this.open_export(window, cx)))
                .into_any_element(),
        };
        h_flex()
            .gap_1()
            .child(
                Button::new("video-undo")
                    .ghost()
                    .small()
                    .icon(Icons::Undo2)
                    .disabled(!can_undo)
                    .tooltip(format!("Undo ({undo_key})"))
                    .on_click(
                        cx.listener(|this, _, _, cx| this.doc.update(cx, |doc, cx| doc.undo(cx))),
                    ),
            )
            .child(
                Button::new("video-redo")
                    .ghost()
                    .small()
                    .icon(Icons::Redo2)
                    .disabled(!can_redo)
                    .tooltip("Redo")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.doc.update(cx, |doc, cx| doc.redo(cx))),
                    ),
            )
            .child(more)
            .child(export)
            .into_any_element()
    }

    fn render_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let current = self.tool;
        v_flex()
            .flex_none()
            .w(px(44.))
            .h_full()
            .py_2()
            .gap_0p5()
            .items_center()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.background)
            .children(TOOLS.iter().enumerate().map(|(ix, tool)| {
                match tool {
                    None => div()
                        .my_1()
                        .w(px(20.))
                        .h(px(1.))
                        .bg(theme.border)
                        .into_any_element(),
                    Some((tool, icon, name, key)) => {
                        let tool = *tool;
                        Button::new(("video-tool", ix))
                            .ghost()
                            .small()
                            .icon(*icon)
                            .selected(current == tool)
                            .tooltip(if key.is_empty() {
                                name.to_string()
                            } else {
                                format!("{name} ({key})")
                            })
                            .accessibility_label(*name)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_tool(tool, cx);
                                this.focus.focus(window, cx);
                            }))
                            .into_any_element()
                    }
                }
            }))
    }
}

/// `kind` with `handle` dragged to `to`.
fn resize_kind(kind: &mut MarkKind, handle: Handle, to: [f32; 2]) {
    match (kind, handle) {
        (MarkKind::Arrow { from, .. } | MarkKind::Line { from, .. }, Handle::From) => *from = to,
        (MarkKind::Arrow { to: end, .. } | MarkKind::Line { to: end, .. }, Handle::To) => *end = to,
        (kind, handle) => {
            if let Some(area) = kind.area() {
                kind.set_area(handle.drag(area, to, None));
            }
        }
    }
}

/// A mark's picture, at `scale` source pixels, cut to what it covers (with
/// room for its shadow): the picture and where it goes (source pixels).
fn sprite(kind: &MarkKind, size: (u32, u32), scale: f32) -> Option<(Frame, Rect)> {
    let full = Rect::new(0., 0., size.0 as f32, size.1 as f32);
    let at = match kind {
        MarkKind::Spotlight { .. } => full,
        kind => {
            let room = kind.stroke().unwrap_or(4.) * 4. + 24.;
            let bounds = match kind {
                // Text's box is laid out by the renderer: room for it.
                MarkKind::Text { size, .. } => kind.bounds().grow(size * 0.6),
                _ => kind.bounds(),
            };
            let grown = bounds.grow(room);
            let x = grown.x.max(0.);
            let y = grown.y.max(0.);
            Rect::new(
                x,
                y,
                (grown.right().min(full.width) - x).max(1.),
                (grown.bottom().min(full.height) - y).max(1.),
            )
        }
    };
    let (width, height) = (
        ((at.width * scale).ceil() as u32).max(1),
        ((at.height * scale).ceil() as u32).max(1),
    );
    let mark = Mark {
        id: 1,
        kind: kind.clone(),
    };
    let layer = crate::render::marks_layer(
        width,
        height,
        &[&mark],
        |p| [(p[0] - at.x) * scale, (p[1] - at.y) * scale],
        scale,
    )?;
    // Back to straight alpha for drawing.
    let mut pixels = layer.take();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for c in &mut pixel[..3] {
                *c = ((*c as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    Some((Frame::from_rgba(width, height, pixels), at))
}

impl Focusable for VideoEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for VideoEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let canvas = self.render_canvas(window, cx);
        let transport = self.render_transport(cx);
        let timeline = self.render_timeline(cx);
        let bar = self.render_bar(cx);
        let status = self.render_status(cx);
        let ready = self.engine().map(Engine::status);
        if let Some(Status::Failed(_)) = ready {
            // The system player cannot play it; stills still show.
        }
        h_flex()
            .size_full()
            .key_context(crate::editor::CONTEXT)
            .on_action(cx.listener(|this, _: &crate::editor::Undo, _, cx| {
                this.doc.update(cx, |doc, cx| doc.undo(cx))
            }))
            .on_action(cx.listener(|this, _: &crate::editor::Redo, _, cx| {
                this.doc.update(cx, |doc, cx| doc.redo(cx))
            }))
            .on_action(
                cx.listener(|this, _: &crate::editor::SaveCopy, window, cx| {
                    this.open_export(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::editor::CopyImage, window, cx| {
                    this.copy_frame(window, cx)
                }),
            )
            .on_key_down(cx.listener(Self::on_key))
            .child(self.render_tools(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(canvas)
                    .child(transport)
                    .child(timeline)
                    .child(bar)
                    .child(status),
            )
    }
}

#[cfg(test)]
mod mark_tests {
    #[test]
    fn marks_made_at_the_end_still_show() {
        assert_eq!(super::mark_time(2., 8.), (2., 5.));
        // At the end: the last three seconds.
        assert_eq!(super::mark_time(8., 8.), (5., 8.));
        assert_eq!(super::mark_time(7., 8.), (5., 8.));
        // A clip shorter than a mark: all of it.
        assert_eq!(super::mark_time(1., 2.), (0., 2.));
    }
}
