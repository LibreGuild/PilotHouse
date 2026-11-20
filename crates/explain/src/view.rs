//! An explanation where its change is read: beside the changes in the Git
//! tab, and on a commit in History.
//!
//! Which explanation belongs here is not kept anywhere: it is the one that
//! explains the content in front (the files not committed, or the ones a
//! commit put in the repository), found again on every look. So the view
//! follows the work: once the files move on it says so, and after a commit
//! the same text is found from that commit.

use std::path::{Path, PathBuf};
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
    AppContext as _, ClipboardItem, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Styled as _, Subscription, Task, Window, div,
};
use ph_agent::explanations::{ExplanationWritten, Explanations};
use ph_agent::reply::{OnLink, Reply};
use ph_review::explanation::file::{self, Shared};
use ph_review::explanation::find::{self, Found, Source};
use ph_review::explanation::is_explanation_file;
use ph_review::walkthrough::parse_file_link;
use ph_store::Store;
use ph_ui::Icons;

/// How long an explanation that was asked for is waited for: a turn that
/// died without saying so leaves it unfinished.
const WAIT: i64 = 20 * 60 * 1000;

/// What an explanation is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The work not committed yet.
    Changes,
    /// One commit.
    Commit { sha: String, subject: String },
}

impl Target {
    /// How the store names it (see `ph_store::explanations`).
    pub fn key(&self) -> &str {
        match self {
            Target::Changes => "changes",
            Target::Commit { sha, .. } => sha,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ExplanationEvent {
    /// Have an agent explain this.
    Ask(Target),
    /// Show this file (its full path) at this 1-based line.
    OpenFile { path: PathBuf, line: Option<u64> },
    /// A file was written to or taken out of the working tree (an
    /// explanation shared, or taken back): what shows Git reads it again.
    FilesChanged,
}

/// What a look found.
#[derive(Default)]
struct Looked {
    /// The repository's top folder.
    top: PathBuf,
    /// One was asked for and is being written.
    writing: bool,
    found: Option<Found>,
    /// How many files are not as the explanation explains them (work that
    /// moved on; for a commit, files changed since).
    behind: usize,
    /// The newer explanation of what changed since, for a commit's.
    newer: Option<String>,
    /// It is a file that is not committed yet: it can be taken out again.
    uncommitted: bool,
    /// The explanation with its code references made links.
    text: String,
}

struct Shown {
    looked: Looked,
    view: Entity<Reply>,
}

enum State {
    Loading,
    Missing { writing: bool },
    Shown(Box<Shown>),
}

pub struct ExplanationView {
    store: Store,
    /// The folder the Git tab shows (in the repository, not always its top).
    root: PathBuf,
    target: Target,
    state: State,
    /// A share or a take-back is under way.
    busy: bool,
    /// The last one did not work: why.
    failed: Option<String>,
    _load: Option<Task<()>>,
    _work: Option<Task<()>>,
    _written: Subscription,
}

impl EventEmitter<ExplanationEvent> for ExplanationView {}

impl ExplanationView {
    pub fn new(store: Store, root: PathBuf, target: Target, cx: &mut Context<Self>) -> Self {
        // Its chat may run anywhere: word that one was written comes from
        // the app's one emitter.
        let explanations = Explanations::global(cx);
        let written = cx.subscribe(&explanations, |this, _, _: &ExplanationWritten, cx| {
            this.reload(cx)
        });
        let mut view = Self {
            store,
            root,
            target,
            state: State::Loading,
            busy: false,
            failed: None,
            _load: None,
            _work: None,
            _written: written,
        };
        view.reload(cx);
        view
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Looks again: the files changed, or an explanation was written.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let (store, root, target) = (self.store.clone(), self.root.clone(), self.target.clone());
        let look = cx.background_spawn(async move { look(&store, &root, &target) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let looked = look.await;
            _ = this.update(cx, |this, cx| this.show(looked, cx));
        }));
    }

    fn show(&mut self, looked: Looked, cx: &mut Context<Self>) {
        self.state = match looked.found {
            None => State::Missing {
                writing: looked.writing,
            },
            Some(_) => {
                // The same text keeps its view (and its drawn diagrams).
                let view = match std::mem::replace(&mut self.state, State::Loading) {
                    State::Shown(shown) if shown.looked.text == looked.text => shown.view,
                    _ => {
                        let on_link = self.on_link(looked.top.clone(), cx);
                        let text = looked.text.clone();
                        cx.new(|cx| Reply::new(&text, cx).on_link(on_link))
                    }
                };
                State::Shown(Box::new(Shown { looked, view }))
            }
        };
        cx.notify();
    }

    /// Links in the explanation: a file of the repository is shown in the
    /// editor, a web page in the browser.
    fn on_link(&self, top: PathBuf, cx: &mut Context<Self>) -> OnLink {
        let this = cx.entity().downgrade();
        Arc::new(move |url, _, cx| match parse_file_link(url) {
            Some((path, line)) => {
                let path = top.join(path);
                _ = this.update(cx, |_, cx| {
                    cx.emit(ExplanationEvent::OpenFile { path, line })
                });
            }
            None if url.starts_with("https://") || url.starts_with("http://") => cx.open_url(url),
            None => {}
        })
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        cx.emit(ExplanationEvent::Ask(self.target.clone()));
    }

    /// Writes the explanation as a file in the repository, so it goes with
    /// the next commit. Only ever on the user's click.
    fn add_to_commit(&mut self, cx: &mut Context<Self>) {
        let State::Shown(shown) = &self.state else {
            return;
        };
        let (found, top) = (shown.looked.found.clone(), shown.looked.top.clone());
        let Some(found) = found else {
            return;
        };
        let work = cx.background_spawn(async move {
            let author = ph_git::git(&top, &["config", "user.name"])
                .map(|name| name.trim().to_string())
                .unwrap_or_default();
            let path = file::write(
                &top,
                &Shared {
                    markdown: found.markdown,
                    written: found.written,
                    author,
                    agent: found.agent,
                    files: found.files,
                },
            )?;
            // With files kept for the commit, it is kept with them. With
            // none, Commit takes every change, and it is one of them:
            // keeping it alone would make the next commit only this file.
            let kept = ph_git::git(&top, &["diff", "--cached", "--name-only"])
                .is_ok_and(|kept| !kept.trim().is_empty());
            if kept {
                ph_git::status::stage_path(&top, &path)?;
            }
            anyhow::Ok(path)
        });
        self.finish(work, "Could not add it to the commit", cx);
    }

    /// Waits for a share or a take-back, then looks again. What it did
    /// shows in the view itself (where the explanation is kept), so only a
    /// failure needs words.
    fn finish<T: Send + 'static>(
        &mut self,
        work: Task<anyhow::Result<T>>,
        failure: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        self.failed = None;
        self._work = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.failed = result.err().map(|err| format!("{failure}: {err:#}"));
                cx.emit(ExplanationEvent::FilesChanged);
                this.reload(cx);
            });
        }));
        cx.notify();
    }

    /// Takes a shared explanation that is not committed yet out of the
    /// repository again. The one kept on this computer stays.
    fn take_out(&mut self, cx: &mut Context<Self>) {
        let State::Shown(shown) = &self.state else {
            return;
        };
        let Some(Source::Shared { path }) = shown.looked.found.as_ref().map(|f| f.source.clone())
        else {
            return;
        };
        let top = shown.looked.top.clone();
        let work = cx.background_spawn(async move {
            // Staged or not, tracked or not: gone from the next commit.
            let _ = ph_git::status::unstage_path(&top, &path);
            std::fs::remove_file(top.join(&path))?;
            anyhow::Ok(())
        });
        self.finish(work, "Could not take it out", cx);
    }

    /// Before there is one: what it is, and the button that asks for it.
    fn render_missing(&self, writing: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let (title, button) = match &self.target {
            Target::Changes => (
                "Understand these changes before you commit them",
                "Explain these changes",
            ),
            Target::Commit { .. } => ("No explanation of this commit yet", "Explain this commit"),
        };
        v_flex()
            .gap_3()
            .max_w(ph_ui::rems_from_px(640.))
            .child(
                div()
                    .text_base()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(title),
            )
            .child(div().text_sm().text_color(muted).child(
                "An agent reads the change and the code around it, and tells you in plain \
                 words: how this part worked before, what is different now, why it was done \
                 this way, what was checked, and what is worth a word with your team. It \
                 takes a minute or two, and stays on this computer unless you add it to a \
                 commit.",
            ))
            .child(if writing {
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("The agent is reading the change and writing (in the chat beside)…")
                    .into_any_element()
            } else {
                h_flex()
                    .child(
                        Button::new("explain-ask")
                            .small()
                            .primary()
                            .icon(Icons::Sparkles)
                            .label(button)
                            .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                    )
                    .into_any_element()
            })
    }

    fn render_shown(&self, shown: &Shown, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let looked = &shown.looked;
        let Some(found) = &looked.found else {
            return div().into_any_element();
        };
        let markdown = found.markdown.clone();
        // Where it is kept, in a few words.
        let place = match (&found.source, looked.uncommitted) {
            (Source::Local { .. }, _) => None,
            (Source::Shared { .. }, true) => Some("a file now, not committed yet"),
            (Source::Shared { .. }, false) => Some("in the repository"),
        };
        let behind = (looked.behind > 0).then(|| {
            let files = if looked.behind == 1 { "file" } else { "files" };
            match (&self.target, &looked.newer) {
                (Target::Changes, _) => format!(
                    "The work moved on since ({} {files}): ask again for it",
                    looked.behind
                ),
                (Target::Commit { .. }, Some(newer)) => {
                    format!("Changed since. Newer: “{newer}”")
                }
                (Target::Commit { .. }, None) => {
                    format!("{} of its {files} changed since", looked.behind)
                }
            }
        });
        let is_local = matches!(found.source, Source::Local { .. });
        let busy = self.busy;
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(match place {
                        Some(place) => format!("{} · {place}", crate::byline(found)),
                        None => crate::byline(found),
                    })
                    .when_some(behind, |this, behind| {
                        this.child(div().text_color(cx.theme().warning).child(behind))
                    })
                    .child(div().flex_1())
                    .when(is_local, |this| {
                        this.child(
                            Button::new("explain-share")
                                .xsmall()
                                .ghost()
                                .icon(Icons::FilePlus)
                                .label("Add to commit")
                                .tooltip(ph_ui::tip(
                                    "Share it with the team: writes it as a file under \
                                     .pilothouse/explanations, to commit with your changes",
                                ))
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.add_to_commit(cx))),
                        )
                    })
                    .when(!is_local && looked.uncommitted, |this| {
                        this.child(
                            Button::new("explain-unshare")
                                .xsmall()
                                .ghost()
                                .label("Take out")
                                .tooltip(ph_ui::tip(
                                    "Remove its file from the repository again. It stays on \
                                     this computer.",
                                ))
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.take_out(cx))),
                        )
                    })
                    .child(
                        Button::new("explain-copy")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Copy)
                            .label("Copy")
                            .tooltip(ph_ui::tip(
                                "Copy all of it as Markdown (a part: select it and copy)",
                            ))
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(markdown.clone()));
                                window.push_notification(
                                    Notification::success("Copied the explanation"),
                                    cx,
                                );
                            }),
                    )
                    .child(
                        Button::new("explain-again")
                            .xsmall()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .label(if looked.writing {
                                "Writing…"
                            } else {
                                "Again"
                            })
                            .tooltip("Write it again, from the change as it is now")
                            .disabled(looked.writing)
                            .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                    ),
            )
            .when_some(self.failed.clone(), |this, failed| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(failed))
            })
            .child(
                div()
                    .max_w(ph_ui::rems_from_px(820.))
                    .child(shown.view.clone()),
            )
            .into_any_element()
    }
}

impl Render for ExplanationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.state {
            State::Loading => div().into_any_element(),
            State::Missing { writing } => self.render_missing(*writing, cx).into_any_element(),
            State::Shown(shown) => {
                // Borrowed apart from `self` for the listeners.
                let shown: &Shown = shown;
                self.render_shown(shown, cx).into_any_element()
            }
        };
        // As high as its text: whoever shows it scrolls it (the Git tab's
        // pane, a commit's detail).
        div().id("explanation").child(body)
    }
}

/// Looks for the explanation of `target` in `root`'s repository. Reads Git
/// and files: not for the UI thread.
fn look(store: &Store, root: &Path, target: &Target) -> Looked {
    let top = ph_git::content::top(root).unwrap_or_else(|_| root.to_path_buf());
    let pending = ph_git::content::pending(&top).unwrap_or_default();
    let key = top.to_string_lossy();
    let writing = store
        .latest_explanation(&key, target.key())
        .ok()
        .flatten()
        .is_some_and(|latest| {
            !latest.finished && ph_store::now_millis() - latest.created_at < WAIT
        });
    let mut looked = Looked {
        top: top.clone(),
        writing,
        ..Looked::default()
    };
    let found = match target {
        Target::Changes => {
            let work: Vec<(String, String)> = pending
                .iter()
                .filter(|(path, _)| !is_explanation_file(path))
                .cloned()
                .collect();
            // The one that explains the work as it is; else the newest
            // about these files that was written since the last commit,
            // which the work has moved on from. An older one is about work
            // that is committed: it is that commit's, not this work's.
            let last_commit = ph_git::git(&top, &["log", "-1", "--format=%ct"])
                .ok()
                .and_then(|time| time.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let found = find::of_content(store, &top, &work, None)
                .into_iter()
                .next()
                .or_else(|| {
                    find::of_paths(store, &top, &work).filter(|found| found.written >= last_commit)
                });
            if let Some(found) = &found {
                // Files changed since it was written, and files it explains
                // that are no longer as it says (or no longer changed).
                looked.behind = work
                    .iter()
                    .filter(|file| !found.files.contains(file))
                    .count()
                    + found
                        .files
                        .iter()
                        .filter(|file| !work.contains(file))
                        .count();
            }
            found
        }
        Target::Commit { sha, .. } => {
            let found = find::of_commit(store, &top, sha, None)
                .ok()
                .and_then(|found| found.into_iter().next());
            if let Some(found) = &found {
                looked.behind = find::changed_since(&top, found).len();
                looked.newer = find::newer(store, &top, found).map(|newer| newer.title);
            }
            found
        }
    };
    if let Some(found) = &found {
        looked.uncommitted = match &found.source {
            Source::Shared { path } => pending.iter().any(|(file, _)| file == path),
            Source::Local { .. } => false,
        };
        looked.text = crate::linked(&top, &found.markdown);
    }
    looked.found = found;
    looked
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository with one commit and work not committed in `src/auth.rs`.
    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ph-explain-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.name", "Test"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        std::fs::write(dir.join("src/auth.rs"), "fn sign_in() {}\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "first"]);
        std::fs::write(dir.join("src/auth.rs"), "fn sign_in() {}\nfn reset() {}\n").unwrap();
        // As Git names it (a temporary folder is behind a link on macOS).
        ph_git::content::top(&dir).unwrap()
    }

    const TEXT: &str = "# Reset links\n\n## In short\n\nYou can reset a password.\n\n\
        ## What changed\n\nA new function (`src/auth.rs:2`), next to `src/gone.rs:1`.\n";

    /// Explains the work not committed in `dir`, as an agent would have.
    fn explain(store: &Store, dir: &Path) -> i64 {
        let content = ph_git::content::pending(dir).unwrap();
        let id = store
            .create_explanation(
                &dir.to_string_lossy(),
                "changes",
                "Claude Code",
                &content,
                "[]",
            )
            .unwrap();
        store
            .finish_explanation(id, "Reset links", TEXT, None)
            .unwrap();
        id
    }

    fn looked(
        view: &Entity<ExplanationView>,
        cx: &mut gpui_kit::VisualTestContext,
    ) -> Option<(String, usize, bool)> {
        cx.run_until_parked();
        view.read_with(cx, |view, _| match &view.state {
            State::Shown(shown) => Some((
                shown.looked.text.clone(),
                shown.looked.behind,
                shown.looked.uncommitted,
            )),
            _ => None,
        })
    }

    /// Waits for the view's look at Git (a background thread) to land.
    fn settle(cx: &mut gpui_kit::VisualTestContext) {
        for _ in 0..40 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn follows_the_work_from_the_changes_to_their_commit(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
        });
        cx.executor().allow_parking();
        let dir = repo("view");
        let store = Store::open_in_memory().unwrap();
        let (view, cx) = cx.add_window_view({
            let (store, dir) = (store.clone(), dir.clone());
            |_, cx| ExplanationView::new(store, dir, Target::Changes, cx)
        });
        settle(cx);
        // Nothing yet: it offers to ask, and asking is the shell's to do.
        assert!(looked(&view, cx).is_none());

        explain(&store, &dir);
        view.update(cx, |view, cx| view.reload(cx));
        settle(cx);
        let (text, behind, uncommitted) = looked(&view, cx).unwrap();
        // A file that is there is a link; one that is not stays code.
        assert!(text.contains("[`src/auth.rs:2`](pilothouse-file:src/auth.rs#2)"));
        assert!(text.contains("next to `src/gone.rs:1`."));
        assert_eq!((behind, uncommitted), (0, false));

        // The work moves on: it says so.
        std::fs::write(
            dir.join("src/auth.rs"),
            "fn sign_in() {}\nfn reset() { soon() }\n",
        )
        .unwrap();
        view.update(cx, |view, cx| view.reload(cx));
        settle(cx);
        // Nothing explains the file as it is now: the one about it is
        // behind (the file as it is, and the file as it was explained).
        assert_eq!(looked(&view, cx).unwrap().1, 2);
        std::fs::write(dir.join("src/auth.rs"), "fn sign_in() {}\nfn reset() {}\n").unwrap();
        std::fs::write(dir.join("src/more.rs"), "fn more() {}\n").unwrap();
        view.update(cx, |view, cx| view.reload(cx));
        settle(cx);
        assert_eq!(looked(&view, cx).unwrap().1, 1);
        std::fs::remove_file(dir.join("src/more.rs")).unwrap();

        // Shared: a file in the repository, kept for the next commit.
        view.update(cx, |view, cx| view.add_to_commit(cx));
        settle(cx);
        let shared = file::all(&dir);
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].1.author, "Test");
        let (_, behind, uncommitted) = looked(&view, cx).unwrap();
        assert_eq!((behind, uncommitted), (0, true));
        // Taken out again: the one on this computer is still there.
        view.update(cx, |view, cx| view.take_out(cx));
        settle(cx);
        assert!(file::all(&dir).is_empty());
        assert!(!looked(&view, cx).unwrap().2);

        // Committed: the same explanation is the commit's.
        git(&dir, &["commit", "-q", "-am", "reset links"]);
        let sha = ph_git::git(&dir, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        view.update(cx, |view, cx| view.reload(cx));
        settle(cx);
        assert!(looked(&view, cx).is_none(), "no work is left to explain");
        // New work on the same file is not what the committed one explains.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "later"]);
        std::fs::write(
            dir.join("src/auth.rs"),
            "fn sign_in() {}\nfn reset() {}\nfn next() {}\n",
        )
        .unwrap();
        view.update(cx, |view, cx| view.reload(cx));
        settle(cx);
        assert!(looked(&view, cx).is_none(), "the old one is its commit's");
        git(&dir, &["checkout", "-q", "--", "src/auth.rs"]);
        let (of_commit, cx) = cx.add_window_view({
            let (store, dir) = (store.clone(), dir.clone());
            |_, cx| {
                let target = Target::Commit {
                    sha,
                    subject: "reset links".into(),
                };
                ExplanationView::new(store, dir, target, cx)
            }
        });
        settle(cx);
        assert_eq!(looked(&of_commit, cx).unwrap().1, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
