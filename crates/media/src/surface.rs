//! A live picture of a device or browser, with an annotation layer.
//!
//! The owner (browser or device panel) pushes frames in with [`SurfaceView::set_frame`]
//! and listens for [`SurfaceEvent`]s: pointer and key input in *frame* pixels,
//! ready to forward to the device or page. In annotate mode the pointer draws
//! shapes instead; in pick mode a click reports a point to inspect.
//!
//! Because PilotHouse draws the frames itself, annotations sit directly on top
//! of the live picture, and a capture includes them at full resolution.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, ImageSource,
    InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement as _, PathBuilder, Pixels, Point,
    Render, RenderImage, ScrollWheelEvent, SharedString, Styled as _, StyledImage as _, Window,
    canvas, div, img, point, px, size,
};

use crate::annotate::{self, Shape, Tool};
use crate::frame::Frame;

pub const SURFACE_CONTEXT: &str = "Surface";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceMode {
    /// Pointer and keys go to the device or page.
    Interact,
    /// The pointer draws with this tool.
    Annotate(Tool),
    /// The next click picks an element to inspect.
    Pick,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerKind {
    Down,
    Up,
    Move,
    /// Wheel delta in frame pixels.
    Scroll {
        dx: f32,
        dy: f32,
    },
}

/// Pointer input, in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePointer {
    pub kind: PointerKind,
    pub x: f32,
    pub y: f32,
    pub right_button: bool,
    pub click_count: usize,
}

#[derive(Debug, Clone)]
pub enum SurfaceEvent {
    Pointer(SurfacePointer),
    Key(Keystroke),
    /// Pick mode: the user clicked here (frame pixels).
    Picked {
        x: f32,
        y: f32,
    },
}

pub struct SurfaceView {
    frame: Option<Frame>,
    /// Images replaced since the last render; freed from the GPU atlas then.
    stale: Vec<Arc<RenderImage>>,
    mode: SurfaceMode,
    shapes: Vec<Shape>,
    drawing: Option<Shape>,
    focus_handle: FocusHandle,
    /// The element's laid-out bounds, recorded during prepaint.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    placeholder: SharedString,
    /// Frames shown in the last second, for the frame-rate readout.
    frame_times: Vec<Instant>,
    pointer_down: bool,
}

impl EventEmitter<SurfaceEvent> for SurfaceView {}

impl SurfaceView {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            frame: None,
            stale: Vec::new(),
            mode: SurfaceMode::Interact,
            shapes: Vec::new(),
            drawing: None,
            focus_handle: cx.focus_handle(),
            bounds: Rc::default(),
            placeholder: placeholder.into(),
            frame_times: Vec::new(),
            pointer_down: false,
        }
    }

    /// Shows a new frame. The previous image's GPU texture is released on the
    /// next render, so a long session does not fill the texture atlas.
    pub fn set_frame(&mut self, frame: Frame, cx: &mut Context<Self>) {
        let new_id = frame.render_image().id;
        if let Some(old) = self.frame.replace(frame)
            && old.render_image().id != new_id
        {
            self.stale.push(old.render_image());
        }
        let now = Instant::now();
        self.frame_times
            .retain(|time| now.duration_since(*time).as_secs_f32() < 1.0);
        self.frame_times.push(now);
        cx.notify();
    }

    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = text.into();
        cx.notify();
    }

    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    /// The pane's size as laid out in the last frame.
    pub fn view_size(&self) -> gpui_kit::Size<Pixels> {
        self.bounds.get().size
    }

    pub fn frames_per_second(&self) -> usize {
        self.frame_times.len()
    }

    pub fn mode(&self) -> SurfaceMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: SurfaceMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.drawing = None;
        cx.notify();
    }

    pub fn has_annotations(&self) -> bool {
        !self.shapes.is_empty()
    }

    /// Adds a mark made by the app, e.g. the box around a picked element.
    pub fn add_annotation(&mut self, shape: Shape, cx: &mut Context<Self>) {
        self.shapes.push(shape);
        cx.notify();
    }

    pub fn undo_annotation(&mut self, cx: &mut Context<Self>) {
        self.shapes.pop();
        cx.notify();
    }

    pub fn clear_annotations(&mut self, cx: &mut Context<Self>) {
        self.shapes.clear();
        cx.notify();
    }

    /// The current frame with the annotations burned in.
    pub fn composite(&self) -> Option<Frame> {
        let frame = self.frame.as_ref()?;
        Some(if self.shapes.is_empty() {
            frame.clone()
        } else {
            annotate::render(frame, &self.shapes)
        })
    }

    /// Where the frame is drawn inside `bounds` (object-fit: contain).
    fn fit(&self) -> Option<Bounds<Pixels>> {
        let frame = self.frame.as_ref()?;
        let bounds = self.bounds.get();
        let (fw, fh) = (frame.width as f32, frame.height as f32);
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if fw <= 0.0 || fh <= 0.0 || bw <= 0.0 || bh <= 0.0 {
            return None;
        }
        let scale = (bw / fw).min(bh / fh);
        let (w, h) = (fw * scale, fh * scale);
        Some(Bounds::new(
            point(
                bounds.origin.x + px((bw - w) / 2.0),
                bounds.origin.y + px((bh - h) / 2.0),
            ),
            size(px(w), px(h)),
        ))
    }

    /// Window position → frame pixels; `None` outside the picture.
    fn to_frame(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let frame = self.frame.as_ref()?;
        let fit = self.fit()?;
        let x =
            f32::from(position.x - fit.origin.x) / f32::from(fit.size.width) * frame.width as f32;
        let y =
            f32::from(position.y - fit.origin.y) / f32::from(fit.size.height) * frame.height as f32;
        (x >= 0.0 && y >= 0.0 && x <= frame.width as f32 && y <= frame.height as f32)
            .then_some((x, y))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let Some((x, y)) = self.to_frame(event.position) else {
            return;
        };
        self.pointer_down = true;
        match self.mode {
            SurfaceMode::Interact => cx.emit(SurfaceEvent::Pointer(SurfacePointer {
                kind: PointerKind::Down,
                x,
                y,
                right_button: event.button == MouseButton::Right,
                click_count: event.click_count,
            })),
            SurfaceMode::Pick => {
                cx.emit(SurfaceEvent::Picked { x, y });
                self.mode = SurfaceMode::Interact;
            }
            SurfaceMode::Annotate(tool) => {
                let at = (x, y);
                match tool {
                    Tool::Pin => {
                        let number = self
                            .shapes
                            .iter()
                            .filter(|shape| matches!(shape, Shape::Pin { .. }))
                            .count() as u32
                            + 1;
                        self.shapes.push(Shape::Pin { at, number });
                    }
                    Tool::Rect => self.drawing = Some(Shape::Rect { from: at, to: at }),
                    Tool::Arrow => self.drawing = Some(Shape::Arrow { from: at, to: at }),
                    Tool::Pen => self.drawing = Some(Shape::Pen { points: vec![at] }),
                }
            }
        }
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((x, y)) = self.to_frame(event.position) else {
            return;
        };
        match (&mut self.drawing, self.mode) {
            (Some(Shape::Rect { to, .. } | Shape::Arrow { to, .. }), _) => *to = (x, y),
            (Some(Shape::Pen { points }), _) => points.push((x, y)),
            (None, SurfaceMode::Interact) if self.pointer_down => {
                cx.emit(SurfaceEvent::Pointer(SurfacePointer {
                    kind: PointerKind::Move,
                    x,
                    y,
                    right_button: false,
                    click_count: 0,
                }));
                return;
            }
            _ => return,
        }
        cx.notify();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let was_down = std::mem::replace(&mut self.pointer_down, false);
        if let Some(shape) = self.drawing.take() {
            self.shapes.push(shape);
            cx.notify();
            return;
        }
        if self.mode == SurfaceMode::Interact && was_down {
            let (x, y) = self.to_frame(event.position).unwrap_or((0.0, 0.0));
            cx.emit(SurfaceEvent::Pointer(SurfacePointer {
                kind: PointerKind::Up,
                x,
                y,
                right_button: event.button == MouseButton::Right,
                click_count: event.click_count,
            }));
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode != SurfaceMode::Interact {
            return;
        }
        let Some((x, y)) = self.to_frame(event.position) else {
            return;
        };
        let (Some(frame), Some(fit)) = (&self.frame, self.fit()) else {
            return;
        };
        let delta = event.delta.pixel_delta(px(20.));
        // Convert the wheel delta from view pixels into frame pixels.
        let scale = frame.width as f32 / f32::from(fit.size.width);
        cx.emit(SurfaceEvent::Pointer(SurfacePointer {
            kind: PointerKind::Scroll {
                dx: -f32::from(delta.x) * scale,
                dy: -f32::from(delta.y) * scale,
            },
            x,
            y,
            right_button: false,
            click_count: 0,
        }));
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // ⌘ shortcuts stay with the app, except the editing ones (copy,
        // paste, cut, select all, undo), which the owner maps for the target.
        let editing = matches!(event.keystroke.key.as_str(), "c" | "v" | "x" | "a" | "z");
        if self.mode != SurfaceMode::Interact || (event.keystroke.modifiers.platform && !editing) {
            return;
        }
        cx.emit(SurfaceEvent::Key(event.keystroke.clone()));
        cx.stop_propagation();
    }
}

impl Focusable for SurfaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SurfaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.stale.drain(..) {
            let _ = window.drop_image(image);
        }
        let bounds_cell = self.bounds.clone();
        let measure = canvas(move |bounds, _, _| bounds_cell.set(bounds), |_, _, _, _| {})
            .absolute()
            .size_full();

        // Annotation overlay, painted in window coordinates over the picture.
        let overlay = self
            .fit()
            .zip(self.frame.as_ref().map(|f| (f.width, f.height)))
            .map(|(fit, (fw, fh))| {
                let mut shapes = self.shapes.clone();
                shapes.extend(self.drawing.clone());
                let to_view = move |(x, y): (f32, f32)| -> Point<Pixels> {
                    point(
                        fit.origin.x + fit.size.width * (x / fw as f32),
                        fit.origin.y + fit.size.height * (y / fh as f32),
                    )
                };
                let view_scale = f32::from(fit.size.width) / fw as f32;
                let color = gpui_kit::rgb(0xff3b30);
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let width = px(3.0);
                        for shape in &shapes {
                            let mut path = PathBuilder::stroke(width);
                            match shape {
                                Shape::Rect { from, to } => {
                                    let corners = [
                                        (from.0, from.1),
                                        (to.0, from.1),
                                        (to.0, to.1),
                                        (from.0, to.1),
                                        (from.0, from.1),
                                    ];
                                    path.move_to(to_view(corners[0]));
                                    for corner in &corners[1..] {
                                        path.line_to(to_view(*corner));
                                    }
                                }
                                Shape::Arrow { from, to } => {
                                    path.move_to(to_view(*from));
                                    path.line_to(to_view(*to));
                                    for head in annotate::arrow_head(
                                        *from,
                                        *to,
                                        14.0 / view_scale.max(0.01),
                                    ) {
                                        path.move_to(to_view(*to));
                                        path.line_to(to_view(head));
                                    }
                                }
                                Shape::Pen { points } => {
                                    let mut points = points.iter();
                                    if let Some(first) = points.next() {
                                        path.move_to(to_view(*first));
                                        for next in points {
                                            path.line_to(to_view(*next));
                                        }
                                    }
                                }
                                Shape::Pin { .. } => continue,
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                )
                .absolute()
                .size_full()
            });
        // Pins are divs, so their numbers use the UI font.
        let pins: Vec<_> = self
            .fit()
            .zip(self.frame.as_ref())
            .map(|(fit, frame)| {
                let bounds = self.bounds.get();
                self.shapes
                    .iter()
                    .filter_map(|shape| match shape {
                        Shape::Pin { at, number } => {
                            let x = fit.origin.x - bounds.origin.x
                                + fit.size.width * (at.0 / frame.width as f32);
                            let y = fit.origin.y - bounds.origin.y
                                + fit.size.height * (at.1 / frame.height as f32);
                            Some(
                                div()
                                    .absolute()
                                    .left(x - px(11.))
                                    .top(y - px(11.))
                                    .size(px(22.))
                                    .rounded_full()
                                    .bg(gpui_kit::rgb(0xff3b30))
                                    .text_color(gpui_kit::white())
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .shadow_md()
                                    .child(number.to_string()),
                            )
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let cursor_crosshair = !matches!(self.mode, SurfaceMode::Interact);
        div()
            .id("surface")
            .key_context(SURFACE_CONTEXT)
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().muted.opacity(0.5))
            .when(cursor_crosshair, |this| this.cursor_crosshair())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_key_down(cx.listener(Self::key_down))
            .child(measure)
            .map(|this| match self.frame.as_ref().map(Frame::render_image) {
                // Absolute, so the picture never sizes the pane (a tall phone
                // frame would otherwise grow it and be cropped).
                Some(image) => this.child(
                    img(ImageSource::Render(image))
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                ),
                None => this.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.placeholder.clone()),
                ),
            })
            .children(overlay)
            .children(pins)
    }
}
