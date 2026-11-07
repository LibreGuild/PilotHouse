//! A pull request's Walkthrough tab: a briefing for whoever reviews it (a
//! tech lead, a manager) that an agent writes after reading the change and
//! the code around it. It goes from plain words to detail: the change in
//! short, why, how the system works today, what changes (with diagrams),
//! how it is done, what it affects, where to look closely, how it is
//! tested, questions for the author and the terms used. See
//! [`ph_review::walkthrough`].
//!
//! It is shown as the chat shows a reply: Markdown with its diagrams drawn,
//! and text that can be selected and copied. A reference to a changed file
//! opens it in the Files tab; one to any other file opens it on the web at
//! the commit the briefing read.

use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div,
};
use ph_agent::reply::{OnLink, Reply};
use ph_github::models::PullRequest;
use ph_review::walkthrough::{self, Walkthrough};
use ph_store::Store;
use ph_store::reviews::PullRef;
use ph_ui::Icons;

use crate::AgentRequest;
use crate::hosting::Hosting;

#[derive(Debug, Clone)]
pub enum WalkthroughEvent {
    Agent(AgentRequest),
    /// Show `path` (at `line`) in the Files tab.
    ShowLine {
        path: String,
        line: Option<u64>,
    },
}

/// The newest briefing, as shown.
struct Shown {
    walkthrough: Walkthrough,
    /// The commit it read.
    sha: String,
    /// What is shown: the briefing with its code references made links.
    text: String,
    view: Entity<Reply>,
}

pub struct WalkthroughView {
    store: Store,
    hosting: Hosting,
    number: u64,
    pull: Option<PullRequest>,
    /// The pull request's changed files (their references open in the
    /// Files tab).
    changed: Vec<String>,
    shown: Option<Shown>,
    /// The newest run was written by an older version, in a shape no longer
    /// shown: asked for again.
    older: bool,
    /// A walkthrough was asked for and is being written.
    waiting: bool,
}

impl EventEmitter<WalkthroughEvent> for WalkthroughView {}

impl WalkthroughView {
    pub fn new(store: Store, hosting: Hosting, number: u64, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            store,
            hosting,
            number,
            pull: None,
            changed: Vec::new(),
            shown: None,
            older: false,
            waiting: false,
        };
        view.reload(&[], cx);
        view
    }

    /// The pull request as last loaded, and its changed files.
    pub fn set_pull(&mut self, pull: PullRequest, changed: &[String], cx: &mut Context<Self>) {
        self.pull = Some(pull);
        self.reload(changed, cx);
    }

    /// Reads the newest walkthrough again (one may have just been written);
    /// `changed`, when known, are the pull request's changed files.
    pub fn reload(&mut self, changed: &[String], cx: &mut Context<Self>) {
        if !changed.is_empty() {
            self.changed = changed.to_vec();
        }
        let pull = PullRef {
            host: self.hosting.account.host.clone(),
            owner: self.hosting.repo.owner.clone(),
            repo: self.hosting.repo.name.clone(),
            number: self.number,
        };
        let run = self
            .store
            .latest_pr_run(&pull, "walkthrough")
            .ok()
            .flatten();
        // A turn that failed leaves its run unfinished: after a while it is
        // no longer waited for.
        self.waiting = run.as_ref().is_some_and(|run| {
            !run.finished && ph_store::now_millis() - run.created_at < 20 * 60 * 1000
        });
        let finished = run.filter(|run| run.finished);
        let read = finished.as_ref().and_then(|run| {
            Walkthrough::from_data(&run.data).map(|walkthrough| (walkthrough, run.head_sha.clone()))
        });
        self.older = finished.is_some() && read.is_none();
        self.shown = read.map(|(walkthrough, sha)| {
            let web = self.hosting.web_url();
            let text = walkthrough.linked(&self.changed, Some(&web), &sha);
            // The same text keeps its view (and its drawn diagrams).
            match self.shown.take() {
                Some(shown) if shown.text == text => Shown {
                    walkthrough,
                    sha,
                    ..shown
                },
                _ => {
                    let on_link = self.on_link(cx);
                    let view = cx.new(|cx| Reply::new(&text, cx).on_link(on_link));
                    Shown {
                        walkthrough,
                        sha,
                        text,
                        view,
                    }
                }
            }
        });
        cx.notify();
    }

    /// Links in the briefing: a changed file opens in the Files tab, a web
    /// page in the browser.
    fn on_link(&self, cx: &mut Context<Self>) -> OnLink {
        let this = cx.entity().downgrade();
        Arc::new(move |url, _, cx| match walkthrough::parse_file_link(url) {
            Some((path, line)) => {
                _ = this.update(cx, |_, cx| {
                    cx.emit(WalkthroughEvent::ShowLine { path, line })
                });
            }
            None if url.starts_with("https://") || url.starts_with("http://") => cx.open_url(url),
            None => {}
        })
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        if let Some(pull) = &self.pull {
            self.waiting = true;
            cx.emit(WalkthroughEvent::Agent(AgentRequest::Walkthrough {
                pull: Box::new(pull.clone()),
            }));
            cx.notify();
        }
    }

    /// Before there is one: what it is, and the button that asks for it.
    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .p_4()
            .gap_3()
            .max_w(gpui_kit::px(640.))
            .child(
                div()
                    .text_base()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child("Understand this pull request before you review it"),
            )
            .child(div().text_sm().text_color(muted).child(
                "An agent reads the change and the code around it, and writes a short briefing \
                 in plain words: what changes and why, how this part of the system works today, \
                 what is different after (with diagrams), how it is done, what it affects, \
                 where to look closely, how it is tested and what to ask the author.",
            ))
            .when(self.older, |this| {
                this.child(div().text_sm().text_color(cx.theme().warning).child(
                    "The walkthrough made by an earlier version is not shown: ask for a new one.",
                ))
            })
            .child(if self.waiting {
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(
                        "The agent is reading the change and writing the briefing (in the chat \
                         beside)…",
                    )
                    .into_any_element()
            } else {
                h_flex()
                    .child(
                        Button::new("walk-ask")
                            .small()
                            .primary()
                            .icon(Icons::Sparkles)
                            .label("Explain this pull request")
                            .disabled(self.pull.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                    )
                    .into_any_element()
            })
    }
}

impl Render for WalkthroughView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shown) = &self.shown else {
            return self.render_empty(cx).into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let stale = self
            .pull
            .as_ref()
            .is_some_and(|pull| pull.head.sha != shown.sha);
        let size = self.pull.as_ref().map(|pull| {
            format!(
                "{} {} · +{} −{}",
                pull.changed_files,
                if pull.changed_files == 1 {
                    "file"
                } else {
                    "files"
                },
                pull.additions,
                pull.deletions
            )
        });
        let markdown = shown.walkthrough.markdown.clone();
        v_flex()
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("At {}", &shown.sha[..shown.sha.len().min(8)]))
                    .when(stale, |this| {
                        this.child(
                            div()
                                .text_color(cx.theme().warning)
                                .child("new commits since: ask again for them"),
                        )
                    })
                    .children(size)
                    .child(div().flex_1())
                    .child(
                        Button::new("walk-copy")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Copy)
                            .label("Copy")
                            .tooltip("Copy all of it as Markdown (a part: select it and copy)")
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(markdown.clone()));
                                window.push_notification(
                                    Notification::success("Copied the walkthrough"),
                                    cx,
                                );
                            }),
                    )
                    .child(
                        Button::new("walk-again")
                            .xsmall()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .label(if self.waiting { "Writing…" } else { "Again" })
                            .tooltip("Write it again, from the pull request as it is now")
                            .disabled(self.waiting || self.pull.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                    ),
            )
            .child(div().max_w(gpui_kit::px(820.)).child(shown.view.clone()))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;
    use ph_github::{Account, Client, Repo};

    use super::*;

    fn hosting() -> Hosting {
        Hosting {
            account: Account {
                id: "test".into(),
                host: "github.com".into(),
                login: "reviewer".into(),
                api_url: None,
            },
            repo: Repo {
                owner: "acme".into(),
                name: "app".into(),
            },
            remote: "origin".into(),
            client: Arc::new(Client::new("http://127.0.0.1:9", "")),
        }
    }

    /// A file and line the Files tab was asked to show.
    type Asked = (String, Option<u64>);

    const BRIEFING: &str = "# Sessions survive restarts\n\n## In short\n\nSessions are \
        kept on disk.\n\n## Where to look closely\n\n- `src/store.rs:40-58`: the write.\n- \
        `src/app.rs:12`: who reads it.\n";

    /// A store with a finished walkthrough of pull request 7 at `sha`.
    fn store_with_walkthrough(sha: &str) -> Store {
        let store = Store::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("ph-walk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store.add_project(&dir).unwrap();
        let pull = PullRef {
            host: "github.com".into(),
            owner: "acme".into(),
            repo: "app".into(),
            number: 7,
        };
        let run = store
            .create_pr_run(
                project.id,
                &pull,
                "Sessions",
                sha,
                "walkthrough",
                "Claude Code",
            )
            .unwrap();
        let walkthrough = Walkthrough {
            markdown: BRIEFING.into(),
        };
        store
            .finish_pr_run(run.id, &walkthrough.summary(), &walkthrough.to_data())
            .unwrap();
        store
    }

    #[gpui_kit::test]
    fn shows_the_briefing_with_its_references_as_links(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
        });
        let store = store_with_walkthrough("abc1234def");
        let (view, cx) = cx.add_window_view(|_, cx| WalkthroughView::new(store, hosting(), 7, cx));
        view.update(cx, |view, cx| {
            view.reload(&["src/store.rs".to_string()], cx)
        });
        cx.run_until_parked();
        let text = view.read_with(cx, |view, _| view.shown.as_ref().unwrap().text.clone());
        // A changed file opens in the Files tab; another on the web.
        assert!(text.contains("[`src/store.rs:40-58`](pilothouse-file:src/store.rs#40)"));
        assert!(text.contains(
            "[`src/app.rs:12`](https://github.com/acme/app/blob/abc1234def/src/app.rs#L12)"
        ));
        // Drawn, in a window.
        cx.run_until_parked();

        // A click on a changed file's link asks for it in the Files tab.
        let shown: Arc<std::sync::Mutex<Vec<Asked>>> = Default::default();
        let seen = shown.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &WalkthroughEvent, _| {
                if let WalkthroughEvent::ShowLine { path, line } = event {
                    seen.lock().unwrap().push((path.clone(), *line));
                }
            })
            .detach()
        });
        let on_link = view.update(cx, |view, cx| view.on_link(cx));
        cx.update(|window, cx| on_link("pilothouse-file:src/store.rs#40", window, cx));
        cx.run_until_parked();
        assert_eq!(
            *shown.lock().unwrap(),
            [("src/store.rs".to_string(), Some(40))]
        );
    }

    #[gpui_kit::test]
    fn an_older_walkthrough_is_asked_for_again(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
        });
        let store = Store::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("ph-walk-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store.add_project(&dir).unwrap();
        let pull = PullRef {
            host: "github.com".into(),
            owner: "acme".into(),
            repo: "app".into(),
            number: 7,
        };
        let run = store
            .create_pr_run(project.id, &pull, "Old", "abc", "walkthrough", "Codex")
            .unwrap();
        // The shape an earlier version stored.
        store
            .finish_pr_run(
                run.id,
                "x",
                r#"{"gist":{"what":"x"},"groups":[],"tour":[]}"#,
            )
            .unwrap();
        let (view, cx) = cx.add_window_view(|_, cx| WalkthroughView::new(store, hosting(), 7, cx));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.shown.is_none());
            assert!(view.older);
            assert!(!view.waiting);
        });
    }
}
