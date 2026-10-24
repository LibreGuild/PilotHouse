//! The project's agent instruction files, and syncing them so every agent
//! reads `AGENTS.md` (see `ph_agent::instructions`).

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};
use ph_agent::instructions::{self, Change, InstructionFile};
use ph_ui::Icons;

pub fn open(root: PathBuf, window: &mut Window, cx: &mut App) {
    let view = cx.new(|_| InstructionsView::new(root));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Agent instructions")
            .w(px(560.))
            .child(view.clone())
    });
}

struct InstructionsView {
    root: PathBuf,
    files: Vec<InstructionFile>,
    plan: Vec<Change>,
    message: Option<(SharedString, bool)>,
}

impl InstructionsView {
    fn new(root: PathBuf) -> Self {
        let mut view = Self {
            root,
            files: Vec::new(),
            plan: Vec::new(),
            message: None,
        };
        view.reload();
        view
    }

    fn reload(&mut self) {
        match instructions::read(&self.root) {
            Ok(files) => {
                self.plan = instructions::plan(&files);
                self.files = files;
            }
            Err(err) => {
                self.files.clear();
                self.plan.clear();
                self.message = Some((format!("{err:#}").into(), true));
            }
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        self.message = Some(match instructions::apply(&self.root, &self.plan) {
            Ok(written) => (
                format!(
                    "Updated {} file{}.",
                    written.len(),
                    if written.len() == 1 { "" } else { "s" }
                )
                .into(),
                false,
            ),
            Err(err) => (format!("{err:#}").into(), true),
        });
        self.reload();
        cx.notify();
    }
}

/// What a file holds, in a few words.
fn status(file: &InstructionFile) -> String {
    match file.text.as_deref() {
        None => "Missing".into(),
        Some(text) if file.imports_shared() => {
            let own = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
                .saturating_sub(1);
            if own == 0 {
                "Imports AGENTS.md".into()
            } else {
                format!(
                    "Imports AGENTS.md, plus {own} line{} of its own",
                    if own == 1 { "" } else { "s" }
                )
            }
        }
        Some(text) => {
            let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
            format!(
                "{lines} line{} of instructions",
                if lines == 1 { "" } else { "s" }
            )
        }
    }
}

impl Render for InstructionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let reader = |name: &str| match name {
            "AGENTS.md" => "Codex, Antigravity and most agents",
            "CLAUDE.md" => "Claude Code",
            _ => "Gemini CLI",
        };
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(muted).child(
                "Each agent reads its own file. Sync makes AGENTS.md the one place to write instructions: the others import it, and nothing is deleted.",
            ))
            .children(self.files.iter().map(|file| {
                let path = self.root.join(file.name);
                let exists = file.text.is_some();
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .child(Icon::new(Icons::FileText).small().text_color(if exists { cx.theme().info } else { muted }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().child(file.name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("{} · read by {}", status(file), reader(file.name))),
                            ),
                    )
                    .when(exists, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("open-{}", file.name)))
                                .xsmall()
                                .ghost()
                                .label("Open")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    if let Err(err) = crate::open::open_paths(std::slice::from_ref(&path), cx) {
                                        tracing::warn!("opening {} failed: {err:#}", path.display());
                                    }
                                }),
                        )
                    })
            }))
            .when(!self.plan.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(div().text_xs().text_color(muted).child("Sync will:"))
                        .children(self.plan.iter().map(|change| {
                            div()
                                .text_sm()
                                .child(format!("• {}: {}", change.name, change.summary))
                        })),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .when_some(self.message.clone(), |this, (message, is_error)| {
                        this.child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(if is_error { cx.theme().danger } else { cx.theme().success })
                                .child(message),
                        )
                    })
                    .child(div().flex_1())
                    .child(if self.plan.is_empty() {
                        div().text_sm().text_color(muted).child("Every agent reads AGENTS.md.").into_any_element()
                    } else {
                        Button::new("sync-instructions")
                            .small()
                            .primary()
                            .label("Sync")
                            .on_click(cx.listener(|this, _, _, cx| this.sync(cx)))
                            .into_any_element()
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_read_plainly() {
        let file = |text: Option<&str>| InstructionFile {
            name: "CLAUDE.md",
            text: text.map(str::to_string),
        };
        assert_eq!(status(&file(None)), "Missing");
        assert_eq!(status(&file(Some("@AGENTS.md\n"))), "Imports AGENTS.md");
        assert_eq!(
            status(&file(Some("@AGENTS.md\n\nUse tabs.\n"))),
            "Imports AGENTS.md, plus 1 line of its own"
        );
        assert_eq!(status(&file(Some("a\n\nb\n"))), "2 lines of instructions");
    }
}
