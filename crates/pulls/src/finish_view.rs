//! Finishing a review: what goes to GitHub, decided in one place and sent
//! in one post. The agent's findings the reviewer ticks (the others stay on
//! this computer), the reviewer's own comments on lines, a note, and how
//! the review ends: a comment, an approval, a request for changes.
//!
//! Until this, nothing of a review has left the computer: findings can be
//! reworded or set aside, and comments written on the files wait here
//! ([`Draft`], kept in the settings so a restart does not lose them).

use std::collections::BTreeSet;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Textarea, TextareaState},
    menu::PopupMenuItem,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Task, Window, div,
};
use ph_github::models::DraftComment;
use ph_github::pulls::ReviewEvent;
use ph_review::github;
use ph_store::Store;
use ph_store::reviews::{Finding, PullRef};
use ph_ui::MenuButton as _;
use serde::{Deserialize, Serialize};

use crate::findings_view::{severity_color, tag_owned};

/// A comment of the reviewer's own on lines of a changed file, written but
/// not sent yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub path: String,
    /// The line in the new file (the last of several).
    pub line: u64,
    /// The first line, for a comment on several.
    #[serde(default)]
    pub start: Option<u64>,
    pub body: String,
}

impl Draft {
    /// `12`, or `12-30`.
    pub fn lines(&self) -> String {
        match self.start.filter(|start| *start < self.line) {
            Some(start) => format!("{start}-{}", self.line),
            None => self.line.to_string(),
        }
    }

    fn comment(&self) -> DraftComment {
        let start = self.start.filter(|start| *start < self.line);
        DraftComment {
            path: self.path.clone(),
            line: self.line,
            side: "RIGHT".into(),
            body: self.body.clone(),
            start_line: start,
            start_side: start.map(|_| "RIGHT".into()),
        }
    }
}

fn drafts_key(pull: &PullRef) -> String {
    format!(
        "review.drafts.{}/{}/{}#{}",
        pull.host, pull.owner, pull.repo, pull.number
    )
}

/// The reviewer's comments on `pull` not sent yet.
pub fn drafts(store: &Store, pull: &PullRef) -> Vec<Draft> {
    store.setting(&drafts_key(pull))
}

pub fn save_drafts(store: &Store, pull: &PullRef, drafts: &[Draft]) {
    if let Err(err) = store.set_setting(&drafts_key(pull), &drafts) {
        tracing::warn!("keeping the comments not sent yet failed: {err:#}");
    }
}

/// Settings: the least severity ticked when the sheet opens, and whether a
/// summary comment is added.
const LEAST_KEY: &str = "review.finish.least";
const SUMMARY_KEY: &str = "review.finish.summary";
/// Whether a review that found nothing is posted as a comment.
const CLEAN_KEY: &str = "review.finish.clean";

/// The least severity ticked by default: nits are the noise authors
/// complain of, so they are left for the reviewer to add.
fn least(store: &Store) -> String {
    let least: String = store.setting(LEAST_KEY);
    if ph_review::SEVERITIES.contains(&least.as_str()) {
        least
    } else {
        "minor".into()
    }
}

#[derive(Debug, Clone)]
pub enum FinishEvent {
    /// The review is on GitHub; what was done, in a sentence.
    Sent(String),
    /// A comment not sent yet was taken out (the page keeps them).
    DraftsChanged(Vec<Draft>),
}

pub struct FinishView {
    store: Store,
    pull: PullRef,
    /// The reviewer's own pull request: GitHub takes only a comment.
    own: bool,
    /// An agent reviewed the pull request on this computer: how many of
    /// its findings are still open, those that cannot be sent among them
    /// (their code not found, or on GitHub already).
    reviewed: Option<usize>,
    findings: Vec<Finding>,
    picked: BTreeSet<String>,
    drafts: Vec<Draft>,
    verdict: ReviewEvent,
    note: Entity<TextareaState>,
    summary: bool,
    /// With nothing left to send on the code, the agent's result is posted
    /// all the same: that the pull request was reviewed and nothing is
    /// open.
    clean: bool,
    least: String,
    busy: bool,
    error: Option<SharedString>,
    _send: Option<Task<()>>,
    _enter: gpui_kit::Subscription,
}

impl EventEmitter<FinishEvent> for FinishView {}

impl FinishView {
    /// `findings`: those that can be sent (open, their code found, not on
    /// GitHub yet). `reviewed`: an agent reviewed the pull request on this
    /// computer, and so many of its findings are still open; with none
    /// there is a result to post all the same.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Store,
        pull: PullRef,
        own: bool,
        reviewed: Option<usize>,
        findings: Vec<Finding>,
        drafts: Vec<Draft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let note_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Say something to the author (optional, Markdown)")
                .auto_grow(3, 10)
        });
        let least = least(&store);
        let summary: Option<bool> = store.setting(SUMMARY_KEY);
        let clean: Option<bool> = store.setting(CLEAN_KEY);
        // ⌘↵ (Ctrl+Enter) in the note sends, as the button does.
        let enter = cx.subscribe(
            &note_state,
            |this, _, event: &gpui_kit::component::input::InputEvent, cx| {
                if matches!(
                    event,
                    gpui_kit::component::input::InputEvent::PressEnter {
                        secondary: true,
                        ..
                    }
                ) {
                    this.send(cx)
                }
            },
        );
        let mut view = Self {
            _enter: enter,
            store,
            pull,
            own,
            reviewed,
            findings,
            picked: BTreeSet::new(),
            drafts,
            verdict: ReviewEvent::Comment,
            note: note_state,
            summary: summary.unwrap_or(true),
            clean: clean.unwrap_or(true),
            least,
            busy: false,
            error: None,
            _send: None,
        };
        view.tick_from_least();
        view
    }

    /// Shows `view` in a dialog.
    pub fn open(view: &Entity<Self>, window: &mut Window, cx: &mut gpui_kit::App) {
        let shown = view.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .title("Finish your review")
                .w(ph_ui::scaled_px(620., window))
                // Enter in the note is a line break, not "OK".
                .on_ok(|_, _, _| false)
                .child(shown.clone())
        });
        view.update(cx, |view, cx| {
            view.note.update(cx, |input, cx| input.focus(window, cx))
        });
    }

    /// Ticks the findings at least as serious as the setting says.
    fn tick_from_least(&mut self) {
        let rank = ph_review::severity_rank(&self.least);
        self.picked = self
            .findings
            .iter()
            .filter(|finding| ph_review::severity_rank(&finding.severity) <= rank)
            .map(|finding| finding.label.clone())
            .collect();
    }

    fn set_least(&mut self, least: &'static str, cx: &mut Context<Self>) {
        self.least = least.into();
        let _ = self.store.set_setting(LEAST_KEY, &self.least);
        self.tick_from_least();
        cx.notify();
    }

    fn note(&self, cx: &Context<Self>) -> String {
        self.note.read(cx).value().trim().to_string()
    }

    /// Whether the agent reviewed the pull request and nothing of what it
    /// found is open any more (it found nothing, or all is dealt with).
    /// Not so while a finding is open that only cannot be sent: "nothing
    /// is open" would not be true of it.
    fn nothing_open(&self) -> bool {
        self.reviewed == Some(0) && self.findings.is_empty()
    }

    /// Whether the agent's result is posted with nothing to put on the
    /// code.
    fn posts_clean(&self) -> bool {
        self.nothing_open() && self.clean
    }

    /// Whether a summary comment is written (or brought up to date).
    fn posts_summary(&self) -> bool {
        (self.summary && !self.picked.is_empty()) || self.posts_clean()
    }

    /// Whether there is anything to send.
    fn sendable(&self, cx: &Context<Self>) -> bool {
        self.verdict != ReviewEvent::Comment
            || !self.picked.is_empty()
            || !self.drafts.is_empty()
            || !self.note(cx).is_empty()
            || self.posts_clean()
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.sendable(cx) {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let finish = github::Finish {
            event: self.verdict,
            note: self.note(cx),
            picked: self.picked.iter().cloned().collect(),
            own: self.drafts.iter().map(Draft::comment).collect(),
            summary: self.posts_summary(),
        };
        let (store, pull) = (self.store.clone(), self.pull.clone());
        let work = cx.background_spawn(async move {
            let said = github::finish(&store, &pull, &finish)?;
            // Sent: the comments no longer wait here.
            save_drafts(&store, &pull, &[]);
            anyhow::Ok(said)
        });
        self._send = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(said) => cx.emit(FinishEvent::Sent(said)),
                    Err(err) => this.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    /// What sending does, in a sentence under the form.
    fn outcome(&self, cx: &Context<Self>) -> String {
        let comments = self.picked.len() + self.drafts.len();
        let what = match self.verdict {
            ReviewEvent::Approve => "an approval",
            ReviewEvent::RequestChanges => "a request for changes",
            ReviewEvent::Comment => "a review",
        };
        if !self.sendable(cx) {
            return if self.own {
                "Nothing to send yet: write a note, or comment on lines in Files."
            } else {
                "Nothing to send yet: tick a finding, write a note, or choose how it ends."
            }
            .into();
        }
        // Only the agent's result, as a comment: no review is made of it.
        if self.verdict == ReviewEvent::Comment
            && comments == 0
            && self.note(cx).is_empty()
            && self.posts_clean()
        {
            return "Posts a comment: what the agent reviewed, and that nothing is open.".into();
        }
        let mut said = format!("Sends {what}");
        match comments {
            0 => {}
            1 => said.push_str(" with 1 comment on the code"),
            n => said.push_str(&format!(" with {n} comments on the code")),
        }
        said.push_str(", in one post");
        if self.posts_summary() {
            said.push_str(", and a summary comment");
        }
        said.push('.');
        said
    }
}

impl Render for FinishView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |text: String| {
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(text)
        };
        let verdicts = [
            (
                ReviewEvent::Comment,
                "comment",
                "Comment",
                "Say what you found without deciding.",
            ),
            (
                ReviewEvent::Approve,
                "approve",
                "Approve",
                "It can be merged as far as you are concerned.",
            ),
            (
                ReviewEvent::RequestChanges,
                "request",
                "Request changes",
                "Something has to change before it is merged.",
            ),
        ];
        // Approving over something serious is the reviewer's call: said,
        // not stopped.
        let serious = self
            .findings
            .iter()
            .filter(|finding| ph_review::severity_rank(&finding.severity) <= 1)
            .count();
        let picked_count = self.picked.len();
        let total = self.findings.len();
        let sendable = self.sendable(cx);
        let least = self.least.clone();
        let view = cx.weak_entity();
        v_flex()
            .id("finish-review")
            .gap_3()
            .when(!self.own, |this| {
                this.child(
                    v_flex().gap_1().child(label("How it ends".into())).child(
                        h_flex().gap_1().children(verdicts.into_iter().map(
                            |(event, id, name, about)| {
                                let selected = self.verdict == event;
                                Button::new(SharedString::from(format!("finish-{id}")))
                                    .small()
                                    .map(|this| {
                                        if selected {
                                            this.primary()
                                        } else {
                                            this.outline()
                                        }
                                    })
                                    .when(event == ReviewEvent::Approve, |this| {
                                        this.icon(IconName::Check)
                                    })
                                    .label(name)
                                    .tooltip(about)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.verdict = event;
                                        cx.notify();
                                    }))
                            },
                        )),
                    ),
                )
            })
            .when(self.own, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("This is your own pull request: a review of it can only comment."),
                )
            })
            .when(
                self.verdict == ReviewEvent::Approve && serious > 0,
                |this| {
                    this.child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().warning.opacity(0.1))
                            .text_sm()
                            .child(match serious {
                                1 => "The agent found 1 critical or major problem that is still open."
                                    .to_string(),
                                n => format!(
                                    "The agent found {n} critical or major problems that are still open."
                                ),
                            }),
                    )
                },
            )
            .child(Textarea::new(&self.note))
            .when(!self.drafts.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(label(match self.drafts.len() {
                            1 => "Your comment".to_string(),
                            n => format!("Your {n} comments"),
                        }))
                        .children(self.drafts.iter().enumerate().map(|(ix, draft)| {
                            h_flex()
                                .gap_2()
                                .items_start()
                                .text_sm()
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w_1_2()
                                        .truncate()
                                        .text_xs()
                                        .pt_0p5()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("{}:{}", draft.path, draft.lines())),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(first_line(&draft.body)),
                                )
                                .child(
                                    Button::new(("finish-drop-draft", ix))
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Close)
                                        .accessibility_label("Do not send this comment")
                                        .tooltip("Delete this comment")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if ix < this.drafts.len() {
                                                this.drafts.remove(ix);
                                                save_drafts(&this.store, &this.pull, &this.drafts);
                                                cx.emit(FinishEvent::DraftsChanged(
                                                    this.drafts.clone(),
                                                ));
                                                cx.notify();
                                            }
                                        })),
                                )
                        })),
                )
            })
            .when(total > 0, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(label(format!(
                                    "The agent's findings · {picked_count} of {total} sent"
                                )))
                                .child(div().flex_1())
                                .child(
                                    Button::new("finish-least")
                                        .xsmall()
                                        .ghost()
                                        .label(match least.as_str() {
                                            "nit" => "Tick: all",
                                            "major" => "Tick: major and above",
                                            "critical" => "Tick: critical only",
                                            _ => "Tick: minor and above",
                                        })
                                        .tooltip("Which findings are ticked to begin with; kept for next time")
                                        .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                                            let choice = |least: &'static str, text: &'static str| {
                                                let view = view.clone();
                                                PopupMenuItem::new(text).on_click(
                                                    move |_, _, cx| {
                                                        _ = view.update(cx, |this, cx| {
                                                            this.set_least(least, cx)
                                                        });
                                                    },
                                                )
                                            };
                                            menu.item(choice("nit", "All, nits too"))
                                                .item(choice("minor", "Minor and above"))
                                                .item(choice("major", "Major and above"))
                                                .item(choice("critical", "Critical only"))
                                        }),
                                ),
                        )
                        .child(
                            v_flex()
                                .id("finish-findings")
                                .max_h(ph_ui::rems_from_px(220.))
                                .overflow_y_scrollbar()
                                .gap_0p5()
                                .children(self.findings.iter().map(|finding| {
                                    let name = finding.label.clone();
                                    let picked = self.picked.contains(&name);
                                    h_flex()
                                        .gap_2()
                                        .items_start()
                                        .py_0p5()
                                        .child(
                                            Checkbox::new(SharedString::from(format!(
                                                "finish-pick-{name}"
                                            )))
                                            .checked(picked)
                                            .on_click(cx.listener(
                                                move |this, checked: &bool, _, cx| {
                                                    if *checked {
                                                        this.picked.insert(name.clone());
                                                    } else {
                                                        this.picked.remove(&name);
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                        )
                                        .child(tag_owned(
                                            finding.severity.clone(),
                                            severity_color(&finding.severity, cx),
                                        ))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_sm()
                                                .when(!picked, |this| {
                                                    this.text_color(cx.theme().muted_foreground)
                                                })
                                                .child(finding.title.clone()),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Ticked findings go on their lines, in your name. The others stay here."),
                        )
                        .when(picked_count > 0, |this| {
                            this.child(
                                Checkbox::new("finish-summary")
                                    .checked(self.summary)
                                    .label("Add a summary comment the author can tick findings off in")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.summary = *checked;
                                        let _ = this.store.set_setting(SUMMARY_KEY, &Some(*checked));
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            })
            // Nothing to put on the code: the review itself is still worth
            // a word to the author.
            .when(self.nothing_open(), |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(label("The agent's review".into()))
                        .child(
                            Checkbox::new("finish-clean")
                                .checked(self.clean)
                                .label("Post the result as a comment: what was reviewed, and that nothing is open")
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.clean = *checked;
                                    let _ = this.store.set_setting(CLEAN_KEY, &Some(*checked));
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.outcome(cx)),
                    )
                    .child(
                        Button::new("finish-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("finish-send")
                            .small()
                            .primary()
                            .loading(self.busy)
                            .label(match self.verdict {
                                ReviewEvent::Approve => "Approve",
                                ReviewEvent::RequestChanges => "Request changes",
                                ReviewEvent::Comment => "Send review",
                            })
                            .disabled(self.busy || !sendable)
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    ),
            )
            .test_support()
    }
}

/// The first line of `text`, for a list.
pub(crate) fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;

    use super::*;

    fn finding(label: &str, severity: &str) -> Finding {
        Finding {
            label: label.into(),
            severity: severity.into(),
            title: format!("Problem {label}"),
            status: "open".into(),
            verified: true,
            ..Finding::default()
        }
    }

    fn pull() -> PullRef {
        PullRef {
            host: "github.com".into(),
            owner: "acme".into(),
            repo: "app".into(),
            number: 7,
        }
    }

    /// Nits are left for the reviewer to add, nothing is sent while there
    /// is nothing to say, and how it ends is said before it is sent.
    #[gpui_kit::test]
    fn ticks_what_matters_and_says_what_it_sends(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
        });
        let store = Store::open_in_memory().unwrap();
        let findings = vec![finding("F1", "major"), finding("F2", "nit")];
        let (view, cx) = cx.add_window_view(|window, cx| {
            FinishView::new(
                store,
                pull(),
                false,
                Some(2),
                findings,
                Vec::new(),
                window,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            assert_eq!(view.picked.iter().collect::<Vec<_>>(), ["F1"]);
            assert_eq!(
                view.outcome(cx),
                "Sends a review with 1 comment on the code, in one post, and a summary comment."
            );
            // Every finding set aside, no note, no verdict: nothing to send.
            view.picked.clear();
            assert!(!view.sendable(cx));
            view.verdict = ReviewEvent::Approve;
            assert!(view.sendable(cx));
            assert_eq!(view.outcome(cx), "Sends an approval, in one post.");
            // The choice of what is ticked is kept for the next review.
            view.set_least("nit", cx);
            assert_eq!(view.picked.len(), 2);
            assert_eq!(least(&view.store), "nit");
        });
    }

    /// A review that found nothing is still said to the author, on one's
    /// own pull request too; without any agent's review there is nothing
    /// to post until something is written.
    #[gpui_kit::test]
    fn a_review_that_found_nothing_can_be_posted(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
        });
        let store = Store::open_in_memory().unwrap();
        let (view, cx) = cx.add_window_view({
            let store = store.clone();
            |window, cx| {
                FinishView::new(
                    store,
                    pull(),
                    true,
                    Some(0),
                    Vec::new(),
                    Vec::new(),
                    window,
                    cx,
                )
            }
        });
        view.update(cx, |view, cx| {
            assert!(view.sendable(cx) && view.posts_summary());
            assert_eq!(
                view.outcome(cx),
                "Posts a comment: what the agent reviewed, and that nothing is open."
            );
            // Unticked: nothing to send, and it stays so next time.
            view.clean = false;
            assert!(!view.sendable(cx));
        });
        // No agent's review; or one with a finding still open that cannot
        // be sent (its code was not found): "nothing is open" is not said.
        for reviewed in [None, Some(1)] {
            let store = store.clone();
            let (view, cx) = cx.add_window_view(move |window, cx| {
                FinishView::new(
                    store,
                    pull(),
                    true,
                    reviewed,
                    Vec::new(),
                    Vec::new(),
                    window,
                    cx,
                )
            });
            view.update(cx, |view, cx| {
                assert!(!view.sendable(cx) && !view.posts_summary());
            });
        }
    }

    /// The reviewer's own comments wait in the settings until they are
    /// sent, and go out with the lines they are about.
    #[test]
    fn comments_wait_until_they_are_sent() {
        let store = Store::open_in_memory().unwrap();
        assert!(drafts(&store, &pull()).is_empty());
        let draft = Draft {
            path: "src/a.rs".into(),
            line: 4,
            start: None,
            body: "Rename this.".into(),
        };
        save_drafts(&store, &pull(), std::slice::from_ref(&draft));
        assert_eq!(drafts(&store, &pull()), [draft]);
    }

    #[test]
    fn a_draft_names_its_lines() {
        let mut draft = Draft {
            path: "src/a.rs".into(),
            line: 30,
            start: Some(12),
            body: "Why twice?".into(),
        };
        assert_eq!(draft.lines(), "12-30");
        let comment = draft.comment();
        assert_eq!((comment.start_line, comment.line), (Some(12), 30));
        // One line: no range, whatever the first line says.
        draft.start = Some(30);
        assert_eq!(draft.lines(), "30");
        assert_eq!(draft.comment().start_line, None);
        assert_eq!(first_line("\n  Rename this.\nMore."), "Rename this.");
    }
}
