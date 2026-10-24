//! Agent instruction files: `AGENTS.md` (Codex, and most agents), and the
//! `CLAUDE.md` and `GEMINI.md` that Claude Code and Gemini read instead.
//!
//! Syncing makes `AGENTS.md` the one place to write instructions: Claude
//! Code and Gemini both follow an `@AGENTS.md` line as an import, so their
//! files start with one. Nothing is deleted: a file's own instructions stay
//! below the import, and when there is no `AGENTS.md` yet it is made from
//! the existing file (which then only imports it).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// The shared file every agent reads.
pub const SHARED: &str = "AGENTS.md";
/// The files of agents that read their own, which import [`SHARED`].
pub const IMPORTERS: [&str; 2] = ["CLAUDE.md", "GEMINI.md"];
/// The line that imports the shared file.
const IMPORT: &str = "@AGENTS.md";

/// One instruction file as found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    pub name: &'static str,
    /// `None` when the file does not exist.
    pub text: Option<String>,
}

impl InstructionFile {
    /// Whether the file pulls in `AGENTS.md`.
    pub fn imports_shared(&self) -> bool {
        self.text
            .as_deref()
            .is_some_and(|text| text.lines().any(|line| line.trim() == IMPORT))
    }
}

/// What syncing would write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: &'static str,
    /// What it is, for the person deciding (lowercase, after the name).
    pub summary: String,
    pub text: String,
}

/// The project's instruction files, in the order `AGENTS.md`, `CLAUDE.md`,
/// `GEMINI.md`.
pub fn read(root: &Path) -> Vec<InstructionFile> {
    std::iter::once(SHARED)
        .chain(IMPORTERS)
        .map(|name| InstructionFile {
            name,
            text: std::fs::read_to_string(root.join(name)).ok(),
        })
        .collect()
}

/// The writes that make every agent read `AGENTS.md`; empty when they
/// already do.
pub fn plan(files: &[InstructionFile]) -> Vec<Change> {
    let find = |name: &str| files.iter().find(|file| file.name == name);
    let has_text = |file: &&InstructionFile| {
        file.text
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    };
    let mut changes = Vec::new();

    // The file that becomes AGENTS.md when there is none: the first agent
    // file with instructions of its own.
    let mut moved = None;
    if find(SHARED).is_none_or(|shared| shared.text.is_none()) {
        let source = IMPORTERS
            .iter()
            .filter_map(|name| find(name))
            .find(|file| has_text(file) && !file.imports_shared());
        let text = source
            .and_then(|file| file.text.clone())
            .unwrap_or_else(|| {
                "# Agent instructions\n\nHow to build, test and work on this project.\n".to_string()
            });
        changes.push(Change {
            name: SHARED,
            summary: match source {
                Some(file) => format!("create from {}'s instructions", file.name),
                None => "create, to fill in".to_string(),
            },
            text,
        });
        moved = source.map(|file| file.name);
    }

    for name in IMPORTERS {
        let file = find(name);
        if file.is_some_and(InstructionFile::imports_shared) {
            continue;
        }
        let own = file
            .and_then(|file| file.text.as_deref())
            .unwrap_or_default();
        let (text, summary) = if moved == Some(name) {
            (
                format!("{IMPORT}\n"),
                format!("import {SHARED} (its instructions move there)"),
            )
        } else if own.trim().is_empty() {
            (format!("{IMPORT}\n"), format!("create, importing {SHARED}"))
        } else {
            (
                format!("{IMPORT}\n\n{}", own.trim_start()),
                format!("import {SHARED} above its own instructions"),
            )
        };
        changes.push(Change {
            name,
            summary,
            text,
        });
    }
    changes
}

/// Writes the changes into `root`.
pub fn apply(root: &Path, changes: &[Change]) -> Result<Vec<PathBuf>> {
    changes
        .iter()
        .map(|change| {
            let path = root.join(change.name);
            std::fs::write(&path, &change.text)
                .with_context(|| format!("writing {}", path.display()))?;
            Ok(path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(
        agents: Option<&str>,
        claude: Option<&str>,
        gemini: Option<&str>,
    ) -> Vec<InstructionFile> {
        vec![
            InstructionFile {
                name: SHARED,
                text: agents.map(str::to_string),
            },
            InstructionFile {
                name: "CLAUDE.md",
                text: claude.map(str::to_string),
            },
            InstructionFile {
                name: "GEMINI.md",
                text: gemini.map(str::to_string),
            },
        ]
    }

    #[test]
    fn claude_instructions_move_to_agents_md() {
        let changes = plan(&files(None, Some("Run cargo test.\n"), None));
        assert_eq!(changes.len(), 3);
        assert_eq!(
            (changes[0].name, changes[0].text.as_str()),
            (SHARED, "Run cargo test.\n")
        );
        assert_eq!(
            changes[1].text, "@AGENTS.md\n",
            "CLAUDE.md only imports now"
        );
        assert_eq!(changes[2].text, "@AGENTS.md\n", "GEMINI.md is created");
    }

    #[test]
    fn existing_instructions_are_kept_below_the_import() {
        let changes = plan(&files(
            Some("Shared.\n"),
            Some("Claude only.\n"),
            Some("@AGENTS.md\n"),
        ));
        assert_eq!(changes.len(), 1, "GEMINI.md already imports");
        assert_eq!(changes[0].text, "@AGENTS.md\n\nClaude only.\n");
    }

    #[test]
    fn nothing_to_do_once_synced() {
        assert!(
            plan(&files(
                Some("x"),
                Some("@AGENTS.md\n"),
                Some("@AGENTS.md\nmore")
            ))
            .is_empty()
        );
        let file = InstructionFile {
            name: "CLAUDE.md",
            text: Some("intro\n  @AGENTS.md  \n".into()),
        };
        assert!(file.imports_shared());
    }

    #[test]
    fn an_empty_project_gets_a_starting_point() {
        let changes = plan(&files(None, None, None));
        assert_eq!(changes[0].summary, "create, to fill in");
        assert!(
            changes[1..]
                .iter()
                .all(|change| change.text == "@AGENTS.md\n")
        );
    }

    #[test]
    fn applying_writes_the_files() {
        let root = std::env::temp_dir().join(format!("ph-instructions-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "Use tabs.\n").unwrap();
        let written = apply(&root, &plan(&read(&root))).unwrap();
        assert_eq!(written.len(), 3);
        assert_eq!(
            std::fs::read_to_string(root.join(SHARED)).unwrap(),
            "Use tabs.\n"
        );
        assert!(plan(&read(&root)).is_empty(), "synced");
    }
}
