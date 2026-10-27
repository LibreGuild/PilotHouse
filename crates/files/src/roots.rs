//! The folders a workspace shows in Files, and the paths that name files in
//! them.
//!
//! With one folder, a path is relative to it (`src/main.rs`), as always.
//! With several, each path starts with its folder's name
//! (`server/src/main.rs`), and the tree's top level lists the folders, like
//! a multi-root workspace in other editors. Two folders with the same name
//! are told apart with a number (`app`, `app 2`).

use std::path::{Path, PathBuf};

use crate::dir_tree::{DirEntry, list_dir};
use crate::file_index::{FileList, index_files};
use crate::search::{SearchResults, search_project};

/// One folder: its name in paths, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots(Vec<Root>);

impl Roots {
    /// `folders` in order, the first the primary one; names from the folder
    /// names, or `label` for a folder given one (the scratch folder).
    pub fn new(folders: &[(PathBuf, Option<String>)]) -> Self {
        let mut roots: Vec<Root> = Vec::new();
        for (path, label) in folders {
            let base = label.clone().unwrap_or_else(|| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "folder".into())
            });
            let mut name = base.clone();
            let mut n = 2;
            while roots.iter().any(|root| root.name == name) {
                name = format!("{base} {n}");
                n += 1;
            }
            roots.push(Root {
                name,
                path: path.clone(),
            });
        }
        Self(roots)
    }

    pub fn single(path: PathBuf) -> Self {
        Self::new(&[(path, None)])
    }

    pub fn list(&self) -> &[Root] {
        &self.0
    }

    pub fn is_multi(&self) -> bool {
        self.0.len() > 1
    }

    /// The primary folder.
    pub fn primary(&self) -> &Path {
        &self.0[0].path
    }

    /// The folder a path is in, and the path within it; `None` for a path
    /// that names no folder, or that would lead out of its folder (`..`, or
    /// an absolute path, which `join` would take as it is).
    pub fn split<'a>(&self, path: &'a str) -> Option<(&Root, &'a str)> {
        let (root, rest) = if self.is_multi() {
            let (name, rest) = path.split_once('/').unwrap_or((path, ""));
            (self.0.iter().find(|root| root.name == name)?, rest)
        } else {
            (&self.0[0], path)
        };
        let inside = Path::new(rest).components().all(|part| {
            matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        inside.then_some((root, rest))
    }

    /// Where `path` is on disk (see [`Self::split`]).
    pub fn absolute(&self, path: &str) -> Option<PathBuf> {
        let (root, rest) = self.split(path)?;
        Some(if rest.is_empty() {
            root.path.clone()
        } else {
            root.path.join(rest)
        })
    }

    /// The path of `absolute`, if it is in one of the folders (the deepest
    /// folder wins when they nest).
    pub fn path_of(&self, absolute: &Path) -> Option<String> {
        let root = self
            .0
            .iter()
            .filter(|root| absolute.starts_with(&root.path))
            .max_by_key(|root| root.path.as_os_str().len())?;
        let rest = absolute
            .strip_prefix(&root.path)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        Some(self.prefixed(root, &rest))
    }

    fn prefixed(&self, root: &Root, rest: &str) -> String {
        match (self.is_multi(), rest.is_empty()) {
            (false, _) => rest.to_string(),
            (true, true) => root.name.clone(),
            (true, false) => format!("{}/{rest}", root.name),
        }
    }

    /// What a tree folder holds: the folders themselves at the top of a
    /// multi-folder workspace.
    pub fn list_dir(&self, dir: &str) -> Vec<DirEntry> {
        if self.is_multi() && dir.is_empty() {
            return self
                .0
                .iter()
                .map(|root| DirEntry {
                    name: root.name.clone(),
                    is_dir: true,
                    ignored: false,
                })
                .collect();
        }
        match self.split(dir) {
            Some((root, rest)) => list_dir(&root.path, rest),
            None => Vec::new(),
        }
    }

    /// Every file of every folder, for the finder.
    pub fn index(&self) -> FileList {
        if !self.is_multi() {
            return index_files(self.primary());
        }
        let files: Vec<std::sync::Arc<str>> = self
            .0
            .iter()
            .flat_map(|root| {
                let listed = index_files(&root.path);
                listed
                    .iter()
                    .map(|file| std::sync::Arc::from(format!("{}/{file}", root.name)))
                    .collect::<Vec<_>>()
            })
            .collect();
        std::sync::Arc::new(files)
    }

    /// Searches every folder; paths in the results name their folder.
    pub fn search(&self, query: &str) -> anyhow::Result<SearchResults> {
        let mut all = SearchResults::default();
        for root in &self.0 {
            let mut found = search_project(&root.path, query)?;
            for hit in &mut found.matches {
                hit.path = self.prefixed(root, &hit.path);
            }
            all.truncated |= found.truncated;
            all.matches.extend(found.matches);
            if all.matches.len() >= crate::search::MAX_RESULTS {
                all.truncated = true;
                all.matches.truncate(crate::search::MAX_RESULTS);
                break;
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("ph-roots-{}", std::process::id()))
            .join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() { serve(); }\n").unwrap();
        dir
    }

    #[test]
    fn one_folder_keeps_plain_paths() {
        let roots = Roots::single(folder("solo"));
        assert!(!roots.is_multi());
        assert_eq!(
            roots.absolute("src/main.rs"),
            Some(folder("solo").join("src/main.rs"))
        );
        assert_eq!(
            roots
                .path_of(&folder("solo").join("src/main.rs"))
                .as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(roots.list_dir("")[0].name, "src");
        assert_eq!(roots.absolute("../outside"), None, "no way out");
        assert_eq!(roots.absolute("src/../../outside"), None);
        assert_eq!(
            roots.absolute("/etc/passwd"),
            None,
            "join would take it whole"
        );
        assert!(roots.list_dir("../..").is_empty());
        assert!(roots.list_dir("/etc").is_empty());
    }

    #[test]
    fn several_folders_name_their_paths() {
        let (client, server) = (folder("client"), folder("server"));
        let other = std::env::temp_dir()
            .join(format!("ph-roots-b-{}", std::process::id()))
            .join("client");
        std::fs::create_dir_all(&other).unwrap();
        let roots = Roots::new(&[
            (client.clone(), None),
            (server.clone(), None),
            (other.clone(), None),
        ]);
        let names: Vec<_> = roots.list().iter().map(|root| root.name.as_str()).collect();
        assert_eq!(names, ["client", "server", "client 2"]);
        assert_eq!(roots.list_dir("").len(), 3, "the folders are the top level");
        assert_eq!(roots.list_dir("server")[0].name, "src");
        assert_eq!(
            roots.absolute("server/src/main.rs"),
            Some(server.join("src/main.rs"))
        );
        assert_eq!(
            roots.path_of(&server.join("src/main.rs")).as_deref(),
            Some("server/src/main.rs")
        );
        assert_eq!(roots.path_of(&other).as_deref(), Some("client 2"));
        assert_eq!(roots.absolute("nope/x"), None);

        assert!(
            roots
                .index()
                .iter()
                .any(|file| &**file == "server/src/main.rs")
        );
        let found = roots.search("serve").unwrap();
        let paths: Vec<_> = found.matches.iter().map(|hit| hit.path.as_str()).collect();
        assert_eq!(paths, ["client/src/main.rs", "server/src/main.rs"]);
    }

    #[test]
    fn labels_name_special_folders() {
        let roots = Roots::new(&[
            (folder("scratch-123"), Some("Scratch".into())),
            (folder("api"), None),
        ]);
        assert_eq!(roots.list()[0].name, "Scratch");
    }
}
