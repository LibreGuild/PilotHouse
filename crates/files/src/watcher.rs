//! One file-system watcher per folder, shared by every panel that cares
//! ([`WatcherHub`]).
//!
//! Events are batched for [`WATCH_DEBOUNCE`] so a build or `git checkout`
//! touching thousands of files produces one [`FilesChanged`] event. Paths
//! inside `.git/` are dropped except the ones that signal a repository change
//! (index, HEAD), which the Changes panel needs. Paths the project ignores
//! (`target/`, `node_modules/`… via `.gitignore`) are dropped on the watcher
//! thread: no panel shows them, and a build writes thousands of them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, WeakEntity};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(150);

/// A batch of changed paths (absolute, sorted, without duplicates).
#[derive(Debug, Clone)]
pub struct FilesChanged {
    pub paths: Vec<PathBuf>,
}

pub struct ProjectWatcher {
    root: PathBuf,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl EventEmitter<FilesChanged> for ProjectWatcher {}

impl ProjectWatcher {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let watcher = start(&root, cx);
        Self {
            root,
            _watcher: watcher,
        }
    }

    /// A watcher that reports nothing (see [`WatcherHub::turn_off`]).
    fn inert(root: PathBuf) -> Self {
        Self {
            root,
            _watcher: None,
        }
    }

    /// Watches one file opened outside any project: its folder, not
    /// recursively (it may be a big one, like Downloads), keeping only
    /// events for the file.
    pub fn for_file(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let watcher = start_file(&path, cx);
        Self {
            root: path,
            _watcher: watcher,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// The watchers of folders in use, so a folder is watched once however many
/// views and schedules look at it. A watcher stops when the last of them
/// lets it go.
#[derive(Default)]
pub struct WatcherHub {
    watchers: HashMap<PathBuf, WeakEntity<ProjectWatcher>>,
    /// No OS watchers (UI tests, whose scheduler must see every thread's
    /// work): folders are shown but not followed.
    off: bool,
}

impl Global for WatcherHub {}

impl WatcherHub {
    /// The watcher of folder `root`, started if none is running.
    pub fn watcher(root: PathBuf, cx: &mut App) -> Entity<ProjectWatcher> {
        let existing = cx
            .try_global::<Self>()
            .and_then(|hub| hub.watchers.get(&root))
            .and_then(WeakEntity::upgrade);
        if let Some(watcher) = existing {
            return watcher;
        }
        let off = cx.try_global::<Self>().is_some_and(|hub| hub.off);
        let watcher = cx.new(|cx| {
            if off {
                ProjectWatcher::inert(root.clone())
            } else {
                ProjectWatcher::new(root.clone(), cx)
            }
        });
        let hub = cx.default_global::<Self>();
        hub.watchers
            .retain(|_, watcher| watcher.upgrade().is_some());
        hub.watchers.insert(root, watcher.downgrade());
        watcher
    }

    /// Stops new watchers from following the file system (for UI tests).
    pub fn turn_off(cx: &mut App) {
        cx.default_global::<Self>().off = true;
    }
}

fn start_file(path: &Path, cx: &mut Context<ProjectWatcher>) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher as _;

    let folder = path.parent()?.to_path_buf();
    let (sender, receiver) = smol::channel::unbounded::<PathBuf>();
    let wanted = path.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        for path in event.paths {
            if path == wanted {
                let _ = sender.send_blocking(path);
            }
        }
    })
    .map_err(|err| tracing::warn!("cannot watch {}: {err}", folder.display()))
    .ok()?;
    watcher
        .watch(&folder, notify::RecursiveMode::NonRecursive)
        .map_err(|err| tracing::warn!("cannot watch {}: {err}", folder.display()))
        .ok()?;
    forward(receiver, cx);
    Some(watcher)
}

/// Returns `None` (and logs) if the OS refuses a watcher; panels then simply
/// do not auto-refresh.
fn start(root: &Path, cx: &mut Context<ProjectWatcher>) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher as _;

    let (sender, receiver) = smol::channel::unbounded::<PathBuf>();
    let git_dir = root.join(".git");
    let ignored = ignore_rules(root);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        for path in event.paths {
            if is_relevant(&git_dir, &path) && !is_ignored(&ignored, &git_dir, &path) {
                let _ = sender.send_blocking(path);
            }
        }
    })
    .map_err(|err| tracing::warn!("cannot watch project folder: {err}"))
    .ok()?;
    watcher
        .watch(root, notify::RecursiveMode::Recursive)
        .map_err(|err| tracing::warn!("cannot watch {}: {err}", root.display()))
        .ok()?;

    forward(receiver, cx);
    Some(watcher)
}

/// Emits the watcher thread's paths in debounced, sorted batches.
fn forward(receiver: smol::channel::Receiver<PathBuf>, cx: &mut Context<ProjectWatcher>) {
    cx.spawn(async move |this, cx| {
        while let Ok(first) = receiver.recv().await {
            let mut paths = vec![first];
            cx.background_executor().timer(WATCH_DEBOUNCE).await;
            while let Ok(path) = receiver.try_recv() {
                paths.push(path);
            }
            paths.sort();
            paths.dedup();
            if this
                .update(cx, |_, cx| cx.emit(FilesChanged { paths }))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

/// The project's root `.gitignore` and `.git/info/exclude`, plus the usual
/// build and dependency folders for projects without a `.gitignore`.
fn ignore_rules(root: &Path) -> ignore::gitignore::Gitignore {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    for file in [root.join(".gitignore"), root.join(".git/info/exclude")] {
        if file.is_file() {
            let _ = builder.add(file);
        }
    }
    for heavy in [
        "node_modules/",
        ".gradle/",
        "__pycache__/",
        ".venv/",
        ".dart_tool/",
        "Pods/",
    ] {
        let _ = builder.add_line(None, heavy);
    }
    builder
        .build()
        .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
}

fn is_ignored(rules: &ignore::gitignore::Gitignore, git_dir: &Path, path: &Path) -> bool {
    // `.git` itself is filtered by `is_relevant`; never treat it as ignored.
    !path.starts_with(git_dir) && rules.matched_path_or_any_parents(path, false).is_ignore()
}

/// Inside `.git/`, only the files that mean "staging, branch, refs or a
/// merge changed" (not objects, logs or lock files).
fn is_relevant(git_dir: &Path, path: &Path) -> bool {
    match path.strip_prefix(git_dir) {
        Ok(inside) => {
            let inside = inside.to_string_lossy().replace('\\', "/");
            matches!(
                inside.as_str(),
                "index" | "HEAD" | "FETCH_HEAD" | "MERGE_HEAD" | "packed-refs"
            ) || (inside.starts_with("refs/") && !inside.ends_with(".lock"))
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_internals_are_filtered() {
        let git = Path::new("/p/.git");
        assert!(is_relevant(git, Path::new("/p/src/main.rs")));
        assert!(is_relevant(git, Path::new("/p/.git/index")));
        assert!(!is_relevant(git, Path::new("/p/.git/objects/ab/cdef")));
        assert!(is_relevant(git, Path::new("/p/.git/refs/heads/main")));
        assert!(!is_relevant(git, Path::new("/p/.git/refs/heads/main.lock")));
    }

    #[test]
    fn ignored_build_output_is_dropped() {
        let root = std::env::temp_dir().join(format!("ph-watch-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".gitignore"), "/target\n*.log\n").unwrap();
        let rules = ignore_rules(&root);
        let git = root.join(".git");
        assert!(is_ignored(&rules, &git, &root.join("target/debug/app")));
        assert!(is_ignored(
            &rules,
            &git,
            &root.join("web/node_modules/x/index.js")
        ));
        assert!(is_ignored(&rules, &git, &root.join("run.log")));
        assert!(!is_ignored(&rules, &git, &root.join("src/main.rs")));
        assert!(!is_ignored(&rules, &git, &git.join("index")));
    }
}
