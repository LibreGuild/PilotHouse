//! The file tree's model: directories are listed lazily, on first expand.
//!
//! The tree widget wants a complete `TreeItem` hierarchy, so this model keeps
//! the listings loaded so far and rebuilds the items from them. A directory
//! that has not been listed yet gets one placeholder child, which is what
//! makes the widget draw it as an expandable folder.
//!
//! Ignored files (`.gitignore` and the like) are listed too, marked, so the
//! tree can show them dimmed; the file index and search still skip them.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use gpui_kit::component::tree::TreeItem;
use ignore::WalkBuilder;

/// One row in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// Matched by an ignore file (`.gitignore`, `.ignore`).
    pub ignored: bool,
}

/// Suffix of the placeholder child's id; a NUL can never appear in a path.
const PLACEHOLDER: &str = "/\u{0}loading";

#[derive(Default)]
pub struct DirTree {
    /// Listings by project-relative directory path (`""` is the root).
    listings: HashMap<String, Vec<DirEntry>>,
    expanded: HashSet<String>,
    /// Paths that are ignored, or inside an ignored folder.
    ignored: Rc<HashSet<String>>,
}

impl DirTree {
    pub fn is_listed(&self, dir: &str) -> bool {
        self.listings.contains_key(dir)
    }

    pub fn set_listing(&mut self, dir: &str, entries: Vec<DirEntry>) {
        // Everything in an ignored folder is ignored, whatever the listing
        // of that folder alone says.
        let inherited = self.ignored.contains(dir);
        let ignored = Rc::make_mut(&mut self.ignored);
        for entry in &entries {
            let path = join(dir, &entry.name);
            if inherited || entry.ignored {
                ignored.insert(path);
            } else {
                ignored.remove(&path);
            }
        }
        self.listings.insert(dir.to_string(), entries);
    }

    /// The ignored paths among those listed, to dim them.
    pub fn ignored(&self) -> Rc<HashSet<String>> {
        self.ignored.clone()
    }

    pub fn set_expanded(&mut self, dir: &str, expanded: bool) {
        if expanded {
            self.expanded.insert(dir.to_string());
        } else {
            self.expanded.remove(dir);
        }
    }

    /// Folds every folder.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    pub fn is_placeholder(id: &str) -> bool {
        id.ends_with(PLACEHOLDER)
    }

    /// Builds the widget's items from the loaded listings.
    pub fn items(&self) -> Vec<TreeItem> {
        self.children_of("")
    }

    fn children_of(&self, dir: &str) -> Vec<TreeItem> {
        let Some(entries) = self.listings.get(dir) else {
            return Vec::new();
        };
        entries
            .iter()
            .map(|entry| {
                let path = join(dir, &entry.name);
                let item = TreeItem::new(path.clone(), entry.name.clone());
                if !entry.is_dir {
                    return item;
                }
                let children = self.children_of(&path);
                let item = if children.is_empty() {
                    // Unlisted or empty folder: keep it expandable.
                    item.child(
                        TreeItem::new(format!("{path}{PLACEHOLDER}"), "Loading…").disabled(true),
                    )
                } else {
                    item.children(children)
                };
                item.expanded(self.expanded.contains(&path))
            })
            .collect()
    }
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// Lists one directory: folders first, then files, both alphabetical.
/// Ignored paths are marked by the same rules the file index skips them by.
pub fn list_dir(root: &Path, dir: &str) -> Vec<DirEntry> {
    let absolute = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    let walk = |filtered: bool| {
        WalkBuilder::new(&absolute)
            .max_depth(Some(1))
            // `standard_filters` sets `hidden` too, so it comes first.
            .standard_filters(filtered)
            .hidden(false)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build()
            .flatten()
            .filter(|entry| entry.depth() == 1)
    };
    let kept: HashSet<std::ffi::OsString> = walk(true)
        .map(|entry| entry.file_name().to_os_string())
        .collect();
    let mut entries: Vec<DirEntry> = walk(false)
        .map(|entry| DirEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: entry.file_type().is_some_and(|kind| kind.is_dir()),
            ignored: !kept.contains(entry.file_name()),
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> DirEntry {
        DirEntry {
            name: name.into(),
            is_dir,
            ignored: false,
        }
    }

    #[test]
    fn ignored_files_are_listed_and_marked_down_the_tree() {
        let root = std::env::temp_dir().join(format!("ph-dirtree-ignored-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("build/out")).unwrap();
        std::fs::write(root.join(".ignore"), "build/\n*.log\n").unwrap();
        std::fs::write(root.join("run.log"), "").unwrap();
        std::fs::write(root.join("main.rs"), "").unwrap();
        let listed = list_dir(&root, "");
        let ignored = |name: &str| listed.iter().find(|e| e.name == name).unwrap().ignored;
        assert!(ignored("build") && ignored("run.log"));
        assert!(!ignored("main.rs") && !ignored(".ignore"));

        let mut tree = DirTree::default();
        tree.set_listing("", listed);
        tree.set_listing("build", list_dir(&root, "build"));
        assert!(
            tree.ignored().contains("build/out"),
            "inside an ignored folder"
        );
        assert!(!tree.ignored().contains("main.rs"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unlisted_folders_get_a_placeholder_child() {
        let mut tree = DirTree::default();
        tree.set_listing("", vec![entry("src", true), entry("README.md", false)]);
        let items = tree.items();
        assert_eq!(items.len(), 2);
        assert!(items[0].is_folder());
        assert!(DirTree::is_placeholder(&items[0].children[0].id));
        assert!(!items[1].is_folder());

        tree.set_listing("src", vec![entry("main.rs", false)]);
        tree.set_expanded("src", true);
        let items = tree.items();
        assert_eq!(items[0].children[0].id.as_ref(), "src/main.rs");
        assert!(items[0].is_expanded());
    }

    #[test]
    fn lists_folders_first() {
        let root = std::env::temp_dir().join(format!("ph-dirtree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("zeta")).unwrap();
        std::fs::write(root.join("alpha.txt"), "").unwrap();
        let names: Vec<String> = list_dir(&root, "")
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["zeta", "alpha.txt"]);
    }
}
