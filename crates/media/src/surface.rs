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
    Render, RenderImage, ScrollEvent, SharedString, Styled as _, StyledImage as _, Window, canvas,
    div, img, point, px, size,
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
    /// A mark of the owner's (the outline of a picked element): drawn and
    /// captured with the user's marks. Clearing the marks takes it too, and
    /// so does the next press on the device or page, which it would no
    /// longer fit.
    highlight: Option<Shape>,
    drawing: Option<Shape>,
    focus_handle: FocusHandle,
    /// The element's laid-out bounds, recorded during prepaint.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    placeholder: SharedString,
    /// Frames shown in the last second, for the frame-rate readout.
    frame_times: Vec<Instant>,
    /// The press being forwarded to the device or page, until it is let go.
    press: Option<Press>,
}

#[derive(Clone, Copy)]
struct Press {
    /// Where the pointer last was inside the picture (frame pixels): a
    /// press let go anywhere else ends here.
    at: (f32, f32),
    right_button: bool,
}

impl EventEmitter<SurfaceEvent> for SurfaceView {}

/// Whether `keystroke` is copy, paste, cut, select all or undo: C, V, X, A
/// or Z with the system's command key (⌘ on macOS, Ctrl on Windows and
/// Linux). A [`SurfaceEvent::Key`] that is one is for the owner to map
/// through the system clipboard; any other goes to the device or page as a
/// key.
pub fn editing_shortcut(keystroke: &Keystroke, cx: &App) -> bool {
    is_editing_shortcut(keystroke, cx.platform_info().is_mac())
}

fn is_editing_shortcut(keystroke: &Keystroke, mac: bool) -> bool {
    let modifiers = &keystroke.modifiers;
    let command = if mac {
        modifiers.platform
    } else {
        // Ctrl with Alt is AltGr there, which types a letter (ą, ć…).
        modifiers.control && !modifiers.alt
    };
    command && matches!(keystroke.key.as_str(), "c" | "v" | "x" | "a" | "z")
}

impl SurfaceView {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        // Closed: the picture it still holds leaves the GPU atlas too
        // (GPUI keeps a texture until told).
        cx.on_release(|this, cx| {
            let held = this.frame.take().map(|frame| frame.render_image());
            for image in this.stale.drain(..).chain(held) {
                ph_ui::images::release_render_image(image, cx);
            }
        })
        .detach();
        Self {
            frame: None,
            stale: Vec::new(),
            mode: SurfaceMode::Interact,
            shapes: Vec::new(),
            highlight: None,
            drawing: None,
            focus_handle: cx.focus_handle(),
            bounds: Rc::default(),
            placeholder: placeholder.into(),
            frame_times: Vec::new(),
            press: None,
        }
    }

    /// Shows a new frame. The previous image's GPU texture is released on the
    /// next render, so a long session does not fill the texture atlas; frames
    /// replaced while the window is not drawing (minimized, covered, on
    /// another desktop) are released at once, all but the last one drawn.
    pub fn set_frame(&mut self, frame: Frame, cx: &mut Context<Self>) {
        let new_id = frame.render_image().id;
        if let Some(old) = self.frame.replace(frame)
            && old.render_image().id != new_id
        {
            self.stale.push(old.render_image());
        }
        if self.stale.len() > 1 {
            let keep = self.stale.len() - 1;
            for image in self.stale.drain(..keep) {
                ph_ui::images::release_render_image(image, cx);
            }
        }
        let now = Instant::now();
        self.frame_times
            .retain(|time| now.duration_since(*time).as_secs_f32() < 1.0);
        self.frame_times.push(now);
        cx.notify();
    }

    /// Shows no picture: the source is gone, or another one is coming, and
    /// the last frame is not its screen. The texture is released as in
    /// [`Self::set_frame`]; a press or a shape being drawn on it ends.
    pub fn clear_frame(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self.frame.take() {
            self.stale.push(old.render_image());
        }
        self.frame_times.clear();
        self.press = None;
        self.drawing = None;
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

    /// Takes every mark off the picture: the user's, and the owner's
    /// highlight with them.
    pub fn clear_annotations(&mut self, cx: &mut Context<Self>) {
        self.shapes.clear();
        self.highlight = None;
        cx.notify();
    }

    /// Shows `shape` as the owner's own mark (the outline of a picked
    /// element), in place of the one before; `None` takes it away. Undo
    /// leaves it alone: that is for the marks the user drew.
    pub fn set_highlight(&mut self, shape: Option<Shape>, cx: &mut Context<Self>) {
        self.highlight = shape;
        cx.notify();
    }

    pub fn has_highlight(&self) -> bool {
        self.highlight.is_some()
    }

    /// What a capture burns in: the user's marks and the owner's highlight.
    pub fn marks(&self) -> Vec<Shape> {
        self.shapes.iter().chain(&self.highlight).cloned().collect()
    }

    /// The current frame with the annotations burned in.
    pub fn composite(&self) -> Option<Frame> {
        let frame = self.frame.as_ref()?;
        let marks = self.marks();
        Some(if marks.is_empty() {
            frame.clone()
        } else {
            annotate::render(frame, &marks)
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
        match self.mode {
            SurfaceMode::Interact => {
                // What was outlined is about to move or go.
                self.highlight = None;
                let right_button = event.button == MouseButton::Right;
                self.press = Some(Press {
                    at: (x, y),
                    right_button,
                });
                cx.emit(SurfaceEvent::Pointer(SurfacePointer {
                    kind: PointerKind::Down,
                    x,
                    y,
                    right_button,
                    click_count: event.click_count,
                }))
            }
            // The click that picks is not a press on the device or page:
            // its release must not reach them as one.
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
        // No button is down, so the release was never heard (it happened
        // outside the window): what was pressed or drawn ends where it
        // last was, instead of following a pointer that only hovers.
        if event.pressed_button.is_none() {
            if let Some(shape) = self.drawing.take() {
                self.shapes.push(shape);
                cx.notify();
            }
            if let Some(press) = self.press.take()
                && self.mode == SurfaceMode::Interact
            {
                cx.emit(SurfaceEvent::Pointer(SurfacePointer {
                    kind: PointerKind::Up,
                    x: press.at.0,
                    y: press.at.1,
                    right_button: press.right_button,
                    click_count: 1,
                }));
            }
            return;
        }
        let Some((x, y)) = self.to_frame(event.position) else {
            return;
        };
        match (&mut self.drawing, self.mode) {
            (Some(Shape::Rect { to, .. } | Shape::Arrow { to, .. }), _) => *to = (x, y),
            (Some(Shape::Pen { points }), _) => points.push((x, y)),
            (None, SurfaceMode::Interact) => {
                let Some(press) = &mut self.press else {
                    return;
                };
                press.at = (x, y);
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

    /// A button was let go, over the picture or anywhere else in the
    /// window (the toolbar, the letterbox, another pane).
    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let press = self.press.take();
        if let Some(shape) = self.drawing.take() {
            self.shapes.push(shape);
            cx.notify();
            return;
        }
        if self.mode == SurfaceMode::Interact
            && let Some(press) = press
        {
            // Outside the picture, the press ends where it last was in it.
            let (x, y) = self.to_frame(event.position).unwrap_or(press.at);
            cx.emit(SurfaceEvent::Pointer(SurfacePointer {
                kind: PointerKind::Up,
                x,
                y,
                right_button: event.button == MouseButton::Right,
                click_count: event.click_count,
            }));
        }
    }

    fn scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
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
        // ⌘ (and the Windows key) shortcuts stay with the app, except the
        // editing ones (copy, paste, cut, select all, undo), which the owner
        // maps for the target: with ⌘ on macOS, with Ctrl elsewhere.
        let editing = editing_shortcut(&event.keystroke, cx);
        // Escape leaves picking or marking; the app's own Escape (Back on a
        // phone) is for when the pointer is the app's.
        if self.mode != SurfaceMode::Interact && event.keystroke.key == "escape" {
            self.set_mode(SurfaceMode::Interact, cx);
            cx.stop_propagation();
            return;
        }
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
                let mut shapes = self.marks();
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
                    .chain(&self.highlight)
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
            // A release is heard only over the picture otherwise, and a
            // drag often ends on the toolbar or outside the pane.
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(Self::scroll))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(key: &str, modifiers: gpui_kit::Modifiers) -> Keystroke {
        Keystroke {
            key: key.into(),
            modifiers,
            ..Default::default()
        }
    }

    #[test]
    fn editing_shortcuts_use_each_systems_command_key() {
        let command = gpui_kit::Modifiers {
            platform: true,
            ..Default::default()
        };
        let control = gpui_kit::Modifiers {
            control: true,
            ..Default::default()
        };
        let alt_gr = gpui_kit::Modifiers {
            control: true,
            alt: true,
            ..Default::default()
        };
        for key in ["c", "v", "x", "a", "z"] {
            assert!(is_editing_shortcut(&keystroke(key, command), true));
            assert!(!is_editing_shortcut(&keystroke(key, control), true));
            assert!(is_editing_shortcut(&keystroke(key, control), false));
            // The Windows key is not the command key there, and AltGr
            // types a letter.
            assert!(!is_editing_shortcut(&keystroke(key, command), false));
            assert!(!is_editing_shortcut(&keystroke(key, alt_gr), false));
        }
        assert!(!is_editing_shortcut(&keystroke("left", control), false));
        assert!(!is_editing_shortcut(&keystroke("s", command), true));
        assert!(!is_editing_shortcut(
            &keystroke("v", gpui_kit::Modifiers::default()),
            false
        ));
    }
}
