//! One file-system watcher per folder, shared by every panel that cares
//! ([`WatcherHub`]).
//!
//! Events are batched for [`WATCH_DEBOUNCE`] so a build or `git checkout`
//! touching thousands of files produces one [`FilesChanged`] event. Paths
//! inside `.git/` are dropped except the ones that signal a repository change
//! (index, HEAD), which the Changes panel needs. Paths the project ignores
//! (`target/`, `node_modules/`… via `.gitignore`) are dropped on the watcher
//! thread, as a build writes thousands of them. The few a panel has on
//! screen (an open log, an ignored folder open in the tree; see
//! [`Follower`]) are kept, and reported apart as [`IgnoredChanged`].
//!
//! Linux has no watch for a whole tree, so there each folder gets one of
//! its own (see [`PER_FOLDER`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task, WeakEntity};
use ignore::gitignore::Gitignore;

const WATCH_DEBOUNCE: Duration = Duration::from_millis(150);

/// Whether each folder gets a watch of its own. Linux (inotify) has no
/// watch for a whole tree: asked for one, `notify` walks every folder before
/// `watch` returns, the ignored ones too (`target/`, `node_modules/`), and
/// each takes one of the user's limited watches. So there the folders are
/// walked here, off the UI thread and without the ignored ones (see
/// [`FolderWatches`]). The other systems follow a tree with one watch.
const PER_FOLDER: bool = cfg!(target_os = "linux");

/// A batch of changed paths (absolute, sorted, without duplicates).
#[derive(Debug, Clone)]
pub struct FilesChanged {
    pub paths: Vec<PathBuf>,
}

/// A batch of changed paths that the project ignores but a panel has on
/// screen (see [`Follower`]), for the views that show them. They are not
/// among [`FilesChanged`]: Git, search and schedules never look at ignored
/// files, and a log that grows would keep all three busy.
#[derive(Debug, Clone)]
pub struct IgnoredChanged {
    pub paths: Vec<PathBuf>,
}

/// The system follows no more folders (Linux: the user's
/// `fs.inotify.max_user_watches` are used up), so changes in some folders
/// are not reported. Sent once.
#[derive(Debug, Clone)]
pub struct WatchLimitReached;

pub struct ProjectWatcher {
    root: PathBuf,
    followed: Arc<Followed>,
    _watcher: Option<Arc<Mutex<notify::RecommendedWatcher>>>,
    /// Gives the folders their watches, where each needs its own.
    _folders: Option<Task<()>>,
}

impl EventEmitter<FilesChanged> for ProjectWatcher {}
impl EventEmitter<IgnoredChanged> for ProjectWatcher {}
impl EventEmitter<WatchLimitReached> for ProjectWatcher {}

impl ProjectWatcher {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        match start(&root, cx) {
            Some(watcher) => watcher,
            None => Self::inert(root),
        }
    }

    /// A watcher that reports nothing (see [`WatcherHub::turn_off`]).
    fn inert(root: PathBuf) -> Self {
        Self {
            root,
            followed: Arc::default(),
            _watcher: None,
            _folders: None,
        }
    }

    /// Watches one file opened outside any project: its folder, not
    /// recursively (it may be a big one, like Downloads), keeping only
    /// events for the file.
    pub fn for_file(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let watcher = start_file(&path, cx);
        Self {
            root: path,
            followed: Arc::default(),
            _watcher: watcher.map(|watcher| Arc::new(Mutex::new(watcher))),
            _folders: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A way for a panel to say which ignored paths it has on screen.
    pub fn follower(&self) -> Follower {
        Follower::new(&self.followed)
    }
}

/// Names the ignored paths one panel has on screen, whose changes are then
/// reported ([`IgnoredChanged`]) though what else is ignored is dropped: its
/// open files, and the folders whose entries its tree lists (a change to an
/// entry of such a folder counts, one deeper inside does not). They are
/// forgotten when this is dropped.
pub struct Follower {
    id: u64,
    followed: Arc<Followed>,
}

impl Follower {
    fn new(followed: &Arc<Followed>) -> Self {
        Self {
            id: followed.next.fetch_add(1, Ordering::Relaxed),
            followed: followed.clone(),
        }
    }

    /// Replaces the paths on screen. Paths that are not ignored may be
    /// among them: those are reported whoever shows them.
    pub fn set(&self, paths: HashSet<PathBuf>) {
        self.followed.set(self.id, Some(paths));
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.followed.set(self.id, None);
    }
}

/// The ignored paths the panels have on screen, by [`Follower`].
#[derive(Default)]
struct Followed {
    paths: Mutex<HashMap<u64, HashSet<PathBuf>>>,
    /// The next follower's number.
    next: AtomicU64,
    /// Where each folder has a watch of its own: tells [`FolderWatches`]
    /// that the paths changed, as an ignored folder has none until then.
    changed: Option<smol::channel::Sender<Folders>>,
}

impl Followed {
    /// Whether `path`, or the folder it is in, is on screen.
    fn has(&self, path: &Path) -> bool {
        let folder = path.parent();
        self.paths
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .values()
            .any(|shown| {
                shown.contains(path) || folder.is_some_and(|folder| shown.contains(folder))
            })
    }

    /// Every path on screen.
    fn all(&self) -> Vec<PathBuf> {
        self.paths
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    /// Replaces a follower's paths, or forgets them (`None`).
    fn set(&self, follower: u64, paths: Option<HashSet<PathBuf>>) {
        {
            let mut all = self.paths.lock().unwrap_or_else(|err| err.into_inner());
            match paths {
                Some(paths) if all.get(&follower) == Some(&paths) => return,
                Some(paths) => {
                    all.insert(follower, paths);
                }
                None => {
                    if all.remove(&follower).is_none() {
                        return;
                    }
                }
            }
        }
        if let Some(changed) = &self.changed {
            let _ = changed.try_send(Folders::Followed);
        }
    }
}

/// What the watcher thread passes on.
#[derive(Debug, PartialEq)]
enum Seen {
    Changed(PathBuf),
    /// An ignored path a panel has on screen.
    Ignored(PathBuf),
    /// The system follows no more folders.
    Limit,
}

/// What [`FolderWatches`] is told.
enum Folders {
    /// Something was made at, or moved to, this path: a folder, maybe.
    Made(PathBuf),
    /// The paths on screen changed (see [`Followed`]).
    Followed,
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
    let (sender, receiver) = smol::channel::unbounded::<Seen>();
    let wanted = path.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if !is_change(&event.kind) {
            return;
        }
        for path in event.paths {
            if path == wanted {
                let _ = sender.send_blocking(Seen::Changed(path));
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
fn start(root: &Path, cx: &mut Context<ProjectWatcher>) -> Option<ProjectWatcher> {
    use notify::Watcher as _;

    let (sender, receiver) = smol::channel::unbounded::<Seen>();
    let git_dir = root.join(".git");
    let rules = Arc::new(ignore_rules(root));
    let folders = PER_FOLDER.then(smol::channel::unbounded::<Folders>);
    let followed = Arc::new(Followed {
        changed: folders.as_ref().map(|(changed, _)| changed.clone()),
        ..Followed::default()
    });
    let callback = {
        let (sender, git_dir) = (sender.clone(), git_dir.clone());
        let (rules, followed) = (rules.clone(), followed.clone());
        let made = folders.as_ref().map(|(made, _)| made.clone());
        move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !is_change(&event.kind) {
                return;
            }
            let made = made.as_ref().filter(|_| may_bring_folder(&event.kind));
            for path in event.paths {
                if let Some(made) = made
                    && is_watched_dir(&rules, &git_dir, &path)
                {
                    let _ = made.try_send(Folders::Made(path.clone()));
                }
                if let Some(seen) = seen(&git_dir, &rules, &followed, path) {
                    let _ = sender.send_blocking(seen);
                }
            }
        }
    };
    let mut watcher = notify::recommended_watcher(callback)
        .map_err(|err| tracing::warn!("cannot watch project folder: {err}"))
        .ok()?;
    if !PER_FOLDER {
        watcher
            .watch(root, notify::RecursiveMode::Recursive)
            .map_err(|err| tracing::warn!("cannot watch {}: {err}", root.display()))
            .ok()?;
    }
    let watcher = Arc::new(Mutex::new(watcher));
    let folders = folders.map(|(_, changes)| {
        let watches = FolderWatches {
            root: root.to_path_buf(),
            git_dir,
            rules,
            followed: followed.clone(),
            watcher: Arc::downgrade(&watcher),
            seen: sender,
            told: false,
            extra: HashSet::new(),
        };
        cx.background_spawn(watches.run(changes))
    });

    forward(receiver, cx);
    Some(ProjectWatcher {
        root: root.to_path_buf(),
        followed,
        _watcher: Some(watcher),
        _folders: folders,
    })
}

/// What the panels are told of a changed path: nothing of Git's own files,
/// and of an ignored path only when a panel has it on screen.
fn seen(git_dir: &Path, rules: &Gitignore, followed: &Followed, path: PathBuf) -> Option<Seen> {
    if !is_relevant(git_dir, &path) {
        None
    } else if !is_ignored(rules, git_dir, &path) {
        Some(Seen::Changed(path))
    } else if followed.has(&path) {
        Some(Seen::Ignored(path))
    } else {
        None
    }
}

/// Emits the watcher thread's paths in debounced, sorted batches.
fn forward(receiver: smol::channel::Receiver<Seen>, cx: &mut Context<ProjectWatcher>) {
    cx.spawn(async move |this, cx| {
        while let Ok(first) = receiver.recv().await {
            let mut batch = vec![first];
            cx.background_executor().timer(WATCH_DEBOUNCE).await;
            while let Ok(seen) = receiver.try_recv() {
                batch.push(seen);
            }
            let (mut paths, mut ignored, mut limit) = (Vec::new(), Vec::new(), false);
            for seen in batch {
                match seen {
                    Seen::Changed(path) => paths.push(path),
                    Seen::Ignored(path) => ignored.push(path),
                    Seen::Limit => limit = true,
                }
            }
            paths.sort();
            paths.dedup();
            ignored.sort();
            ignored.dedup();
            let told = this.update(cx, |_, cx| {
                if !paths.is_empty() {
                    cx.emit(FilesChanged { paths });
                }
                if !ignored.is_empty() {
                    cx.emit(IgnoredChanged { paths: ignored });
                }
                if limit {
                    cx.emit(WatchLimitReached);
                }
            });
            if told.is_err() {
                break;
            }
        }
    })
    .detach();
}

/// Gives each folder its watch where the system has none for a whole tree
/// (see [`PER_FOLDER`]): the folders there at the start, those made later,
/// and ignored folders for as long as a panel has them on screen.
struct FolderWatches<W> {
    root: PathBuf,
    git_dir: PathBuf,
    rules: Arc<Gitignore>,
    followed: Arc<Followed>,
    /// Not kept alive from here: the watcher ends with its
    /// [`ProjectWatcher`], and the work here with it.
    watcher: Weak<Mutex<W>>,
    /// Where to say that the system follows no more folders.
    seen: smol::channel::Sender<Seen>,
    /// That was said.
    told: bool,
    /// Ignored folders that have a watch, being on screen.
    extra: HashSet<PathBuf>,
}

impl<W: notify::Watcher> FolderWatches<W> {
    async fn run(mut self, changes: smol::channel::Receiver<Folders>) {
        let root = self.root.clone();
        self.add_tree(&root);
        while let Ok(change) = changes.recv().await {
            match change {
                Folders::Made(path) => {
                    // A link to a folder is not followed (see `each_folder`).
                    if std::fs::symlink_metadata(&path).is_ok_and(|made| made.is_dir()) {
                        self.add_tree(&path);
                    }
                }
                Folders::Followed => self.follow(),
            }
        }
    }

    /// Watches `top` and the folders in it, but for the ignored ones.
    fn add_tree(&mut self, top: &Path) {
        let (rules, git_dir) = (self.rules.clone(), self.git_dir.clone());
        each_folder(
            top,
            &|folder| is_watched_dir(&rules, &git_dir, folder),
            &mut |folder| self.watch(folder),
        );
    }

    /// Watches one folder. `false` when no more can be watched: the
    /// system's limit is reached, or the watcher is gone.
    fn watch(&mut self, folder: &Path) -> bool {
        let Some(watcher) = self.watcher.upgrade() else {
            return false;
        };
        let watched = watcher
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .watch(folder, notify::RecursiveMode::NonRecursive);
        match watched {
            Ok(()) => true,
            Err(notify::Error {
                kind: notify::ErrorKind::MaxFilesWatch,
                ..
            }) => {
                if !std::mem::replace(&mut self.told, true) {
                    tracing::warn!(
                        "cannot follow every folder of {}: the system's limit of watches \
                         (fs.inotify.max_user_watches) is reached",
                        self.root.display()
                    );
                    let _ = self.seen.try_send(Seen::Limit);
                }
                false
            }
            // Gone meanwhile, or not ours to read: it is left out, and the
            // other folders are still followed.
            Err(err) => {
                if folder == self.root {
                    tracing::warn!("cannot watch {}: {err}", folder.display());
                }
                true
            }
        }
    }

    /// Gives the ignored folders on screen a watch (the walk left them
    /// out), and takes it from those no longer there.
    fn follow(&mut self) {
        // Copied out first: the watcher thread waits for this lock, and
        // the folders are looked at on disk below.
        let shown = self.followed.all();
        let wanted: HashSet<PathBuf> = shown
            .iter()
            .filter_map(|path| self.unwatched_folder(path))
            .collect();
        if let Some(watcher) = self.watcher.upgrade() {
            let mut watcher = watcher.lock().unwrap_or_else(|err| err.into_inner());
            for gone in self.extra.difference(&wanted) {
                // Refused when the folder is deleted: so is its watch.
                let _ = watcher.unwatch(gone);
            }
        }
        // Asked again for those that have one: a folder deleted and made
        // again has lost it.
        self.extra = wanted
            .into_iter()
            .filter(|folder| self.watch(folder))
            .collect();
    }

    /// The folder to watch for an ignored path on screen (a folder whose
    /// entries are listed, or an open file's folder), if the walk gave it
    /// no watch.
    fn unwatched_folder(&self, path: &Path) -> Option<PathBuf> {
        let folder = if path.is_dir() { path } else { path.parent()? };
        let ours = folder.starts_with(&self.root)
            && folder != self.root
            && !folder.starts_with(&self.git_dir);
        (ours && !is_watched_dir(&self.rules, &self.git_dir, folder)).then(|| folder.to_path_buf())
    }
}

/// Calls `watch` for `top` and for every folder under it that is `wanted`
/// (one that is not is not looked into), until `watch` says to stop. A
/// folder is watched before it is read, so a folder made in it meanwhile is
/// either listed or reported.
fn each_folder(top: &Path, wanted: &dyn Fn(&Path) -> bool, watch: &mut dyn FnMut(&Path) -> bool) {
    let mut todo = vec![top.to_path_buf()];
    while let Some(folder) = todo.pop() {
        if !watch(&folder) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            // `file_type` does not follow a link. A linked folder is not
            // walked: it may lead anywhere (out of the project, or round in
            // a circle), and the other systems do not follow it either.
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                let path = entry.path();
                if wanted(&path) {
                    todo.push(path);
                }
            }
        }
    }
}

/// Whether folder `dir` (inside the project) gets a watch in the walk: not
/// an ignored one, and of `.git` only what [`is_relevant`] lets through
/// (`.git` itself, for `index` and `HEAD`, and `refs`).
fn is_watched_dir(rules: &Gitignore, git_dir: &Path, dir: &Path) -> bool {
    match dir.strip_prefix(git_dir) {
        Ok(inside) => inside.as_os_str().is_empty() || inside.starts_with("refs"),
        Err(_) => !is_ignored_as(rules, git_dir, dir, true),
    }
}

/// Whether an event may have brought a folder: something was made, or
/// moved or renamed to where it is now.
fn may_bring_folder(kind: &notify::EventKind) -> bool {
    use notify::EventKind;
    use notify::event::{CreateKind, ModifyKind};

    match kind {
        EventKind::Create(CreateKind::File) => false,
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_)) => true,
        _ => false,
    }
}

/// Whether an event says something changed. Opening, reading and closing a
/// file are events too on Linux (inotify reports them): taken as changes,
/// every read of a file would be answered by reading it again, for ever.
/// A write is always reported as a change of its own as well.
fn is_change(kind: &notify::EventKind) -> bool {
    !matches!(kind, notify::EventKind::Access(_))
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
    is_ignored_as(rules, git_dir, path, false)
}

/// [`is_ignored`], for a path known to be a folder (`is_dir`) or not: a
/// rule such as `target/` is for folders only.
fn is_ignored_as(rules: &Gitignore, git_dir: &Path, path: &Path, is_dir: bool) -> bool {
    // `.git` itself is filtered by `is_relevant`; never treat it as ignored.
    // A path outside the rules' folder (the system reporting the real path
    // of a folder reached through a link) is not matched: the matcher
    // asserts it is inside, and this runs in the watcher's callback, where
    // a panic would end the app.
    !path.starts_with(git_dir)
        && path.starts_with(rules.path())
        && rules.matched_path_or_any_parents(path, is_dir).is_ignore()
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
    fn reading_a_file_is_not_a_change() {
        use notify::EventKind;
        use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, ModifyKind};
        use notify::event::{RemoveKind, RenameMode};

        // What inotify reports when a file is listed, read and closed.
        for read in [
            AccessKind::Open(AccessMode::Any),
            AccessKind::Read,
            AccessKind::Close(AccessMode::Read),
            AccessKind::Close(AccessMode::Write),
            AccessKind::Any,
        ] {
            assert!(!is_change(&EventKind::Access(read)), "{read:?}");
        }
        for change in [
            EventKind::Create(CreateKind::File),
            EventKind::Create(CreateKind::Folder),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(RemoveKind::File),
            // What a system reports when it cannot say more.
            EventKind::Any,
            EventKind::Other,
        ] {
            assert!(is_change(&change), "{change:?}");
        }
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

    /// A project folder of its own for a test, with `/target` ignored.
    fn project(name: &str, folders: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ph-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".gitignore"), "/target\n*.log\n").unwrap();
        for folder in folders {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        root
    }

    #[test]
    fn ignored_paths_on_screen_are_reported_apart() {
        let root = project("followed", &[]);
        let (rules, git) = (ignore_rules(&root), root.join(".git"));
        let followed = Arc::new(Followed::default());
        let tell = |path: &str| seen(&git, &rules, &followed, root.join(path));
        let ignored = |path: &str| Some(Seen::Ignored(root.join(path)));

        assert_eq!(
            tell("src/main.rs"),
            Some(Seen::Changed(root.join("src/main.rs")))
        );
        assert_eq!(tell("run.log"), None);
        assert_eq!(tell("target/debug"), None);
        assert_eq!(tell(".git/objects/ab/cdef"), None);

        // An open log, and an ignored folder open in the tree.
        let panel = Follower::new(&followed);
        panel.set([root.join("run.log"), root.join("target")].into());
        assert_eq!(tell("run.log"), ignored("run.log"));
        assert_eq!(tell("other.log"), None);
        assert_eq!(tell("target/debug"), ignored("target/debug"));
        assert_eq!(tell("target/debug/app"), None, "deeper than what is listed");
        assert_eq!(
            tell("src/main.rs"),
            Some(Seen::Changed(root.join("src/main.rs")))
        );

        // What another panel shows counts as well, until it is gone.
        let other = Follower::new(&followed);
        other.set([root.join("target/debug")].into());
        assert_eq!(tell("target/debug/app"), ignored("target/debug/app"));
        drop(other);
        assert_eq!(tell("target/debug/app"), None);
        panel.set(HashSet::new());
        assert_eq!(tell("run.log"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Stands in for the system's watcher: keeps the folders it is asked to
    /// watch, `room` of them at most.
    #[derive(Default)]
    struct Recorder {
        watched: HashSet<PathBuf>,
        room: usize,
    }

    impl notify::Watcher for Recorder {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(Self::default())
        }

        fn watch(&mut self, path: &Path, _: notify::RecursiveMode) -> notify::Result<()> {
            if !self.watched.contains(path) && self.watched.len() >= self.room {
                return Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch));
            }
            self.watched.insert(path.to_path_buf());
            Ok(())
        }

        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            self.watched.remove(path);
            Ok(())
        }

        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    fn folder_watches(
        root: &Path,
        watcher: &Arc<Mutex<Recorder>>,
    ) -> (FolderWatches<Recorder>, smol::channel::Receiver<Seen>) {
        let (seen, told) = smol::channel::unbounded();
        let watches = FolderWatches {
            root: root.to_path_buf(),
            git_dir: root.join(".git"),
            rules: Arc::new(ignore_rules(root)),
            followed: Arc::default(),
            watcher: Arc::downgrade(watcher),
            seen,
            told: false,
            extra: HashSet::new(),
        };
        (watches, told)
    }

    /// The watched folders as paths within `root`, in order.
    fn watched(root: &Path, watcher: &Mutex<Recorder>) -> Vec<String> {
        let mut folders: Vec<String> = watcher
            .lock()
            .unwrap()
            .watched
            .iter()
            .map(|folder| {
                let inside = folder.strip_prefix(root).unwrap();
                inside.to_string_lossy().replace('\\', "/")
            })
            .collect();
        folders.sort();
        folders
    }

    #[test]
    fn each_folder_gets_a_watch_but_the_ignored_ones() {
        let root = project(
            "folders",
            &[
                "src/app",
                "target/debug/deps",
                "web/node_modules/x",
                ".git/objects/ab",
                ".git/refs/heads",
            ],
        );
        let watcher = Arc::new(Mutex::new(Recorder {
            room: 100,
            ..Recorder::default()
        }));
        let (mut watches, told) = folder_watches(&root, &watcher);
        watches.add_tree(&root);
        let walked = [
            "",
            ".git",
            ".git/refs",
            ".git/refs/heads",
            "src",
            "src/app",
            "web",
        ];
        assert_eq!(watched(&root, &watcher), walked);

        // A folder made later, with what is in it by then.
        std::fs::create_dir_all(root.join("src/new/deep")).unwrap();
        std::fs::create_dir_all(root.join("target/release")).unwrap();
        let gets_watch =
            |folder: &str| is_watched_dir(&watches.rules, &watches.git_dir, &root.join(folder));
        assert!(gets_watch("src/new"));
        assert!(!gets_watch("target/release") && !gets_watch(".git/objects/cd"));
        watches.add_tree(&root.join("src/new"));
        let mut grown = walked.to_vec();
        grown.extend(["src/new", "src/new/deep"]);
        grown.sort();
        assert_eq!(watched(&root, &watcher), grown);

        // An ignored folder open in the tree and the folder of an open
        // ignored file have a watch for as long as they are on screen; a
        // file in a folder that is walked needs none.
        let panel = Follower::new(&watches.followed);
        panel.set(
            [
                root.join("target"),
                root.join("web/node_modules/x/index.js"),
                root.join("src/main.rs"),
                root.join("run.log"),
            ]
            .into(),
        );
        watches.follow();
        let mut shown = grown.clone();
        shown.extend(["target", "web/node_modules/x"]);
        shown.sort();
        assert_eq!(watched(&root, &watcher), shown);
        drop(panel);
        watches.follow();
        assert_eq!(watched(&root, &watcher), grown);
        assert!(told.try_recv().is_err(), "no limit was reached");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_limit_of_watches_is_said_once() {
        let root = project("limit", &["a/b", "c", "d"]);
        let watcher = Arc::new(Mutex::new(Recorder {
            room: 3,
            ..Recorder::default()
        }));
        let (mut watches, told) = folder_watches(&root, &watcher);
        watches.add_tree(&root);
        assert_eq!(watcher.lock().unwrap().watched.len(), 3);
        assert_eq!(told.try_recv(), Ok(Seen::Limit));
        watches.add_tree(&root.join("a"));
        assert!(told.try_recv().is_err(), "said once");

        // The watcher is gone (its workspace closed): the walk ends.
        let (mut watches, _told) = folder_watches(&root, &watcher);
        drop(watcher);
        assert!(!watches.watch(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_what_is_made_or_moved_may_bring_a_folder() {
        use notify::EventKind;
        use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};

        for kind in [
            EventKind::Create(CreateKind::Folder),
            EventKind::Create(CreateKind::Any),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
        ] {
            assert!(may_bring_folder(&kind), "{kind:?}");
        }
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Remove(RemoveKind::Folder),
        ] {
            assert!(!may_bring_folder(&kind), "{kind:?}");
        }
    }
}
