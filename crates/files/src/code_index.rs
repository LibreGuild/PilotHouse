//! Where things are defined in a folder, kept for every open file of it.
//!
//! Go to definition without a language server (or before one is ready),
//! the outline of a file and Go to Symbol read this index instead of
//! scanning the folder again for each file and each click. It is built
//! once, on background threads (all cores), the first time a file asks;
//! afterwards only the files the watcher reports changed are read again,
//! the next time someone asks. Files above [`MAX_FILE`] bytes (generated
//! or vendored code) are skipped, and at most [`MAX_FILES`] are read, so a
//! huge monorepo costs a bounded amount of memory.
//!
//! The index lives as long as some open file holds it: closing the last
//! file of a folder frees it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};

use ignore::{WalkBuilder, WalkState};

use crate::symbols::{Symbol, scan_text};

/// Files larger than this are not read.
const MAX_FILE: u64 = 512 * 1024;
/// Most source files read.
const MAX_FILES: usize = 20_000;

/// The indexes in use, by folder.
static INDEXES: LazyLock<Mutex<HashMap<PathBuf, Weak<CodeIndex>>>> =
    LazyLock::new(Default::default);

pub struct CodeIndex {
    root: PathBuf,
    /// Built on first use; locked while it is built or brought up to date,
    /// on a background thread.
    files: Mutex<Option<HashMap<Arc<str>, Vec<Symbol>>>>,
    /// Folder-relative paths changed since they were read. A separate lock,
    /// so the UI thread never waits for a build to note a change.
    stale: Mutex<HashSet<Arc<str>>>,
}

impl CodeIndex {
    /// The index of `root`, shared with the other open files of it.
    pub fn for_root(root: &Path) -> Arc<Self> {
        let mut indexes = lock(&INDEXES);
        if let Some(index) = indexes.get(root).and_then(Weak::upgrade) {
            return index;
        }
        indexes.retain(|_, index| index.strong_count() > 0);
        let index = Arc::new(Self {
            root: root.to_path_buf(),
            files: Mutex::new(None),
            stale: Mutex::new(HashSet::new()),
        });
        indexes.insert(root.to_path_buf(), Arc::downgrade(&index));
        index
    }

    /// The index of `root` if a file uses one (nothing is built for a
    /// change in a folder no one looks up).
    pub fn existing(root: &Path) -> Option<Arc<Self>> {
        lock(&INDEXES).get(root).and_then(Weak::upgrade)
    }

    /// Notes that `paths` (absolute) changed, appeared or went away.
    pub fn invalidate(&self, paths: &[PathBuf]) {
        let mut stale = lock(&self.stale);
        for path in paths {
            if let Ok(relative) = path.strip_prefix(&self.root) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                if !relative.split('/').any(|part| part == ".git") {
                    stale.insert(relative.into());
                }
            }
        }
    }

    /// Builds the index now if it is not yet (for a background thread,
    /// ahead of the first question).
    pub fn warm(&self) {
        drop(self.current());
    }

    /// Where `name` is defined, nearer files first: `here` (folder-relative)
    /// itself, then by folder depth.
    pub fn definitions(&self, name: &str, here: Option<&str>) -> Vec<Symbol> {
        let guard = self.current();
        let Some(files) = guard.as_ref() else {
            return Vec::new();
        };
        let mut found: Vec<Symbol> = files
            .values()
            .flatten()
            .filter(|symbol| symbol.name == name)
            .cloned()
            .collect();
        found.sort_by(|a, b| {
            let depth = |symbol: &Symbol| symbol.path.matches('/').count();
            (Some(a.path.as_ref()) != here, depth(a), &a.path, a.line).cmp(&(
                Some(b.path.as_ref()) != here,
                depth(b),
                &b.path,
                b.line,
            ))
        });
        found
    }

    /// Every symbol whose name contains `query` (ignoring case), names that
    /// start with it first, at most `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Symbol> {
        let query = query.trim().to_lowercase();
        let guard = self.current();
        let Some(files) = guard.as_ref() else {
            return Vec::new();
        };
        let mut found: Vec<(bool, &Symbol)> = files
            .values()
            .flatten()
            .filter_map(|symbol| {
                let name = symbol.name.to_lowercase();
                name.contains(&query)
                    .then(|| (!name.starts_with(&query), symbol))
            })
            .collect();
        found.sort_by(|a, b| {
            (a.0, a.1.name.len(), &a.1.name, &a.1.path).cmp(&(
                b.0,
                b.1.name.len(),
                &b.1.name,
                &b.1.path,
            ))
        });
        found
            .into_iter()
            .take(limit)
            .map(|(_, symbol)| symbol.clone())
            .collect()
    }

    /// The index, built if need be and with changed files read again.
    fn current(&self) -> MutexGuard<'_, Option<HashMap<Arc<str>, Vec<Symbol>>>> {
        let mut files = lock(&self.files);
        match files.as_mut() {
            None => {
                // What changed before the build is in it already.
                lock(&self.stale).clear();
                *files = Some(build(&self.root));
            }
            Some(index) => {
                let stale: Vec<Arc<str>> = lock(&self.stale).drain().collect();
                for relative in stale {
                    let path = self.root.join(relative.as_ref());
                    match read_symbols(&path, &relative) {
                        Some(symbols) => {
                            index.insert(relative, symbols);
                        }
                        None => {
                            index.remove(&relative);
                        }
                    }
                }
            }
        }
        files
    }
}

/// Reads every source file under `root` (skipping what Git ignores) on all
/// cores.
fn build(root: &Path) -> HashMap<Arc<str>, Vec<Symbol>> {
    let found = Mutex::new(HashMap::new());
    let read = AtomicUsize::new(0);
    WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build_parallel()
        .run(|| {
            let (found, read) = (&found, &read);
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    return WalkState::Continue;
                }
                let Ok(relative) = entry.path().strip_prefix(root) else {
                    return WalkState::Continue;
                };
                let relative: Arc<str> = relative.to_string_lossy().replace('\\', "/").into();
                if !is_source(&relative) {
                    return WalkState::Continue;
                }
                if read.fetch_add(1, Ordering::Relaxed) >= MAX_FILES {
                    return WalkState::Quit;
                }
                if let Some(symbols) = read_symbols(entry.path(), &relative) {
                    lock(found).insert(relative, symbols);
                }
                WalkState::Continue
            })
        });
    if read.load(Ordering::Relaxed) >= MAX_FILES {
        tracing::warn!(
            "code index capped at {MAX_FILES} files for {}",
            root.display()
        );
    }
    found.into_inner().unwrap_or_else(|err| err.into_inner())
}

/// A source file's symbols, or `None` when it is gone, too big or not
/// source.
fn read_symbols(path: &Path, relative: &Arc<str>) -> Option<Vec<Symbol>> {
    if !is_source(relative) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some(scan_text(relative, extension(relative), &text))
}

fn extension(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
}

fn is_source(path: &str) -> bool {
    crate::symbols::has_patterns(extension(path))
}

/// A lock, even after a thread panicked holding it (the data is a cache).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("ph-code-index-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        root
    }

    #[test]
    fn finds_definitions_and_follows_changes() {
        let root = folder("follow");
        std::fs::write(root.join("src/a.rs"), "pub fn load() {}\nstruct Store;\n").unwrap();
        std::fs::write(root.join("src/deep/b.rs"), "fn load() {}\n").unwrap();
        std::fs::write(root.join("notes.md"), "fn load() {}\n").unwrap();
        let index = CodeIndex::for_root(&root);
        assert!(Arc::ptr_eq(&index, &CodeIndex::for_root(&root)), "shared");

        let found = index.definitions("load", Some("src/deep/b.rs"));
        let places: Vec<(&str, usize)> = found
            .iter()
            .map(|symbol| (symbol.path.as_ref(), symbol.line))
            .collect();
        assert_eq!(places, [("src/deep/b.rs", 1), ("src/a.rs", 1)]);

        // A file changes, one appears, one goes away.
        std::fs::write(root.join("src/a.rs"), "\n\npub fn load() {}\n").unwrap();
        std::fs::write(root.join("src/c.rs"), "fn load() {}\n").unwrap();
        std::fs::remove_file(root.join("src/deep/b.rs")).unwrap();
        index.invalidate(&[
            root.join("src/a.rs"),
            root.join("src/c.rs"),
            root.join("src/deep/b.rs"),
        ]);
        let found = index.definitions("load", None);
        let places: Vec<(&str, usize)> = found
            .iter()
            .map(|symbol| (symbol.path.as_ref(), symbol.line))
            .collect();
        assert_eq!(places, [("src/a.rs", 3), ("src/c.rs", 1)]);
        assert!(index.definitions("Store", None).is_empty());

        let search: Vec<String> = index
            .search("LO", 10)
            .into_iter()
            .map(|symbol| symbol.name)
            .collect();
        assert_eq!(search, ["load", "load"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn freed_with_its_last_user() {
        let root = folder("freed");
        let index = CodeIndex::for_root(&root);
        assert!(CodeIndex::existing(&root).is_some());
        drop(index);
        assert!(CodeIndex::existing(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
