//! A QR code drawn as a picture: dark squares on white with the margin a
//! reader needs, so it scans from the screen in a light or a dark theme,
//! and can be copied or saved as a PNG for a slide or a printout. (It used
//! to be drawn with text characters, which scanned only at one font size
//! and theme.)

use std::path::PathBuf;

use gpui_kit::component::{
    Disableable as _, WindowExt as _, h_flex,
    input::{InputEvent, Textarea, TextareaState},
    notification::Notification,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, Entity, Image, ImageFormat,
    IntoElement, ParentElement as _, Pixels, Render, Styled as _, Subscription, Window, canvas,
    div, fill, point, px, size,
};
use ph_ui::Icons;

use super::parts::{self, Tone};
use super::{Ctx, ToolView};
use crate::cert::{Correction, Qr};

/// Squares of margin around the code, as the standard asks.
const MARGIN: usize = 4;

pub(crate) struct QrView {
    ctx: Ctx,
    text: Entity<TextareaState>,
    correction: usize,
    _input: Subscription,
}

impl QrView {
    fn correction(&self) -> Correction {
        [
            Correction::Low,
            Correction::Medium,
            Correction::Quartile,
            Correction::High,
        ][self.correction]
    }

    fn code(&self, cx: &App) -> Option<anyhow::Result<Qr>> {
        let text = self.text.read(cx).value().to_string();
        (!text.is_empty()).then(|| crate::cert::qr_code(&text, self.correction()))
    }

    fn copy_picture(&self, cx: &mut Context<Self>) {
        let Some(Ok(code)) = self.code(cx) else {
            return;
        };
        if let Ok(png) = crate::cert::qr_png(&code, 12) {
            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                ImageFormat::Png,
                png,
            )));
        }
    }

    fn save(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(code)) = self.code(cx) else {
            return;
        };
        let Ok(png) = crate::cert::qr_png(&code, 16) else {
            return;
        };
        let folder = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let chosen = cx.prompt_for_new_path(&folder, Some("qr-code.png"));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let written = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::write(&path, png) }
                })
                .await;
            _ = cx.update(|window, cx| {
                let notification = match written {
                    Ok(()) => Notification::success(format!(
                        "Saved {}.",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )),
                    Err(err) => Notification::error(format!("Could not save it: {err}")),
                };
                window.push_notification(notification, cx);
            });
        })
        .detach();
    }
}

/// The code drawn square by square, each a whole number of device pixels
/// so that no seam or blur shows between them.
fn picture(code: Qr, side: f32) -> AnyElement {
    let squares = code.width + MARGIN * 2;
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let scale = window.scale_factor();
            let device = (f32::from(bounds.size.width) * scale / squares as f32)
                .floor()
                .max(1.);
            let unit = px(device / scale);
            let whole = unit * squares as f32;
            let origin = bounds.origin;
            window.paint_quad(fill(
                Bounds::new(origin, size(whole, whole)),
                gpui_kit::white(),
            ));
            for row in 0..code.width {
                for column in 0..code.width {
                    if code.dark[row * code.width + column] {
                        let at = point(
                            origin.x + unit * (column + MARGIN) as f32,
                            origin.y + unit * (row + MARGIN) as f32,
                        );
                        window
                            .paint_quad(fill(Bounds::new(at, size(unit, unit)), gpui_kit::black()));
                    }
                }
            }
        },
    )
    .size(ph_ui::rems_from_px(side))
    .into_any_element()
}

impl ToolView for QrView {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = parts::area(
            "Text or a link: the address of a dev server for a phone, a Wi-Fi login…",
            window,
            cx,
        );
        let subscription = cx.subscribe(&text, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            ctx,
            text,
            correction: 1,
            _input: subscription,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.text
            .update(cx, |input, cx| input.set_value(text, window, cx));
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.text.update(cx, |text, cx| text.focus(window, cx));
    }
}

impl Render for QrView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (_, height) = super::size(window);
        // As large as the room under the input allows.
        let room = f32::from(height) / f32::from(ph_ui::scaled_px(1., window)).max(0.01);
        let side = (room - 230.).clamp(140., 320.);
        let text = self.text.read(cx).value().to_string();
        let code = self.code(cx);
        let content: AnyElement = match code {
            None => parts::hint("Type text or a link to make its code.", cx).into_any_element(),
            Some(Err(err)) => parts::failure(format!("{err:#}"), cx).into_any_element(),
            Some(Ok(code)) => {
                let said = format!(
                    "Version {} · {} × {} squares · {} bytes",
                    code.version,
                    code.width,
                    code.width,
                    text.len()
                );
                h_flex()
                    .items_start()
                    .gap_4()
                    .child(picture(code, side))
                    .child(
                        v_flex()
                            .items_start()
                            .gap_2()
                            .child(parts::badge(
                                "Scans from the screen or on paper",
                                Tone::Good,
                                cx,
                            ))
                            .child(parts::label(said, cx))
                            .child(
                                parts::action("qr-copy", Icons::Image, "Copy picture")
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_picture(cx))),
                            )
                            .child(
                                parts::action(
                                    "qr-save",
                                    gpui_kit::component::IconName::ArrowDown,
                                    "Save as PNG…",
                                )
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                            ),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                h_flex()
                    .gap_2()
                    .child(parts::label("Can survive damage", cx))
                    .child(parts::modes(
                        "utility-modes",
                        &["7%", "15%", "25%", "30%"],
                        self.correction,
                        cx.listener(|this, ix: &usize, _, cx| {
                            this.correction = *ix;
                            cx.notify();
                        }),
                    )),
            ))
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(parts::label("Text", cx).flex_1())
                    .child(
                        parts::action("utility-paste", Icons::ClipboardPaste, "Paste").on_click(
                            cx.listener(|this, _, window, cx| {
                                if let Some(text) = parts::clipboard_text(cx) {
                                    this.take(text, window, cx);
                                }
                            }),
                        ),
                    )
                    .child(
                        parts::action("utility-clear", Icons::Eraser, "Clear")
                            .disabled(text.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.take(String::new(), window, cx);
                                this.focus(window, cx);
                            })),
                    ),
            )
            .child(
                Textarea::new(&self.text)
                    .h(ph_ui::rems_from_px(72.))
                    .flex_none(),
            )
            .child(div().flex_1().min_h_0().pt_2().child(content))
    }
}
