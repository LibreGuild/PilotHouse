//! A color: a large swatch, the color in every form CSS, Android, Flutter
//! and SwiftUI write it, how well white and black text read on it (and any
//! text color given), and steps from light to dark to pick a neighbour.
//! The swatch opens a picker.

use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, h_flex,
    input::{Input as Field, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div,
};

use super::parts::{self, Tone, Value};
use super::{Ctx, ToolView};
use crate::color::{self, Rgba};

fn hsla(color: Rgba) -> Hsla {
    gpui_kit::Rgba {
        r: color.r as f32 / 255.,
        g: color.g as f32 / 255.,
        b: color.b as f32 / 255.,
        a: color.a as f32,
    }
    .into()
}

pub(crate) struct ColorView {
    ctx: Ctx,
    input: Entity<InputState>,
    against: Entity<InputState>,
    picker: Entity<ColorPickerState>,
    _inputs: Vec<Subscription>,
}

impl ColorView {
    fn set(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.sync_picker(window, cx);
        cx.notify();
    }

    fn sync_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(color) = color::parse(&self.input.read(cx).value()) {
            self.picker
                .update(cx, |picker, cx| picker.set_value(hsla(color), window, cx));
        }
    }

    /// White and black text on the color, and a text color given.
    fn render_contrast(&self, color: Rgba, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let against = color::parse(&self.against.read(cx).value()).ok();
        let sample = |id: &'static str, text: Rgba, name: String| {
            let ratio = color::contrast(text, color);
            let grade = color::grade(ratio);
            let tone = match grade {
                "AAA" | "AA" => Tone::Good,
                "AA large" => Tone::Warn,
                _ => Tone::Bad,
            };
            h_flex()
                .id(id)
                .flex_1()
                .min_w(ph_ui::rems_from_px(150.))
                .gap_2()
                .p_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_none()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .bg(hsla(color))
                        .text_color(hsla(text))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child("Aa"),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_sm().child(format!("{ratio:.2} : 1")))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(name),
                        ),
                )
                .child(div().flex_1())
                .child(parts::badge(grade, tone, cx))
        };
        h_flex()
            .flex_wrap()
            .gap_2()
            .child(sample("color-on-white", Rgba::WHITE, "White text".into()))
            .child(sample("color-on-black", Rgba::BLACK, "Black text".into()))
            .when_some(against, |this, text| {
                this.child(sample(
                    "color-on-given",
                    text,
                    format!("{} text", text.hex()),
                ))
            })
            .into_any_element()
    }
}

impl ToolView for ColorView {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = parts::line(
            "#3b82f6, rgb(59 130 246), hsl(217 91% 60%), tomato, 0xFF3B82F6",
            window,
            cx,
        );
        let against = parts::line("A text color to check on it (optional)", window, cx);
        let picker = cx.new(|cx| ColorPickerState::new(window, cx));
        let inputs = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.sync_picker(window, cx);
                }
                cx.notify();
            }),
            cx.subscribe(&against, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe_in(
                &picker,
                window,
                |this, _, event: &ColorPickerEvent, window, cx| {
                    let ColorPickerEvent::Change(Some(picked)) = event else {
                        return;
                    };
                    let rgb = picked.to_rgb();
                    let byte = |value: f32| (value * 255.).round().clamp(0., 255.) as u8;
                    let picked = Rgba {
                        r: byte(rgb.r),
                        g: byte(rgb.g),
                        b: byte(rgb.b),
                        a: (rgb.a as f64 * 100.).round() / 100.,
                    };
                    let typed = color::parse(&this.input.read(cx).value()).ok();
                    if typed.map(|typed| typed.hex()) != Some(picked.hex()) {
                        this.input
                            .update(cx, |input, cx| input.set_value(picked.hex(), window, cx));
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            ctx,
            input,
            against,
            picker,
            _inputs: inputs,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.set(text.trim().to_string(), window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for ColorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.input.read(cx).value().to_string();
        let parsed = (!text.trim().is_empty()).then(|| color::parse(&text));
        let theme = cx.theme();
        let content: AnyElement = match parsed {
            None => {
                parts::hint("Type a color, or pick one with the swatch.", cx).into_any_element()
            }
            Some(Err(err)) => v_flex()
                .gap_2()
                .child(parts::failure(format!("{err:#}"), cx))
                .children(parts::suggestion(&text, &self.ctx, cx))
                .into_any_element(),
            Some(Ok(color)) => {
                let shades = color::shades(color);
                let strip = h_flex()
                    .gap_1()
                    .children(shades.into_iter().enumerate().map(|(ix, shade)| {
                        let hex = shade.hex();
                        let same = shade.hex() == Rgba { a: 1.0, ..color }.hex();
                        div()
                            .id(("color-shade", ix))
                            .flex_1()
                            .h(ph_ui::rems_from_px(32.))
                            .rounded(theme.radius)
                            .bg(hsla(shade))
                            .cursor_pointer()
                            .when(same, |this| this.border_2().border_color(theme.foreground))
                            .tooltip({
                                let hex = hex.clone();
                                move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(hex.clone())
                                        .build(window, cx)
                                }
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set(hex.clone(), window, cx)
                            }))
                    }));
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_start()
                            .child(
                                div()
                                    .flex_none()
                                    .size(ph_ui::rems_from_px(112.))
                                    .rounded(theme.radius)
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(hsla(color)),
                            )
                            .child(v_flex().flex_1().min_w_0().gap_0p5().children(
                                color::forms(color).into_iter().map(|(name, value)| {
                                    Value::new(
                                        SharedString::from(format!("color-{name}")),
                                        name,
                                        value,
                                    )
                                    .label_width(72.)
                                }),
                            )),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(parts::label("Lighter and darker (click one to use it)", cx))
                            .child(strip),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(parts::label("How text reads on it (WCAG)", cx))
                            .child(self.render_contrast(color, cx))
                            .child(Field::new(&self.against).small()),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(&self.ctx, div()))
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        ColorPicker::new(&self.picker)
                            .small()
                            .accessibility_label("Pick a color"),
                    )
                    .child(div().flex_1().child(Field::new(&self.input).small())),
            )
            .child(
                v_flex()
                    .id("color-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
    }
}
