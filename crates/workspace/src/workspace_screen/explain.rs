//! Explanations of changes, from the workspace's side: the view beside the
//! changes in the Git tab and on a commit in History, getting an agent to
//! write one, and *Why Is This Here?* from the editor. The views are
//! `ph_explain`'s, the model `ph_review::explanation`'s; here they meet the
//! chat, the Git panel and the editor.
//!
//! An explanation is written in a chat of its own that only reads (as a
//! review of the changes is), with a prompt that carries what the chats
//! that made the change were asked. Nothing is written without the user
//! asking: the Explain button, Again, or the offer in *Why Is This Here?*.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use ph_explain::{ExplanationEvent, ExplanationView, Target, WhyEvent, WhyView};
use ph_review::explanation::find;
use ph_review::explanation::is_explanation_file;
use ph_review::explanation::prompt::{Size, Subject, explanation_prompt};
use ph_store::Store;
use ph_store::projects::ProjectId;

use super::*;

/// Whether this build explains changes.
pub(crate) fn explains() -> bool {
    ph_brand::enabled(ph_brand::Feature::Explanations)
}

/// What the chat needs to write an explanation.
pub(crate) struct Prepared {
    pub id: i64,
    /// The repository's top folder, where the agent reads.
    pub top: PathBuf,
    pub title: String,
    pub display: String,
    pub prompt: String,
}

/// Gets an explanation of `target` in `root`'s repository ready for agent
/// `agent` to write: the prompt, with what the chats that made the change
/// were asked, and its row in the store. `None` when there is one already
/// that explains the work as it is and `again` was not asked for: an agent
/// is not run for what can be read. Reads Git and the chats: not for the UI
/// thread.
pub(crate) fn prepare(
    store: &Store,
    project_id: ProjectId,
    root: &Path,
    target: &Target,
    agent: &str,
    again: bool,
) -> Result<Option<Prepared>> {
    let top = ph_git::content::top(root)?;
    let (subject, content, lines, commit) = match target {
        Target::Changes => (
            Subject::Changes,
            ph_git::content::pending(&top)?,
            ph_git::content::lines_pending(&top),
            None,
        ),
        Target::Commit { sha, subject } => (
            Subject::Commit {
                sha: sha.clone(),
                subject: subject.clone(),
            },
            ph_git::content::of_commit(&top, sha)?,
            ph_git::content::lines_of_commit(&top, sha),
            Some(sha.as_str()),
        ),
    };
    // An explanation's own file is not part of the work.
    let content: Vec<(String, String)> = content
        .into_iter()
        .filter(|(path, _)| !is_explanation_file(path))
        .collect();
    if content.is_empty() {
        bail!(match target {
            Target::Changes => "There are no changes to explain.",
            Target::Commit { .. } => "This commit changes no files to explain.",
        });
    }
    if !again {
        let explained = find::of_content(store, &top, &content, None)
            .into_iter()
            .next()
            .is_some_and(|found| content.iter().all(|file| found.files.contains(file)));
        if explained {
            return Ok(None);
        }
    }
    let changed: Vec<(String, Vec<String>)> = ph_git::content::changed_lines(&top, commit)
        .unwrap_or_default()
        .into_iter()
        .filter(|(path, _)| !is_explanation_file(path))
        .collect();
    let asked = ph_agent::explanations::asked_about(store, project_id, &top, &changed);
    let files: Vec<String> = content.iter().map(|(path, _)| path.clone()).collect();
    let size = Size {
        files: files.len(),
        lines,
    };
    let prompt = explanation_prompt(&subject, &files, size, &asked);
    let id = store.create_explanation(
        &top.to_string_lossy(),
        target.key(),
        agent,
        &content,
        &serde_json::to_string(&asked).unwrap_or_else(|_| "[]".into()),
    )?;
    let (title, display) = match target {
        Target::Changes => (
            format!("{}your changes", ph_agent::explanations::CHAT_TITLE),
            format!(
                "Explain the changes that are not committed yet ({} {}).",
                files.len(),
                if files.len() == 1 { "file" } else { "files" }
            ),
        ),
        Target::Commit { sha, subject } => (
            format!("{}{subject}", ph_agent::explanations::CHAT_TITLE),
            format!("Explain commit {} “{subject}”.", &sha[..sha.len().min(8)]),
        ),
    };
    Ok(Some(Prepared {
        id,
        top,
        title,
        display,
        prompt,
    }))
}

impl WorkspaceScreen {
    /// Gives a new Git panel the view that explains its changes, where the
    /// build explains changes (else the panel offers nothing of it).
    pub(super) fn attach_explanation(
        &mut self,
        changes: &Entity<ChangesPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !explains() {
            return;
        }
        let Some(root) = self.project.folders().get(self.git_folder).cloned() else {
            return;
        };
        let store = AppState::store(cx).clone();
        let view = cx.new(|cx| ExplanationView::new(store, root, Target::Changes, cx));
        let watch = self.watch_explanation(&view, window, cx);
        self._subscriptions.push(watch);
        changes.update(cx, |changes, cx| {
            changes.set_explanation(Some(view.clone().into()), cx)
        });
        self.explanations.insert(self.git_folder, view);
    }

    /// What an explanation's view asks for: an agent to write it, a file in
    /// the editor, Git read again after it shared or took back its file.
    fn watch_explanation(
        &mut self,
        view: &Entity<ExplanationView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            view,
            window,
            |this, _, event: &ExplanationEvent, window, cx| match event {
                // Asked for in the view: also when there is one (Again).
                ExplanationEvent::Ask(target) => this.explain(target.clone(), true, window, cx),
                ExplanationEvent::OpenFile { path, line } => {
                    this.open_path(path, *line, window, cx)
                }
                ExplanationEvent::FilesChanged => {
                    if let Some(changes) = this.changes.get(&this.git_folder) {
                        changes.update(cx, |changes, cx| changes.refresh(cx));
                    }
                }
            },
        )
    }

    /// Shows a file given by its full path, at a 1-based line.
    fn open_path(
        &mut self,
        path: &Path,
        line: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(name) = self.roots().path_of(path) {
            let line = line.map(|line| line.saturating_sub(1) as u32);
            self.open_file(&name, line, window, cx);
        }
    }

    /// The working tree of the Git tab's folder was read: its explanation
    /// may be behind now, or belong to a commit.
    pub(super) fn changes_read(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.explanations.get(&self.git_folder) {
            view.update(cx, |view, cx| view.reload(cx));
        }
    }

    /// History shows a commit (or none now): its explanation goes with it.
    pub(super) fn commit_shown(
        &mut self,
        commit: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_explanation = None;
        let (Some((sha, subject)), true) = (commit, explains()) else {
            return;
        };
        let Some(root) = self.project.folders().get(self.git_folder).cloned() else {
            return;
        };
        let store = AppState::store(cx).clone();
        let target = Target::Commit {
            sha: sha.clone(),
            subject,
        };
        let view = cx.new(|cx| ExplanationView::new(store, root, target, cx));
        let watch = self.watch_explanation(&view, window, cx);
        if let Some(changes) = self.changes.get(&self.git_folder) {
            changes.update(cx, |changes, cx| {
                changes.set_commit_explanation(&sha, view.clone().into(), false, cx)
            });
        }
        self.commit_explanation = Some((view, watch));
    }

    /// The Explain button over the changes: shows their explanation, and
    /// has one written when there is none of the work as it is.
    pub(super) fn explain_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.explain(Target::Changes, false, window, cx);
    }

    /// Has an agent explain `target` in the Git tab's folder, in a chat of
    /// its own that only reads; what it writes shows in the Git tab.
    /// `again`: also when an explanation of the work as it is exists.
    pub(crate) fn explain(
        &mut self,
        target: Target,
        again: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !explains() {
            return;
        }
        let Some(root) = self.project.folders().get(self.git_folder).cloned() else {
            return;
        };
        let store = AppState::store(cx).clone();
        let (project_id, agent) = (self.project.id, ph_agent::chats::default_agent(&store));
        let asked = target.clone();
        let work = cx.background_spawn(async move {
            prepare(&store, project_id, &root, &asked, agent.name, again)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                let prepared = match result {
                    Ok(prepared) => prepared,
                    Err(err) => {
                        window.push_notification(
                            Notification::error(format!("{err:#}")).title("Explain"),
                            cx,
                        );
                        return;
                    }
                };
                if let Some(prepared) = prepared {
                    this.reveal_chat(cx);
                    this.chat.update(cx, |chat, cx| {
                        chat.start_review_turn(
                            None,
                            Some(ph_agent::ReviewTurn::Explanation { id: prepared.id }),
                            Some(agent),
                            prepared.top,
                            prepared.title,
                            prepared.display,
                            prepared.prompt,
                            window,
                            cx,
                        )
                    });
                }
                this.show_explanation(&target, window, cx);
            });
        })
        .detach();
    }

    /// Brings the explanation of `target` into view in the Git tab (being
    /// written, or the one there is).
    fn show_explanation(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        let changes = self.changes(window, cx);
        match target {
            Target::Changes => {
                if let Some(view) = self.explanations.get(&self.git_folder) {
                    view.update(cx, |view, cx| view.reload(cx));
                }
                changes.update(cx, |changes, cx| changes.show_explanation(cx));
            }
            Target::Commit { sha, .. } => {
                // The commit History shows: its view says it is being
                // written. Another commit (asked about from the editor) is
                // read in the chat, and found from its lines after.
                if let Some((view, _)) = &self.commit_explanation
                    && view.read(cx).target() == target
                {
                    let view = view.clone();
                    view.update(cx, |view, cx| view.reload(cx));
                    changes.update(cx, |changes, cx| {
                        changes.set_commit_explanation(sha, view.into(), true, cx)
                    });
                } else {
                    return;
                }
            }
        }
        self.show_tab(ContextTab::Changes, window, cx);
    }

    /// *Why Is This Here?* for 1-based line `line` of `file` (a full path):
    /// looks it up off the UI thread, then says it in a pop-up.
    pub(super) fn why_is_this_here(
        &mut self,
        file: PathBuf,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !explains() {
            return;
        }
        let store = AppState::store(cx).clone();
        let project_id = self.project.id;
        let look = cx
            .background_spawn(async move { ph_explain::look_up(&store, project_id, &file, line) });
        cx.spawn_in(window, async move |this, cx| {
            let result = look.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok(why) => {
                    let top = why.top.clone();
                    let view = WhyView::open(why, window, cx);
                    this._why = Some(cx.subscribe_in(
                        &view,
                        window,
                        move |this, _, event: &WhyEvent, window, cx| match event {
                            WhyEvent::OpenChat(session) => {
                                this.select_session(*session, cx);
                                this.reveal_chat(cx);
                            }
                            WhyEvent::Explain(target) => {
                                // In the folder the file is in, which may
                                // not be the one the Git tab shows.
                                let folder = this.project.folders().iter().position(|folder| {
                                    top.starts_with(folder) || folder.starts_with(&top)
                                });
                                if let Some(folder) = folder
                                    && folder != this.git_folder
                                {
                                    this.set_git_folder(folder, window, cx);
                                }
                                this.explain(target.clone(), true, window, cx);
                            }
                            WhyEvent::OpenFile { path, line } => {
                                this.open_path(path, *line, window, cx)
                            }
                        },
                    ));
                }
                Err(err) => window.push_notification(
                    Notification::new()
                        .title("Why is this here?")
                        .message(format!("{err:#}")),
                    cx,
                ),
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn an_explanation_is_readied_once_for_the_work_as_it_is() {
        let dir = std::env::temp_dir().join(format!("ph-explain-prepare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("src/auth.rs"), "fn sign_in() {}\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "first"]);
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&dir).unwrap();
        let target = Target::Changes;

        // Nothing to explain yet.
        assert!(prepare(&store, project.id, &dir, &target, "Claude Code", false).is_err());

        std::fs::write(
            dir.join("src/auth.rs"),
            "fn sign_in() {}\nfn reset_password() {}\n",
        )
        .unwrap();
        let prepared = prepare(&store, project.id, &dir, &target, "Claude Code", false)
            .unwrap()
            .unwrap();
        assert_eq!(prepared.title, "Explain: your changes");
        assert!(prepared.prompt.contains("- `src/auth.rs`"));
        assert!(
            prepared
                .prompt
                .contains("a small change (1 changed line in 1 file)")
        );
        // No chat made it: the prompt says so, instead of guessing a why.
        assert!(
            prepared
                .prompt
                .contains("There is no record of what was asked for")
        );
        // It waits in the store for its text, under the content it explains.
        let waiting = store.explanation(prepared.id).unwrap().unwrap();
        assert!(!waiting.finished);
        assert_eq!(waiting.files.len(), 1);

        // Written: asked for again by the button, no agent is run for it.
        store
            .finish_explanation(prepared.id, "Reset", "# Reset", None)
            .unwrap();
        assert!(
            prepare(&store, project.id, &dir, &target, "Claude Code", false)
                .unwrap()
                .is_none()
        );
        // Again writes a new one; so does work that moved on.
        assert!(
            prepare(&store, project.id, &dir, &target, "Claude Code", true)
                .unwrap()
                .is_some()
        );
        std::fs::write(dir.join("src/mail.rs"), "fn send() {}\n").unwrap();
        assert!(
            prepare(&store, project.id, &dir, &target, "Claude Code", false)
                .unwrap()
                .is_some()
        );

        // A commit is explained as it was made.
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "reset and mail"]);
        let sha = ph_git::git(&dir, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let commit = Target::Commit {
            sha: sha.clone(),
            subject: "reset and mail".into(),
        };
        let prepared = prepare(&store, project.id, &dir, &commit, "Codex", true)
            .unwrap()
            .unwrap();
        assert_eq!(prepared.title, "Explain: reset and mail");
        assert!(prepared.prompt.contains(&format!("git show {sha}")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
