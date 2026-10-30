//! Changed files as a tree of their folders, for the Git tab's View as
//! Tree: folders first, then files, each sorted by name; a folder holding
//! only one folder shows as one row (`crates/files/src`), as VS Code does.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    /// A folder: its path (the key it folds by), the name shown, how deep
    /// it sits, and the files under it (indexes into the paths given).
    Folder {
        path: String,
        label: String,
        depth: usize,
        files: Vec<usize>,
    },
    /// File `ix` of the paths given, at `depth`.
    File { ix: usize, depth: usize },
}

/// The rows for `paths` (with `/` between folders); folders in `folded`
/// show without what is in them.
pub fn layout(paths: &[&str], folded: &HashSet<String>) -> Vec<TreeRow> {
    let mut entries: Vec<(usize, &str)> = paths.iter().copied().enumerate().collect();
    entries.sort_by(|a, b| a.1.cmp(b.1));
    let mut rows = Vec::new();
    add_level(&entries, "", 0, folded, &mut rows);
    rows
}

/// Adds the rows of `entries` (paths relative to `parent`), at `depth`.
fn add_level(
    entries: &[(usize, &str)],
    parent: &str,
    depth: usize,
    folded: &HashSet<String>,
    rows: &mut Vec<TreeRow>,
) {
    // Folders by name, in order, each with what is under it.
    let mut folders: Vec<(&str, Vec<(usize, &str)>)> = Vec::new();
    let mut files = Vec::new();
    for &(ix, path) in entries {
        match path.split_once('/') {
            Some((folder, rest)) => match folders.last_mut() {
                Some((name, inside)) if *name == folder => inside.push((ix, rest)),
                _ => folders.push((folder, vec![(ix, rest)])),
            },
            None => files.push((ix, path)),
        }
    }
    for (name, mut inside) in folders {
        // One folder and nothing else inside: shown as one row.
        let mut label = name.to_string();
        while let Some((first, _)) = inside.first().and_then(|(_, rest)| rest.split_once('/')) {
            let first = first.to_string();
            let all_inside = inside.iter().all(|(_, rest)| {
                rest.split_once('/')
                    .is_some_and(|(folder, _)| folder == first)
            });
            if !all_inside {
                break;
            }
            label = format!("{label}/{first}");
            for entry in &mut inside {
                entry.1 = &entry.1[first.len() + 1..];
            }
        }
        let path = if parent.is_empty() {
            label.clone()
        } else {
            format!("{parent}/{label}")
        };
        rows.push(TreeRow::Folder {
            path: path.clone(),
            label,
            depth,
            files: inside.iter().map(|(ix, _)| *ix).collect(),
        });
        if !folded.contains(&path) {
            add_level(&inside, &path, depth + 1, folded, rows);
        }
    }
    rows.extend(files.into_iter().map(|(ix, _)| TreeRow::File { ix, depth }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(rows: &[TreeRow], paths: &[&str]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                TreeRow::Folder { label, depth, .. } => format!("{}{label}/", "  ".repeat(*depth)),
                TreeRow::File { ix, depth } => {
                    let name = paths[*ix].rsplit('/').next().unwrap();
                    format!("{}{name}", "  ".repeat(*depth))
                }
            })
            .collect()
    }

    #[test]
    fn folders_first_and_single_folders_joined() {
        let paths = [
            "README.md",
            "crates/files/src/lib.rs",
            "crates/files/src/editing.rs",
            "crates/git/src/status.rs",
            "Cargo.toml",
            "docs/a.md",
        ];
        let rows = layout(&paths, &HashSet::new());
        assert_eq!(
            shown(&rows, &paths),
            [
                "crates/",
                "  files/src/",
                "    editing.rs",
                "    lib.rs",
                "  git/src/",
                "    status.rs",
                "docs/",
                "  a.md",
                "Cargo.toml",
                "README.md",
            ]
        );
        match &rows[0] {
            TreeRow::Folder { path, files, .. } => {
                assert_eq!(path, "crates");
                assert_eq!(files.len(), 3);
            }
            other => panic!("{other:?}"),
        }
        match &rows[1] {
            TreeRow::Folder { path, .. } => assert_eq!(path, "crates/files/src"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn folded_folders_hide_their_files() {
        let paths = ["src/a.rs", "src/deep/b.rs", "top.rs"];
        let folded: HashSet<String> = ["src".to_string()].into();
        assert_eq!(shown(&layout(&paths, &folded), &paths), ["src/", "top.rs"]);
        let folded: HashSet<String> = ["src/deep".to_string()].into();
        assert_eq!(
            shown(&layout(&paths, &folded), &paths),
            ["src/", "  deep/", "  a.rs", "top.rs"]
        );
    }
}
