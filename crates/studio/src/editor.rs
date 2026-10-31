//! The image editor: tools on the left, the picture in the middle, its
//! settings on the right.
//!
//! The picture shown is the document's preview; marks, the crop and what is
//! being dragged are drawn over it by the window until the pointer is let
//! go, so a drag never waits for a new preview. Positions under the pointer
//! go through [`Placement`] to pixels of the turned original, where the
//! edit keeps them.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    notification::Notification,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, Entity, FocusHandle,
    Focusable, Hsla, Image, ImageFormat, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement as _,
    PathBuilder, PinchEvent, Pixels, Point, Render, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, Window, actions,
    canvas, div, img, point, px,
};
use ph_ui::{Icons, MenuButton as _};

use crate::document::{ImageDocument, View};
use crate::edit::{Color, ImageEdit, MarkKind, MarkType, Rect};
use crate::encode::Format;

pub(crate) mod bar;
mod export;

actions!(
    studio,
    [
        /// Undo the last change (⌘Z).
        Undo,
        /// Redo it (⇧⌘Z).
        Redo,
        /// Delete the selected mark.
        DeleteMark,
        /// Leave the crop or drop the selection (Esc).
        Cancel,
        /// Save a copy of the result (⌘S).
        SaveCopy,
        /// Copy the result as a picture (⇧⌘C).
        CopyImage,
    ]
);

pub(crate) const CONTEXT: &str = "ImageEditor";

pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        gpui_kit::KeyBinding::new("secondary-z", Undo, context),
        gpui_kit::KeyBinding::new("secondary-shift-z", Redo, context),
        gpui_kit::KeyBinding::new("ctrl-y", Redo, context),
        gpui_kit::KeyBinding::new("backspace", DeleteMark, context),
        gpui_kit::KeyBinding::new("delete", DeleteMark, context),
        gpui_kit::KeyBinding::new("escape", Cancel, context),
        gpui_kit::KeyBinding::new("secondary-s", SaveCopy, context),
        gpui_kit::KeyBinding::new("secondary-shift-c", CopyImage, context),
    ]);
    crate::slides::editor::bind_keys(cx);
}

/// What the pointer does on the picture, and what the bar under it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Crop,
    /// Colors (the pointer selects and moves marks).
    Adjust,
    /// The result's size (the pointer selects and moves marks).
    Resize,
    /// The frame (the pointer selects and moves marks).
    Frame,
    Mark(MarkType),
}

impl Tool {
    /// Whether the pointer selects and moves marks.
    fn picks(self) -> bool {
        matches!(
            self,
            Tool::Select | Tool::Adjust | Tool::Resize | Tool::Frame
        )
    }
}

/// The toolbar, in order: tool, icon, name, key (none when empty). `None`
/// is a divider.
const TOOLS: &[Option<(Tool, Icons, &str, &str)>] = &[
    Some((Tool::Select, Icons::MousePointer2, "Select and move", "V")),
    None,
    Some((Tool::Crop, Icons::Crop, "Crop and turn", "C")),
    Some((Tool::Adjust, Icons::SlidersHorizontal, "Adjust colors", "")),
    Some((Tool::Resize, Icons::Scaling, "Resize", "")),
    Some((
        Tool::Frame,
        Icons::AppWindow,
        "Frame (for docs and slides)",
        "",
    )),
    None,
    Some((
        Tool::Mark(MarkType::Arrow),
        Icons::MoveUpRight,
        "Arrow",
        "A",
    )),
    Some((Tool::Mark(MarkType::Rect), Icons::Square, "Rectangle", "R")),
    Some((Tool::Mark(MarkType::Ellipse), Icons::Circle, "Ellipse", "O")),
    Some((Tool::Mark(MarkType::Line), Icons::Slash, "Line", "L")),
    Some((Tool::Mark(MarkType::Pen), Icons::Pencil, "Pen", "P")),
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
        "S",
    )),
];

/// Colors offered for marks.
pub(crate) const PALETTE: [Color; 8] = [
    Color([255, 59, 48, 255]),
    Color([255, 149, 0, 255]),
    Color([255, 204, 0, 255]),
    Color([52, 199, 89, 255]),
    Color([10, 132, 255, 255]),
    Color([175, 82, 222, 255]),
    Color([255, 255, 255, 255]),
    Color([0, 0, 0, 255]),
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    /// As large as fits, never past its own size.
    Fit,
    Scale(f32),
}

/// Space kept around the picture.
const MARGIN: f32 = 28.;
/// How far the picture zooms out and in.
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 32.;
/// How much of a zoomed picture always stays in sight.
const KEEP_IN_SIGHT: f32 = 64.;
/// How near (in screen pixels) the pointer must be to pick something.
const REACH: f32 = 8.;

/// Where the picture is on screen: its top left corner (window
/// coordinates), screen pixels per pixel of what is shown, and what is
/// shown.
#[derive(Debug, Clone, Copy)]
struct Placement {
    origin: Point<Pixels>,
    scale: f32,
    view: View,
    /// The shown picture's size (the result, or the turned original).
    size: (f32, f32),
}

impl Placement {
    fn on_picture(self, position: Point<Pixels>) -> [f32; 2] {
        [
            f32::from(position.x - self.origin.x) / self.scale,
            f32::from(position.y - self.origin.y) / self.scale,
        ]
    }

    fn on_screen(self, p: [f32; 2]) -> Point<Pixels> {
        point(
            self.origin.x + px(p[0] * self.scale),
            self.origin.y + px(p[1] * self.scale),
        )
    }
}

/// A corner or edge being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    /// A line's start or end.
    From,
    To,
}

pub(crate) const CORNERS: [Handle; 4] = [
    Handle::TopLeft,
    Handle::TopRight,
    Handle::BottomRight,
    Handle::BottomLeft,
];
pub(crate) const EDGES: [Handle; 8] = [
    Handle::TopLeft,
    Handle::Top,
    Handle::TopRight,
    Handle::Right,
    Handle::BottomRight,
    Handle::Bottom,
    Handle::BottomLeft,
    Handle::Left,
];

impl Handle {
    pub(crate) fn at(self, r: Rect) -> [f32; 2] {
        let (cx, cy) = (r.x + r.width / 2., r.y + r.height / 2.);
        match self {
            Handle::TopLeft | Handle::From => [r.x, r.y],
            Handle::Top => [cx, r.y],
            Handle::TopRight => [r.right(), r.y],
            Handle::Right => [r.right(), cy],
            Handle::BottomRight | Handle::To => [r.right(), r.bottom()],
            Handle::Bottom => [cx, r.bottom()],
            Handle::BottomLeft => [r.x, r.bottom()],
            Handle::Left => [r.x, cy],
        }
    }

    /// `r` with this handle dragged to `p`, keeping `aspect` (width over
    /// height) at corners when given.
    pub(crate) fn drag(self, r: Rect, p: [f32; 2], aspect: Option<f32>) -> Rect {
        let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.right(), r.bottom());
        match self {
            Handle::TopLeft => (x0, y0) = (p[0], p[1]),
            Handle::Top => y0 = p[1],
            Handle::TopRight => (x1, y0) = (p[0], p[1]),
            Handle::Right => x1 = p[0],
            Handle::BottomRight => (x1, y1) = (p[0], p[1]),
            Handle::Bottom => y1 = p[1],
            Handle::BottomLeft => (x0, y1) = (p[0], p[1]),
            Handle::Left => x0 = p[0],
            Handle::From | Handle::To => {}
        }
        if let Some(aspect) = aspect.filter(|_| CORNERS.contains(&self)) {
            let width = (x1 - x0).abs().max(1.);
            let height = width / aspect;
            match self {
                Handle::TopLeft | Handle::TopRight => y0 = y1 - height * (y1 - y0).signum(),
                _ => y1 = y0 + height * (y1 - y0).signum(),
            }
        }
        Rect::from_corners([x0, y0], [x1, y1])
    }
}

/// A drag under way, in pixels of the turned original.
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
        /// The crop when the drag began.
        start: Rect,
        handle: Option<Handle>,
        from: [f32; 2],
        to: [f32; 2],
        /// Drawing a new crop rather than changing the one there.
        new: bool,
    },
    Pan {
        from: Point<Pixels>,
        start: (f32, f32),
    },
}

pub struct ImageEditor {
    pub(crate) doc: Entity<ImageDocument>,
    focus: FocusHandle,
    pub(crate) tool: Tool,
    /// The color new marks get.
    pub(crate) color: Color,
    pub(crate) selected: Option<u32>,
    drag: Option<Drag>,
    zoom: Zoom,
    pan: (f32, f32),
    /// The canvas, as last laid out (window coordinates).
    bounds: Rc<Cell<Bounds<Pixels>>>,
    placement: Option<Placement>,
    /// The original is shown while Compare is held.
    comparing: bool,
    /// The proportions a crop keeps (width over height).
    pub(crate) crop_aspect: Option<f32>,
    /// Under the pointer: its pixel in the shown picture and the color.
    hover: Option<([u32; 2], [u8; 4])>,
    pub(crate) controls: bar::Controls,
    _subscriptions: Vec<Subscription>,
}

impl ImageEditor {
    pub fn new(doc: Entity<ImageDocument>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (controls, mut subscriptions) = bar::Controls::new(window, cx);
        subscriptions.push(cx.observe_in(&doc, window, |this, _, window, cx| {
            this.sync_controls(window, cx);
            cx.notify();
        }));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            doc,
            focus,
            tool: Tool::Select,
            color: PALETTE[0],
            selected: None,
            drag: None,
            zoom: Zoom::Fit,
            pan: (0., 0.),
            bounds: Rc::new(Cell::new(Bounds::default())),
            placement: None,
            comparing: false,
            crop_aspect: None,
            hover: None,
            controls,
            _subscriptions: subscriptions,
        }
    }

    pub fn document(&self) -> &Entity<ImageDocument> {
        &self.doc
    }

    pub(crate) fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if self.tool == Tool::Crop && tool != Tool::Crop {
            self.doc
                .update(cx, |doc, cx| doc.set_view(View::Result, cx));
        }
        if tool == Tool::Crop {
            self.selected = None;
            self.doc.update(cx, |doc, cx| doc.set_view(View::Crop, cx));
        }
        self.tool = tool;
        self.drag = None;
        cx.notify();
    }

    pub(crate) fn select(&mut self, id: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        // A text left empty goes when it is let go of.
        if let Some(old) = self.selected.filter(|old| Some(*old) != id) {
            let empty = self.doc.read(cx).edit().mark(old).is_some_and(
                |mark| matches!(&mark.kind, MarkKind::Text { text, .. } if text.trim().is_empty()),
            );
            if empty {
                self.doc.update(cx, |doc, cx| {
                    doc.update(
                        "remove empty text",
                        |edit, _| edit.marks.retain(|m| m.id != old),
                        cx,
                    )
                });
            }
        }
        self.selected = id;
        self.doc.update(cx, |doc, _| doc.settle());
        self.sync_controls(window, cx);
        cx.notify();
    }

    /// Changes the selected mark.
    pub(crate) fn update_selected(
        &mut self,
        what: &'static str,
        change: impl FnOnce(&mut MarkKind),
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected else { return };
        self.doc.update(cx, |doc, cx| {
            doc.update(
                what,
                |edit, _| {
                    if let Some(mark) = edit.mark_mut(id) {
                        change(&mut mark.kind);
                    }
                },
                cx,
            )
        });
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
        self.doc.update(cx, |doc, cx| doc.undo(cx));
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
        self.doc.update(cx, |doc, cx| doc.redo(cx));
    }

    fn delete_mark(&mut self, _: &DeleteMark, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selected {
            self.doc.update(cx, |doc, cx| {
                doc.update("delete", |edit, _| edit.marks.retain(|m| m.id != id), cx)
            });
            self.select(None, window, cx);
        }
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        } else if self.tool != Tool::Select {
            self.set_tool(Tool::Select, cx);
        } else {
            self.select(None, window, cx);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Only on the picture: letters typed in a field are text.
        if !self.focus.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.secondary() || keystroke.modifiers.alt || keystroke.modifiers.control
        {
            return;
        }
        let key = keystroke.key.to_ascii_uppercase();
        if let Some((tool, ..)) = TOOLS
            .iter()
            .flatten()
            .find(|(_, _, _, shortcut)| !shortcut.is_empty() && *shortcut == key)
        {
            self.set_tool(*tool, cx);
            cx.stop_propagation();
        }
    }

    /// Shows the original while Compare is held.
    pub(crate) fn set_comparing(&mut self, comparing: bool, cx: &mut Context<Self>) {
        if self.comparing != comparing {
            self.comparing = comparing;
            cx.notify();
        }
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom_at(factor, None, cx);
    }

    /// Zooms by `factor`, keeping the point under `anchor` (the canvas's
    /// middle when `None`) where it is.
    fn zoom_at(&mut self, factor: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let Some(placement) = self.placement else {
            return;
        };
        let bounds = self.bounds.get();
        let scale = (placement.scale * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let anchor = anchor.unwrap_or_else(|| bounds.center());
        let under = placement.on_picture(anchor);
        // The picture's corner that keeps `under` at `anchor`, as a pan
        // away from centered.
        let corner = (
            f32::from(anchor.x) - under[0] * scale,
            f32::from(anchor.y) - under[1] * scale,
        );
        self.pan = (
            corner.0
                - f32::from(bounds.origin.x)
                - (f32::from(bounds.size.width) - placement.size.0 * scale) / 2.,
            corner.1
                - f32::from(bounds.origin.y)
                - (f32::from(bounds.size.height) - placement.size.1 * scale) / 2.,
        );
        self.zoom = Zoom::Scale(scale);
        cx.notify();
    }

    /// Two fingers pinched on the trackpad.
    fn on_pinch(&mut self, event: &PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.delta != 0. {
            self.zoom_at(1. + event.delta, Some(event.position), cx);
        }
        cx.stop_propagation();
    }

    fn fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = Zoom::Fit;
        self.pan = (0., 0.);
        cx.notify();
    }

    fn on_wheel(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(window.line_height());
        if event.modifiers.secondary() {
            // ⌘ and the wheel, or Ctrl and a pinch on Windows touchpads.
            let dy = f32::from(delta.y);
            if dy != 0. {
                self.zoom_at(
                    (dy * 0.004).clamp(-0.4, 0.4).exp(),
                    Some(event.position),
                    cx,
                );
            }
        } else if matches!(self.zoom, Zoom::Scale(_)) {
            self.pan.0 += f32::from(delta.x);
            self.pan.1 += f32::from(delta.y);
            cx.notify();
        }
        cx.stop_propagation();
    }

    /// The turned original's size, and where its point `p` is shown.
    fn shown_of(&self, p: [f32; 2], placement: &Placement, cx: &App) -> [f32; 2] {
        match placement.view {
            View::Crop => p,
            View::Result => {
                let doc = self.doc.read(cx);
                doc.edit().to_result(doc.size(), p)
            }
        }
    }

    fn original_of(&self, shown: [f32; 2], placement: &Placement, cx: &App) -> [f32; 2] {
        match placement.view {
            View::Crop => shown,
            View::Result => {
                let doc = self.doc.read(cx);
                doc.edit().from_result(doc.size(), shown)
            }
        }
    }

    /// Original pixels per screen pixel along each axis, for reach.
    fn reach(&self, placement: &Placement, cx: &App) -> f32 {
        let per_shown = match placement.view {
            View::Crop => 1.,
            View::Result => {
                let doc = self.doc.read(cx);
                let (sx, sy) = doc.edit().scale(doc.size());
                1. / ((sx + sy) / 2.)
            }
        };
        REACH / placement.scale * per_shown
    }

    /// The mark under `p` (topmost first).
    fn mark_at(&self, p: [f32; 2], reach: f32, cx: &App) -> Option<u32> {
        let edit = self.doc.read(cx).edit();
        edit.marks.iter().rev().find_map(|mark| {
            let hit = match &mark.kind {
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
                    // An outline is picked on its line, so what is inside
                    // stays reachable.
                    let bounds = mark.kind.bounds();
                    bounds.grow(reach + stroke).contains(p)
                        && !bounds.grow(-(reach + stroke)).contains(p)
                }
                kind => kind.bounds().grow(reach).contains(p),
            };
            hit.then_some(mark.id)
        })
    }

    /// The handle of the selected mark under `p`.
    fn handle_at(&self, p: [f32; 2], reach: f32, cx: &App) -> Option<Handle> {
        let mark = self.doc.read(cx).edit().mark(self.selected?)?;
        let near =
            |q: [f32; 2]| (q[0] - p[0]).abs() <= reach * 1.5 && (q[1] - p[1]).abs() <= reach * 1.5;
        match &mark.kind {
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

    /// The crop as it is (all of the turned original when none).
    fn crop_rect(&self, cx: &App) -> Rect {
        let doc = self.doc.read(cx);
        doc.edit().crop_rect(doc.size())
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let Some(placement) = self.placement else {
            return;
        };
        if self.doc.read(cx).original().is_none() {
            return;
        }
        let shown = placement.on_picture(event.position);
        let p = self.original_of(shown, &placement, cx);
        let reach = self.reach(&placement, cx);
        let zoomed = matches!(self.zoom, Zoom::Scale(_));
        let outside = shown[0] < 0.
            || shown[1] < 0.
            || shown[0] > placement.size.0
            || shown[1] > placement.size.1;
        match self.tool {
            Tool::Select | Tool::Adjust | Tool::Resize | Tool::Frame => {
                if let Some((id, handle)) = self.selected.zip(self.handle_at(p, reach, cx)) {
                    self.drag = Some(Drag::Resize { id, handle, to: p });
                } else if let Some(id) = self.mark_at(p, reach, cx) {
                    if self.selected != Some(id) {
                        self.select(Some(id), window, cx);
                    }
                    self.drag = Some(Drag::Move { id, from: p, to: p });
                } else {
                    self.select(None, window, cx);
                    if zoomed {
                        self.drag = Some(Drag::Pan {
                            from: event.position,
                            start: self.pan,
                        });
                    }
                }
            }
            Tool::Crop => {
                let crop = self.crop_rect(cx);
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
            // Marks start on the picture only.
            Tool::Mark(_) if outside => {}
            Tool::Mark(MarkType::Text) => {
                let kind = MarkKind::Text {
                    x: p[0],
                    y: p[1],
                    text: String::new(),
                    size: 0.,
                    color: self.color,
                    background: None,
                    bold: true,
                    align: crate::edit::Align::Left,
                };
                self.add_mark(kind, window, cx);
                self.set_tool(Tool::Select, cx);
                // After this press is done: it focuses the picture.
                cx.on_next_frame(window, |this, window, cx| {
                    this.controls.focus_text(window, cx)
                });
            }
            Tool::Mark(MarkType::Step) => {
                self.add_mark(MarkType::Step.mark(p, p, self.color), window, cx);
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

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(placement) = self.placement else {
            return;
        };
        let shown = placement.on_picture(event.position);
        self.hover = self.color_at(shown, &placement, cx);
        if event.pressed_button != Some(MouseButton::Left) {
            if self.drag.take().is_some() {
                cx.notify();
            }
            cx.notify();
            return;
        }
        let p = self.original_of(shown, &placement, cx);
        let reach = self.reach(&placement, cx);
        match &mut self.drag {
            Some(Drag::Create { to, points, .. }) => {
                *to = p;
                // Points closer than a couple of screen pixels add nothing.
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
            Some(Drag::Pan { from, start }) => {
                self.pan = (
                    start.0 + f32::from(event.position.x - from.x),
                    start.1 + f32::from(event.position.y - from.y),
                );
            }
            None => {}
        }
        cx.notify();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        let Some(placement) = self.placement else {
            return;
        };
        let reach = self.reach(&placement, cx);
        match drag {
            Drag::Create {
                kind,
                from,
                to,
                points,
            } => {
                let tiny = (to[0] - from[0]).hypot(to[1] - from[1]) < reach;
                if tiny {
                    cx.notify();
                    return;
                }
                let mark = match kind {
                    MarkType::Pen => MarkKind::Pen {
                        points,
                        color: self.color,
                        stroke: 0.,
                    },
                    kind => kind.mark(from, to, self.color),
                };
                self.add_mark(mark, window, cx);
            }
            Drag::Move { id, from, to } => {
                let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
                if dx != 0. || dy != 0. {
                    self.doc.update(cx, |doc, cx| {
                        doc.settle();
                        doc.update(
                            "move",
                            |edit, _| {
                                if let Some(mark) = edit.mark_mut(id) {
                                    mark.kind.translate(dx, dy);
                                }
                            },
                            cx,
                        );
                        doc.settle();
                    });
                }
            }
            Drag::Resize { id, handle, to } => {
                self.doc.update(cx, |doc, cx| {
                    doc.settle();
                    doc.update(
                        "resize mark",
                        |edit, _| resize_mark(edit, id, handle, to),
                        cx,
                    );
                    doc.settle();
                });
            }
            Drag::Crop { .. } => {
                let crop = self.dragged_crop(&drag, cx);
                if let Some(crop) = crop.filter(|crop| crop.width >= 4. && crop.height >= 4.) {
                    self.doc.update(cx, |doc, cx| {
                        doc.settle();
                        doc.update("crop", |edit, _| edit.crop = Some(crop), cx);
                        doc.settle();
                    });
                }
            }
            Drag::Pan { .. } => {}
        }
        cx.notify();
    }

    fn add_mark(&mut self, kind: MarkKind, window: &mut Window, cx: &mut Context<Self>) {
        let mut added = None;
        self.doc.update(cx, |doc, cx| {
            doc.settle();
            doc.update(
                "add mark",
                |edit, size| added = edit.add_mark(kind, size).ok(),
                cx,
            );
            doc.settle();
        });
        if added.is_some() {
            self.select(added, window, cx);
        }
    }

    /// The crop a crop drag makes, clamped to the picture.
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
        let doc = self.doc.read(cx);
        let (width, height) = doc.edit().turned_size(doc.size());
        let rect = if *new {
            let mut to = *to;
            if let Some(aspect) = self.crop_aspect {
                let w = to[0] - from[0];
                let h = w.abs() / aspect;
                to[1] = from[1] + h * if to[1] < from[1] { -1. } else { 1. };
            }
            Rect::from_corners(*from, to)
        } else if let Some(handle) = handle {
            handle.drag(*start, *to, self.crop_aspect)
        } else {
            let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
            Rect::new(
                (start.x + dx).clamp(0., width as f32 - start.width),
                (start.y + dy).clamp(0., height as f32 - start.height),
                start.width,
                start.height,
            )
        };
        Some(rect.clamped(width as f32, height as f32))
    }

    /// The pixel under `shown` and its color, from the preview.
    fn color_at(
        &self,
        shown: [f32; 2],
        placement: &Placement,
        cx: &App,
    ) -> Option<([u32; 2], [u8; 4])> {
        let preview = self.doc.read(cx).preview()?;
        if shown[0] < 0.
            || shown[1] < 0.
            || shown[0] >= placement.size.0
            || shown[1] >= placement.size.1
        {
            return None;
        }
        let (pw, ph) = preview.image.dimensions();
        let x = (shown[0] / placement.size.0 * pw as f32) as u32;
        let y = (shown[1] / placement.size.1 * ph as f32) as u32;
        let color = preview.image.get_pixel(x.min(pw - 1), y.min(ph - 1)).0;
        Some(([shown[0] as u32, shown[1] as u32], color))
    }

    // Saving.

    /// Asks where, and saves a copy of the result there.
    pub(crate) fn save_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        if doc.original().is_none() {
            return;
        }
        let settings = doc.export().clone();
        let source = doc.path().to_path_buf();
        let folder = source.parent().map(PathBuf::from).unwrap_or_default();
        let stem = source
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "picture".into());
        let name = format!("{stem}-edited.{}", settings.format.extension());
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        let doc = self.doc.clone();
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(mut path))) = chosen.await else {
                return;
            };
            let mut settings = settings;
            match path
                .extension()
                .and_then(|extension| Format::from_extension(&extension.to_string_lossy()))
            {
                Some(format) => settings.format = format,
                None => {
                    path.set_extension(settings.format.extension());
                }
            }
            let saving = doc.update(cx, |doc, cx| doc.save(path.clone(), settings, true, cx));
            let result = saving.await;
            _ = cx.update(|window, cx| {
                let notification = match result {
                    Ok(encoded) => Notification::success(format!(
                        "Saved {} ({}).",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        crate::control::kilobytes(encoded.bytes.len())
                    )),
                    Err(err) => Notification::error(format!("Not saved: {err:#}")),
                };
                window.push_notification(notification, cx);
            });
        })
        .detach();
    }

    /// Writes the result over the original, once the user confirms.
    pub(crate) fn replace_original(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .doc
            .read(cx)
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let answer = window.prompt(
            gpui_kit::PromptLevel::Warning,
            &format!("Replace {name} with the result?"),
            Some("The file is overwritten; its old pixels are gone unless it is in Git."),
            &["Replace", "Cancel"],
            cx,
        );
        let doc = self.doc.clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let saving = doc.update(cx, |doc, cx| doc.replace_original(cx));
            let result = saving.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.select(None, window, cx);
                let notification = match result {
                    Ok(_) => Notification::success(format!("Replaced {name}.")),
                    Err(err) => Notification::error(format!("Not saved: {err:#}")),
                };
                window.push_notification(notification, cx);
            });
        })
        .detach();
    }

    fn save_action(&mut self, _: &SaveCopy, window: &mut Window, cx: &mut Context<Self>) {
        self.open_export(window, cx);
    }

    /// Puts the result on the clipboard as a PNG.
    pub(crate) fn copy_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let made = self.doc.read(cx).result(cx);
        cx.spawn_in(window, async move |_, cx| {
            let png = async {
                let image = made.await?;
                let mut bytes = Vec::new();
                image::ImageEncoder::write_image(
                    image::codecs::png::PngEncoder::new(&mut bytes),
                    &image,
                    image.width(),
                    image.height(),
                    image::ExtendedColorType::Rgba8,
                )?;
                anyhow::Ok(bytes)
            };
            let result = cx.background_spawn(png).await;
            _ = cx.update(|window, cx| match result {
                Ok(bytes) => {
                    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                        ImageFormat::Png,
                        bytes,
                    )));
                    window.push_notification(Notification::success("Copied the picture."), cx);
                }
                Err(err) => window
                    .push_notification(Notification::error(format!("Not copied: {err:#}")), cx),
            });
        })
        .detach();
    }

    fn copy_action(&mut self, _: &CopyImage, window: &mut Window, cx: &mut Context<Self>) {
        self.copy_image(window, cx);
    }

    /// Asks for a folder and writes an app icon set of the result there.
    pub(crate) fn make_icons(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Make icons here".into()),
        });
        let made = self.doc.read(cx).result(cx);
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(parent) = paths.into_iter().next() else {
                return;
            };
            let result = cx
                .background_spawn(async move {
                    let image = made.await?;
                    crate::icons::write_icon_set(&image, &parent)
                })
                .await;
            _ = cx.update(|window, cx| match result {
                Ok(folder) => {
                    window.push_notification(
                        Notification::success(format!(
                            "Made app icons in {}.",
                            folder.file_name().unwrap_or_default().to_string_lossy()
                        )),
                        cx,
                    );
                    cx.reveal_path(&folder);
                }
                Err(err) => window
                    .push_notification(Notification::error(format!("No icons made: {err:#}")), cx),
            });
        })
        .detach();
    }

    // Drawing.

    /// The buttons for the window's title bar: undo, redo, compare, more,
    /// export.
    pub fn title_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let doc = self.doc.read(cx);
        let ready = doc.original().is_some();
        let (can_undo, can_redo, edited) =
            (doc.can_undo(), doc.can_redo(), !doc.edit().is_unchanged());
        let (undo_key, export_key) = if cfg!(target_os = "macos") {
            ("⌘Z", "⌘S")
        } else {
            ("Ctrl+Z", "Ctrl+S")
        };
        let editor = cx.weak_entity();
        let more = Button::new("studio-more")
            .ghost()
            .small()
            .icon(Icons::Ellipsis)
            .disabled(!ready)
            .tooltip("More")
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                let (copy, icons, replace, reset) = (
                    editor.clone(),
                    editor.clone(),
                    editor.clone(),
                    editor.clone(),
                );
                menu.item(
                    PopupMenuItem::new("Copy the picture").on_click(move |_, window, cx| {
                        _ = copy.update(cx, |this, cx| this.copy_image(window, cx));
                    }),
                )
                .item(
                    PopupMenuItem::new("Make app icons…").on_click(move |_, window, cx| {
                        _ = icons.update(cx, |this, cx| this.make_icons(window, cx));
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Start over").on_click(move |_, window, cx| {
                        _ = reset.update(cx, |this, cx| {
                            this.select(None, window, cx);
                            this.doc.update(cx, |doc, cx| doc.reset(cx));
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("Replace the original…").on_click(move |_, window, cx| {
                        _ = replace.update(cx, |this, cx| this.replace_original(window, cx));
                    }),
                )
            });
        h_flex()
            .gap_1()
            .child(
                Button::new("studio-undo")
                    .ghost()
                    .small()
                    .icon(Icons::Undo2)
                    .disabled(!can_undo)
                    .tooltip(format!("Undo ({undo_key})"))
                    .on_click(cx.listener(|this, _, window, cx| this.undo(&Undo, window, cx))),
            )
            .child(
                Button::new("studio-redo")
                    .ghost()
                    .small()
                    .icon(Icons::Redo2)
                    .disabled(!can_redo)
                    .tooltip("Redo")
                    .on_click(cx.listener(|this, _, window, cx| this.redo(&Redo, window, cx))),
            )
            .child(
                div()
                    .id("studio-compare")
                    .child(
                        Button::new("studio-compare-button")
                            .ghost()
                            .small()
                            .icon(Icons::Eye)
                            .disabled(!edited)
                            .selected(self.comparing),
                    )
                    .tooltip(|window, cx| {
                        Tooltip::new("Hold to see the original").build(window, cx)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.set_comparing(true, cx)),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.set_comparing(false, cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.set_comparing(false, cx)),
                    ),
            )
            .child(more)
            .child(
                Button::new("studio-export")
                    .primary()
                    .small()
                    .icon(Icons::Download)
                    .label("Export…")
                    .disabled(!ready)
                    .tooltip(format!("Save the result as a new file ({export_key})"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_export(window, cx))),
            )
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
                        Button::new(("studio-tool", ix))
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

    /// Lays the picture out in `bounds` (the canvas), as zoomed.
    fn place(&self, bounds: Bounds<Pixels>, cx: &App) -> Option<Placement> {
        let doc = self.doc.read(cx);
        let original = doc.original()?;
        let edit = doc.edit();
        let view = if self.comparing {
            View::Result
        } else {
            doc.view()
        };
        let size = match (self.comparing, view) {
            (true, _) => (original.size.0 as f32, original.size.1 as f32),
            (false, View::Crop) => {
                let (w, h) = edit.turned_size(original.size);
                (w as f32, h as f32)
            }
            (false, View::Result) => {
                let (w, h) = edit.result_size(original.size);
                (w as f32, h as f32)
            }
        };
        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if width <= 0. || height <= 0. {
            return None;
        }
        let scale = match self.zoom {
            Zoom::Fit => ((width - 2. * MARGIN) / size.0)
                .min((height - 2. * MARGIN) / size.1)
                .clamp(0.01, 1.),
            Zoom::Scale(scale) => scale,
        };
        let pan = if self.zoom == Zoom::Fit {
            (0., 0.)
        } else {
            // Never lost off the edge.
            let reach =
                |canvas: f32, picture: f32| ((canvas + picture) / 2. - KEEP_IN_SIGHT).max(0.);
            (
                self.pan
                    .0
                    .clamp(-reach(width, size.0 * scale), reach(width, size.0 * scale)),
                self.pan.1.clamp(
                    -reach(height, size.1 * scale),
                    reach(height, size.1 * scale),
                ),
            )
        };
        let origin = point(
            bounds.origin.x + px((width - size.0 * scale) / 2. + pan.0),
            bounds.origin.y + px((height - size.1 * scale) / 2. + pan.1),
        );
        Some(Placement {
            origin,
            scale,
            view,
            size,
        })
    }

    fn render_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let bounds_cell = self.bounds.clone();
        // The picture is placed from the canvas's size as last laid out: a
        // new size (the window, the chat opening or closing) draws again
        // at once.
        let measure = canvas(
            move |bounds, window, _| {
                if bounds_cell.get() != bounds {
                    bounds_cell.set(bounds);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let bounds = self.bounds.get();
        self.placement = self.place(bounds, cx);
        let doc = self.doc.read(cx);
        let body: AnyElement = match (doc.error(), self.placement, doc.preview()) {
            (Some(error), _, _) => crate::editor::notice(error.clone(), cx),
            (None, Some(placement), Some(preview)) => {
                let image = if self.comparing {
                    doc.original().map(|original| original.frame.render_image())
                } else {
                    Some(preview.frame.render_image())
                };
                let local = |p: Point<Pixels>| p - bounds.origin;
                let at = local(placement.origin);
                let picture = div()
                    .absolute()
                    .left(at.x)
                    .top(at.y)
                    .w(px(placement.size.0 * placement.scale))
                    .h(px(placement.size.1 * placement.scale))
                    .bg(theme.secondary)
                    .shadow_md()
                    .children(
                        image.map(|image| img(image).size_full().object_fit(ObjectFit::Fill)),
                    );
                div()
                    .size_full()
                    .child(picture)
                    .when(!self.comparing, |this| {
                        this.children(self.render_overlay(&placement, bounds, cx))
                    })
                    .into_any_element()
            }
            _ => crate::editor::notice("Opening…", cx),
        };
        let stale = self.doc.update(cx, |doc, _| doc.take_stale());
        for image in stale {
            let _ = window.drop_image(image);
        }
        div()
            .id("studio-canvas")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(theme.muted.opacity(0.5))
            .track_focus(&self.focus)
            .cursor(match self.tool {
                Tool::Select | Tool::Adjust | Tool::Resize | Tool::Frame => {
                    gpui_kit::CursorStyle::Arrow
                }
                Tool::Mark(MarkType::Text) => gpui_kit::CursorStyle::IBeam,
                _ => gpui_kit::CursorStyle::Crosshair,
            })
            .child(measure)
            .child(body)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_wheel))
            .on_pinch(cx.listener(Self::on_pinch))
            .child(self.render_zoom(cx))
            .into_any_element()
    }

    /// The selection, the crop and the drag under way, over the picture.
    fn render_overlay(
        &self,
        placement: &Placement,
        bounds: Bounds<Pixels>,
        cx: &App,
    ) -> Vec<AnyElement> {
        let theme = cx.theme();
        let ring: Hsla = theme.ring;
        let mut out = Vec::new();
        let screen =
            |p: [f32; 2]| placement.on_screen(self.shown_of(p, placement, cx)) - bounds.origin;
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

        if self.tool == Tool::Crop {
            let crop = self
                .drag
                .as_ref()
                .and_then(|drag| self.dragged_crop(drag, cx))
                .unwrap_or_else(|| self.crop_rect(cx));
            let (x, y, w, h) = screen_rect(crop);
            let image = (
                placement.origin - bounds.origin,
                px(placement.size.0 * placement.scale),
                px(placement.size.1 * placement.scale),
            );
            let shade = gpui_kit::black().opacity(0.55);
            let (ix, iy, iw, ih) = (image.0.x, image.0.y, image.1, image.2);
            // Outside the crop, dimmed: above, below, left, right.
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
                    // Thirds, to frame by.
                    .child(
                        div()
                            .absolute()
                            .left(w / 3.)
                            .top_0()
                            .h_full()
                            .w(px(1.))
                            .bg(gpui_kit::white().opacity(0.35)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(w * 2. / 3.)
                            .top_0()
                            .h_full()
                            .w(px(1.))
                            .bg(gpui_kit::white().opacity(0.35)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(h / 3.)
                            .left_0()
                            .w_full()
                            .h(px(1.))
                            .bg(gpui_kit::white().opacity(0.35)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(h * 2. / 3.)
                            .left_0()
                            .w_full()
                            .h(px(1.))
                            .bg(gpui_kit::white().opacity(0.35)),
                    )
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
            Some(Drag::Create {
                kind,
                from,
                to,
                points,
            }) => {
                let color = to_hsla(self.color);
                match kind {
                    MarkType::Arrow | MarkType::Line | MarkType::Pen => {
                        let line: Vec<Point<Pixels>> = if *kind == MarkType::Pen {
                            points.iter().map(|p| screen(*p) + bounds.origin).collect()
                        } else {
                            vec![screen(*from) + bounds.origin, screen(*to) + bounds.origin]
                        };
                        out.push(
                            canvas(
                                |_, _, _| {},
                                move |_, _, window, _| {
                                    let mut path = PathBuilder::stroke(px(2.5));
                                    if let Some(first) = line.first() {
                                        path.move_to(*first);
                                        for p in &line[1..] {
                                            path.line_to(*p);
                                        }
                                    }
                                    if let Ok(path) = path.build() {
                                        window.paint_path(path, color);
                                    }
                                },
                            )
                            .absolute()
                            .size_full()
                            .into_any_element(),
                        );
                    }
                    kind => {
                        let (x, y, w, h) = screen_rect(Rect::from_corners(*from, *to));
                        let hides = matches!(
                            kind,
                            MarkType::Blur | MarkType::Pixelate | MarkType::Spotlight
                        );
                        out.push(
                            div()
                                .absolute()
                                .left(x)
                                .top(y)
                                .w(w)
                                .h(h)
                                .border_2()
                                .border_color(if hides { ring } else { color })
                                .when(*kind == MarkType::Ellipse, |this| this.rounded_full())
                                .bg(if hides {
                                    ring.opacity(0.15)
                                } else {
                                    color.opacity(0.08)
                                })
                                .into_any_element(),
                        );
                    }
                }
            }
            Some(Drag::Move { id, from, to }) => {
                if let Some(mark) = self.doc.read(cx).edit().mark(*id) {
                    let mut moved = mark.kind.clone();
                    moved.translate(to[0] - from[0], to[1] - from[1]);
                    out.push(outline(moved.bounds().grow(1.), ring));
                }
            }
            Some(Drag::Resize { id, handle: h, to }) => {
                let mut edit = self.doc.read(cx).edit().clone();
                resize_mark(&mut edit, *id, *h, *to);
                if let Some(mark) = edit.mark(*id) {
                    match &mark.kind {
                        MarkKind::Arrow { from, to, .. } | MarkKind::Line { from, to, .. } => {
                            let line = [screen(*from) + bounds.origin, screen(*to) + bounds.origin];
                            out.push(
                                canvas(
                                    |_, _, _| {},
                                    move |_, _, window, _| {
                                        let mut path = PathBuilder::stroke(px(2.));
                                        path.move_to(line[0]);
                                        path.line_to(line[1]);
                                        if let Ok(path) = path.build() {
                                            window.paint_path(path, ring);
                                        }
                                    },
                                )
                                .absolute()
                                .size_full()
                                .into_any_element(),
                            );
                        }
                        kind => out.push(outline(kind.bounds(), ring)),
                    }
                }
            }
            _ => {}
        }

        // The selection, with its handles.
        if !matches!(self.drag, Some(Drag::Move { .. } | Drag::Resize { .. }))
            && let Some(mark) = self
                .selected
                .and_then(|id| self.doc.read(cx).edit().mark(id))
        {
            match &mark.kind {
                MarkKind::Arrow { from, to, .. } | MarkKind::Line { from, to, .. } => {
                    out.push(handle(screen(*from)));
                    out.push(handle(screen(*to)));
                }
                kind => {
                    let mut bounds = kind.bounds();
                    if let MarkKind::Text { size, text, .. } = kind
                        && text.is_empty()
                    {
                        bounds.width = size * 3.;
                        bounds.height = size * 1.25;
                    }
                    out.push(outline(bounds.grow(2.), ring));
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

    fn render_zoom(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let percent = format!("{:.0}%", self.placement.map_or(100., |p| p.scale * 100.));
        h_flex()
            .absolute()
            .bottom_2()
            .right_2()
            .gap_0p5()
            .p_0p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_sm()
            .child(
                Button::new("studio-zoom-out")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Minus)
                    .tooltip("Zoom out (⌘ and scroll)")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1. / 1.25, cx))),
            )
            .child(
                Button::new("studio-zoom-actual")
                    .ghost()
                    .xsmall()
                    .label(percent)
                    .tooltip("Actual size")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.zoom = Zoom::Scale(1.);
                        this.pan = (0., 0.);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("studio-zoom-in")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Plus)
                    .tooltip("Zoom in (⌘ and scroll)")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.25, cx))),
            )
            .child(
                Button::new("studio-zoom-fit")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Minimize)
                    .tooltip("Fit")
                    .selected(self.zoom == Zoom::Fit)
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
    }

    /// The line under the bar: the original, the result's size, and the
    /// pixel under the pointer.
    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let doc = self.doc.read(cx);
        let mut parts: Vec<AnyElement> = Vec::new();
        if let Some(original) = doc.original() {
            let format = original.format.map_or("", |format| format.label());
            parts.push(
                div()
                    .child(format!(
                        "{} × {} {format} · {}",
                        original.size.0,
                        original.size.1,
                        crate::control::kilobytes(original.bytes as usize)
                    ))
                    .into_any_element(),
            );
            let (width, height) = doc.edit().result_size(original.size);
            if (width, height) != original.size || !doc.edit().is_unchanged() {
                parts.push(div().opacity(0.6).child("→").into_any_element());
                parts.push(
                    div()
                        .text_color(theme.foreground)
                        .child(format!("{width} × {height}"))
                        .into_any_element(),
                );
            }
        }
        h_flex()
            .flex_none()
            .h_8()
            .px_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .children(parts)
            .child(div().flex_1())
            .when_some(self.hover, |this, ([x, y], [r, g, b, a])| {
                let hex = if a == 255 {
                    format!("#{r:02x}{g:02x}{b:02x}")
                } else {
                    format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
                };
                this.child(format!("{x}, {y}")).child(
                    h_flex()
                        .gap_1()
                        .child(
                            div()
                                .size(px(10.))
                                .rounded(px(2.))
                                .border_1()
                                .border_color(theme.border)
                                .bg(gpui_kit::Rgba {
                                    r: r as f32 / 255.,
                                    g: g as f32 / 255.,
                                    b: b as f32 / 255.,
                                    a: a as f32 / 255.,
                                }),
                        )
                        .child(hex),
                )
            })
    }
}

/// How far `p` is from the segment `a`–`b`.
pub(crate) fn near_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx * dx + dy * dy;
    let t = if length == 0. {
        0.
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length).clamp(0., 1.)
    };
    (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
}

/// Mark `id` with its `handle` dragged to `to`.
fn resize_mark(edit: &mut ImageEdit, id: u32, handle: Handle, to: [f32; 2]) {
    let Some(mark) = edit.mark_mut(id) else {
        return;
    };
    match (&mut mark.kind, handle) {
        (MarkKind::Arrow { from, .. } | MarkKind::Line { from, .. }, Handle::From) => *from = to,
        (MarkKind::Arrow { to: end, .. } | MarkKind::Line { to: end, .. }, Handle::To) => *end = to,
        (kind, handle) => {
            if let Some(area) = kind.area() {
                kind.set_area(handle.drag(area, to, None));
            }
        }
    }
}

pub(crate) fn to_hsla(color: Color) -> Hsla {
    let [r, g, b, a] = color.0;
    gpui_kit::Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a: a as f32 / 255.,
    }
    .into()
}

/// A message filling the canvas.
pub(crate) fn notice(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

impl Focusable for ImageEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ImageEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let canvas = self.render_canvas(window, cx);
        let bar = self.render_bar(cx);
        h_flex()
            .size_full()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::delete_mark))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::save_action))
            .on_action(cx.listener(Self::copy_action))
            .on_key_down(cx.listener(Self::on_key))
            .child(self.render_tools(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(div().flex_1().min_h_0().flex().child(canvas))
                    .child(bar)
                    .child(self.render_status(cx)),
            )
    }
}
