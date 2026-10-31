//! The bar under the picture: only what the tool in use needs (the
//! selected mark's color and size, the crop's proportions, the colors, the
//! size, the frame). Its sliders and fields follow the document, so a
//! change an agent (or undo) makes shows in them too.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use ph_ui::Icons;

use super::{ImageEditor, PALETTE, Tool, to_hsla};
use crate::edit::{Align, Color, Frame, MarkKind, MarkType, Paint, Rect, Size, Turn};

/// Backgrounds offered for a frame (`None`: see-through).
const BACKGROUNDS: [(Option<Paint>, &str); 8] = [
    (
        Some(Paint::Gradient([
            Color([99, 102, 241, 255]),
            Color([168, 85, 247, 255]),
        ])),
        "Violet",
    ),
    (
        Some(Paint::Gradient([
            Color([14, 165, 233, 255]),
            Color([59, 130, 246, 255]),
        ])),
        "Blue",
    ),
    (
        Some(Paint::Gradient([
            Color([244, 63, 94, 255]),
            Color([251, 146, 60, 255]),
        ])),
        "Sunset",
    ),
    (
        Some(Paint::Gradient([
            Color([15, 23, 42, 255]),
            Color([51, 65, 85, 255]),
        ])),
        "Night",
    ),
    (
        Some(Paint::Solid(Color([241, 245, 249, 255]))),
        "Light gray",
    ),
    (Some(Paint::Solid(Color::WHITE)), "White"),
    (Some(Paint::Solid(Color([24, 24, 27, 255]))), "Black"),
    (None, "None (see-through)"),
];

/// Crop proportions offered (width over height; `None` free).
const ASPECTS: [(&str, Option<f32>); 6] = [
    ("Free", None),
    ("1:1", Some(1.)),
    ("4:3", Some(4. / 3.)),
    ("3:2", Some(3. / 2.)),
    ("16:9", Some(16. / 9.)),
    ("9:16", Some(9. / 16.)),
];

/// What a slider changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Knob {
    Brightness,
    Contrast,
    Saturation,
    Blur,
    Sharpen,
    Padding,
    Radius,
    Shadow,
    Quality,
    Stroke,
    TextSize,
    StepSize,
    Dim,
    Strength,
    Pixel,
}

impl Knob {
    const ALL: [Knob; 15] = [
        Knob::Brightness,
        Knob::Contrast,
        Knob::Saturation,
        Knob::Blur,
        Knob::Sharpen,
        Knob::Padding,
        Knob::Radius,
        Knob::Shadow,
        Knob::Quality,
        Knob::Stroke,
        Knob::TextSize,
        Knob::StepSize,
        Knob::Dim,
        Knob::Strength,
        Knob::Pixel,
    ];

    /// Range and step.
    fn range(self) -> (f32, f32, f32) {
        match self {
            Knob::Brightness | Knob::Contrast | Knob::Saturation => (-100., 100., 1.),
            Knob::Blur => (0., 50., 0.5),
            Knob::Sharpen | Knob::Shadow | Knob::Dim => (0., 100., 1.),
            Knob::Padding => (0., 256., 1.),
            Knob::Radius => (0., 64., 1.),
            Knob::Quality => (10., 100., 1.),
            Knob::Stroke => (1., 60., 0.5),
            Knob::TextSize | Knob::StepSize => (8., 300., 1.),
            Knob::Strength => (2., 100., 1.),
            Knob::Pixel => (2., 120., 1.),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Knob::Brightness => "Brightness",
            Knob::Contrast => "Contrast",
            Knob::Saturation => "Saturation",
            Knob::Blur => "Blur",
            Knob::Sharpen => "Sharpen",
            Knob::Padding => "Padding",
            Knob::Radius => "Corners",
            Knob::Shadow => "Shadow",
            Knob::Quality => "Quality",
            Knob::Stroke => "Line width",
            Knob::TextSize | Knob::StepSize => "Size",
            Knob::Dim => "Dim",
            Knob::Strength => "Strength",
            Knob::Pixel => "Square size",
        }
    }
}

/// The inspector's sliders and fields.
pub(crate) struct Controls {
    sliders: Vec<(Knob, Entity<SliderState>)>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    pub(crate) limit: Entity<InputState>,
    text: Entity<TextareaState>,
    /// A resize keeps the crop's proportions.
    lock: bool,
}

impl Controls {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<ImageEditor>,
    ) -> (Self, Vec<Subscription>) {
        let mut subscriptions = Vec::new();
        let sliders = Knob::ALL
            .into_iter()
            .map(|knob| {
                let (min, max, step) = knob.range();
                let slider = cx.new(|_| SliderState::new().min(min).max(max).step(step));
                subscriptions.push(cx.subscribe_in(
                    &slider,
                    window,
                    move |this, _, event: &SliderEvent, window, cx| {
                        let (value, released) = match event {
                            SliderEvent::Change(value) => (value.start(), false),
                            SliderEvent::Release(value) => (value.start(), true),
                        };
                        this.knob_changed(knob, value, window, cx);
                        if released {
                            this.doc.update(cx, |doc, _| doc.settle());
                        }
                    },
                ));
                (knob, slider)
            })
            .collect();
        let number = |placeholder: &str, window: &mut Window, cx: &mut Context<ImageEditor>| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let width = number("Width", window, cx);
        let height = number("Height", window, cx);
        let limit = number("No limit", window, cx);
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Type the text")
                .auto_grow(1, 4)
        });
        for (input, side) in [(&width, true), (&height, false)] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let value = input.read(cx).value().trim().parse::<u32>().ok();
                        this.resize_typed(side, value, window, cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &limit,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().trim().parse::<u32>().ok();
                    this.doc.update(cx, |doc, cx| {
                        doc.set_export(|export| export.max_kb = value, cx)
                    });
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &text,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    this.update_selected(
                        "text",
                        |kind| {
                            if let MarkKind::Text { text, .. } = kind {
                                *text = value;
                            }
                        },
                        cx,
                    );
                }
            },
        ));
        (
            Self {
                sliders,
                width,
                height,
                limit,
                text,
                lock: true,
            },
            subscriptions,
        )
    }

    pub(crate) fn slider(&self, knob: Knob) -> &Entity<SliderState> {
        &self
            .sliders
            .iter()
            .find(|(k, _)| *k == knob)
            .expect("every knob has a slider")
            .1
    }

    /// Puts the keyboard in the text field (a text mark was just placed).
    pub(crate) fn focus_text(&self, window: &mut Window, cx: &mut App) {
        self.text.update(cx, |text, cx| {
            text.set_value("", window, cx);
            text.focus(window, cx);
        });
    }
}

impl ImageEditor {
    /// The sliders and fields show what the document holds.
    pub(crate) fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.doc.read(cx);
        let edit = doc.edit().clone();
        let export = doc.export().clone();
        let size = doc.size();
        let mark = self
            .selected
            .and_then(|id| edit.mark(id))
            .map(|mark| mark.kind.clone());
        let frame = edit.frame.unwrap_or_default();
        let value = |knob: Knob| -> Option<f32> {
            Some(match knob {
                Knob::Brightness => edit.adjust.brightness,
                Knob::Contrast => edit.adjust.contrast,
                Knob::Saturation => edit.adjust.saturation,
                Knob::Blur => edit.adjust.blur,
                Knob::Sharpen => edit.adjust.sharpen,
                Knob::Padding => frame.padding as f32,
                Knob::Radius => frame.radius as f32,
                Knob::Shadow => frame.shadow as f32,
                Knob::Quality => export.quality as f32,
                Knob::Stroke => mark.as_ref()?.stroke()?,
                Knob::TextSize => match mark.as_ref()? {
                    MarkKind::Text { size, .. } => *size,
                    _ => return None,
                },
                Knob::StepSize => match mark.as_ref()? {
                    MarkKind::Step { size, .. } => *size,
                    _ => return None,
                },
                Knob::Dim => match mark.as_ref()? {
                    MarkKind::Spotlight { dim, .. } => *dim as f32,
                    _ => return None,
                },
                Knob::Strength => match mark.as_ref()? {
                    MarkKind::Blur { strength, .. } => *strength,
                    _ => return None,
                },
                Knob::Pixel => match mark.as_ref()? {
                    MarkKind::Pixelate { size, .. } => *size,
                    _ => return None,
                },
            })
        };
        for (knob, slider) in &self.controls.sliders {
            if let Some(value) = value(*knob) {
                let (min, max, _) = knob.range();
                let value = value.clamp(min, max);
                if (slider.read(cx).value().start() - value).abs() > 0.001 {
                    slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
                }
            }
        }
        let (out_w, out_h) = edit.output_size(size);
        let fields = [
            (
                self.controls.width.clone(),
                (size.0 > 0).then(|| out_w.to_string()),
            ),
            (
                self.controls.height.clone(),
                (size.0 > 0).then(|| out_h.to_string()),
            ),
            (
                self.controls.limit.clone(),
                Some(export.max_kb.map(|kb| kb.to_string()).unwrap_or_default()),
            ),
        ];
        for (input, wanted) in fields {
            let Some(wanted) = wanted else { continue };
            let focused = input.read(cx).focus_handle(cx).is_focused(window);
            let current = input.read(cx).value().to_string();
            // Typing is never overwritten; "080" stays as typed.
            if !focused && current.trim().parse::<u32>().ok() != wanted.parse::<u32>().ok() {
                input.update(cx, |input, cx| input.set_value(wanted, window, cx));
            }
        }
        if let Some(MarkKind::Text { text, .. }) = &mark {
            let input = &self.controls.text;
            if input.read(cx).value().as_ref() != text.as_str()
                && !input.read(cx).focus_handle(cx).is_focused(window)
            {
                input.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
            }
        }
    }

    fn knob_changed(&mut self, knob: Knob, value: f32, _: &mut Window, cx: &mut Context<Self>) {
        let what: &'static str = knob.label();
        match knob {
            Knob::Quality => self.doc.update(cx, |doc, cx| {
                doc.set_export(|export| export.quality = value.round() as u8, cx)
            }),
            Knob::Stroke => self.update_selected(what, |kind| kind.set_stroke(value), cx),
            Knob::TextSize | Knob::StepSize | Knob::Dim | Knob::Strength | Knob::Pixel => self
                .update_selected(
                    what,
                    |kind| match kind {
                        MarkKind::Text { size, .. } | MarkKind::Step { size, .. } => *size = value,
                        MarkKind::Spotlight { dim, .. } => *dim = value.round() as u8,
                        MarkKind::Blur { strength, .. } => *strength = value,
                        MarkKind::Pixelate { size, .. } => *size = value,
                        _ => {}
                    },
                    cx,
                ),
            knob => self.doc.update(cx, |doc, cx| {
                doc.update(
                    what,
                    |edit, _| match knob {
                        Knob::Brightness => edit.adjust.brightness = value,
                        Knob::Contrast => edit.adjust.contrast = value,
                        Knob::Saturation => edit.adjust.saturation = value,
                        Knob::Blur => edit.adjust.blur = value,
                        Knob::Sharpen => edit.adjust.sharpen = value,
                        Knob::Padding | Knob::Radius | Knob::Shadow => {
                            let frame = edit.frame.get_or_insert_with(Frame::default);
                            match knob {
                                Knob::Padding => frame.padding = value.round() as u32,
                                Knob::Radius => frame.radius = value.round() as u32,
                                _ => frame.shadow = value.round() as u8,
                            }
                        }
                        _ => {}
                    },
                    cx,
                )
            }),
        }
    }

    /// A width (`side`) or height typed: the other follows when locked.
    fn resize_typed(
        &mut self,
        width: bool,
        value: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(value) = value.filter(|value| (1..=crate::edit::MAX_SIDE).contains(value)) else {
            return;
        };
        let lock = self.controls.lock;
        self.doc.update(cx, |doc, cx| {
            doc.update(
                "resize",
                |edit, size| {
                    let crop = edit.crop_rect(size);
                    let (current_w, current_h) = edit.output_size(size);
                    let ratio = crop.width / crop.height;
                    let (w, h) = match (width, lock) {
                        (true, true) => (value, (value as f32 / ratio).round().max(1.) as u32),
                        (true, false) => (value, current_h),
                        (false, true) => ((value as f32 * ratio).round().max(1.) as u32, value),
                        (false, false) => (current_w, value),
                    };
                    edit.resize = Some(Size {
                        width: w,
                        height: h,
                    });
                },
                cx,
            )
        });
        self.sync_controls(window, cx);
    }

    /// The crop made to `aspect` (largest centered), shown to adjust.
    fn crop_to(&mut self, aspect: Option<f32>, cx: &mut Context<Self>) {
        self.crop_aspect = aspect;
        if let Some(aspect) = aspect {
            self.doc.update(cx, |doc, cx| {
                doc.update(
                    "crop",
                    |edit, size| {
                        let current = edit.crop_rect(size);
                        let (cx_, cy_) = (
                            current.x + current.width / 2.,
                            current.y + current.height / 2.,
                        );
                        let (tw, th) = edit.turned_size(size);
                        let (mut w, mut h) = (current.width, current.width / aspect);
                        if h > current.height {
                            (w, h) = (current.height * aspect, current.height);
                        }
                        let rect = Rect::new(cx_ - w / 2., cy_ - h / 2., w, h);
                        edit.crop = Some(rect.clamped(tw as f32, th as f32));
                    },
                    cx,
                )
            });
        }
        self.set_tool(Tool::Crop, cx);
    }

    /// The bar under the picture: what the tool in use (or the mark
    /// selected) needs, and nothing else.
    pub(crate) fn render_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let doc = self.doc.read(cx);
        let ready = doc.original().is_some();
        let selected = self
            .selected
            .filter(|_| self.tool.picks())
            .and_then(|id| doc.edit().mark(id))
            .map(|mark| mark.kind.clone());
        let hint = |text: &'static str| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element()
        };
        let items: Vec<AnyElement> = if !ready {
            Vec::new()
        } else if let Some(kind) = selected {
            self.mark_bar(&kind, cx)
        } else {
            match self.tool {
                Tool::Crop => self.crop_bar(cx),
                Tool::Adjust => self.adjust_bar(cx),
                Tool::Resize => self.size_bar(cx),
                Tool::Frame => self.frame_bar(cx),
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
                        MarkType::Text => "Click where the text goes, then type it here.",
                        MarkType::Step => "Click to place 1, 2, 3…",
                        MarkType::Blur | MarkType::Pixelate => {
                            "Drag over what to hide: emails, tokens, names."
                        }
                        MarkType::Spotlight => "Drag around what matters; the rest dims.",
                        _ => "Drag on the picture.",
                    }));
                    items
                }
                _ => vec![hint(
                    "Pick a tool on the left, or describe the edit to the agent. Click a mark to change it.",
                )],
            }
        };
        h_flex()
            .flex_none()
            // Always two rows high, so the picture above never moves.
            .h(px(92.))
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

    /// A slider with its name and value above it.
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

    fn mark_bar(&self, kind: &MarkKind, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let mut items: Vec<AnyElement> = vec![
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(kind.kind().label())
                .into_any_element(),
        ];
        if let Some(color) = kind.color() {
            let pick = cx.listener(|this, color: &Color, _, cx| {
                this.color = *color;
                this.update_selected("color", |kind| kind.set_color(*color), cx);
            });
            items.push(swatches(color.with_alpha(255), Rc::new(pick)));
        }
        match kind {
            MarkKind::Text {
                size,
                bold,
                background,
                color,
                align,
                ..
            } => {
                let pick = cx.listener(|this, align: &Align, _, cx| {
                    let align = *align;
                    this.update_selected(
                        "align",
                        |kind| {
                            if let MarkKind::Text { align: current, .. } = kind {
                                *current = align;
                            }
                        },
                        cx,
                    );
                });
                let alignment = align_buttons(*align, Rc::new(pick));
                items.push(
                    div()
                        .w(px(220.))
                        .child(Textarea::new(&self.controls.text).small())
                        .into_any_element(),
                );
                items.push(self.knob(Knob::TextSize, format!("{size:.0} px"), cx));
                let (bold, background, color) = (*bold, *background, *color);
                items.push(
                    Switch::new("studio-text-bold")
                        .checked(bold)
                        .label("Bold")
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let checked = *checked;
                            this.update_selected(
                                "bold",
                                |kind| {
                                    if let MarkKind::Text { bold, .. } = kind {
                                        *bold = checked;
                                    }
                                },
                                cx,
                            );
                        }))
                        .into_any_element(),
                );
                items.push(
                    Switch::new("studio-text-box")
                        .checked(background.is_some())
                        .label("Box")
                        .tooltip("A box behind the text, so it reads on anything")
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let boxed = checked.then(|| contrasting(color));
                            this.update_selected(
                                "text box",
                                |kind| {
                                    if let MarkKind::Text { background, .. } = kind {
                                        *background = boxed;
                                    }
                                },
                                cx,
                            );
                        }))
                        .into_any_element(),
                );
                items.push(alignment);
            }
            MarkKind::Step { size, .. } => {
                items.push(self.knob(Knob::StepSize, format!("{size:.0} px"), cx))
            }
            MarkKind::Spotlight { dim, .. } => {
                items.push(self.knob(Knob::Dim, format!("{dim}%"), cx))
            }
            MarkKind::Blur { strength, .. } => {
                items.push(self.knob(Knob::Strength, format!("{strength:.0}"), cx))
            }
            MarkKind::Pixelate { size, .. } => {
                items.push(self.knob(Knob::Pixel, format!("{size:.0} px"), cx))
            }
            kind => {
                if let Some(stroke) = kind.stroke() {
                    items.push(self.knob(Knob::Stroke, format!("{stroke:.1} px"), cx));
                }
                if let MarkKind::Rect { fill, color, .. } | MarkKind::Ellipse { fill, color, .. } =
                    kind
                {
                    let (filled, color) = (fill.is_some(), *color);
                    items.push(
                        Switch::new("studio-mark-fill")
                            .checked(filled)
                            .label("Filled")
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                let fill_with = checked.then(|| color.with_alpha(70));
                                this.update_selected(
                                    "fill",
                                    |kind| {
                                        if let MarkKind::Rect { fill, .. }
                                        | MarkKind::Ellipse { fill, .. } = kind
                                        {
                                            *fill = fill_with;
                                        }
                                    },
                                    cx,
                                );
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        items.push(
            Button::new("studio-delete-mark")
                .ghost()
                .xsmall()
                .icon(Icons::Trash)
                .tooltip("Delete (⌫)")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.delete_mark(&super::DeleteMark, window, cx)
                }))
                .into_any_element(),
        );
        items
    }

    fn adjust_bar(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let adjust = self.doc.read(cx).edit().adjust;
        let signed = |value: f32| {
            if value > 0. {
                format!("+{value:.0}")
            } else {
                format!("{value:.0}")
            }
        };
        let toggle = |id: &'static str,
                      label: &'static str,
                      on: bool,
                      set: fn(&mut crate::edit::Adjust, bool),
                      cx: &mut Context<Self>| {
            Switch::new(id)
                .checked(on)
                .label(label)
                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    let checked = *checked;
                    this.doc.update(cx, |doc, cx| {
                        doc.settle();
                        doc.update(label, |edit, _| set(&mut edit.adjust, checked), cx);
                    })
                }))
                .into_any_element()
        };
        let mut items = vec![
            self.knob(Knob::Brightness, signed(adjust.brightness), cx),
            self.knob(Knob::Contrast, signed(adjust.contrast), cx),
            self.knob(Knob::Saturation, signed(adjust.saturation), cx),
            self.knob(Knob::Blur, format!("{:.1}", adjust.blur), cx),
            self.knob(Knob::Sharpen, format!("{:.0}", adjust.sharpen), cx),
            toggle(
                "studio-grayscale",
                "Grayscale",
                adjust.grayscale,
                |a, on| a.grayscale = on,
                cx,
            ),
            toggle(
                "studio-invert",
                "Invert",
                adjust.invert,
                |a, on| a.invert = on,
                cx,
            ),
        ];
        if !adjust.is_identity() {
            items.push(
                Button::new("studio-adjust-reset")
                    .ghost()
                    .xsmall()
                    .label("Reset")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.doc.update(cx, |doc, cx| {
                            doc.settle();
                            doc.update("adjust", |edit, _| edit.adjust = Default::default(), cx);
                            doc.settle();
                        })
                    }))
                    .into_any_element(),
            );
        }
        items
    }

    fn crop_bar(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let doc = self.doc.read(cx);
        let cropped = doc.edit().crop.is_some();
        let crop = doc.edit().crop_rect(doc.size());
        let turn = |id: &'static str,
                    icon: Icons,
                    tip: &'static str,
                    turn: Turn,
                    cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(tip)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.doc.update(cx, |doc, cx| {
                        doc.settle();
                        doc.update("turn", |edit, size| edit.turn(turn, size), cx);
                        doc.settle();
                    })
                }))
        };
        let aspect = self.crop_aspect;
        vec![
            h_flex()
                .gap_0p5()
                .child(turn(
                    "studio-rotate-left",
                    Icons::RotateCcw,
                    "Turn left",
                    Turn::RotateLeft,
                    cx,
                ))
                .child(turn(
                    "studio-rotate-right",
                    Icons::RotateCw,
                    "Turn right",
                    Turn::RotateRight,
                    cx,
                ))
                .child(turn(
                    "studio-flip-h",
                    Icons::FlipHorizontal2,
                    "Flip left to right",
                    Turn::FlipHorizontal,
                    cx,
                ))
                .child(turn(
                    "studio-flip-v",
                    Icons::FlipVertical2,
                    "Flip upside down",
                    Turn::FlipVertical,
                    cx,
                ))
                .into_any_element(),
            h_flex()
                .gap_1()
                .children(ASPECTS.iter().enumerate().map(|(ix, (label, ratio))| {
                    let ratio = *ratio;
                    Button::new(("studio-aspect", ix))
                        .xsmall()
                        .when(aspect == ratio, |this| this.primary())
                        .when(aspect != ratio, |this| this.ghost())
                        .label(*label)
                        .on_click(cx.listener(move |this, _, _, cx| this.crop_to(ratio, cx)))
                }))
                .into_any_element(),
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("{:.0} × {:.0}", crop.width, crop.height))
                .into_any_element(),
            h_flex()
                .gap_1()
                .when(cropped, |this| {
                    this.child(
                        Button::new("studio-crop-clear")
                            .ghost()
                            .xsmall()
                            .label("Clear")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.doc.update(cx, |doc, cx| {
                                    doc.settle();
                                    doc.update("crop", |edit, _| edit.crop = None, cx);
                                })
                            })),
                    )
                })
                .child(
                    Button::new("studio-crop-done")
                        .primary()
                        .xsmall()
                        .label("Done")
                        .on_click(cx.listener(|this, _, _, cx| this.set_tool(Tool::Select, cx))),
                )
                .into_any_element(),
        ]
    }

    fn size_bar(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let resized = self.doc.read(cx).edit().resize.is_some();
        let lock = self.controls.lock;
        let percent = |ix: usize, percent: u32, cx: &mut Context<Self>| {
            Button::new(("studio-percent", ix))
                .ghost()
                .xsmall()
                .label(format!("{percent}%"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.doc.update(cx, |doc, cx| {
                        doc.settle();
                        doc.update(
                            "resize",
                            |edit, size| {
                                let crop = edit.crop_rect(size);
                                let scale = percent as f32 / 100.;
                                edit.resize = Some(Size {
                                    width: (crop.width * scale).round().max(1.) as u32,
                                    height: (crop.height * scale).round().max(1.) as u32,
                                });
                            },
                            cx,
                        );
                    })
                }))
        };
        vec![
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .w(px(84.))
                        .child(Input::new(&self.controls.width).small()),
                )
                .child(
                    Button::new("studio-size-lock")
                        .ghost()
                        .xsmall()
                        .icon(if lock { Icons::Link } else { Icons::Unlink })
                        .tooltip(if lock {
                            "Proportions kept"
                        } else {
                            "Proportions free"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.controls.lock = !this.controls.lock;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .w(px(84.))
                        .child(Input::new(&self.controls.height).small()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("px"),
                )
                .into_any_element(),
            h_flex()
                .gap_1()
                .child(percent(0, 25, cx))
                .child(percent(1, 50, cx))
                .child(percent(2, 75, cx))
                .when(resized, |this| {
                    this.child(
                        Button::new("studio-size-clear")
                            .ghost()
                            .xsmall()
                            .label("Original size")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.doc.update(cx, |doc, cx| {
                                    doc.settle();
                                    doc.update("resize", |edit, _| edit.resize = None, cx);
                                })
                            })),
                    )
                })
                .into_any_element(),
        ]
    }

    fn frame_bar(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let frame = self.doc.read(cx).edit().frame;
        let mut items = vec![
            Switch::new("studio-frame")
                .checked(frame.is_some())
                .label("Frame")
                .tooltip("Space, a background, round corners and a shadow: for screenshots in docs and slides")
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    let checked = *checked;
                    this.doc.update(cx, |doc, cx| {
                        doc.settle();
                        doc.update("frame", |edit, _| edit.frame = checked.then(Frame::default), cx);
                        doc.settle();
                    })
                }))
                .into_any_element(),
        ];
        let Some(frame) = frame else {
            items.push(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        "Puts the picture on a background, with space, round corners and a shadow.",
                    )
                    .into_any_element(),
            );
            return items;
        };
        let chosen = frame.background;
        items.push(
            h_flex()
                .gap_1()
                .children(BACKGROUNDS.iter().enumerate().map(|(ix, (paint, name))| {
                    let paint = *paint;
                    let fill = match paint {
                        Some(Paint::Solid(color)) => div().bg(to_hsla(color)),
                        Some(Paint::Gradient([from, to])) => div().bg(gpui_kit::linear_gradient(
                            135.,
                            gpui_kit::linear_color_stop(to_hsla(from), 0.),
                            gpui_kit::linear_color_stop(to_hsla(to), 1.),
                        )),
                        None => div().border_1().border_color(theme.border),
                    };
                    div()
                        .id(("studio-background", ix))
                        .size(px(22.))
                        .rounded(px(6.))
                        .p(px(2.))
                        .border_2()
                        .border_color(if chosen == paint {
                            theme.foreground
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .cursor_pointer()
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(*name).build(window, cx)
                        })
                        .child(fill.size_full().rounded(px(4.)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.doc.update(cx, |doc, cx| {
                                doc.settle();
                                doc.update(
                                    "frame background",
                                    |edit, _| {
                                        edit.frame.get_or_insert_with(Frame::default).background =
                                            paint
                                    },
                                    cx,
                                );
                            })
                        }))
                }))
                .into_any_element(),
        );
        items.push(self.knob(Knob::Padding, format!("{} px", frame.padding), cx));
        items.push(self.knob(Knob::Radius, format!("{} px", frame.radius), cx));
        items.push(self.knob(Knob::Shadow, format!("{}%", frame.shadow), cx));
        items
    }
}

impl MarkType {
    pub(crate) fn label(self) -> &'static str {
        match self {
            MarkType::Rect => "Rectangle",
            MarkType::Ellipse => "Ellipse",
            MarkType::Arrow => "Arrow",
            MarkType::Line => "Line",
            MarkType::Pen => "Pen",
            MarkType::Text => "Text",
            MarkType::Step => "Numbered step",
            MarkType::Highlight => "Highlight",
            MarkType::Blur => "Blur",
            MarkType::Pixelate => "Pixelate",
            MarkType::Spotlight => "Spotlight",
        }
    }

    pub(crate) fn has_color(self) -> bool {
        !matches!(
            self,
            MarkType::Blur | MarkType::Pixelate | MarkType::Spotlight
        )
    }
}

/// A box color that reads behind text of `color`.
pub(crate) fn contrasting(color: Color) -> Color {
    let [r, g, b, _] = color.0.map(|c| c as f32 / 255.);
    if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.6 {
        Color([0, 0, 0, 200])
    } else {
        Color([255, 255, 255, 230])
    }
}

/// The mark colors, `chosen` ringed; `pick` gets the one clicked.
/// What an alignment does when picked.
pub(crate) type PickAlign = Rc<dyn Fn(&Align, &mut Window, &mut App)>;

/// Left, centered and right buttons for a text's lines, `current` on.
pub(crate) fn align_buttons(current: Align, pick: PickAlign) -> AnyElement {
    let choices = [
        (Align::Left, Icons::TextAlignStart, "Left"),
        (Align::Center, Icons::TextAlignCenter, "Centered"),
        (Align::Right, Icons::TextAlignEnd, "Right"),
    ];
    h_flex()
        .gap_0p5()
        .children(choices.into_iter().map(|(align, icon, name)| {
            let pick = pick.clone();
            Button::new(gpui_kit::SharedString::from(format!("text-align-{name}")))
                .ghost()
                .xsmall()
                .icon(icon)
                .selected(align == current)
                .tooltip(name)
                .accessibility_label(name)
                .on_click(move |_, window, cx| pick(&align, window, cx))
        }))
        .into_any_element()
}

/// What a color swatch does when picked.
pub(crate) type Pick = Rc<dyn Fn(&Color, &mut Window, &mut App)>;

pub(crate) fn swatches(chosen: Color, pick: Pick) -> AnyElement {
    h_flex()
        .gap_1p5()
        .flex_wrap()
        .children(PALETTE.iter().enumerate().map(|(ix, color)| {
            let (color, pick) = (*color, pick.clone());
            div()
                .id(("studio-swatch", ix))
                .size(px(22.))
                .rounded_full()
                .p(px(2.))
                .border_2()
                .border_color(if color == chosen {
                    to_hsla(Color([128, 128, 128, 255]))
                } else {
                    gpui_kit::transparent_black()
                })
                .cursor_pointer()
                .child(
                    div()
                        .size_full()
                        .rounded_full()
                        .border_1()
                        .border_color(to_hsla(Color([0, 0, 0, 40])))
                        .bg(to_hsla(color)),
                )
                .on_click(move |_, window, cx| pick(&color, window, cx))
        }))
        .into_any_element()
}
