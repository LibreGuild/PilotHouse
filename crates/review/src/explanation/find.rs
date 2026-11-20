//! Finding the explanation of a commit, a line or a file.
//!
//! An explanation says which content it explains: each file's path and the
//! blob id of its content. A commit says which content it put in the
//! repository. Where the two meet, the explanation is that commit's. So the
//! question "which explanation is the newest for this line" needs no answer
//! from us: Git knows which commit last wrote the line, and that commit's
//! content picks the explanation.
//!
//! Explanations come from two places and are looked up the same way: the
//! ones kept on this computer (the store), and the ones shared as files in
//! the repository ([`super::file`]). One that is both is shown as the file,
//! which is what the team reads and may have corrected.

use std::path::Path;

use anyhow::Result;
use ph_store::Store;

use super::file;

/// Where an explanation is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// On this computer: its id in the store, and the chat it was written in.
    Local { id: i64, session: Option<i64> },
    /// A file in the repository, by its path from the root.
    Shared { path: String },
}

/// An explanation that was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub title: String,
    pub markdown: String,
    /// When it was written, in Unix seconds.
    pub written: i64,
    /// Who had it written; empty for one kept on this computer (the user).
    pub author: String,
    /// The agent that wrote it.
    pub agent: String,
    pub source: Source,
    /// The files it explains, each with the blob id of its content.
    pub files: Vec<(String, String)>,
    /// What was asked in the chats that made the change (JSON, see
    /// [`super::prompt::Asked`]); only kept on this computer.
    pub asked: String,
}

impl Found {
    fn local(explanation: ph_store::explanations::Explanation) -> Self {
        Self {
            title: if explanation.title.trim().is_empty() {
                super::title_of(&explanation.markdown)
            } else {
                explanation.title
            },
            markdown: explanation.markdown,
            written: explanation.created_at / 1000,
            author: String::new(),
            agent: explanation.agent,
            source: Source::Local {
                id: explanation.id,
                session: explanation.session_id,
            },
            files: explanation.files,
            asked: explanation.asked,
        }
    }

    fn shared(path: String, shared: file::Shared) -> Self {
        Self {
            title: super::title_of(&shared.markdown),
            markdown: shared.markdown,
            written: shared.written,
            author: shared.author,
            agent: shared.agent,
            source: Source::Shared { path },
            files: shared.files,
            asked: String::new(),
        }
    }

    /// Whether it explains `path` as it is with content `blob`.
    fn explains(&self, path: &str, blob: &str) -> bool {
        self.files
            .iter()
            .any(|(file, id)| file == path && id == blob)
    }

    /// How many of `content`'s files it explains as they are there.
    fn matches(&self, content: &[(String, String)]) -> usize {
        content
            .iter()
            .filter(|(path, blob)| self.explains(path, blob))
            .count()
    }

    /// Whether this is the shared file of `local` (one kept on this
    /// computer): a file that explains the same content.
    fn stands_for(&self, local: &Found) -> bool {
        let sorted = |files: &[(String, String)]| {
            let mut files = files.to_vec();
            files.sort();
            files
        };
        self.is_shared() && sorted(&self.files) == sorted(&local.files)
    }

    pub fn is_shared(&self) -> bool {
        matches!(self.source, Source::Shared { .. })
    }

    /// The what-was-asked record kept with it.
    pub fn asked(&self) -> Vec<super::prompt::Asked> {
        serde_json::from_str(&self.asked).unwrap_or_default()
    }
}

/// The explanations of `content` (files with the blob ids of their
/// content), best first: the one that explains `path` as it is there (when
/// one is asked about), then the one that explains most of the files, then
/// the newest. Reads the store and the repository's folder of explanations.
pub fn of_content(
    store: &Store,
    root: &Path,
    content: &[(String, String)],
    path: Option<&str>,
) -> Vec<Found> {
    // An explanation's files are never part of what one explains.
    let content: Vec<(String, String)> = content
        .iter()
        .filter(|(path, _)| !super::is_explanation_file(path))
        .cloned()
        .collect();
    if content.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<Found> = file::all(root)
        .into_iter()
        .map(|(path, shared)| Found::shared(path, shared))
        .filter(|found| found.matches(&content) > 0)
        .collect();
    let blobs: Vec<String> = content.iter().map(|(_, blob)| blob.clone()).collect();
    let local = store.explanations_of(&blobs).unwrap_or_default();
    for explanation in local {
        let explanation = Found::local(explanation);
        // The same blob id at another path is another file (two empty
        // files, a copy): only path and content together count.
        if explanation.matches(&content) == 0 {
            continue;
        }
        // Shared since: the file stands for it.
        if !found.iter().any(|shared| shared.stands_for(&explanation)) {
            found.push(explanation);
        }
    }
    let asked_blob = path.and_then(|path| {
        content
            .iter()
            .find(|(file, _)| file == path)
            .map(|(_, blob)| blob.as_str())
    });
    found.sort_by_key(|found| {
        let on_path = match (path, asked_blob) {
            (Some(path), Some(blob)) => found.explains(path, blob),
            _ => false,
        };
        (
            std::cmp::Reverse(on_path),
            std::cmp::Reverse(found.matches(&content)),
            std::cmp::Reverse(found.written),
        )
    });
    found
}

/// The explanations of commit `commit` (see [`of_content`]): those that
/// explain what the commit put in the repository.
pub fn of_commit(
    store: &Store,
    root: &Path,
    commit: &str,
    path: Option<&str>,
) -> Result<Vec<Found>> {
    let content = ph_git::content::of_commit(root, commit)?;
    Ok(of_content(store, root, &content, path))
}

/// The explanations that explain file `path` in any state, newest first:
/// what there is to offer when the newest change to a line has none.
pub fn naming(store: &Store, root: &Path, path: &str) -> Vec<Found> {
    let mut found: Vec<Found> = file::all(root)
        .into_iter()
        .map(|(file, shared)| Found::shared(file, shared))
        .filter(|found| found.files.iter().any(|(file, _)| file == path))
        .collect();
    for explanation in store.explanations_naming(path).unwrap_or_default() {
        let explanation = Found::local(explanation);
        if !found.iter().any(|shared| shared.stands_for(&explanation)) {
            found.push(explanation);
        }
    }
    found.sort_by_key(|found| std::cmp::Reverse(found.written));
    found
}

/// The newest explanation that names any file of `content`, in whatever
/// state: what there is about work that has moved on since it was explained.
pub fn of_paths(store: &Store, root: &Path, content: &[(String, String)]) -> Option<Found> {
    /// A change of very many files is not looked up file by file.
    const PATHS: usize = 60;
    content
        .iter()
        .filter(|(path, _)| !super::is_explanation_file(path))
        .take(PATHS)
        .filter_map(|(path, _)| naming(store, root, path).into_iter().next())
        .max_by_key(|found| found.written)
}

/// The files `found` explains whose content at `HEAD` is no longer what it
/// explains: they changed since (or, for work not committed yet, are not
/// there yet).
pub fn changed_since(root: &Path, found: &Found) -> Vec<String> {
    found
        .files
        .iter()
        .filter(|(path, blob)| {
            let now = ph_git::content::at_head(root, path);
            match now {
                Some(now) => now != *blob,
                None => blob != super::GONE,
            }
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// The explanation of what stands at `HEAD` now in the files that changed
/// since `found` was written: the newer one to read. `None` when the newest
/// change has none.
pub fn newer(store: &Store, root: &Path, found: &Found) -> Option<Found> {
    let changed = changed_since(root, found);
    let now: Vec<(String, String)> = changed
        .iter()
        .filter_map(|path| Some((path.clone(), ph_git::content::at_head(root, path)?)))
        .collect();
    of_content(store, root, &now, None)
        .into_iter()
        .find(|newer| newer.source != found.source && newer.written >= found.written)
}
