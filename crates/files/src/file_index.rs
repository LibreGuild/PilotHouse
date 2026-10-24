//! The list of every file in a project, for the fuzzy finder and `@file`.
//!
//! Built on a background thread with the `ignore` crate, so `.gitignore`d
//! paths (build output, dependencies) are skipped exactly like `git` and
//! ripgrep skip them. `.git` itself is always excluded.

use std::path::Path;
use std::sync::Arc;

use ignore::WalkBuilder;

/// Stop indexing after this many files; huge monorepos stay responsive and
/// the finder still covers everything most people search for.
const MAX_FILES: usize = 200_000;

/// Project-relative paths with `/` separators, sorted.
pub type FileList = Arc<Vec<Arc<str>>>;

pub fn index_files(root: &Path) -> FileList {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if let Ok(relative) = entry.path().strip_prefix(root) {
            files.push(Arc::from(relative.to_string_lossy().replace('\\', "/")));
        }
        if files.len() >= MAX_FILES {
            tracing::warn!(
                "file index capped at {MAX_FILES} files for {}",
                root.display()
            );
            break;
        }
    }
    files.sort();
    Arc::new(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_gitignore_and_skips_git_dir() {
        let root = std::env::temp_dir().join(format!("ph-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("target/out.o"), "").unwrap();
        std::fs::write(root.join(".git/HEAD"), "").unwrap();
        // `ignore` only applies .gitignore inside a git repository.
        std::fs::write(root.join(".git/config"), "").unwrap();

        let files = index_files(&root);
        let files: Vec<&str> = files.iter().map(|file| file.as_ref()).collect();
        assert!(files.contains(&"src/main.rs"));
        assert!(files.contains(&".gitignore"));
        assert!(!files.iter().any(|file| file.starts_with("target/")));
        assert!(!files.iter().any(|file| file.starts_with(".git/")));
    }
}
