//! *Why is this here?* for a line in the editor.
//!
//! Git knows which commit last wrote the line, and that commit's content
//! picks its explanation (`ph_review::explanation::find`). So a line a
//! teammate rewrote yesterday shows their explanation, and the lines around
//! it that they left alone still show the older one: there is no "newest
//! explanation of the file" to choose.
//!
//! - The commit has an explanation: it is shown, with a word when the file
//!   has changed since and which explanation is newer.
//! - The line is not committed yet: the request in a chat that wrote it,
//!   and the explanation of the work in progress if there is one.
//! - The commit has none: what Git knows about it, an offer to have it
//!   explained now, and the newest explanation of the file, marked for what
//!   it is. An older explanation is never shown as the answer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div,
};
use ph_agent::reply::{OnLink, Reply};
use ph_git::content::LineOrigin;
use ph_review::explanation::find::{self, Found};
use ph_review::explanation::prompt::Asked;
use ph_review::explanation::short_of;
use ph_review::walkthrough::parse_file_link;
use ph_store::Store;
use ph_store::projects::ProjectId;
use ph_ui::Icons;

use crate::Target;

/// What there is to say about a line.
#[derive(Debug, Clone)]
pub enum Answer {
    /// The commit that wrote it has an explanation. `behind`: the file has
    /// changed since; `newer`: the explanation of what stands there now.
    Explained {
        found: Box<Found>,
        behind: bool,
        newer: Option<Box<Found>>,
    },
    /// It is not committed yet: the request in a chat that wrote it, and
    /// the explanation of the work in progress, where there is one.
    Pending {
        asked: Option<Asked>,
        found: Option<Box<Found>>,
    },
    /// The commit that wrote it has no explanation. `other`: the newest
    /// explanation that names the file, which is about another change.
    Unexplained { other: Option<Box<Found>> },
}

/// A line and what there is to say about it.
#[derive(Debug, Clone)]
pub struct Why {
    /// The repository's top folder, and the file's path from it.
    pub top: PathBuf,
    pub path: String,
    /// 1-based.
    pub line: u32,
    pub origin: LineOrigin,
    pub answer: Answer,
}

/// Finds out why line `line` (1-based) of `file` (a full path) is there.
/// Reads Git, files and the workspace's chats: not for the UI thread.
pub fn look_up(store: &Store, project_id: ProjectId, file: &Path, line: u32) -> Result<Why> {
    let folder = file.parent().context("the file has no folder")?;
    let top = ph_git::content::top(folder).context("this file is not in a Git repository")?;
    // As Git names it (a link in the path, a network drive).
    let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let top_real = std::fs::canonicalize(&top).unwrap_or_else(|_| top.clone());
    let path = file
        .strip_prefix(&top_real)
        .or_else(|_| file.strip_prefix(&top))
        .context("the file is outside its repository")?
        .to_string_lossy()
        .replace('\\', "/");
    let origin =
        ph_git::content::line_origin(&top, &path, line)?.context("Git does not know this line")?;
    let answer = if origin.uncommitted() {
        let asked =
            ph_agent::explanations::asked_for_line(store, project_id, &top, &path, &origin.text);
        let pending = ph_git::content::pending(&top).unwrap_or_default();
        let now = pending
            .iter()
            .find(|(file, _)| *file == path)
            .map(|(_, blob)| blob.clone());
        // Only one that explains this file as it is now.
        let found = find::of_content(store, &top, &pending, Some(&path))
            .into_iter()
            .next()
            .filter(|found| {
                now.as_ref()
                    .is_some_and(|now| found.files.contains(&(path.clone(), now.clone())))
            });
        Answer::Pending {
            asked,
            found: found.map(Box::new),
        }
    } else {
        // The file may have had another name in that commit.
        let found = find::of_commit(store, &top, &origin.commit, Some(&origin.path))?
            .into_iter()
            .next();
        match found {
            Some(found) => {
                let behind = !find::changed_since(&top, &found).is_empty();
                let newer = behind.then(|| find::newer(store, &top, &found)).flatten();
                Answer::Explained {
                    found: Box::new(found),
                    behind,
                    newer: newer.map(Box::new),
                }
            }
            None => Answer::Unexplained {
                other: find::naming(store, &top, &path)
                    .into_iter()
                    .next()
                    .map(Box::new),
            },
        }
    };
    Ok(Why {
        top,
        path,
        line,
        origin,
        answer,
    })
}

#[derive(Debug, Clone)]
pub enum WhyEvent {
    /// Show chat `0` of the workspace.
    OpenChat(i64),
    /// Have an agent explain this.
    Explain(Target),
    /// Show this file (its full path) at this 1-based line.
    OpenFile { path: PathBuf, line: Option<u64> },
}

/// What a *Why is this here?* pop-up shows.
pub struct WhyView {
    why: Why,
    /// The explanation read in full, once asked for.
    full: Option<Entity<Reply>>,
    _reading: Option<gpui_kit::Task<()>>,
}

impl EventEmitter<WhyEvent> for WhyView {}

impl WhyView {
    pub fn new(why: Why) -> Self {
        Self {
            why,
            full: None,
            _reading: None,
        }
    }

    /// Opens the pop-up for `why`. The view is returned for its events
    /// (open a chat, explain a commit, show a file).
    pub fn open(why: Why, window: &mut Window, cx: &mut gpui_kit::App) -> Entity<Self> {
        let view = cx.new(|_| Self::new(why));
        let shown = view.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .title("Why is this here?")
                .w(ph_ui::scaled_px(640., window))
                .child(shown.clone())
        });
        view
    }

    /// The explanation there is to read in full, if any.
    fn readable(&self) -> Option<&Found> {
        match &self.why.answer {
            Answer::Explained { found, .. } => Some(found),
            Answer::Pending { found, .. } => found.as_deref(),
            Answer::Unexplained { other } => other.as_deref(),
        }
    }

    fn read_all(&mut self, cx: &mut Context<Self>) {
        let Some(found) = self.readable() else {
            return;
        };
        // Which references name a file is asked of the file system, which
        // can be a network drive: not on the UI thread.
        let (markdown, top) = (found.markdown.clone(), self.why.top.clone());
        let text = cx.background_spawn(async move { crate::linked(&top, &markdown) });
        self._reading = Some(cx.spawn(async move |this, cx| {
            let text = text.await;
            _ = this.update(cx, |this, cx| this.show_all(&text, cx));
        }));
    }

    fn show_all(&mut self, text: &str, cx: &mut Context<Self>) {
        let (this, top) = (cx.entity().downgrade(), self.why.top.clone());
        let on_link: OnLink = Arc::new(move |url, window, cx| match parse_file_link(url) {
            Some((path, line)) => {
                let path = top.join(path);
                window.close_dialog(cx);
                _ = this.update(cx, |_, cx| cx.emit(WhyEvent::OpenFile { path, line }));
            }
            None if url.starts_with("https://") || url.starts_with("http://") => cx.open_url(url),
            None => {}
        });
        self.full = Some(cx.new(|cx| Reply::new(text, cx).on_link(on_link)));
        cx.notify();
    }

    /// An explanation in short: its title, *In short*, who wrote it and
    /// when, and the way to all of it. Once all of it shows (below), only
    /// who wrote it and when: the rest would stand there twice.
    fn summary(
        &self,
        found: &Found,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let whole = self.full.is_some();
        v_flex()
            .gap_1p5()
            .when(!whole, |this| {
                this.child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(found.title.clone()),
                )
                .child(div().text_sm().child(short_of(&found.markdown)))
            })
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(crate::byline(found))
                    .when_some(note, |this, note| {
                        this.child(div().text_color(cx.theme().warning).child(note))
                    }),
            )
            .when(!whole, |this| {
                this.child(
                    h_flex().child(
                        Button::new("why-read")
                            .small()
                            .outline()
                            .label("Read all of it")
                            .on_click(cx.listener(|this, _, _, cx| this.read_all(cx))),
                    ),
                )
            })
    }

    /// The request that wrote the line, with the way to its chat.
    fn request(
        &self,
        asked: &Asked,
        lead: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let request: String = asked.request.trim().chars().take(400).collect();
        let more = if request.len() < asked.request.trim().len() {
            "…"
        } else {
            ""
        };
        let session = asked.session;
        v_flex()
            .gap_1()
            .child(div().text_xs().text_color(muted).child(format!(
                "{lead} “{}” with {}:",
                asked.chat.trim(),
                asked.agent.trim()
            )))
            .child(
                div()
                    .pl_2()
                    .border_l_2()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .child(format!("{request}{more}")),
            )
            .when_some(session, |this, session| {
                this.child(
                    h_flex().child(
                        Button::new("why-chat")
                            .xsmall()
                            .ghost()
                            .icon(Icons::MessageSquare)
                            .label("Open the chat")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                window.close_dialog(cx);
                                cx.emit(WhyEvent::OpenChat(session));
                            })),
                    ),
                )
            })
    }

    /// The button that asks for an explanation of `target`.
    fn explain_button(&self, target: Target, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let label = match &target {
            Target::Changes => "Explain these changes",
            Target::Commit { .. } => "Explain this commit",
        };
        h_flex().child(
            Button::new("why-explain")
                .small()
                .outline()
                .icon(Icons::Sparkles)
                .label(label)
                .on_click(cx.listener(move |_, _, window, cx| {
                    window.close_dialog(cx);
                    cx.emit(WhyEvent::Explain(target.clone()));
                })),
        )
    }
}

impl Render for WhyView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let origin = self.why.origin.clone();
        // The line asked about, as it reads.
        let line = div()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().muted)
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .truncate()
            .child(SharedString::from(format!(
                "{}:{}  {}",
                self.why.path,
                self.why.line,
                origin.text.trim()
            )));
        let commit = || {
            format!(
                "{} · {} · {}",
                origin.author,
                ph_ui::time::ago(origin.time),
                &origin.commit[..origin.commit.len().min(8)]
            )
        };
        let body = match self.why.answer.clone() {
            Answer::Explained {
                found,
                behind,
                newer,
            } => {
                let note = behind.then(|| match &newer {
                    Some(newer) => format!("Parts of this changed since. Newer: “{}”", newer.title),
                    None => "Parts of this changed since it was written".to_string(),
                });
                // Kept with it on this computer: what was asked for.
                let asked = found.asked().into_iter().next_back();
                v_flex()
                    .gap_3()
                    .child(self.summary(&found, note, cx))
                    .when_some(asked, |this, asked| {
                        this.child(self.request(&asked, "Asked for in", cx))
                    })
                    .child(div().text_xs().text_color(muted).child(format!(
                        "From the commit “{}” ({})",
                        origin.summary,
                        commit()
                    )))
            }
            Answer::Pending { asked, found } => v_flex()
                .gap_3()
                .child(div().text_sm().child("This line is not committed yet."))
                .when_some(asked.clone(), |this, asked| {
                    this.child(self.request(&asked, "It was written in", cx))
                })
                .when(asked.is_none(), |this| {
                    this.child(div().text_xs().text_color(muted).child(
                        "No chat here shows an edit of this file: it was written by hand, in a \
                         terminal, or by an agent whose edits are not recorded.",
                    ))
                })
                .map(|this| match &found {
                    Some(found) => this.child(self.summary(found, None, cx)),
                    None => this.child(self.explain_button(Target::Changes, cx)),
                }),
            Answer::Unexplained { other } => {
                let target = Target::Commit {
                    sha: origin.commit.clone(),
                    subject: origin.summary.clone(),
                };
                v_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(div().text_sm().child(origin.summary.clone()))
                            .child(div().text_xs().text_color(muted).child(commit())),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("Nobody has explained this change. Git only knows the above."),
                    )
                    .child(self.explain_button(target, cx))
                    .when_some(other, |this, other| {
                        // Never as the answer: it is about another change.
                        let when = if other.written <= origin.time {
                            "written before this change"
                        } else {
                            "about a later change"
                        };
                        this.child(
                            v_flex()
                                .gap_1p5()
                                .pt_3()
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div().text_xs().text_color(muted).child(format!(
                                        "The newest explanation of this file, {when}:"
                                    )),
                                )
                                .child(self.summary(&other, None, cx)),
                        )
                    })
            }
        };
        v_flex()
            .id("why-here")
            .gap_3()
            .max_h(ph_ui::rems_from_px(520.))
            .overflow_y_scroll()
            .child(line)
            .child(body)
            .when_some(self.full.clone(), |this, full| {
                this.child(
                    div()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(full),
                )
            })
    }
}
