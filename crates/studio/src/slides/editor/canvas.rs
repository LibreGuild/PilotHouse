//! The slide being edited: its picture, and over it the selection with its
//! handles, the guides things snap to, the box being dragged out, and the
//! text being typed in place.

use gpui_kit::component::{
    ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    input::{InputEvent, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, CursorStyle, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, ParentElement as _, PathBuilder, PinchEvent, Pixels, Point, ScrollEvent,
    Styled as _, StyledImage as _, Window, canvas, div, fill, img, point, px,
};
use ph_ui::Icons;
use serde_json::json;

use super::SlidesEditor;
use super::geometry::{self, Guide, Handle, P, Snaps};
use crate::slides::connectors;
use crate::slides::document::Purpose;
use crate::slides::model::*;
use crate::slides::ops::Op;

/// Space kept around the slide.
const MARGIN: f32 = 28.;
/// How near (screen pixels) the pointer must be to pick a handle or snap.
const REACH: f32 = 7.;
/// How far (screen pixels) above the frame the turning handle is.
const ROTATE_ABOVE: f32 = 22.;

/// Where the slide is on screen.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Placement {
    /// The slide's top left corner (window coordinates).
    pub origin: Point<Pixels>,
    /// Screen pixels a point.
    pub scale: f32,
    pub size: (f32, f32),
}

impl Placement {
    pub fn to_slide(self, p: Point<Pixels>) -> P {
        [
            f32::from(p.x - self.origin.x) / self.scale,
            f32::from(p.y - self.origin.y) / self.scale,
        ]
    }

    pub fn to_screen(self, p: P) -> Point<Pixels> {
        point(
            self.origin.x + px(p[0] * self.scale),
            self.origin.y + px(p[1] * self.scale),
        )
    }
}

/// What the pointer is doing.
pub(crate) enum Drag {
    /// Moving the selection (as it was at the start).
    Move {
        from: P,
        frames: Vec<(String, Frame)>,
        moved: bool,
        /// Clicked on a selected element: alone, if it did not move.
        narrow: Option<String>,
    },
    Resize {
        id: String,
        handle: Handle,
        original: Frame,
        /// Pictures and icons keep their proportions from a corner.
        proportional: bool,
    },
    Rotate {
        id: String,
        original: Frame,
    },
    /// Dragging out a box to select what is inside.
    Marquee {
        from: P,
        to: P,
        before: Vec<String>,
    },
    /// Cropping a picture from a side or corner: the picture stays where
    /// it is, its frame cuts it.
    Crop {
        id: String,
        handle: Handle,
        original: Frame,
        crop: [f32; 4],
    },
    /// Moving one end of a line (`end`: the second), fixing it to the
    /// side of an element it is dropped on.
    LineEnd {
        id: String,
        end: bool,
        original: Frame,
    },
}

/// A line (not turned), whose ends are dragged rather than its frame.
fn is_line(element: &Element) -> bool {
    matches!(&element.kind, Kind::Shape(shape) if shape.geometry == Geometry::Line)
        && element.frame.rotation.rem_euclid(360.).abs() < 0.01
}

/// What is drawn over the slide.
enum Mark {
    Poly(Vec<Point<Pixels>>, Hsla, f32),
    Line(Point<Pixels>, Point<Pixels>, Hsla, f32),
    Handle(Point<Pixels>, bool),
    Box(Bounds<Pixels>, Hsla),
}

impl SlidesEditor {
    fn place(&self, bounds: Bounds<Pixels>, cx: &App) -> Option<Placement> {
        let size = &self.deck(cx).size;
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if w < 40. || h < 40. || size.width <= 0. || size.height <= 0. {
            return None;
        }
        let fit = ((w - MARGIN * 2.) / size.width)
            .min((h - MARGIN * 2.) / size.height)
            .max(0.05);
        let scale = fit * self.zoom;
        let (sw, sh) = (size.width * scale, size.height * scale);
        let center = bounds.center();
        Some(Placement {
            origin: point(
                center.x - px(sw / 2.) + px(self.pan[0]),
                center.y - px(sh / 2.) + px(self.pan[1]),
            ),
            scale,
            size: (size.width, size.height),
        })
    }

    /// The top-most element under `p`, by id.
    fn element_at(&self, p: P, reach: f32, cx: &App) -> Option<String> {
        let slide = self.slide(cx)?;
        slide
            .elements
            .iter()
            .rev()
            .find(|element| geometry::hits(element, p, reach))
            .map(|element| element.id.clone())
    }

    /// The handle of the (single) selection under `p`.
    fn handle_at(&self, p: P, placement: &Placement, cx: &App) -> Option<(String, Handle, Frame)> {
        let [id] = self.selected.as_slice() else {
            return None;
        };
        let (_, element) = self.deck(cx).element(id)?;
        let reach = REACH / placement.scale;
        let above = ROTATE_ABOVE / placement.scale;
        let near = |at: P| (at[0] - p[0]).hypot(at[1] - p[1]) <= reach;
        let frame = element.frame;
        if is_line(element) || self.cropping.as_deref() == Some(id.as_str()) {
            return None;
        }
        if near(Handle::Rotate.at(&frame, above)) {
            return Some((id.clone(), Handle::Rotate, frame));
        }
        Handle::SIDES
            .into_iter()
            .find(|handle| near(handle.at(&frame, above)))
            .map(|handle| (id.clone(), handle, frame))
    }

    /// A line's end, or a side of the picture being cropped, under `p`.
    fn grip_at(&self, p: P, placement: &Placement, cx: &App) -> Option<Drag> {
        let [id] = self.selected.as_slice() else {
            return None;
        };
        let (_, element) = self.deck(cx).element(id)?;
        let reach = REACH / placement.scale;
        let near = |at: P| (at[0] - p[0]).hypot(at[1] - p[1]) <= reach;
        let frame = element.frame;
        if is_line(element) {
            let (a, b) = connectors::line_ends(&frame);
            // The end nearer the pointer, when both are (a short line).
            let end = match (near(a), near(b)) {
                (true, true) => (b[0] - p[0]).hypot(b[1] - p[1]) < (a[0] - p[0]).hypot(a[1] - p[1]),
                (false, true) => true,
                (true, false) => false,
                _ => return None,
            };
            return Some(Drag::LineEnd {
                id: id.clone(),
                end,
                original: frame,
            });
        }
        if self.cropping.as_deref() == Some(id.as_str())
            && let Kind::Picture(picture) = &element.kind
        {
            let handle = Handle::SIDES
                .into_iter()
                .find(|handle| near(handle.at(&frame, 0.)))?;
            return Some(Drag::Crop {
                id: id.clone(),
                handle,
                original: frame,
                crop: picture.crop,
            });
        }
        None
    }

    /// Where a line's end dropped at `p` goes: the side of an element
    /// within reach (its middle), else `p`.
    fn line_target(&mut self, line: &str, p: P, reach: f32, cx: &App) -> (P, Option<(String, u8)>) {
        self.sites.clear();
        let Some(slide) = self.slide(cx) else {
            return (p, None);
        };
        let mut best: Option<(f32, P, String, u8)> = None;
        for element in slide
            .elements
            .iter()
            .filter(|element| element.id != line && !is_line(element))
        {
            let [l, t, r, b] = geometry::bounds(&element.frame);
            let close = p[0] >= l - reach * 3.
                && p[0] <= r + reach * 3.
                && p[1] >= t - reach * 3.
                && p[1] <= b + reach * 3.;
            if !close {
                continue;
            }
            for side in 0..4u8 {
                let at = connectors::site(&element.frame, side);
                self.sites.push(at);
                let distance = (at[0] - p[0]).hypot(at[1] - p[1]);
                if distance <= reach * 1.5
                    && best.as_ref().is_none_or(|(known, ..)| distance < *known)
                {
                    best = Some((distance, at, element.id.clone(), side));
                }
            }
        }
        match best {
            Some((_, at, id, side)) => (at, Some((id, side))),
            None => (p, None),
        }
    }

    /// Crops picture `id`: its fit becomes "stretch" with the same part
    /// shown (so the frame's sides cut the picture where they are).
    pub(crate) fn start_crop(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_editing(window, cx);
        let Some((_, element)) = self.deck(cx).element(id) else {
            return;
        };
        let Kind::Picture(picture) = &element.kind else {
            return;
        };
        let media = self.doc.read(cx).media().join(&picture.media);
        let (iw, ih) = image::image_dimensions(&media).unwrap_or((1, 1));
        let mut frame = element.frame;
        let mut crop = picture.crop.map(|side| side.clamp(0., 0.95));
        // What the picture shows now, as a share of each side.
        let (vw, vh) = (
            iw as f32 * (1. - crop[0] - crop[2]).max(0.05),
            ih as f32 * (1. - crop[1] - crop[3]).max(0.05),
        );
        let (shown, framed) = (vw / vh.max(1.), frame.w / frame.h.max(1.));
        match picture.fit {
            Fit::Stretch => {}
            Fit::Cover => {
                // The sides cover cuts off, added to the crop.
                let (wide, tall) = (1. - crop[0] - crop[2], 1. - crop[1] - crop[3]);
                if shown > framed {
                    let cut = wide * (1. - framed / shown) / 2.;
                    crop[0] += cut;
                    crop[2] += cut;
                } else {
                    let cut = tall * (1. - shown / framed) / 2.;
                    crop[1] += cut;
                    crop[3] += cut;
                }
            }
            Fit::Contain => {
                // The frame shrinks to the picture.
                let (cx_, cy_) = (frame.x + frame.w / 2., frame.y + frame.h / 2.);
                if shown > framed {
                    frame.h = frame.w / shown;
                } else {
                    frame.w = frame.h * shown;
                }
                frame.x = cx_ - frame.w / 2.;
                frame.y = cy_ - frame.h / 2.;
            }
        }
        let (index, id_owned) = (self.current, id.to_string());
        self.change(
            "crop",
            move |deck| {
                let Some(slide) = deck.slides.get_mut(index) else {
                    return;
                };
                let slide = std::sync::Arc::make_mut(slide);
                if let Some(element) = slide
                    .elements
                    .iter_mut()
                    .find(|element| element.id == id_owned)
                {
                    element.frame = frame;
                    if let Kind::Picture(picture) = &mut element.kind {
                        picture.fit = Fit::Stretch;
                        picture.crop = crop;
                    }
                }
            },
            cx,
        );
        self.doc.update(cx, |doc, _| doc.settle());
        self.cropping = Some(id.to_string());
        cx.notify();
    }

    pub(crate) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(placement) = self.placement else {
            return;
        };
        let p = placement.to_slide(event.position);
        // A click away from the text being typed ends typing.
        if let Some(editing) = &self.editing {
            let inside = self
                .deck(cx)
                .element(&editing.id)
                .is_some_and(|(_, element)| geometry::hits(element, p, 2. / placement.scale));
            if inside {
                return;
            }
            self.finish_editing(window, cx);
        }
        self.focus.focus(window, cx);
        if let Some(drag) = self.grip_at(p, &placement, cx) {
            self.drag = Some(drag);
            return;
        }
        // A click away from the picture being cropped ends cropping.
        if self
            .cropping
            .as_ref()
            .is_some_and(|id| self.element_at(p, 3. / placement.scale, cx).as_ref() != Some(id))
        {
            self.cropping = None;
        }
        if let Some((id, handle, original)) = self.handle_at(p, &placement, cx) {
            self.drag = Some(if handle == Handle::Rotate {
                Drag::Rotate { id, original }
            } else {
                let proportional = self.deck(cx).element(&id).is_some_and(|(_, element)| {
                    matches!(element.kind, Kind::Picture(_) | Kind::Icon(_))
                });
                Drag::Resize {
                    id,
                    handle,
                    original,
                    proportional,
                }
            });
            return;
        }
        let adding = event.modifiers.shift || event.modifiers.platform;
        match self.element_at(p, 3. / placement.scale, cx) {
            Some(id) => {
                if event.click_count >= 2 && !adding {
                    self.select(vec![id.clone()], cx);
                    self.open_element(&id, window, cx);
                    // The editor's own focus-on-click would take the
                    // field's focus away.
                    cx.stop_propagation();
                    return;
                }
                let mut narrow = None;
                if adding {
                    let mut selected = self.selected.clone();
                    if let Some(at) = selected.iter().position(|known| *known == id) {
                        selected.remove(at);
                    } else {
                        selected.push(id.clone());
                    }
                    self.select(selected, cx);
                } else if self.selected.contains(&id) {
                    narrow = Some(id.clone());
                } else {
                    self.select(vec![id.clone()], cx);
                }
                let frames = self
                    .selected_elements(cx)
                    .into_iter()
                    .map(|element| (element.id.clone(), element.frame))
                    .collect();
                self.drag = Some(Drag::Move {
                    from: p,
                    frames,
                    moved: false,
                    narrow,
                });
            }
            None => {
                let before = if adding {
                    self.selected.clone()
                } else {
                    Vec::new()
                };
                self.select(before.clone(), cx);
                self.drag = Some(Drag::Marquee {
                    from: p,
                    to: p,
                    before,
                });
            }
        }
        cx.notify();
    }

    pub(crate) fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(placement) = self.placement else {
            return;
        };
        if self.drag.is_none() || !event.dragging() {
            return;
        }
        let p = placement.to_slide(event.position);
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        self.drag_to(&mut drag, p, event, &placement, cx);
        self.drag = Some(drag);
    }

    /// Snapping lines: the slide's and those of the elements not in `skip`.
    fn snaps(&self, skip: &[String], cx: &App) -> Snaps {
        let (width, height) = {
            let size = &self.deck(cx).size;
            (size.width, size.height)
        };
        let frames: Vec<Frame> = self
            .slide(cx)
            .map(|slide| {
                slide
                    .elements
                    .iter()
                    .filter(|element| !skip.contains(&element.id))
                    .map(|element| element.frame)
                    .collect()
            })
            .unwrap_or_default();
        Snaps::new(width, height, frames)
    }

    fn drag_to(
        &mut self,
        drag: &mut Drag,
        p: P,
        event: &MouseMoveEvent,
        placement: &Placement,
        cx: &mut Context<Self>,
    ) {
        let reach = REACH / placement.scale;
        let snapping = !event.modifiers.platform;
        match drag {
            Drag::Move {
                from,
                frames,
                moved,
                ..
            } => {
                let mut delta = [p[0] - from[0], p[1] - from[1]];
                if !*moved && delta[0].hypot(delta[1]) * placement.scale < 3. {
                    return;
                }
                *moved = true;
                // Shift keeps it on one axis.
                if event.modifiers.shift {
                    if delta[0].abs() > delta[1].abs() {
                        delta[1] = 0.;
                    } else {
                        delta[0] = 0.;
                    }
                }
                let frames = frames.clone();
                let ids: Vec<String> = frames.iter().map(|(id, _)| id.clone()).collect();
                let moved_box = geometry::union(frames.iter().map(|(_, frame)| Frame {
                    x: frame.x + delta[0],
                    y: frame.y + delta[1],
                    ..*frame
                }));
                let (shift, guides) = match (snapping, moved_box) {
                    (true, Some(bounds)) => self.snaps(&ids, cx).snap_box(bounds, reach),
                    _ => ([0., 0.], Vec::new()),
                };
                self.guides = guides;
                let (dx, dy) = (delta[0] + shift[0], delta[1] + shift[1]);
                let index = self.current;
                self.change(
                    "move",
                    move |deck| {
                        let Some(slide) = deck.slides.get_mut(index) else {
                            return;
                        };
                        let slide = std::sync::Arc::make_mut(slide);
                        let moving: Vec<&String> = frames.iter().map(|(id, _)| id).collect();
                        for (id, frame) in &frames {
                            if let Some(element) = slide.elements.iter_mut().find(|e| &e.id == id) {
                                element.frame.x = frame.x + dx;
                                element.frame.y = frame.y + dy;
                                // A line moved without what it is fixed to
                                // lets go of it.
                                if let Kind::Shape(shape) = &mut element.kind
                                    && let Some(connect) = shape.connect.as_mut()
                                {
                                    for end in [&mut connect.start, &mut connect.end] {
                                        if end.as_ref().is_some_and(|(to, _)| !moving.contains(&to))
                                        {
                                            *end = None;
                                        }
                                    }
                                    if connect.start.is_none() && connect.end.is_none() {
                                        shape.connect = None;
                                    }
                                }
                            }
                        }
                    },
                    cx,
                );
            }
            Drag::Resize {
                id,
                handle,
                original,
                proportional,
            } => {
                let (id, handle, original) = (id.clone(), *handle, *original);
                let keep = event.modifiers.shift != *proportional;
                let mut target = p;
                self.guides.clear();
                if snapping && original.rotation.rem_euclid(360.).abs() < 0.01 {
                    let (x, y) = geometry::moves_axes(handle);
                    let (snapped, guides) = self
                        .snaps(std::slice::from_ref(&id), cx)
                        .snap_point(p, x, y, reach);
                    target = snapped;
                    self.guides = guides;
                }
                let frame = geometry::resize(&original, handle, target, keep, event.modifiers.alt);
                self.set_frame(&id, frame, "resize", cx);
            }
            Drag::Rotate { id, original } => {
                let (id, original) = (id.clone(), *original);
                let rotation = geometry::rotation(&original, p, event.modifiers.shift);
                // A whole turn snaps back to none.
                let rotation = if rotation.min(360. - rotation) < 2. {
                    0.
                } else {
                    rotation
                };
                self.set_frame(
                    &id,
                    Frame {
                        rotation,
                        ..original
                    },
                    "rotate",
                    cx,
                );
            }
            Drag::Crop {
                id,
                handle,
                original,
                crop,
            } => {
                let (id, original, crop) = (id.clone(), *original, *crop);
                let moved = geometry::resize(&original, *handle, p, false, false);
                let (frame, crop) = cropped(&original, crop, &moved);
                let index = self.current;
                self.change(
                    "crop",
                    move |deck| {
                        let Some(slide) = deck.slides.get_mut(index) else {
                            return;
                        };
                        let slide = std::sync::Arc::make_mut(slide);
                        if let Some(element) =
                            slide.elements.iter_mut().find(|element| element.id == id)
                        {
                            element.frame = frame;
                            if let Kind::Picture(picture) = &mut element.kind {
                                picture.crop = crop;
                            }
                        }
                    },
                    cx,
                );
            }
            Drag::LineEnd { id, end, original } => {
                let (id, end, original) = (id.clone(), *end, *original);
                let (target, fixed) = if snapping {
                    self.line_target(&id, p, reach, cx)
                } else {
                    (p, None)
                };
                let (a, b) = connectors::line_ends(&original);
                let frame = if end {
                    connectors::line_frame(a, target)
                } else {
                    connectors::line_frame(target, b)
                };
                let index = self.current;
                self.change(
                    "line",
                    move |deck| {
                        let Some(slide) = deck.slides.get_mut(index) else {
                            return;
                        };
                        let slide = std::sync::Arc::make_mut(slide);
                        let Some(element) =
                            slide.elements.iter_mut().find(|element| element.id == id)
                        else {
                            return;
                        };
                        element.frame = frame;
                        if let Kind::Shape(shape) = &mut element.kind {
                            let mut connect = shape.connect.take().unwrap_or_default();
                            if end {
                                connect.end = fixed;
                            } else {
                                connect.start = fixed;
                            }
                            shape.connect = (connect.start.is_some() || connect.end.is_some())
                                .then_some(connect);
                        }
                    },
                    cx,
                );
            }
            Drag::Marquee { from, to, before } => {
                *to = p;
                let (l, t) = (from[0].min(p[0]), from[1].min(p[1]));
                let (r, b) = (from[0].max(p[0]), from[1].max(p[1]));
                let mut selected = before.clone();
                if let Some(slide) = self.slide(cx) {
                    for element in &slide.elements {
                        let [el, et, er, eb] = geometry::bounds(&element.frame);
                        if el >= l
                            && et >= t
                            && er <= r
                            && eb <= b
                            && !selected.contains(&element.id)
                        {
                            selected.push(element.id.clone());
                        }
                    }
                }
                self.select(selected, cx);
                cx.notify();
            }
        }
    }

    fn set_frame(&mut self, id: &str, frame: Frame, what: &'static str, cx: &mut Context<Self>) {
        let (index, id) = (self.current, id.to_string());
        self.change(
            what,
            move |deck| {
                let Some(slide) = deck.slides.get_mut(index) else {
                    return;
                };
                let slide = std::sync::Arc::make_mut(slide);
                if let Some(element) = slide.elements.iter_mut().find(|element| element.id == id) {
                    element.frame = frame;
                }
            },
            cx,
        );
    }

    pub(crate) fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if let Drag::Move {
            moved: false,
            narrow: Some(id),
            ..
        } = drag
        {
            self.select(vec![id], cx);
        }
        self.guides.clear();
        self.sites.clear();
        self.doc.update(cx, |doc, _| doc.settle());
        cx.notify();
    }

    fn on_wheel(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(window.line_height());
        if event.modifiers.platform || event.modifiers.control {
            let factor = (1. - f32::from(delta.y) / 300.).clamp(0.8, 1.25);
            self.zoom_at(factor, Some(event.position), cx);
        } else if self.zoom > 1.0001 {
            self.pan[0] += f32::from(delta.x);
            self.pan[1] += f32::from(delta.y);
            self.clamp_pan(cx);
            cx.notify();
        }
    }

    fn on_pinch(&mut self, event: &PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_at(1. + event.delta, Some(event.position), cx);
    }

    /// Zooms by `factor`, keeping the point under `anchor` where it is.
    pub(crate) fn zoom_at(
        &mut self,
        factor: f32,
        anchor: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let zoom = (self.zoom * factor).clamp(0.25, 8.);
        let factor = zoom / self.zoom;
        if (factor - 1.).abs() < 0.0001 {
            return;
        }
        let bounds = self.bounds.get();
        let center = bounds.center();
        let anchor = anchor.unwrap_or(center);
        // The slide's centre moves away from the anchor by the factor.
        let offset = [
            f32::from(anchor.x - center.x) - self.pan[0],
            f32::from(anchor.y - center.y) - self.pan[1],
        ];
        self.pan[0] -= offset[0] * (factor - 1.);
        self.pan[1] -= offset[1] * (factor - 1.);
        self.zoom = zoom;
        if zoom <= 1. {
            self.pan = [0., 0.];
        }
        self.clamp_pan(cx);
        cx.notify();
    }

    pub(crate) fn fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.;
        self.pan = [0., 0.];
        cx.notify();
    }

    fn clamp_pan(&mut self, cx: &App) {
        let Some(placement) = self.place(self.bounds.get(), cx) else {
            return;
        };
        let bounds = self.bounds.get();
        let (sw, sh) = (
            placement.size.0 * placement.scale,
            placement.size.1 * placement.scale,
        );
        let room = [
            ((sw - f32::from(bounds.size.width)) / 2. + MARGIN).max(0.),
            ((sh - f32::from(bounds.size.height)) / 2. + MARGIN).max(0.),
        ];
        self.pan[0] = self.pan[0].clamp(-room[0], room[0]);
        self.pan[1] = self.pan[1].clamp(-room[1], room[1]);
    }

    /// Double-click: type in it, or the dialog for what it holds.
    pub(crate) fn open_element(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, element)) = self.deck(cx).element(id) else {
            return;
        };
        match &element.kind {
            Kind::Text(_) | Kind::Shape(_) | Kind::Code(_) => self.start_editing(id, window, cx),
            Kind::Table(_) => self.open_table(id.to_string(), window, cx),
            Kind::Chart(_) => self.open_chart(id.to_string(), window, cx),
            Kind::Icon(_) => self.open_icons(Some(id.to_string()), window, cx),
            Kind::Picture(_) => self.replace_picture(id.to_string(), window, cx),
            Kind::Svg(_) => self.open_svg(Some(id.to_string()), window, cx),
            Kind::Group(_) | Kind::Raw(_) => {}
        }
    }

    /// Starts typing in element `id` (its text as Markdown, or its code).
    pub(crate) fn start_editing(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, element)) = self.deck(cx).element(id) else {
            return;
        };
        let (text, code) = match &element.kind {
            Kind::Code(code) => (code.code.clone(), true),
            Kind::Text(_) | Kind::Shape(_) => {
                let text = element
                    .kind
                    .text()
                    .map(crate::slides::markdown::from_text)
                    .unwrap_or_default();
                (text, false)
            }
            _ => return,
        };
        if matches!(&element.kind, Kind::Shape(shape) if shape.geometry == Geometry::Line) {
            return;
        }
        self.finish_editing(window, cx);
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(if code { "Code" } else { "Type here" })
                .auto_grow(1, 400)
                .default_value(text.clone())
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let watch = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Blur => {
                        // Typing ends when the field is left (not when a menu
                        // of the bar takes focus for a moment).
                        if this
                            .editing
                            .as_ref()
                            .is_some_and(|editing| &editing.input == input)
                        {
                            this.finish_editing(window, cx);
                        }
                    }
                    InputEvent::Change => this.share_selection(cx),
                    _ => {}
                }
            },
        );
        self.select(vec![id.to_string()], cx);
        self.editing = Some(super::Editing {
            id: id.to_string(),
            input,
            before: text,
            code,
        });
        self.typing_watch = Some(watch);
        cx.notify();
    }

    /// Ends typing: what was typed becomes the element's text (an empty
    /// text box that was never filled goes).
    pub(crate) fn finish_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        self.typing_watch = None;
        let value = editing.input.read(cx).value().to_string();
        self.focus.focus(window, cx);
        cx.notify();
        let Some((_, element)) = self.deck(cx).element(&editing.id) else {
            return;
        };
        let empty_box = matches!(element.kind, Kind::Text(_)) && element.placeholder.is_none();
        if value == editing.before {
            if empty_box && value.trim().is_empty() {
                self.apply(
                    "delete",
                    vec![Op::RemoveElement { id: editing.id }],
                    window,
                    cx,
                );
                self.select(Vec::new(), cx);
            }
            return;
        }
        let op = if editing.code {
            Op::UpdateElement {
                id: editing.id.clone(),
                set: json!({ "kind": { "code": value } }),
            }
        } else if empty_box && value.trim().is_empty() {
            self.select(Vec::new(), cx);
            Op::RemoveElement {
                id: editing.id.clone(),
            }
        } else {
            let old = element.kind.text().cloned().unwrap_or_default();
            Op::UpdateElement {
                id: editing.id.clone(),
                set: json!({ "text": { "paragraphs": typed_paragraphs(&old, &value) } }),
            }
        };
        self.apply("typing", vec![op], window, cx);
    }

    /// Keys while typing: Escape ends it; the format shortcuts mark the
    /// selected text as Markdown.
    fn editing_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        if key == "escape" {
            cx.stop_propagation();
            self.finish_editing(window, cx);
            return;
        }
        let Some(editing) = &self.editing else {
            return;
        };
        if editing.code || !(modifiers.platform || modifiers.control) || modifiers.alt {
            return;
        }
        let marker = match (key, modifiers.shift) {
            ("b", false) => "**",
            ("i", false) => "*",
            ("x", true) => "~~",
            ("e", false) => "`",
            _ => return,
        };
        cx.stop_propagation();
        let input = editing.input.clone();
        input.update(cx, |input, cx| {
            let value = input.value().to_string();
            let range = input.selected_range();
            let (start, end) = (range.start.min(value.len()), range.end.min(value.len()));
            let selected = &value[start..end];
            // Marked already: unmarked; else marked.
            let wrapped = value[..start].ends_with(marker) && value[end..].starts_with(marker);
            let (text, from, to) = if wrapped {
                let from = start - marker.len();
                let to = end + marker.len();
                (
                    format!("{}{selected}{}", &value[..from], &value[to..]),
                    from,
                    from + selected.len(),
                )
            } else {
                (
                    format!(
                        "{}{marker}{selected}{marker}{}",
                        &value[..start],
                        &value[end..]
                    ),
                    start + marker.len(),
                    end + marker.len(),
                )
            };
            input.set_value(text, window, cx);
            input.set_selected_range(from..to, cx);
        });
    }

    pub(crate) fn render_canvas(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let bounds_cell = self.bounds.clone();
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
        let count = self.deck(cx).slides.len();
        let body: AnyElement = match self.placement {
            _ if count == 0 => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("This presentation has no slides."),
                )
                .child(
                    Button::new("slides-first")
                        .primary()
                        .icon(Icons::Plus)
                        .label("Add a slide")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.new_slide(Some("title".into()), window, cx)
                        })),
                )
                .into_any_element(),
            None => div().into_any_element(),
            Some(placement) => {
                let width =
                    (placement.size.0 * placement.scale * window.scale_factor()).round() as u32;
                let hide = self.editing.as_ref().map(|editing| editing.id.clone());
                let current = self.current;
                let image = self.doc.update(cx, |doc, cx| {
                    doc.picture(current, Purpose::Canvas, width, hide.as_deref(), cx)
                });
                let at = placement.origin - bounds.origin;
                let background = self
                    .deck(cx)
                    .theme
                    .resolve(&self.deck(cx).theme.background_color());
                let slide = div()
                    .absolute()
                    .left(at.x)
                    .top(at.y)
                    .w(px(placement.size.0 * placement.scale))
                    .h(px(placement.size.1 * placement.scale))
                    .bg(gpui_kit::rgb(
                        (background.r as u32) << 16
                            | (background.g as u32) << 8
                            | background.b as u32,
                    ))
                    .shadow_md()
                    .children(
                        image.map(|image| img(image).size_full().object_fit(ObjectFit::Fill)),
                    );
                div()
                    .size_full()
                    .child(slide)
                    .child(self.render_overlay(&placement, cx))
                    .children(self.render_pins(&placement, bounds, cx))
                    .children(self.render_typing(&placement, bounds, cx))
                    .into_any_element()
            }
        };
        let cursor = self.cursor;
        div()
            .id("slides-canvas")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(theme.muted.opacity(0.5))
            .cursor(cursor)
            .child(measure)
            .child(body)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.drag.is_some() {
                    this.mouse_move(event, window, cx);
                } else {
                    this.hover(event.position, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(Self::on_wheel))
            .on_pinch(cx.listener(Self::on_pinch))
            .into_any_element()
    }

    /// The pointer's look over the slide: over a handle, its direction.
    fn hover(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(placement) = self.placement else {
            return;
        };
        let p = placement.to_slide(position);
        let cursor = match (
            self.grip_at(p, &placement, cx),
            self.handle_at(p, &placement, cx),
        ) {
            (Some(Drag::LineEnd { .. }), _) => CursorStyle::Crosshair,
            (Some(Drag::Crop { handle, .. }), _) => handle.cursor(0.),
            (_, Some((_, handle, frame))) => handle.cursor(frame.rotation),
            _ => CursorStyle::Arrow,
        };
        let hovered = self.element_at(p, 3. / placement.scale, cx);
        if cursor != self.cursor || hovered != self.hovered {
            self.cursor = cursor;
            self.hovered = hovered;
            cx.notify();
        }
    }

    /// The selection, its handles, the guides and the box being dragged.
    fn render_overlay(&self, placement: &Placement, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let ring: Hsla = theme.ring;
        let guide: Hsla = gpui_kit::hsla(0.89, 0.85, 0.6, 1.);
        let mut marks = Vec::new();
        let screen = |p: P| placement.to_screen(p);
        let editing = self.editing.as_ref().map(|editing| editing.id.as_str());
        if let Some(slide) = self.slide(cx) {
            if let Some(hovered) = self
                .hovered
                .as_ref()
                .filter(|id| !self.selected.contains(id))
                && let Some(element) = slide.elements.iter().find(|element| &element.id == hovered)
                && self.drag.is_none()
                && !is_line(element)
            {
                let corners = geometry::corners(&element.frame).map(screen).to_vec();
                marks.push(Mark::Poly(corners, ring.opacity(0.45), 1.));
            }
            let single = self.selected.len() == 1;
            for element in slide
                .elements
                .iter()
                .filter(|element| self.selected.contains(&element.id))
            {
                let frame = element.frame;
                // A line shows its ends, not its box.
                if !is_line(element) {
                    let corners = geometry::corners(&frame).map(screen).to_vec();
                    marks.push(Mark::Poly(
                        corners,
                        ring,
                        if editing.is_some() { 1. } else { 1.5 },
                    ));
                }
                let still = single
                    && editing.is_none()
                    && !matches!(
                        self.drag,
                        Some(super::canvas::Drag::Move { moved: true, .. })
                    );
                if still && is_line(element) {
                    let (a, b) = connectors::line_ends(&frame);
                    let fixed = match &element.kind {
                        Kind::Shape(Shape {
                            connect: Some(connect),
                            ..
                        }) => (connect.start.is_some(), connect.end.is_some()),
                        _ => (false, false),
                    };
                    marks.push(Mark::Handle(screen(a), fixed.0));
                    marks.push(Mark::Handle(screen(b), fixed.1));
                } else if still && self.cropping.as_ref() == Some(&element.id) {
                    // Crop marks: thick bars along each side and corner.
                    let thick: Hsla = theme.foreground;
                    let arm = 10. / placement.scale;
                    for handle in Handle::SIDES {
                        let at = handle.at(&frame, 0.);
                        let local = geometry::to_local(&frame, at);
                        let (sx, sy) = (local[0].signum(), local[1].signum());
                        let (h1, v1) = (
                            geometry::to_slide(&frame, [local[0] - sx * arm, local[1]]),
                            geometry::to_slide(&frame, [local[0], local[1] - sy * arm]),
                        );
                        if sx != 0. {
                            marks.push(Mark::Line(screen(v1), screen(at), thick, 4.));
                        }
                        if sy != 0. {
                            marks.push(Mark::Line(screen(h1), screen(at), thick, 4.));
                        }
                        if sx == 0. {
                            let (l, r) = (
                                geometry::to_slide(&frame, [local[0] - arm, local[1]]),
                                geometry::to_slide(&frame, [local[0] + arm, local[1]]),
                            );
                            marks.push(Mark::Line(screen(l), screen(r), thick, 4.));
                        }
                        if sy == 0. {
                            let (t, b) = (
                                geometry::to_slide(&frame, [local[0], local[1] - arm]),
                                geometry::to_slide(&frame, [local[0], local[1] + arm]),
                            );
                            marks.push(Mark::Line(screen(t), screen(b), thick, 4.));
                        }
                    }
                } else if still {
                    let above = ROTATE_ABOVE / placement.scale;
                    let top = screen(Handle::Top.at(&frame, above));
                    let turn = screen(Handle::Rotate.at(&frame, above));
                    marks.push(Mark::Line(top, turn, ring, 1.));
                    marks.push(Mark::Handle(turn, true));
                    for handle in Handle::SIDES {
                        marks.push(Mark::Handle(screen(handle.at(&frame, above)), false));
                    }
                }
            }
        }
        for site in &self.sites {
            marks.push(Mark::Handle(screen(*site), true));
        }
        let (width, height) = placement.size;
        for line in &self.guides {
            let (a, b) = match *line {
                Guide::Vertical(x) => ([x, 0.], [x, height]),
                Guide::Horizontal(y) => ([0., y], [width, y]),
            };
            marks.push(Mark::Line(screen(a), screen(b), guide, 1.));
        }
        if let Some(super::canvas::Drag::Marquee { from, to, .. }) = &self.drag {
            let (a, b) = (screen(*from), screen(*to));
            let bounds = Bounds::from_corners(
                point(a.x.min(b.x), a.y.min(b.y)),
                point(a.x.max(b.x), a.y.max(b.y)),
            );
            marks.push(Mark::Box(bounds, ring));
        }
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                for mark in &marks {
                    match mark {
                        Mark::Poly(points, color, width) => {
                            let mut path = PathBuilder::stroke(px(*width));
                            path.move_to(points[0]);
                            for p in &points[1..] {
                                path.line_to(*p);
                            }
                            path.line_to(points[0]);
                            if let Ok(path) = path.build() {
                                window.paint_path(path, *color);
                            }
                        }
                        Mark::Line(a, b, color, width) => {
                            let mut path = PathBuilder::stroke(px(*width));
                            path.move_to(*a);
                            path.line_to(*b);
                            if let Ok(path) = path.build() {
                                window.paint_path(path, *color);
                            }
                        }
                        Mark::Handle(at, round) => {
                            let size = px(8.);
                            let bounds = Bounds::new(
                                point(at.x - size / 2., at.y - size / 2.),
                                gpui_kit::size(size, size),
                            );
                            window.paint_quad(
                                fill(bounds, gpui_kit::white())
                                    .border_widths(px(1.))
                                    .border_color(ring)
                                    .corner_radii(if *round { size / 2. } else { px(1.5) }),
                            );
                        }
                        Mark::Box(bounds, color) => {
                            window.paint_quad(
                                fill(*bounds, color.opacity(0.08))
                                    .border_widths(px(1.))
                                    .border_color(*color),
                            );
                        }
                    }
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    /// The field typed in, over the element whose text it is (its own text
    /// is left out of the picture meanwhile).
    fn render_typing(
        &self,
        placement: &Placement,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let editing = self.editing.as_ref()?;
        let (_, element) = self.deck(cx).element(&editing.id)?;
        let theme = &self.deck(cx).theme;
        let frame = geometry::bounds(&element.frame);
        let at = placement.to_screen([frame[0], frame[1]]) - bounds.origin;
        let (w, h) = (
            (frame[2] - frame[0]) * placement.scale,
            (frame[3] - frame[1]) * placement.scale,
        );
        // The text's own size and colour, as near as the field can.
        let (size, color, family, insets) = match &element.kind {
            Kind::Code(code) => (
                code.font_size,
                if code.dark { Rgba::WHITE } else { Rgba::BLACK },
                theme.fonts.code.clone(),
                [8.; 4],
            ),
            kind => {
                let body = kind.text().cloned().unwrap_or_default();
                let place = element.placeholder.and_then(|kind| {
                    self.slide(cx)
                        .and_then(|slide| theme.layout(&slide.layout))
                        .and_then(|layout| layout.placeholder(kind))
                });
                let on = match kind {
                    Kind::Shape(shape) => Some(&shape.fill),
                    Kind::Text(text) => Some(&text.fill),
                    _ => None,
                }
                .and_then(|fill| match fill {
                    Fill::Solid { color, opacity } if *opacity > 0.5 => {
                        Some(crate::slides::svg::contrasting(theme.resolve(color), theme))
                    }
                    _ => None,
                });
                let paras = crate::slides::text::paragraphs(
                    &body,
                    &crate::slides::text::Context {
                        theme,
                        place,
                        color: on,
                        size: None,
                        bold: None,
                    },
                );
                let style = paras.first().map(|para| para.style.clone());
                let (size, color, family) = match style {
                    Some(style) => (style.size, style.color, style.font.clone()),
                    None => {
                        let base = if element.placeholder.is_some_and(|kind| kind.is_title()) {
                            &theme.title
                        } else {
                            &theme.body
                        };
                        (
                            place.and_then(|place| place.size).unwrap_or(base.size),
                            theme.resolve(&base.color),
                            theme.font(&base.font).to_string(),
                        )
                    }
                };
                (size, color, family, body.insets)
            }
        };
        let background = theme.resolve(&theme.background_color());
        let editing_entity = cx.entity();
        Some(
            div()
                .absolute()
                .left(at.x)
                .top(at.y)
                .w(px(w.max(60.)))
                .min_h(px(h.max(24.)))
                .pl(px(insets[0] * placement.scale))
                .pt(px(insets[1] * placement.scale))
                .pr(px(insets[2] * placement.scale))
                .pb(px(insets[3] * placement.scale))
                .bg(to_hsla(background).opacity(0.94))
                .border_1()
                .border_color(cx.theme().ring)
                .text_size(px((size * placement.scale).max(9.)))
                .line_height(gpui_kit::relative(1.2))
                .text_color(to_hsla(color))
                .font_family(family)
                .capture_key_down(move |event: &KeyDownEvent, window, cx| {
                    editing_entity.update(cx, |this, cx| this.editing_key(event, window, cx));
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Textarea::new(&editing.input).appearance(false))
                .into_any_element(),
        )
    }
}

pub(crate) fn to_hsla(color: Rgba) -> Hsla {
    let rgba = gpui_kit::Rgba {
        r: color.r as f32 / 255.,
        g: color.g as f32 / 255.,
        b: color.b as f32 / 255.,
        a: color.a as f32 / 255.,
    };
    rgba.into()
}

/// What was typed (Markdown) as paragraphs: a line as it was keeps all its
/// styles; a changed line is read as typed, keeping the colour, size and
/// font its paragraph had (which Markdown cannot say).
pub(crate) fn typed_paragraphs(old: &TextBody, typed: &str) -> Vec<Paragraph> {
    use crate::slides::markdown;
    let line_of = |paragraph: &Paragraph| {
        markdown::from_text(&TextBody {
            paragraphs: vec![paragraph.clone()],
            ..TextBody::default()
        })
    };
    // Each paragraph as one logical line (its line breaks joined).
    let joined = |text: String| markdown::paragraph_lines(&text).join("\u{2028}");
    let before: Vec<String> = old.paragraphs.iter().map(line_of).map(joined).collect();
    let fresh = markdown::to_text(typed);
    let lines: Vec<String> = markdown::paragraph_lines(typed)
        .into_iter()
        .map(|line| line.replace('\n', "\u{2028}"))
        .collect();
    fresh
        .paragraphs
        .into_iter()
        .zip(lines.iter().map(String::as_str))
        .enumerate()
        .map(|(ix, (mut paragraph, line))| {
            if let Some(same) = before
                .iter()
                .position(|known| known == line.trim_end_matches('\r'))
            {
                return old.paragraphs[same].clone();
            }
            if let Some(was) = old.paragraphs.get(ix) {
                // One look across the old line: carried to the new.
                let first = was.runs.first().cloned().unwrap_or_default();
                let uniform = |pick: fn(&Run) -> Option<String>| {
                    let value = pick(&first);
                    was.runs
                        .iter()
                        .all(|run| pick(run) == value)
                        .then_some(value)
                        .flatten()
                };
                let color = uniform(|run| run.color.as_ref().map(|color| color.0.clone()));
                let font = uniform(|run| run.font.clone().filter(|font| font != "code"));
                let size = was
                    .runs
                    .iter()
                    .all(|run| run.size == first.size)
                    .then_some(first.size)
                    .flatten();
                for run in &mut paragraph.runs {
                    if run.color.is_none() {
                        run.color = color.clone().map(Color);
                    }
                    if run.font.is_none() {
                        run.font = font.clone();
                    }
                    if run.size.is_none() {
                        run.size = size;
                    }
                }
                paragraph.align = paragraph.align.or(was.align);
                paragraph.space_before = paragraph.space_before.or(was.space_before);
                paragraph.space_after = paragraph.space_after.or(was.space_after);
                paragraph.line_spacing = paragraph.line_spacing.or(was.line_spacing);
            }
            paragraph
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slides::markdown;

    #[test]
    fn typing_is_exact_where_lines_changed() {
        let mut old = markdown::to_text("- Costs fell **40%** in a year\n- Demand doubled");
        old.paragraphs[1].runs[0].color = Some(Color::from("accent2"));
        old.paragraphs[1].align = Some(Align::Center);
        // Bold taken off the first line; the second line kept as it was.
        let typed = typed_paragraphs(&old, "- Costs fell 40% in a year\n- Demand doubled\n- New");
        assert_eq!(typed.len(), 3);
        assert!(typed[0].runs.iter().all(|run| run.bold.is_none()));
        assert_eq!(typed[1], old.paragraphs[1]);
        // A changed line keeps its paragraph's colour and alignment.
        let typed = typed_paragraphs(&old, "- Costs fell 40% in a year\n- Demand tripled");
        assert_eq!(typed[1].runs[0].color, Some(Color::from("accent2")));
        assert_eq!(typed[1].align, Some(Align::Center));
    }
}

/// The frame and crop after a crop handle moved `original` to `moved`: the
/// picture stays where it is, at most all of it shown.
fn cropped(original: &Frame, crop: [f32; 4], moved: &Frame) -> (Frame, [f32; 4]) {
    let [mut cl, mut ct, mut cr, mut cb] = crop.map(|side| side.clamp(0., 0.95));
    // The whole picture's size, in points.
    let full_w = original.w / (1. - cl - cr).max(0.05);
    let full_h = original.h / (1. - ct - cb).max(0.05);
    // How far each side came in (points); flipped, the picture's sides are
    // the other way round.
    let left = moved.x - original.x;
    let right = (original.x + original.w) - (moved.x + moved.w);
    let top = moved.y - original.y;
    let bottom = (original.y + original.h) - (moved.y + moved.h);
    let (left, right) = if original.flip_h {
        (right, left)
    } else {
        (left, right)
    };
    let (top, bottom) = if original.flip_v {
        (bottom, top)
    } else {
        (top, bottom)
    };
    cl = (cl + left / full_w).max(0.);
    cr = (cr + right / full_w).max(0.);
    ct = (ct + top / full_h).max(0.);
    cb = (cb + bottom / full_h).max(0.);
    // Something of it stays.
    let over_w = (cl + cr - 0.95).max(0.);
    let over_h = (ct + cb - 0.95).max(0.);
    if left != 0. {
        cl -= over_w
    } else {
        cr -= over_w
    }
    if top != 0. {
        ct -= over_h
    } else {
        cb -= over_h
    }
    let (l, r) = if original.flip_h { (cr, cl) } else { (cl, cr) };
    let (t, b) = if original.flip_v { (cb, ct) } else { (ct, cb) };
    let (ol, or) = if original.flip_h {
        (crop[2], crop[0])
    } else {
        (crop[0], crop[2])
    };
    let (ot, ob) = if original.flip_v {
        (crop[3], crop[1])
    } else {
        (crop[1], crop[3])
    };
    let x = original.x + (l - ol.clamp(0., 0.95)) * full_w;
    let y = original.y + (t - ot.clamp(0., 0.95)) * full_h;
    let w = original.w - (l - ol.clamp(0., 0.95)) * full_w - (r - or.clamp(0., 0.95)) * full_w;
    let h = original.h - (t - ot.clamp(0., 0.95)) * full_h - (b - ob.clamp(0., 0.95)) * full_h;
    (
        Frame {
            x,
            y,
            w: w.max(1.),
            h: h.max(1.),
            ..*original
        },
        [cl, ct, cr, cb],
    )
}

#[cfg(test)]
mod crop_tests {
    use super::*;

    #[test]
    fn cropping_keeps_the_picture_in_place() {
        let original = Frame::new(100., 100., 200., 100.);
        // The left side 50 points in: a quarter of the picture cut.
        let moved = Frame {
            x: 150.,
            w: 150.,
            ..original
        };
        let (frame, crop) = cropped(&original, [0.; 4], &moved);
        assert_eq!((frame.x, frame.w), (150., 150.));
        assert!((crop[0] - 0.25).abs() < 1e-5);
        // Back out past the picture's edge: no further than all of it.
        let wider = Frame {
            x: 50.,
            w: 250.,
            ..frame
        };
        let (frame, crop) = cropped(&frame, crop, &wider);
        assert!((frame.x - 100.).abs() < 1e-4 && crop[0].abs() < 1e-6);
    }
}
