//! Under the video: the controls to play and step (with two marks to
//! measure the time between), the timeline of kept parts and marks, the
//! bar for what is selected, and the Export dialog.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::PopupMenuItem,
    scroll::ScrollableElement as _,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, ParentElement as _, Pixels, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Subscription, Window, canvas, div, img, px,
};
use ph_ui::{Icons, MenuButton as _};

use super::{Drag, Selection, TimeGrab, Tool, VideoEditor};
use crate::edit::{Align, Color, MarkKind, MarkType, Rect, Size};
pub(crate) use crate::editor::bar::contrasting;
use crate::editor::bar::{align_buttons, swatches};
use crate::editor::to_hsla;
use crate::video::document::THUMB_HEIGHT;
use crate::video::edit::{SPEEDS, VideoEdit, VideoFormat};
use crate::video::export::Plan;

/// What a slider changes on the selected mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Knob {
    Stroke,
    TextSize,
    StepSize,
    Dim,
    Strength,
    Pixel,
}

impl Knob {
    const ALL: [Knob; 6] = [
        Knob::Stroke,
        Knob::TextSize,
        Knob::StepSize,
        Knob::Dim,
        Knob::Strength,
        Knob::Pixel,
    ];

    fn range(self) -> (f32, f32) {
        match self {
            Knob::Stroke => (1., 60.),
            Knob::TextSize | Knob::StepSize => (8., 300.),
            Knob::Dim => (0., 100.),
            Knob::Strength => (2., 100.),
            Knob::Pixel => (2., 120.),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Knob::Stroke => "Line width",
            Knob::TextSize | Knob::StepSize => "Size",
            Knob::Dim => "Dim",
            Knob::Strength => "Strength",
            Knob::Pixel => "Square size",
        }
    }

    /// The mark's value for this knob, if it has one.
    fn of(self, kind: &MarkKind) -> Option<f32> {
        match (self, kind) {
            (Knob::Stroke, kind) => kind.stroke(),
            (Knob::TextSize, MarkKind::Text { size, .. }) => Some(*size),
            (Knob::StepSize, MarkKind::Step { size, .. }) => Some(*size),
            (Knob::Dim, MarkKind::Spotlight { dim, .. }) => Some(*dim as f32),
            (Knob::Strength, MarkKind::Blur { strength, .. }) => Some(*strength),
            (Knob::Pixel, MarkKind::Pixelate { size, .. }) => Some(*size),
            _ => None,
        }
    }

    fn set(self, kind: &mut MarkKind, value: f32) {
        match (self, kind) {
            (Knob::Stroke, kind) => kind.set_stroke(value),
            (Knob::TextSize, MarkKind::Text { size, .. })
            | (Knob::StepSize, MarkKind::Step { size, .. }) => *size = value,
            (Knob::Dim, MarkKind::Spotlight { dim, .. }) => *dim = value.round() as u8,
            (Knob::Strength, MarkKind::Blur { strength, .. }) => *strength = value,
            (Knob::Pixel, MarkKind::Pixelate { size, .. }) => *size = value,
            _ => {}
        }
    }
}

pub(crate) struct Controls {
    sliders: Vec<(Knob, Entity<SliderState>)>,
    text: Entity<TextareaState>,
    limit: Entity<InputState>,
}

impl Controls {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<VideoEditor>,
    ) -> (Self, Vec<Subscription>) {
        let mut subscriptions = Vec::new();
        let sliders = Knob::ALL
            .into_iter()
            .map(|knob| {
                let (min, max) = knob.range();
                let slider = cx.new(|_| SliderState::new().min(min).max(max).step(0.5));
                subscriptions.push(cx.subscribe_in(
                    &slider,
                    window,
                    move |this, _, event: &SliderEvent, _, cx| {
                        let (value, released) = match event {
                            SliderEvent::Change(value) => (value.start(), false),
                            SliderEvent::Release(value) => (value.start(), true),
                        };
                        this.update_mark(
                            knob.label(),
                            |overlay| knob.set(&mut overlay.kind, value),
                            cx,
                        );
                        if released {
                            this.doc.update(cx, |doc, _| doc.settle());
                        }
                    },
                ));
                (knob, slider)
            })
            .collect();
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Type the text")
                .auto_grow(1, 3)
        });
        subscriptions.push(cx.subscribe_in(
            &text,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    this.update_mark(
                        "text",
                        |overlay| {
                            if let MarkKind::Text { text, .. } = &mut overlay.kind {
                                *text = value;
                            }
                        },
                        cx,
                    );
                }
            },
        ));
        let limit = cx.new(|cx| InputState::new(window, cx).placeholder("No limit"));
        subscriptions.push(cx.subscribe_in(
            &limit,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().trim().parse::<u32>().ok();
                    this.doc.update(cx, |doc, cx| {
                        doc.set_export(|export| export.max_mb = value, cx)
                    });
                }
            },
        ));
        (
            Self {
                sliders,
                text,
                limit,
            },
            subscriptions,
        )
    }

    fn slider(&self, knob: Knob) -> &Entity<SliderState> {
        &self
            .sliders
            .iter()
            .find(|(k, _)| *k == knob)
            .expect("a slider per knob")
            .1
    }

    pub(crate) fn focus_text(&self, window: &mut Window, cx: &mut App) {
        self.text.update(cx, |text, cx| {
            text.set_value("", window, cx);
            text.focus(window, cx);
        });
    }
}

/// `seconds` as `1:05.250`.
fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.);
    let minutes = (seconds / 60.).floor();
    format!("{}:{:06.3}", minutes as u64, seconds - minutes * 60.)
}

/// Where a mark (source `start`..`end`) shows in the result, if anywhere.
fn shown_span(edit: &VideoEdit, duration: f64, start: f64, end: f64) -> Option<(f64, f64)> {
    let mut at = 0.;
    let mut span: Option<(f64, f64)> = None;
    for piece in edit.pieces(duration) {
        let (from, to) = (start.max(piece.start), end.min(piece.end));
        if to > from {
            let a = at + (from - piece.start) / piece.speed as f64;
            let b = at + (to - piece.start) / piece.speed as f64;
            span = Some(span.map_or((a, b), |(x, y)| (x.min(a), y.max(b))));
        }
        at += piece.length();
    }
    span
}

impl VideoEditor {
    pub(crate) fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        let mark = match self.selection {
            Selection::Mark(id) => doc.edit().overlay(id).map(|overlay| overlay.kind.clone()),
            _ => None,
        };
        let limit = doc
            .export()
            .max_mb
            .map(|mb| mb.to_string())
            .unwrap_or_default();
        if let Some(kind) = &mark {
            for (knob, slider) in &self.controls.sliders {
                if let Some(value) = knob.of(kind) {
                    let (min, max) = knob.range();
                    let value = value.clamp(min, max);
                    if (slider.read(cx).value().start() - value).abs() > 0.001 {
                        slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
                    }
                }
            }
            if let MarkKind::Text { text, .. } = kind {
                let input = &self.controls.text;
                if input.read(cx).value().as_ref() != text.as_str()
                    && !input.read(cx).focus_handle(cx).is_focused(window)
                {
                    input.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
                }
            }
        }
        let input = &self.controls.limit;
        if !input.read(cx).focus_handle(cx).is_focused(window)
            && input.read(cx).value().trim().parse::<u32>().ok() != limit.parse::<u32>().ok()
        {
            input.update(cx, |input, cx| input.set_value(limit, window, cx));
        }
    }

    // Play and step.

    pub(crate) fn render_transport(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let doc = self.doc.read(cx);
        let duration = doc.duration();
        let length = doc.edit().output_duration(duration);
        let now = self.result_time(cx);
        let frame = doc.frame_at(self.position).map(|(ix, _)| ix + 1);
        let playing = self.playing();
        let button = |id: &'static str, icon: Icons, tip: &'static str| {
            Button::new(id).ghost().small().icon(icon).tooltip(tip)
        };
        let (a, b) = self.marks_ab;
        let between = a.zip(b).map(|(a, b)| (b - a).abs());
        let fps = doc.info().map_or(30., |info| info.fps);
        h_flex()
            .flex_none()
            .h_10()
            .px_2()
            .gap_1()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                button("video-start", Icons::ChevronsLeft, "To the start (Home)")
                    .on_click(cx.listener(|this, _, _, cx| this.seek_result(0., cx))),
            )
            .child(
                button(
                    "video-back",
                    Icons::ChevronLeft,
                    "A frame back (←; ⇧← a second)",
                )
                .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
            )
            .child(
                Button::new("video-play")
                    .ghost()
                    .small()
                    .icon(if playing { Icons::Pause } else { Icons::Play })
                    .tooltip(if playing {
                        "Pause (Space)"
                    } else {
                        "Play (Space)"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx))),
            )
            .child(
                button(
                    "video-forward",
                    Icons::ChevronRight,
                    "A frame on (→; ⇧→ a second)",
                )
                .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
            )
            .child(
                div()
                    .ml_2()
                    .text_sm()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(format!("{} / {}", clock(now), clock(length))),
            )
            .when_some(frame, |this, frame| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("frame {frame}")),
                )
            })
            .child(div().flex_1())
            // Two moments, and the time between: for latency.
            .child(
                Button::new("video-mark-a")
                    .ghost()
                    .xsmall()
                    .label("A")
                    .selected(a.is_some())
                    .tooltip("Mark this moment as A (again to clear)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let now = this.result_time(cx);
                        this.marks_ab.0 = if this.marks_ab.0.is_some_and(|a| (a - now).abs() < 1e-6)
                        {
                            None
                        } else {
                            Some(now)
                        };
                        cx.notify();
                    })),
            )
            .child(
                Button::new("video-mark-b")
                    .ghost()
                    .xsmall()
                    .label("B")
                    .selected(b.is_some())
                    .tooltip("Mark this moment as B (again to clear)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let now = this.result_time(cx);
                        this.marks_ab.1 = if this.marks_ab.1.is_some_and(|b| (b - now).abs() < 1e-6)
                        {
                            None
                        } else {
                            Some(now)
                        };
                        cx.notify();
                    })),
            )
            .when_some(between, |this, between| {
                this.child(
                    div()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(format!(
                            "Δ {:.0} ms · {:.0} frames",
                            between * 1000.,
                            between * fps
                        )),
                )
            })
            .child(div().w(px(8.)))
            .child(
                Button::new("video-split")
                    .ghost()
                    .small()
                    .icon(Icons::Scissors)
                    .tooltip("Split here (S)")
                    .on_click(cx.listener(|this, _, _, cx| this.split_here(cx))),
            )
            .child(
                Button::new("video-mute")
                    .ghost()
                    .small()
                    .icon(if doc.edit().mute {
                        Icons::VolumeOff
                    } else {
                        Icons::Volume2
                    })
                    .tooltip(if doc.edit().mute {
                        "Sound off in the result"
                    } else {
                        "Sound on in the result"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.doc.update(cx, |doc, cx| {
                            doc.settle();
                            doc.update("mute", |edit, _, _| edit.mute = !edit.mute, cx);
                        });
                        let mute = this.doc.read(cx).edit().mute;
                        if let Some(engine) = this.engine() {
                            engine.set_muted(mute || this.rate != 1.);
                        }
                    })),
            )
            .into_any_element()
    }

    // The timeline.

    /// The result time under window x `x` on a timeline `bounds` wide.
    fn time_at(&self, x: Pixels, bounds: Bounds<Pixels>, length: f64) -> f64 {
        let width = f32::from(bounds.size.width).max(1.);
        ((f32::from(x - bounds.origin.x) / width) as f64 * length).clamp(0., length)
    }

    /// The marks' rows on the timeline: each mark's id, row and span.
    fn lanes(&self, cx: &App) -> Vec<(u32, usize, f64, f64)> {
        let doc = self.doc.read(cx);
        let (edit, duration) = (doc.edit(), doc.duration());
        let mut rows: Vec<f64> = Vec::new();
        let mut out = Vec::new();
        let mut spans: Vec<(u32, f64, f64)> = edit
            .overlays
            .iter()
            .filter_map(|o| shown_span(edit, duration, o.start, o.end).map(|(a, b)| (o.id, a, b)))
            .collect();
        spans.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (id, a, b) in spans {
            let row = match rows.iter().position(|end| *end <= a) {
                Some(row) => row,
                None => {
                    rows.push(0.);
                    rows.len() - 1
                }
            };
            rows[row] = b;
            out.push((id, row, a, b));
        }
        out
    }

    /// Scrolls the marks' rows so mark `id`'s bar shows.
    pub(super) fn reveal_mark_row(&self, id: u32, cx: &App) {
        let Some(row) = self
            .lanes(cx)
            .into_iter()
            .find(|lane| lane.0 == id)
            .map(|lane| lane.1)
        else {
            return;
        };
        let (top, bottom) = (row as f32 * LANE, (row + 1) as f32 * LANE);
        let shown = -f32::from(self.mark_rows.offset().y);
        let room = LANES as f32 * LANE;
        let to = if top < shown {
            top
        } else if bottom > shown + room {
            bottom - room
        } else {
            return;
        };
        let mut offset = self.mark_rows.offset();
        offset.y = px(-to);
        self.mark_rows.set_offset(offset);
    }

    fn timeline_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        let bounds = self.timeline.get();
        let doc = self.doc.read(cx);
        let duration = doc.duration();
        let length = doc.edit().output_duration(duration);
        if length <= 0. {
            return;
        }
        let t = self.time_at(event.position.x, bounds, length);
        let per_second = f32::from(bounds.size.width) as f64 / length;
        let reach = 6. / per_second;
        let y = f32::from(event.position.y - bounds.origin.y);
        let strip = (RULER..RULER + THUMB_HEIGHT as f32).contains(&y);
        if strip {
            // A kept part's edge, else the part itself.
            let mut at = 0.;
            let pieces = doc.edit().pieces(duration);
            for (ix, piece) in pieces.iter().enumerate() {
                let end = at + piece.length();
                if (t - at).abs() <= reach && ix > 0 || (t - at).abs() <= reach && ix == 0 && t > at
                {
                    self.drag = Some(Drag::Edge {
                        part: ix,
                        end: false,
                        at: t,
                    });
                    return;
                }
                if (t - end).abs() <= reach {
                    self.drag = Some(Drag::Edge {
                        part: ix,
                        end: true,
                        at: t,
                    });
                    return;
                }
                if t >= at && t < end {
                    self.select(Selection::Part(ix), window, cx);
                }
                at = end;
            }
        }
        self.seek_result(t, cx);
        self.drag = Some(Drag::Scrub);
    }

    /// A mark's bar pressed: selects it and starts moving it in time (its
    /// whole span, or the end pressed near). `span` is where it shows.
    fn grab_mark(
        &mut self,
        id: u32,
        (a, b): (f64, f64),
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        let bounds = self.timeline.get();
        let doc = self.doc.read(cx);
        let length = doc.edit().output_duration(doc.duration());
        if length <= 0. {
            return;
        }
        let t = self.time_at(event.position.x, bounds, length);
        let reach = 6. / (f32::from(bounds.size.width) as f64 / length);
        let what = if (t - a).abs() <= reach {
            TimeGrab::Start
        } else if (t - b).abs() <= reach {
            TimeGrab::End
        } else {
            TimeGrab::Whole
        };
        self.select(Selection::Mark(id), window, cx);
        if !(a..=b).contains(&self.result_time(cx)) {
            self.seek_result(a, cx);
        }
        self.drag = Some(Drag::MarkTime {
            id,
            what,
            grab: t - a,
            at: t,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn timeline_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let bounds = self.timeline.get();
        let doc = self.doc.read(cx);
        let length = doc.edit().output_duration(doc.duration());
        let t = self.time_at(event.position.x, bounds, length);
        match &mut self.drag {
            Some(Drag::Scrub) => self.seek_result(t, cx),
            Some(Drag::Edge { at, .. } | Drag::MarkTime { at, .. }) => {
                *at = t;
                cx.notify();
            }
            _ => {}
        }
    }

    fn timeline_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Only the timeline's own drags: one on the picture ends there.
        if !matches!(
            self.drag,
            Some(Drag::Scrub | Drag::Edge { .. } | Drag::MarkTime { .. })
        ) {
            return;
        }
        match self.drag.take() {
            Some(Drag::Edge { part, end, at }) => {
                self.update_timeline(
                    "part edge",
                    |edit, duration| {
                        let pieces = edit.pieces(duration);
                        let before: f64 = pieces[..part].iter().map(|p| p.length()).sum();
                        let piece = pieces[part];
                        let source = piece.start + (at - before) * piece.speed as f64;
                        edit.move_edge(duration, part, end, source);
                    },
                    cx,
                );
            }
            Some(Drag::MarkTime { id, what, grab, at }) => {
                self.doc.update(cx, |doc, cx| {
                    doc.settle();
                    doc.update(
                        "mark time",
                        |edit, duration, _| {
                            let to_source = |t: f64| edit.to_source(duration, t).0;
                            let Some(overlay) = edit.overlay(id).cloned() else {
                                return;
                            };
                            let (start, end) = match what {
                                TimeGrab::Whole => {
                                    let start = to_source(at - grab);
                                    (start, start + (overlay.end - overlay.start))
                                }
                                TimeGrab::Start => {
                                    (to_source(at).min(overlay.end - 0.1), overlay.end)
                                }
                                TimeGrab::End => {
                                    (overlay.start, to_source(at).max(overlay.start + 0.1))
                                }
                            };
                            if let Some(overlay) = edit.overlay_mut(id) {
                                (overlay.start, overlay.end) = (start, end.min(duration));
                            }
                        },
                        cx,
                    );
                    doc.settle();
                });
            }
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn render_timeline(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let cell = self.timeline.clone();
        let bounds = self.timeline.get();
        let doc = self.doc.read(cx);
        let duration = doc.duration();
        let edit = doc.edit().clone();
        let length = edit.output_duration(duration);
        let width = f32::from(bounds.size.width);
        let lanes = self.lanes(cx);
        // Always the same height, so the picture above never moves.
        let height = RULER + THUMB_HEIGHT as f32 + 4. + LANES as f32 * LANE + 6.;
        let x_of = |t: f64| px((t / length.max(1e-6)) as f32 * width);

        // Pictures along the strip, a source frame for each.
        let aspect = doc
            .info()
            .map(|info| {
                let crop = edit.crop_rect(info.size);
                crop.width / crop.height
            })
            .unwrap_or(16. / 9.);
        let thumb_width = THUMB_HEIGHT as f32 * aspect;
        let count = ((width / thumb_width).ceil() as usize).clamp(1, 64);
        let times: Vec<f64> = (0..count)
            .map(|ix| {
                let at = ((ix as f32 + 0.5) * thumb_width / width.max(1.)) as f64 * length;
                edit.to_source(duration, at.min(length)).0
            })
            .collect();
        let thumbs: Vec<AnyElement> = times
            .iter()
            .enumerate()
            .filter_map(|(ix, t)| {
                let frame = doc.thumb(*t)?;
                Some(
                    img(frame.render_image())
                        .absolute()
                        .left(px(ix as f32 * thumb_width))
                        .top(px(RULER))
                        .w(px(thumb_width))
                        .h(px(THUMB_HEIGHT as f32))
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                )
            })
            .collect();
        if width > 0. {
            self.doc.update(cx, |doc, cx| doc.want_thumbs(&times, cx));
        }

        // The kept parts over the strip.
        let mut parts = Vec::new();
        let mut at = 0.;
        for (ix, piece) in edit.pieces(duration).iter().enumerate() {
            let end = at + piece.length();
            let selected = self.selection == Selection::Part(ix);
            parts.push(
                div()
                    .absolute()
                    .left(x_of(at))
                    .top(px(RULER))
                    .w(x_of(end) - x_of(at))
                    .h(px(THUMB_HEIGHT as f32))
                    .rounded(px(4.))
                    .border_2()
                    .border_color(if selected {
                        theme.ring
                    } else {
                        theme.background.opacity(0.9)
                    })
                    .when(piece.speed != 1., |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(3.))
                                .left(px(4.))
                                .px_1()
                                .rounded(px(3.))
                                .bg(gpui_kit::black().opacity(0.7))
                                .text_xs()
                                .text_color(gpui_kit::white())
                                .child(format!("{}×", piece.speed)),
                        )
                    })
                    .into_any_element(),
            );
            at = end;
        }
        // An edge being dragged.
        if let Some(Drag::Edge { at, .. }) = &self.drag {
            parts.push(
                div()
                    .absolute()
                    .left(x_of(*at) - px(1.))
                    .top(px(RULER))
                    .w(px(3.))
                    .h(px(THUMB_HEIGHT as f32))
                    .bg(theme.ring)
                    .into_any_element(),
            );
        }

        // The ruler: seconds, at a step that leaves room for the labels.
        let per_second = width as f64 / length.max(1e-6);
        let step = [
            0.1, 0.25, 0.5, 1., 2., 5., 10., 15., 30., 60., 120., 300., 600.,
        ]
        .into_iter()
        .find(|step| step * per_second >= 64.)
        .unwrap_or(1200.);
        let ticks: Vec<AnyElement> = (0..=((length / step) as usize).min(400))
            // A label needs room before the end.
            .filter(|ix| (*ix as f64 * step) * per_second <= width as f64 - 40.)
            .map(|ix| {
                let t = ix as f64 * step;
                div()
                    .absolute()
                    .left(x_of(t))
                    .top_0()
                    .h(px(RULER))
                    .border_l_1()
                    .border_color(theme.border)
                    .pl_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if step < 1. {
                        format!("{t:.2}")
                    } else {
                        clock(t).trim_end_matches(".000").to_string()
                    })
                    .into_any_element()
            })
            .collect();

        // The marks' bars.
        let rows = lanes.iter().map(|(_, row, ..)| row + 1).max().unwrap_or(0);
        let bars: Vec<AnyElement> = lanes
            .iter()
            .map(|(id, row, a, b)| {
                let (id, row, span) = (*id, *row, (*a, *b));
                let overlay = edit.overlay(id);
                let (mut a, mut b) = span;
                if let Some(Drag::MarkTime {
                    id: dragged,
                    what,
                    grab,
                    at,
                }) = &self.drag
                    && *dragged == id
                {
                    match what {
                        TimeGrab::Whole => (a, b) = (at - grab, at - grab + (b - a)),
                        TimeGrab::Start => a = *at,
                        TimeGrab::End => b = *at,
                    }
                }
                let color = overlay
                    .and_then(|o| o.kind.color())
                    .map_or(theme.muted_foreground, |color| {
                        to_hsla(color.with_alpha(255))
                    });
                let label = overlay.map_or_else(String::new, |o| match &o.kind {
                    MarkKind::Text { text, .. } => {
                        text.lines().next().unwrap_or_default().to_string()
                    }
                    kind => kind.kind().label().to_string(),
                });
                let selected = self.selection == Selection::Mark(id);
                div()
                    .id(("video-mark-bar", id as usize))
                    .absolute()
                    .left(x_of(a))
                    .top(px(row as f32 * LANE))
                    .w((x_of(b) - x_of(a)).max(px(4.)))
                    .h(px(LANE - 2.))
                    .flex()
                    .items_center()
                    .rounded(px(3.))
                    .bg(color.opacity(if selected { 0.95 } else { 0.55 }))
                    .when(selected, |this| {
                        this.border_1().border_color(theme.foreground)
                    })
                    .overflow_hidden()
                    .px_1()
                    .text_xs()
                    .text_color(gpui_kit::white())
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.grab_mark(id, span, event, window, cx)
                        }),
                    )
                    .into_any_element()
            })
            .collect();

        let playhead = x_of(self.result_time(cx));
        let markers: Vec<AnyElement> = [
            self.marks_ab.0.map(|t| ("A", t)),
            self.marks_ab.1.map(|t| ("B", t)),
        ]
        .into_iter()
        .flatten()
        .map(|(name, t)| {
            div()
                .absolute()
                .left(x_of(t) - px(5.))
                .top_0()
                .w(px(10.))
                .text_xs()
                .text_color(theme.info)
                .child(name)
                .into_any_element()
        })
        .collect();
        div()
            .id("video-timeline")
            .flex_none()
            .w_full()
            .px_3()
            .pt_1()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(height))
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                if cell.get() != bounds {
                                    cell.set(bounds);
                                    window.refresh();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(RULER))
                            .w_full()
                            .h(px(THUMB_HEIGHT as f32))
                            .rounded(px(4.))
                            .bg(theme.muted),
                    )
                    .children(thumbs)
                    .children(parts)
                    .children(ticks)
                    // The marks' rows: as many as overlap, scrolling past
                    // the room there is.
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(RULER + THUMB_HEIGHT as f32 + 4.))
                            .w_full()
                            .h(px(LANES as f32 * LANE))
                            .vertical_scrollbar(&self.mark_rows)
                            .child(
                                div()
                                    .id("video-mark-rows")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.mark_rows)
                                    .child(
                                        div()
                                            .relative()
                                            .w_full()
                                            .h(px(rows as f32 * LANE))
                                            .children(bars),
                                    ),
                            ),
                    )
                    .children(markers)
                    .child(
                        div()
                            .absolute()
                            .left(playhead - px(1.))
                            .top_0()
                            .w(px(2.))
                            .h_full()
                            .bg(theme.danger),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(playhead - px(5.))
                            .top_0()
                            .size(px(10.))
                            .rounded_full()
                            .bg(theme.danger),
                    ),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(Self::timeline_down))
            .on_mouse_move(cx.listener(Self::timeline_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::timeline_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::timeline_up))
            .into_any_element()
    }

    // The bar for what is selected.

    fn knob(&self, knob: Knob, value: String, cx: &App) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .w(px(140.))
            .gap_0p5()
            .child(
                h_flex()
                    .justify_between()
                    .text_xs()
                    .child(div().text_color(theme.foreground).child(knob.label()))
                    .child(div().text_color(theme.muted_foreground).child(value)),
            )
            .child(Slider::new(self.controls.slider(knob)).accessibility_label(knob.label()))
            .into_any_element()
    }

    pub(crate) fn render_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let hint = |text: &'static str| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element()
        };
        let doc = self.doc.read(cx);
        let ready = doc.info().is_some();
        let items: Vec<AnyElement> = if !ready {
            Vec::new()
        } else if let (Selection::Mark(id), true) = (self.selection, self.tool.picks())
            && let Some(overlay) = doc.edit().overlay(id).cloned()
        {
            self.mark_bar(&overlay, cx)
        } else {
            match self.tool {
                Tool::Crop => self.crop_bar(cx),
                Tool::Resize => self.size_bar(cx),
                Tool::Mark(kind) => {
                    let mut items = Vec::new();
                    if kind.has_color() {
                        let pick = cx.listener(|this, color: &Color, _, cx| {
                            this.color = *color;
                            cx.notify();
                        });
                        items.push(swatches(self.color, Rc::new(pick)));
                    }
                    items.push(hint(match kind {
                        MarkType::Text => "Click where the text goes; it shows for 3 seconds from here.",
                        MarkType::Step => "Click to place 1, 2, 3…; each shows for 3 seconds.",
                        MarkType::Blur | MarkType::Pixelate => "Drag over what to hide; it stays hidden for 3 seconds (drag its bar to change).",
                        _ => "Drag on the video; it shows for 3 seconds from here.",
                    }));
                    items
                }
                _ => match self.selection {
                    Selection::Part(ix) => self.part_bar(ix, cx),
                    _ => vec![hint(
                        "Space plays · ← → steps a frame · S splits · click a part of the timeline to speed it up or cut it",
                    )],
                },
            }
        };
        h_flex()
            .flex_none()
            // Always two rows high, so the picture above never moves.
            .h(px(BAR_HEIGHT))
            .content_center()
            .px_3()
            .py_2()
            .gap_4()
            .flex_wrap()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .children(items)
            .into_any_element()
    }

    fn part_bar(&self, ix: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let doc = self.doc.read(cx);
        let pieces = doc.edit().pieces(doc.duration());
        let Some(piece) = pieces.get(ix).copied() else {
            return Vec::new();
        };
        let count = pieces.len();
        vec![
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(format!("Part {} of {count}", ix + 1))
                .into_any_element(),
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!(
                    "{} – {} of the source",
                    clock(piece.start),
                    clock(piece.end)
                ))
                .into_any_element(),
            h_flex()
                .gap_1()
                .children(SPEEDS.iter().enumerate().map(|(n, speed)| {
                    let speed = *speed;
                    Button::new(("video-speed", n))
                        .xsmall()
                        .when(piece.speed == speed, |this| this.primary())
                        .when(piece.speed != speed, |this| this.ghost())
                        .label(format!("{speed}×"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_timeline(
                                "speed",
                                |edit, duration| edit.set_piece_speed(duration, ix, speed),
                                cx,
                            )
                        }))
                }))
                .into_any_element(),
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(if piece.speed == 1. {
                    ""
                } else {
                    "Silent when sped up or slowed down."
                })
                .into_any_element(),
            Button::new("video-delete-part")
                .ghost()
                .xsmall()
                .icon(Icons::Trash)
                .label("Cut this part")
                .disabled(count < 2)
                .on_click(cx.listener(|this, _, window, cx| this.delete_selected(window, cx)))
                .into_any_element(),
        ]
    }

    fn mark_bar(&self, overlay: &crate::video::Overlay, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let kind = &overlay.kind;
        let mut items = vec![
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(kind.kind().label())
                .into_any_element(),
        ];
        if let Some(color) = kind.color() {
            let pick = cx.listener(|this, color: &Color, _, cx| {
                this.color = *color;
                this.update_mark("color", |overlay| overlay.kind.set_color(*color), cx);
            });
            items.push(swatches(color.with_alpha(255), Rc::new(pick)));
        }
        if let MarkKind::Text {
            background,
            color,
            align,
            ..
        } = kind
        {
            items.push(
                div()
                    .w(px(200.))
                    .child(Textarea::new(&self.controls.text).small())
                    .into_any_element(),
            );
            let pick = cx.listener(|this, align: &Align, _, cx| {
                let align = *align;
                this.update_mark(
                    "align",
                    |overlay| {
                        if let MarkKind::Text { align: current, .. } = &mut overlay.kind {
                            *current = align;
                        }
                    },
                    cx,
                );
            });
            items.push(align_buttons(*align, Rc::new(pick)));
            let (boxed, color) = (background.is_some(), *color);
            items.push(
                Switch::new("video-text-box")
                    .checked(boxed)
                    .label("Box")
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        let boxed = checked.then(|| contrasting(color));
                        this.update_mark(
                            "text box",
                            |overlay| {
                                if let MarkKind::Text { background, .. } = &mut overlay.kind {
                                    *background = boxed;
                                }
                            },
                            cx,
                        );
                    }))
                    .into_any_element(),
            );
        }
        for knob in Knob::ALL {
            if let Some(value) = knob.of(kind) {
                let value = match knob {
                    Knob::Dim => format!("{value:.0}%"),
                    _ => format!("{value:.0}"),
                };
                items.push(self.knob(knob, value, cx));
            }
        }
        // When it shows.
        let (start, end) = (overlay.start, overlay.end);
        let fading = overlay.fade > 0.;
        items.push(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{} → {}", clock(start), clock(end))),
                )
                .child(
                    Button::new("video-mark-from-here")
                        .ghost()
                        .xsmall()
                        .label("Start here")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let at = this.position;
                            this.update_mark(
                                "mark start",
                                |overlay| {
                                    overlay.start = at.min(overlay.end - 0.1);
                                },
                                cx,
                            );
                        })),
                )
                .child(
                    Button::new("video-mark-to-here")
                        .ghost()
                        .xsmall()
                        .label("End here")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let at = this.position;
                            this.update_mark(
                                "mark end",
                                |overlay| {
                                    overlay.end = at.max(overlay.start + 0.1);
                                },
                                cx,
                            );
                        })),
                )
                .into_any_element(),
        );
        items.push(
            Switch::new("video-mark-fade")
                .checked(fading)
                .label("Fade")
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    let fade = if *checked { 0.3 } else { 0. };
                    this.update_mark("fade", |overlay| overlay.fade = fade, cx);
                }))
                .into_any_element(),
        );
        items.push(
            Button::new("video-delete-mark")
                .ghost()
                .xsmall()
                .icon(Icons::Trash)
                .tooltip("Delete (⌫)")
                .on_click(cx.listener(|this, _, window, cx| this.delete_selected(window, cx)))
                .into_any_element(),
        );
        items
    }

    fn crop_bar(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        const ASPECTS: [(&str, Option<f32>); 5] = [
            ("Free", None),
            ("16:9", Some(16. / 9.)),
            ("9:16", Some(9. / 16.)),
            ("1:1", Some(1.)),
            ("4:3", Some(4. / 3.)),
        ];
        let doc = self.doc.read(cx);
        let cropped = doc.edit().crop.is_some();
        let crop = doc.edit().crop_rect(doc.size());
        let aspect = self.crop_aspect;
        vec![
            h_flex()
                .gap_1()
                .children(ASPECTS.iter().enumerate().map(|(ix, (label, ratio))| {
                    let ratio = *ratio;
                    Button::new(("video-aspect", ix))
                        .xsmall()
                        .when(aspect == ratio, |this| this.primary())
                        .when(aspect != ratio, |this| this.ghost())
                        .label(*label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.crop_aspect = ratio;
                            if let Some(ratio) = ratio {
                                this.doc.update(cx, |doc, cx| {
                                    doc.settle();
                                    doc.update(
                                        "crop",
                                        |edit, _, size| {
                                            let current = edit.crop_rect(size);
                                            let (cx_, cy_) = (
                                                current.x + current.width / 2.,
                                                current.y + current.height / 2.,
                                            );
                                            let (mut w, mut h) =
                                                (current.width, current.width / ratio);
                                            if h > current.height {
                                                (w, h) = (current.height * ratio, current.height);
                                            }
                                            edit.crop = Some(
                                                Rect::new(cx_ - w / 2., cy_ - h / 2., w, h)
                                                    .clamped(size.0 as f32, size.1 as f32),
                                            );
                                        },
                                        cx,
                                    );
                                });
                            }
                            cx.notify();
                        }))
                }))
                .into_any_element(),
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{:.0} × {:.0}", crop.width, crop.height))
                .into_any_element(),
            h_flex()
                .gap_1()
                .when(cropped, |this| {
                    this.child(
                        Button::new("video-crop-clear")
                            .ghost()
                            .xsmall()
                            .label("Clear")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.doc.update(cx, |doc, cx| {
                                    doc.settle();
                                    doc.update("crop", |edit, _, _| edit.crop = None, cx);
                                })
                            })),
                    )
                })
                .child(
                    Button::new("video-crop-done")
                        .primary()
                        .xsmall()
                        .label("Done")
                        .on_click(cx.listener(|this, _, _, cx| this.set_tool(Tool::Select, cx))),
                )
                .into_any_element(),
        ]
    }

    fn size_bar(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let doc = self.doc.read(cx);
        let (width, height) = doc.edit().output_size(doc.size());
        let presets: [(&str, Option<u32>); 4] = [
            ("Original", None),
            ("1080p", Some(1080)),
            ("720p", Some(720)),
            ("480p", Some(480)),
        ];
        vec![
            h_flex()
                .gap_1()
                .children(presets.iter().enumerate().map(|(ix, (label, lines))| {
                    let lines = *lines;
                    let chosen = match lines {
                        None => doc.edit().resize.is_none(),
                        Some(lines) => height.min(width) == lines,
                    };
                    Button::new(("video-size", ix))
                        .xsmall()
                        .when(chosen, |this| this.primary())
                        .when(!chosen, |this| this.ghost())
                        .label(*label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.doc.update(cx, |doc, cx| {
                                doc.settle();
                                doc.update(
                                    "resize",
                                    |edit, _, size| {
                                        edit.resize = lines.map(|lines| {
                                            let crop = edit.crop_rect(size);
                                            // The shorter side gets the lines.
                                            let scale = lines as f32 / crop.width.min(crop.height);
                                            Size {
                                                width: (crop.width * scale).round() as u32,
                                                height: (crop.height * scale).round() as u32,
                                            }
                                        });
                                    },
                                    cx,
                                );
                            })
                        }))
                }))
                .into_any_element(),
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{width} × {height}"))
                .into_any_element(),
        ]
    }

    pub(crate) fn render_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let doc = self.doc.read(cx);
        let text = match doc.info() {
            Some(info) => {
                let (width, height) = doc.edit().output_size(info.size);
                format!(
                    "{} × {} · {:.0} fps · {} · read with {}   →   {width} × {height} · {}",
                    info.size.0,
                    info.size.1,
                    info.fps,
                    clock(info.duration),
                    info.reader(),
                    clock(doc.edit().output_duration(info.duration))
                )
            }
            None => String::new(),
        };
        h_flex()
            .flex_none()
            .h_8()
            .px_3()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(text)
            .into_any_element()
    }

    // Export.

    pub(crate) fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.read(cx).info().is_none() || self.doc.read(cx).job().is_some() {
            return;
        }
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let start = editor.clone();
            dialog
                .title("Export")
                .w(px(440.))
                .child(export_body(&editor, cx))
                .footer(
                    DialogFooter::new()
                        .w_full()
                        .child(div().flex_1())
                        .child(
                            Button::new("video-export-cancel-dialog")
                                .ghost()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("video-export-go")
                                .primary()
                                .icon(Icons::Download)
                                .label("Export…")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    start.update(cx, |editor, cx| editor.export_to(window, cx));
                                }),
                        ),
                )
        });
    }
}

/// The ruler above the strip, each row of marks, and the rows shown at
/// once (more scroll).
const RULER: f32 = 18.;
const LANE: f32 = 18.;
const LANES: usize = 3;
/// The bar under the timeline: room for two rows of controls.
pub(crate) const BAR_HEIGHT: f32 = 92.;

/// The Export dialog's settings, read each time it draws.
fn export_body(editor: &Entity<VideoEditor>, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let this = editor.read(cx);
    let doc_entity = this.doc.clone();
    let doc = doc_entity.read(cx);
    let Some(info) = doc.info().cloned() else {
        return div().into_any_element();
    };
    let export = doc.export().clone();
    let plan = Plan::new(&info, doc.edit(), &export);
    let limit = this.controls.limit.clone();
    let pick = doc_entity.clone();
    let format_menu = Button::new("video-format")
        .outline()
        .small()
        .label(export.format.label())
        .dropdown_caret(true)
        .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
            VideoFormat::ALL
                .into_iter()
                .filter(|format| crate::video::export::writable(*format))
                .fold(menu, |menu, format| {
                    let doc = pick.clone();
                    menu.item(
                        PopupMenuItem::new(format!("{}  ·  {}", format.label(), format.hint()))
                            .checked(format == export.format)
                            .on_click(move |_, _, cx| {
                                doc.update(cx, |doc, cx| doc.set_export(|e| e.format = format, cx))
                            }),
                    )
                })
        });
    let chips = |id: &'static str,
                 values: &[(&'static str, Option<u32>)],
                 chosen: Option<u32>,
                 set: fn(&mut crate::video::VideoExport, Option<u32>)| {
        h_flex()
            .gap_1()
            .children(values.iter().enumerate().map(|(ix, (label, value))| {
                let (value, doc) = (*value, doc_entity.clone());
                Button::new((id, ix))
                    .xsmall()
                    .when(chosen == value, |this| this.primary())
                    .when(chosen != value, |this| this.ghost())
                    .label(*label)
                    .on_click(move |_, _, cx| {
                        doc.update(cx, |doc, cx| doc.set_export(|e| set(e, value), cx))
                    })
            }))
    };
    let label = |text: &'static str| div().w(px(80.)).text_sm().child(text);
    let gif = export.format == VideoFormat::Gif;
    let estimate = match plan.estimate(export.format) {
        Some(bytes) => format!(
            "{} × {} · {:.0} fps · {} · about {}",
            plan.size.0,
            plan.size.1,
            plan.fps,
            clock(plan.seconds),
            crate::control::kilobytes(bytes as usize)
        ),
        None => format!(
            "{} × {} · {:.0} fps · {}",
            plan.size.0,
            plan.size.1,
            plan.fps,
            clock(plan.seconds)
        ),
    };
    let muted = doc.edit().mute;
    let sound = if !export.format.has_sound() {
        "No sound in a GIF."
    } else if doc.edit().mute || !info.audio {
        "No sound."
    } else if plan.audio.len() < doc.edit().pieces(info.duration).len() {
        "With sound, except the parts sped up or slowed down."
    } else {
        "With sound."
    };
    v_flex()
        .gap_3()
        .child(h_flex().gap_2().child(label("Format")).child(format_menu))
        .child(
            h_flex()
                .gap_2()
                .child(label("At most"))
                .child(div().w(px(80.)).child(Input::new(&limit).small()))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("MB"),
                )
                .child(chips(
                    "video-limit",
                    &[
                        ("10", Some(10)),
                        ("25", Some(25)),
                        ("50", Some(50)),
                        ("100", Some(100)),
                    ],
                    export.max_mb,
                    |e, v| e.max_mb = v,
                )),
        )
        .child(h_flex().gap_2().child(label("Frames")).child(chips(
            "video-fps",
            &if gif {
                [
                    ("12/s", None),
                    ("8/s", Some(8)),
                    ("15/s", Some(15)),
                    ("24/s", Some(24)),
                ]
            } else {
                [
                    ("As recorded", None),
                    ("60/s", Some(60)),
                    ("30/s", Some(30)),
                    ("24/s", Some(24)),
                ]
            },
            export.fps,
            |e, v| e.fps = v,
        )))
        .when(gif, |this| {
            this.child(h_flex().gap_2().child(label("Width")).child(chips(
                "video-gif-width",
                &[
                    ("480", Some(480)),
                    ("720", Some(720)),
                    ("960", Some(960)),
                    ("1280", Some(1280)),
                ],
                Some(export.gif_width),
                |e, v| e.gif_width = v.unwrap_or(720),
            )))
        })
        .when(export.format.has_sound() && info.audio, |this| {
            let doc = doc_entity.clone();
            this.child(
                h_flex().gap_2().child(label("Sound")).child(
                    Switch::new("video-export-sound")
                        .checked(!muted)
                        .label(if muted { "Off: a smaller file" } else { "On" })
                        .on_click(move |on: &bool, _, cx| {
                            let mute = !*on;
                            doc.update(cx, |doc, cx| {
                                doc.settle();
                                doc.update("mute", |edit, _, _| edit.mute = mute, cx);
                            })
                        }),
                ),
            )
        })
        .child(
            div()
                .p_2()
                .rounded(theme.radius)
                .bg(theme.muted.opacity(0.6))
                .text_sm()
                .child(estimate),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(sound),
        )
        .into_any_element()
}
