//! One project's repository state, shared by the status bar, the branch
//! picker and the Git panel: branch, upstream, ahead/behind, staged and
//! conflicted files, and the git operation running right now.
//!
//! It re-reads `git status` when the project watcher reports a change (the
//! watcher passes `.git` HEAD, index and ref changes through). Operations
//! run on a background thread one at a time; the result comes back as a
//! [`RepoEvent`] so the window can show a notification.

use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui_kit::{AppContext as _, Context, Entity, EventEmitter, SharedString, Subscription, Task};
use ph_files::{FilesChanged, ProjectWatcher};
use ph_git::RepoStatus;

#[derive(Debug, Clone)]
pub enum RepoEvent {
    /// An operation finished; the text says what happened.
    Done(SharedString),
    Failed(SharedString),
}

pub struct Repo {
    root: PathBuf,
    /// `None` until read, or when the folder is not a repository.
    status: Option<RepoStatus>,
    loaded: bool,
    /// What runs now (`"Pulling"`), shown instead of starting another.
    busy: Option<SharedString>,
    /// A merge, rebase or cherry-pick in progress.
    operation: Option<&'static str>,
    refresh_again: bool,
    _refresh: Option<Task<()>>,
    _task: Option<Task<()>>,
    _watch: Subscription,
}

impl EventEmitter<RepoEvent> for Repo {}

impl Repo {
    pub fn new(watcher: &Entity<ProjectWatcher>, cx: &mut Context<Self>) -> Self {
        let root = watcher.read(cx).root().to_path_buf();
        let watch = cx.subscribe(watcher, |this, _, _: &FilesChanged, cx| this.refresh(cx));
        let mut repo = Self {
            root,
            status: None,
            loaded: false,
            busy: None,
            operation: None,
            refresh_again: false,
            _refresh: None,
            _task: None,
            _watch: watch,
        };
        repo.refresh(cx);
        repo
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn status(&self) -> Option<&RepoStatus> {
        self.status.as_ref()
    }

    /// Whether the first read finished (and whether this is a repository).
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn is_repository(&self) -> bool {
        self.status.is_some()
    }

    pub fn branch(&self) -> Option<&str> {
        self.status.as_ref()?.branch.as_deref()
    }

    pub fn busy(&self) -> Option<&SharedString> {
        self.busy.as_ref()
    }

    pub fn operation(&self) -> Option<&'static str> {
        self.operation
    }

    /// Re-reads the status; a burst of requests folds into one more read.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self._refresh.is_some() {
            self.refresh_again = true;
            return;
        }
        let root = self.root.clone();
        let read = cx.background_spawn(async move {
            if !ph_git::is_repository(&root) {
                return (None, None);
            }
            let status = ph_git::status(&root).ok();
            let operation = ph_git::conflicts::operation(&root);
            (status, operation)
        });
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let (status, operation) = read.await;
            _ = this.update(cx, |this, cx| {
                this._refresh = None;
                this.loaded = true;
                if this.status != status || this.operation != operation {
                    this.status = status;
                    this.operation = operation;
                    cx.notify();
                }
                if std::mem::take(&mut this.refresh_again) {
                    this.refresh(cx);
                }
            });
        }));
    }

    /// Runs `op` in the background unless another operation is running.
    /// `label` names it while it runs (`"Pushing"`); `op` returns what to
    /// tell the user when it succeeds.
    pub fn run(
        &mut self,
        label: impl Into<SharedString>,
        op: impl FnOnce(&Path) -> Result<String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.into());
        cx.notify();
        let root = self.root.clone();
        let work = cx.background_spawn(async move { op(&root) });
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            _ = this.update(cx, |this, cx| {
                this.busy = None;
                this._task = None;
                cx.emit(match result {
                    Ok(message) => RepoEvent::Done(message.into()),
                    Err(err) => RepoEvent::Failed(format!("{err:#}").into()),
                });
                this.refresh(cx);
                cx.notify();
            });
        }));
    }
}
