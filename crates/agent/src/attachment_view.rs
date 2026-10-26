//! Attachment chips you can look inside: resting on one shows a large
//! preview, clicking it (or the preview) opens the whole image or text in a
//! dialog. Used for the
//! composer's chips and for attachments in sent messages.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    hover_card::HoverCard,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClipboardItem, ElementId, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, img, px,
};
use ph_ui::Icons;

/// Most of a file read for a preview; the dialog says when it is cut.
const FILE_PREVIEW_BYTES: usize = 256 * 1024;
/// Lines shown in the hover card.
const CARD_LINES: usize = 24;
/// How long the pointer rests on an attachment before its preview shows, so
/// passing over one does not pop it up.
const CARD_DELAY: Duration = Duration::from_secs(2);

/// What an attachment shows when you look inside.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    Image(PathBuf),
    Text(String),
    /// A file on disk, read when shown.
    File(PathBuf),
    None,
}

/// Builds the preview when it is shown (cards and files are read then).
pub type PreviewSource = Rc<dyn Fn(&App) -> Preview>;

/// A chip: thumbnail or icon, label, optional trailing button (remove).
pub fn chip(
    id: impl Into<ElementId>,
    label: String,
    icon: Icons,
    thumbnail: Option<PathBuf>,
    preview: PreviewSource,
    trailing: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let id = id.into();
    let leading: AnyElement = match &thumbnail {
        Some(path) => img(path.clone())
            .size_6()
            .rounded(cx.theme().radius)
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => Icon::new(icon)
            .xsmall()
            .text_color(cx.theme().muted_foreground)
            .into_any_element(),
    };
    let open = {
        let (preview, label) = (preview.clone(), label.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            open_dialog(label.clone(), preview(cx), window, cx)
        }
    };
    let body = h_flex()
        .id(id.clone())
        .role(gpui_kit::accesskit::Role::Button)
        .aria_label(label.clone())
        .gap_1p5()
        .pl_1()
        .pr_0p5()
        .py_0p5()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted)
        .text_xs()
        .cursor_pointer()
        .hover(|style| {
            style
                .border_color(cx.theme().ring)
                .bg(cx.theme().secondary_hover)
        })
        .on_click(open)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .min_w_5()
                .child(leading),
        )
        .child(div().max_w_48().truncate().pr_1().child(label.clone()))
        .children(trailing)
        .test_support();
    hover(id, label, body.into_any_element(), preview)
}

/// A sent image shown as a thumbnail, with the same hover and click.
pub fn thumbnail(id: impl Into<ElementId>, label: String, path: PathBuf, cx: &App) -> AnyElement {
    let id = id.into();
    let preview: PreviewSource = {
        let path = path.clone();
        Rc::new(move |_| Preview::Image(path.clone()))
    };
    let open = {
        let (preview, label) = (preview.clone(), label.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            open_dialog(label.clone(), preview(cx), window, cx)
        }
    };
    let body = div()
        .id(id.clone())
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .overflow_hidden()
        .cursor_pointer()
        .hover(|style| style.border_color(cx.theme().ring))
        .on_click(open)
        .child(img(path).h_24().max_w_48().object_fit(ObjectFit::Cover));
    hover(id, label, body.into_any_element(), preview)
}

fn hover(id: ElementId, label: String, trigger: AnyElement, preview: PreviewSource) -> AnyElement {
    HoverCard::new(SharedString::from(format!("{id}-hover")))
        .open_delay(CARD_DELAY)
        .trigger(trigger)
        .content(move |_, _, cx| {
            let shown = preview(cx);
            let open = {
                let (preview, label) = (preview.clone(), label.clone());
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                    open_dialog(label.clone(), preview(cx), window, cx)
                }
            };
            div()
                .id("attachment-card")
                .cursor_pointer()
                .on_click(open)
                .child(card(shown, cx))
                .into_any_element()
        })
        .into_any_element()
}

/// The hover card: a large image or the first lines of text.
fn card(preview: Preview, cx: &App) -> AnyElement {
    let hint = div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child("Click to open");
    match preview {
        Preview::Image(path) => v_flex()
            .gap_1p5()
            .child(
                img(path)
                    .max_w(px(480.))
                    .max_h(px(340.))
                    .object_fit(ObjectFit::Contain)
                    .rounded(cx.theme().radius),
            )
            .child(hint)
            .into_any_element(),
        Preview::Text(text) => text_card(&text, None, hint, cx),
        Preview::File(path) => match read_text(&path) {
            Ok((text, _)) => text_card(&text, Some(&path), hint, cx),
            Err(reason) => file_note(&path, &reason, cx),
        },
        Preview::None => div().into_any_element(),
    }
}

fn text_card(text: &str, path: Option<&Path>, hint: gpui_kit::Div, cx: &App) -> AnyElement {
    let lines: Vec<&str> = text.lines().take(CARD_LINES).collect();
    let more = text.lines().count() > CARD_LINES;
    v_flex()
        .w(px(460.))
        .gap_1p5()
        .when_some(path, |this, path| {
            this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(short_path(path)),
            )
        })
        .child(
            div()
                .p_2()
                .rounded(cx.theme().radius)
                .bg(cx.theme().secondary)
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .overflow_hidden()
                .child(lines.join("\n"))
                .when(more, |this| {
                    this.child(div().text_color(cx.theme().muted_foreground).child("…"))
                }),
        )
        .child(hint)
        .into_any_element()
}

fn file_note(path: &Path, reason: &str, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .text_xs()
        .child(div().truncate().child(short_path(path)))
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(reason.to_string()),
        )
        .into_any_element()
}

/// The last few parts of a path: enough to place a file, short enough to read.
fn short_path(path: &Path) -> String {
    let parts: Vec<_> = path.components().collect();
    let tail = parts.len().saturating_sub(3);
    let shown: PathBuf = parts[tail..].iter().collect();
    if tail > 0 {
        format!("…{}{}", std::path::MAIN_SEPARATOR, shown.display())
    } else {
        shown.display().to_string()
    }
}

/// Opens the whole attachment in a dialog.
pub fn open_dialog(label: String, preview: Preview, window: &mut Window, cx: &mut App) {
    let body: Rc<dyn Fn(&App) -> AnyElement> = match preview {
        Preview::None => return,
        Preview::Image(path) => Rc::new(move |cx| {
            div()
                .flex()
                .justify_center()
                .child(
                    img(path.clone())
                        .max_w_full()
                        .max_h(px(620.))
                        .object_fit(ObjectFit::Contain)
                        .rounded(cx.theme().radius),
                )
                .into_any_element()
        }),
        Preview::Text(text) => {
            let text: SharedString = text.into();
            Rc::new(move |cx| full_text(text.clone(), None, cx))
        }
        Preview::File(path) => {
            let shown = read_text(&path);
            Rc::new(move |cx| match &shown {
                Ok((text, cut)) => full_text(text.clone().into(), cut.then_some(&path), cx),
                Err(reason) => file_note(&path, reason, cx),
            })
        }
    };
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog.title(label.clone()).w(px(860.)).child(body(cx))
    });
}

fn full_text(text: SharedString, cut: Option<&PathBuf>, cx: &App) -> AnyElement {
    let copy = text.clone();
    v_flex()
        .gap_2()
        .child(
            div()
                .id("attachment-text")
                .max_h(px(560.))
                .overflow_y_scroll()
                .p_3()
                .rounded(cx.theme().radius)
                .bg(cx.theme().secondary)
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .child(text),
        )
        .child(
            h_flex()
                .justify_between()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(match cut {
                    Some(_) => "Showing the first 256 KB.",
                    None => "",
                })
                .child(
                    Button::new("copy-attachment")
                        .small()
                        .ghost()
                        .icon(IconName::Copy)
                        .label("Copy")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()))
                        }),
                ),
        )
        .into_any_element()
}

/// The start of a text file, and whether it was cut. Binary files are not
/// shown.
fn read_text(path: &Path) -> Result<(String, bool), String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|err| format!("Cannot read it: {err}"))?;
    let mut bytes = Vec::new();
    file.take(FILE_PREVIEW_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Cannot read it: {err}"))?;
    let cut = bytes.len() > FILE_PREVIEW_BYTES;
    bytes.truncate(FILE_PREVIEW_BYTES);
    if bytes.contains(&0) {
        return Err("Binary file".into());
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), cut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_and_refuses_binary() {
        let dir = std::env::temp_dir().join(format!("ph-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("a.txt");
        std::fs::write(&text, "one\ntwo").unwrap();
        assert_eq!(read_text(&text).unwrap(), ("one\ntwo".to_string(), false));
        let big = dir.join("big.txt");
        std::fs::write(&big, "x".repeat(FILE_PREVIEW_BYTES + 10)).unwrap();
        let (shown, cut) = read_text(&big).unwrap();
        assert!(cut && shown.len() == FILE_PREVIEW_BYTES);
        let binary = dir.join("b.bin");
        std::fs::write(&binary, [0u8, 1, 2]).unwrap();
        assert!(read_text(&binary).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
