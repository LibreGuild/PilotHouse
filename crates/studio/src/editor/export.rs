//! Export: how the result is saved (format, quality, a size limit), with
//! what the file would weigh, worked out only while this is open.

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    input::Input,
    menu::PopupMenuItem,
    slider::Slider,
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, ParentElement as _, Styled as _, Window, div, px,
};
use ph_ui::{Icons, MenuButton as _};

use super::ImageEditor;
use super::bar::Knob;
use crate::document::Estimate;
use crate::encode::Format;

impl ImageEditor {
    /// Opens the Export dialog.
    pub(crate) fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.read(cx).original().is_none() {
            return;
        }
        self.doc.update(cx, |doc, cx| doc.set_estimating(true, cx));
        let editor = cx.entity();
        let stop = {
            let doc = self.doc.clone();
            move |cx: &mut App| doc.update(cx, |doc, cx| doc.set_estimating(false, cx))
        };
        let (on_close, on_cancel) = (stop.clone(), stop.clone());
        window.open_dialog(cx, move |dialog, _, cx| {
            let (on_close, on_cancel) = (on_close.clone(), on_cancel.clone());
            let save = editor.clone();
            let stop = stop.clone();
            dialog
                .title("Export")
                .w(px(420.))
                .child(body(&editor, cx))
                .footer(
                    DialogFooter::new()
                        .w_full()
                        .child(div().flex_1())
                        .child(
                            Button::new("studio-export-cancel")
                                .ghost()
                                .label("Cancel")
                                .on_click({
                                    let stop = stop.clone();
                                    move |_, window, cx| {
                                        stop(cx);
                                        window.close_dialog(cx);
                                    }
                                }),
                        )
                        .child(
                            Button::new("studio-export-save")
                                .primary()
                                .icon(Icons::Download)
                                .label("Save a copy…")
                                .on_click(move |_, window, cx| {
                                    stop(cx);
                                    window.close_dialog(cx);
                                    save.update(cx, |editor, cx| editor.save_copy(window, cx));
                                }),
                        ),
                )
                .on_close(move |_, _, cx| on_close(cx))
                .on_cancel(move |_, _, cx| {
                    on_cancel(cx);
                    true
                })
        });
    }
}

/// The dialog's settings, read from the editor each time it draws.
fn body(editor: &Entity<ImageEditor>, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let this = editor.read(cx);
    let doc = this.doc.read(cx);
    let export = doc.export().clone();
    let estimate = doc.estimate().cloned();
    let quality = this.controls.slider(Knob::Quality).clone();
    let limit = this.controls.limit.clone();
    let doc_entity = this.doc.clone();

    let pick = doc_entity.clone();
    let format_menu = Button::new("studio-format")
        .outline()
        .small()
        .label(export.format.label())
        .dropdown_caret(true)
        .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
            Format::ALL.into_iter().fold(menu, |menu, format| {
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
    let (weight, failed) = match &estimate {
        Some(Estimate::Done {
            bytes,
            width,
            height,
            note,
        }) => (
            format!(
                "{width} × {height}, about {}{}",
                crate::control::kilobytes(*bytes),
                note.as_ref()
                    .map(|note| format!(" ({note})"))
                    .unwrap_or_default()
            ),
            false,
        ),
        Some(Estimate::Failed(error)) => (error.clone(), true),
        _ => ("Working out the size…".to_string(), false),
    };
    let label = |text: &'static str| div().w(px(72.)).text_sm().child(text);
    let palette_doc = doc_entity.clone();
    v_flex()
        .gap_3()
        .child(h_flex().gap_2().child(label("Format")).child(format_menu))
        .when(export.format.has_quality(), |this| {
            this.child(
                h_flex()
                    .gap_2()
                    .child(label("Quality"))
                    .child(div().flex_1().child(Slider::new(&quality)))
                    .child(div().w(px(28.)).text_sm().child(export.quality.to_string())),
            )
        })
        .when(export.format == Format::Png, |this| {
            this.child(
                h_flex().gap_2().child(label("")).child(
                    Switch::new("studio-palette")
                        .checked(export.palette)
                        .label("256 colors: often a third of the size")
                        .on_click(move |checked: &bool, _, cx| {
                            let checked = *checked;
                            palette_doc
                                .update(cx, |doc, cx| doc.set_export(|e| e.palette = checked, cx))
                        }),
                ),
            )
        })
        .child(
            h_flex()
                .gap_2()
                .child(label("At most"))
                .child(div().w(px(110.)).child(Input::new(&limit).small()))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("KB"),
                ),
        )
        .child(
            div()
                .p_2()
                .rounded(theme.radius)
                .bg(theme.muted.opacity(0.6))
                .text_sm()
                .text_color(if failed {
                    theme.danger
                } else {
                    theme.foreground
                })
                .child(weight),
        )
        .child(div().text_xs().text_color(theme.muted_foreground).child(
            if export.format.keeps_transparency() {
                "Saved pictures carry no camera or location data."
            } else {
                "See-through parts become white. Saved pictures carry no camera or location data."
            },
        ))
        .into_any_element()
}
