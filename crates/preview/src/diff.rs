//! Read-only diffs, shared by `.diff` files, the Git tab's review list,
//! commit details and stashes: lines tinted by kind, long hunks cut short.

use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, App, IntoElement, ParentElement as _, Styled as _, div};
use ph_git::{FileChange, Hunk};

/// Lines drawn per hunk before "… N more lines" (keeps huge diffs fast).
pub const MAX_HUNK_LINES: usize = 300;

/// A hunk's lines (without its header row).
pub fn hunk_lines(hunk: &Hunk, cx: &App) -> Vec<AnyElement> {
    let added_bg = cx.theme().success.opacity(0.12);
    let removed_bg = cx.theme().danger.opacity(0.12);
    let mono = cx.theme().mono_font_family.clone();
    let hidden = hunk.lines.len().saturating_sub(MAX_HUNK_LINES);
    let mut rows: Vec<AnyElement> = hunk
        .lines
        .iter()
        .take(MAX_HUNK_LINES)
        .map(|line| {
            let row = div()
                .px_3()
                .font_family(mono.clone())
                .whitespace_nowrap()
                .overflow_hidden()
                .child(line.clone());
            match line.chars().next() {
                Some('+') => row.bg(added_bg),
                Some('-') => row.bg(removed_bg),
                _ => row.text_color(cx.theme().muted_foreground),
            }
            .into_any_element()
        })
        .collect();
    if hidden > 0 {
        rows.push(
            div()
                .px_3()
                .py_1()
                .text_color(cx.theme().muted_foreground)
                .child(format!("… {hidden} more lines"))
                .into_any_element(),
        );
    }
    rows
}

/// One file's whole diff, read-only.
pub fn file_diff(file: &FileChange, cx: &App) -> AnyElement {
    let (icon, tint) = ph_ui::file_icon(&file.path);
    v_flex()
        // Lists of diffs scroll; a card must not shrink to fit.
        .flex_none()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .overflow_hidden()
        .text_xs()
        .child(
            h_flex()
                .px_2()
                .py_1()
                .gap_1p5()
                .bg(cx.theme().secondary)
                .child(Icon::new(icon).text_color(tint))
                .child(div().flex_1().truncate().text_sm().child(file.path.clone()))
                .child(div().text_color(cx.theme().muted_foreground).child(format!(
                    "+{} −{}",
                    file.added(),
                    file.removed()
                ))),
        )
        .when(file.binary, |this| {
            this.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(cx.theme().muted_foreground)
                    .child("Binary file"),
            )
        })
        .children(file.hunks.iter().map(|hunk| {
            v_flex()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .truncate()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(hunk.header.clone()),
                )
                .children(hunk_lines(hunk, cx))
        }))
        .into_any_element()
}

/// A `.diff` or `.patch` file: each file's changes, tinted.
#[derive(Default)]
pub struct DiffPreview {
    files: Vec<FileChange>,
}

impl DiffPreview {
    pub fn set_text(&mut self, text: &str, cx: &mut gpui_kit::Context<Self>) {
        self.files = ph_git::parse_diff(text);
        cx.notify();
    }
}

impl gpui_kit::Render for DiffPreview {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::{InteractiveElement as _, StatefulInteractiveElement as _};
        if self.files.is_empty() {
            return crate::notice("No changes in this patch.", cx);
        }
        div()
            .id("diff-preview")
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().background)
            .p_3()
            .child(
                v_flex()
                    .gap_3()
                    .children(self.files.iter().map(|file| file_diff(file, cx))),
            )
            .into_any_element()
    }
}
