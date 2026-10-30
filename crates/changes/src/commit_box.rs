//! The commit box at the top of the Git tab: what is staged, a message
//! (typed, or written by an agent from the staged diff), amend, commit.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Textarea, TextareaState},
    notification::Notification,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Task, Window, div,
};

use crate::repo::Repo;

/// Writes text with an agent: `(instructions and material, project folder)`.
/// Set by the workspace, which knows the agents.
pub type Writer = Rc<dyn Fn(String, PathBuf, &mut App) -> Task<anyhow::Result<String>>>;

/// Diff sent to the agent for a message; enough to describe any commit.
const DIFF_LIMIT: usize = 16_000;

pub struct CommitBox {
    repo: Entity<Repo>,
    message: Entity<TextareaState>,
    amend: bool,
    writer: Option<Writer>,
    writing: bool,
    committing: bool,
    _task: Option<Task<()>>,
}

impl CommitBox {
    pub fn new(repo: Entity<Repo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Commit message")
                .auto_grow(3, 10)
        });
        cx.observe(&repo, |_, _, cx| cx.notify()).detach();
        Self {
            repo,
            message,
            amend: false,
            writer: None,
            writing: false,
            committing: false,
            _task: None,
        }
    }

    pub fn set_writer(&mut self, writer: Writer) {
        self.writer = Some(writer);
    }

    fn root(&self, cx: &App) -> PathBuf {
        self.repo.read(cx).root().to_path_buf()
    }

    fn toggle_amend(&mut self, amend: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.amend = amend;
        // Amending starts from the last message.
        if amend && self.message.read(cx).value().trim().is_empty() {
            let root = self.root(cx);
            let read = cx.background_spawn(async move { ph_git::commit::last_message(&root) });
            self._task = Some(cx.spawn_in(window, async move |this, cx| {
                if let Ok(message) = read.await {
                    _ = this.update_in(cx, |this, window, cx| {
                        this.message
                            .update(cx, |input, cx| input.set_value(message, window, cx));
                    });
                }
            }));
        }
        cx.notify();
    }

    /// Asks the agent for a message describing the staged diff or, with
    /// nothing staged, every change (what Commit All commits), as VS Code
    /// does.
    fn write_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(writer) = self.writer.clone() else {
            return;
        };
        let root = self.root(cx);
        let staged = self
            .repo
            .read(cx)
            .status()
            .is_some_and(|status| !status.staged.is_empty());
        self.writing = true;
        cx.notify();
        let diff = cx.background_spawn({
            let root = root.clone();
            async move {
                if staged {
                    ph_git::staged_diff(&root)
                } else {
                    ph_git::all_changes_diff(&root)
                }
            }
        });
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let prompt = match diff.await {
                Ok(diff) if !diff.trim().is_empty() => commit_prompt(&diff),
                Ok(_) => {
                    _ = this.update_in(cx, |this, window, cx| {
                        this.writing = false;
                        window.push_notification(
                            Notification::warning("There are no changes to describe."),
                            cx,
                        );
                    });
                    return;
                }
                Err(err) => {
                    _ = this.update_in(cx, |this, window, cx| {
                        this.writing = false;
                        window.push_notification(Notification::error(format!("{err:#}")), cx);
                    });
                    return;
                }
            };
            let Ok(task) = this.update(cx, |_, cx| writer(prompt, root, cx)) else {
                return;
            };
            let result = task.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.writing = false;
                match result {
                    Ok(text) => {
                        let text = clean_message(&text);
                        this.message
                            .update(cx, |input, cx| input.set_value(text, window, cx));
                    }
                    Err(err) => window.push_notification(
                        Notification::error(format!("{err:#}")).title("Could not write a message"),
                        cx,
                    ),
                }
                cx.notify();
            });
        }));
    }

    /// Commits what is staged; with nothing staged, every change (Commit
    /// All, VS Code's smart commit).
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message.read(cx).value().to_string();
        let (root, amend) = (self.root(cx), self.amend);
        let all = !amend
            && self
                .repo
                .read(cx)
                .status()
                .is_some_and(|status| status.staged.is_empty() && !status.changes.is_empty());
        self.committing = true;
        cx.notify();
        let work = cx.background_spawn(async move {
            if all {
                ph_git::stage_all(&root)?;
            }
            ph_git::commit::commit(&root, &message, amend)
        });
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.committing = false;
                match result {
                    Ok(id) => {
                        this.amend = false;
                        this.message
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        window.push_notification(
                            Notification::success(format!("Committed {id}")),
                            cx,
                        );
                    }
                    Err(err) => window.push_notification(
                        Notification::error(format!("{err:#}")).title("Commit failed"),
                        cx,
                    ),
                }
                this.repo.update(cx, |repo, cx| repo.refresh(cx));
                cx.notify();
            });
        }));
    }

    fn unstage(&mut self, path: String, cx: &mut Context<Self>) {
        self.repo.update(cx, |repo, cx| {
            repo.run(
                "Unstaging",
                move |root| {
                    ph_git::unstage_path(root, &path)?;
                    Ok(format!("Unstaged {path}"))
                },
                cx,
            )
        });
    }
}

impl Render for CommitBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (staged, changed) = self
            .repo
            .read(cx)
            .status()
            .map(|status| (status.staged.clone(), !status.changes.is_empty()))
            .unwrap_or_default();
        let has_message = !self.message.read(cx).value().trim().is_empty();
        // Nothing staged: Commit All keeps every change first.
        let commit_all = staged.is_empty() && changed && !self.amend;
        let can_commit =
            has_message && (!staged.is_empty() || self.amend || changed) && !self.committing;
        v_flex()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(Textarea::new(&self.message).small())
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Checkbox::new("commit-amend")
                            .label("Amend")
                            .checked(self.amend)
                            .on_click(cx.listener(|this, checked: &bool, window, cx| {
                                this.toggle_amend(*checked, window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .when(self.writer.is_some(), |this| {
                        this.child(
                            Button::new("write-commit-message")
                                .small()
                                .ghost()
                                .icon(ph_ui::Icons::Sparkles)
                                .label(if self.writing {
                                    "Writing…"
                                } else {
                                    "Write message"
                                })
                                .disabled(self.writing || (staged.is_empty() && !changed))
                                .tooltip(if staged.is_empty() {
                                    "An agent writes the message from every change"
                                } else {
                                    "An agent writes the message from the staged changes"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.write_message(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("commit")
                            .small()
                            .primary()
                            .when(self.committing, |this| this.child(Spinner::new().xsmall()))
                            .label(if self.amend {
                                "Amend commit"
                            } else if commit_all {
                                "Commit All"
                            } else {
                                "Commit"
                            })
                            .when(commit_all, |this| {
                                this.tooltip("Nothing is kept yet: keeps every change, new files too, and commits them")
                            })
                            .disabled(!can_commit)
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    ),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if staged.is_empty() {
                                SharedString::from(if changed {
                                    "Nothing kept yet: Commit All commits every change, or Keep files below."
                                } else {
                                    "No changes."
                                })
                            } else {
                                format!("Staged ({})", staged.len()).into()
                            }),
                    )
                    .children(staged.into_iter().enumerate().map(|(ix, path)| {
                        let icon = ph_ui::file_icon_element(&path, gpui_kit::px(14.));
                        let unstage = path.clone();
                        h_flex()
                            .id(("staged", ix))
                            .group("staged-row")
                            .gap_1p5()
                            .px_1()
                            .rounded(cx.theme().radius)
                            .hover(|style| style.bg(cx.theme().list_hover))
                            .text_sm()
                            .child(icon)
                            .child(div().flex_1().truncate().child(path))
                            .child(
                                div()
                                    .invisible()
                                    .group_hover("staged-row", |style| style.visible())
                                    .child(
                                        Button::new(("unstage", ix))
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Minus)
                                            .tooltip("Unstage")
                                            .accessibility_label("Unstage")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.unstage(unstage.clone(), cx)
                                            })),
                                    ),
                            )
                    })),
            )
    }
}

/// The instructions for a commit message, with the diff (cut to fit).
pub fn commit_prompt(diff: &str) -> String {
    format!(
        "Write a git commit message for the staged changes below. Use the \
         conventional style: a summary line under 72 characters in the \
         imperative mood, then a blank line and a short body only if it adds \
         something. Reply with the message only, no code fences, no \
         commentary, and do not use any tools.\n\n```diff\n{}\n```",
        cut(diff, DIFF_LIMIT)
    )
}

/// At most `limit` bytes of `text`, on a character boundary, marked if cut.
pub fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut: {} more bytes)", &text[..end], text.len() - end)
}

/// Strips what agents sometimes add around the message (fences, quotes).
pub fn clean_message(text: &str) -> String {
    let text = text.trim();
    let text = text
        .strip_prefix("```")
        .map(|rest| rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest))
        .and_then(|rest| rest.trim_end().strip_suffix("```"))
        .unwrap_or(text);
    text.trim().trim_matches('"').trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_agent_replies() {
        assert_eq!(clean_message("```\nFix the thing\n```"), "Fix the thing");
        assert_eq!(
            clean_message("```text\nFix it\n\nBody\n```\n"),
            "Fix it\n\nBody"
        );
        assert_eq!(clean_message("\"Add a\""), "Add a");
        assert_eq!(clean_message("  Plain  "), "Plain");
    }

    #[test]
    fn cuts_on_a_char_boundary() {
        assert_eq!(cut("abc", 10), "abc");
        let cut_text = cut("ééé", 3);
        assert!(cut_text.starts_with('é') && cut_text.contains("cut"));
    }
}
