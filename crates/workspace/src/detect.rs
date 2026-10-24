//! What kind of project a folder is, from the files at its root.
//!
//! PilotHouse offers features that fit the project instead of showing every
//! tool to everyone (see docs/architecture.md, "Feature loading"). Detection
//! only reads directory entries, so it is cheap enough to run on every open.

use std::path::Path;

/// A technology or platform the project uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stack {
    Rust,
    Node,
    Python,
    Go,
    Android,
    Ios,
    Web,
    Docker,
}

impl Stack {
    pub fn label(self) -> &'static str {
        match self {
            Stack::Rust => "Rust",
            Stack::Node => "Node",
            Stack::Python => "Python",
            Stack::Go => "Go",
            Stack::Android => "Android",
            Stack::Ios => "iOS",
            Stack::Web => "Web",
            Stack::Docker => "Docker",
        }
    }
}

/// Root-level marker files for each stack. A name ending in `/` is a folder.
const SIGNALS: &[(Stack, &[&str])] = &[
    (Stack::Rust, &["Cargo.toml"]),
    (Stack::Node, &["package.json"]),
    (
        Stack::Python,
        &["pyproject.toml", "requirements.txt", "setup.py"],
    ),
    (Stack::Go, &["go.mod"]),
    (
        Stack::Android,
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
            "AndroidManifest.xml",
        ],
    ),
    (
        Stack::Docker,
        &[
            "Dockerfile",
            "docker-compose.yml",
            "docker-compose.yaml",
            "compose.yaml",
        ],
    ),
    (
        Stack::Web,
        &[
            "vite.config.ts",
            "vite.config.js",
            "next.config.js",
            "next.config.mjs",
            "index.html",
        ],
    ),
];

/// Detects the stacks of the project at `root`, in a stable order.
pub fn detect(root: &Path) -> Vec<Stack> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    let has = |name: &str| names.iter().any(|existing| existing == name);

    let mut stacks: Vec<Stack> = SIGNALS
        .iter()
        .filter(|(_, markers)| markers.iter().any(|marker| has(marker)))
        .map(|(stack, _)| *stack)
        .collect();
    // Xcode projects are folders named `*.xcodeproj` / `*.xcworkspace`.
    if names
        .iter()
        .any(|name| name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace"))
    {
        stacks.push(Stack::Ios);
    }
    stacks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_markers_at_the_root() {
        let root = std::env::temp_dir().join(format!("ph-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("App.xcodeproj")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("build.gradle.kts"), "").unwrap();
        let stacks = detect(&root);
        assert_eq!(stacks, vec![Stack::Rust, Stack::Android, Stack::Ios]);
        assert!(detect(&root.join("missing")).is_empty());
    }
}
