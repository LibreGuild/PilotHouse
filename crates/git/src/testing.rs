//! Throwaway repositories for tests.

use std::path::PathBuf;

use crate::run::git;

/// A fresh repository with one committed file (`README.md`).
pub fn temp_repo(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ph-git-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let run = |args: &[&str]| git(&root, args).unwrap();
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "# Test\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-q", "-m", "init"]);
    root
}
