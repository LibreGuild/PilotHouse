//! Agent instruction files: `AGENTS.md` (Codex, and most agents), and the
//! `CLAUDE.md` and `GEMINI.md` that Claude Code and Gemini read instead.
//!
//! Syncing makes `AGENTS.md` the one place to write instructions: Claude
//! Code and Gemini both follow an `@AGENTS.md` line as an import, so their
//! files start with one. Nothing is deleted: a file's own instructions stay
//! below the import, and when there is no `AGENTS.md` yet it is made from
//! the existing file (which then only imports it).
//!
//! A file that is a symbolic link is left as it is, and never read or
//! written through: `CLAUDE.md -> AGENTS.md` is one file already (an import
//! written into it would make `AGENTS.md` import itself), and a link in a
//! cloned repository can lead anywhere on the disk.

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
    /// `None` when the file does not exist, and for a link.
    pub text: Option<String>,
    /// Where it leads, when it is a symbolic link.
    pub link: Option<Link>,
}

/// Where an instruction file that is a symbolic link leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// To another of the instruction files here: the two are one file.
    To(&'static str),
    /// Anywhere else.
    Elsewhere,
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
/// `GEMINI.md`. Fails when one is there but cannot be read (not UTF-8, no
/// permission): taken as missing, it would be written over.
pub fn read(root: &Path) -> anyhow::Result<Vec<InstructionFile>> {
    let names = || std::iter::once(SHARED).chain(IMPORTERS);
    names()
        .map(|name| {
            let path = root.join(name);
            if is_link(&path) {
                // Which of the others it is, if one: by where both really
                // are (a link that leads nowhere is none of them).
                let target = std::fs::canonicalize(&path).ok();
                let other = names().filter(|other| *other != name).find(|other| {
                    let other = root.join(other);
                    target.is_some()
                        && !is_link(&other)
                        && std::fs::canonicalize(other).ok() == target
                });
                let link = other.map_or(Link::Elsewhere, Link::To);
                return Ok(InstructionFile {
                    name,
                    text: None,
                    link: Some(link),
                });
            }
            let text = match std::fs::read_to_string(path) {
                Ok(text) => Some(text),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => {
                    anyhow::bail!("{name} could not be read ({err}), so nothing is changed")
                }
            };
            Ok(InstructionFile {
                name,
                text,
                link: None,
            })
        })
        .collect()
}

/// Whether `path` is a symbolic link (wherever it leads, or nowhere).
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|found| found.file_type().is_symlink())
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
    let shared = find(SHARED);
    // A link counts as there: agents read through it, and it is not ours
    // to replace.
    if shared.is_none_or(|shared| shared.text.is_none() && shared.link.is_none()) {
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
        // A link is left as it is, and so is the file `AGENTS.md` itself
        // is a link to: an import there would be `AGENTS.md` importing
        // itself.
        if file.is_some_and(|file| file.link.is_some())
            || shared.is_some_and(|shared| shared.link == Some(Link::To(name)))
        {
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

/// Writes the changes into `root`. Nothing is written through a symbolic
/// link: none of the changes is made when one of the files is a link, and
/// each file is written beside its place and moved in, which replaces what
/// is there instead of following it.
pub fn apply(root: &Path, changes: &[Change]) -> Result<Vec<PathBuf>> {
    if let Some(change) = changes
        .iter()
        .find(|change| is_link(&root.join(change.name)))
    {
        anyhow::bail!(
            "{} is a link to another file, so nothing is changed",
            change.name
        );
    }
    changes
        .iter()
        .map(|change| {
            let path = root.join(change.name);
            let partial = root.join(format!("{}.partial", change.name));
            write_new(&partial, &change.text)
                .and_then(|()| std::fs::rename(&partial, &path))
                .inspect_err(|_| {
                    let _ = std::fs::remove_file(&partial);
                })
                .with_context(|| format!("writing {}", path.display()))?;
            Ok(path)
        })
        .collect()
}

/// Writes `text` to a file made for it at `path`: what was left there
/// before is removed, not written into (it could be a link).
fn write_new(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let _ = std::fs::remove_file(path);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(text.as_bytes())
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
                link: None,
            },
            InstructionFile {
                name: "CLAUDE.md",
                text: claude.map(str::to_string),
                link: None,
            },
            InstructionFile {
                name: "GEMINI.md",
                text: gemini.map(str::to_string),
                link: None,
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
            link: None,
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
    fn a_link_is_left_as_it_is() {
        // `CLAUDE.md -> AGENTS.md`: one file already.
        let mut found = files(Some("Shared.\n"), None, Some("@AGENTS.md\n"));
        found[1].link = Some(Link::To(SHARED));
        assert!(plan(&found).is_empty());
        // `AGENTS.md -> CLAUDE.md`: no import is written into the file
        // both are; the third file imports as ever.
        let mut found = files(None, Some("Use tabs.\n"), None);
        found[0].link = Some(Link::To("CLAUDE.md"));
        let changes = plan(&found);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            (changes[0].name, changes[0].text.as_str()),
            ("GEMINI.md", "@AGENTS.md\n")
        );
        // A link to anywhere else is not what `AGENTS.md` is made from.
        let mut found = files(None, None, Some("Gemini only.\n"));
        found[1].link = Some(Link::Elsewhere);
        let changes = plan(&found);
        assert_eq!(
            changes.iter().map(|change| change.name).collect::<Vec<_>>(),
            [SHARED, "GEMINI.md"]
        );
        assert_eq!(changes[0].text, "Gemini only.\n");
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_read_or_written_through_a_link() {
        let root =
            std::env::temp_dir().join(format!("ph-instructions-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let outside = root.join("outside");
        let project = root.join("project");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        use std::os::unix::fs::symlink as link;

        // One file under two names: nothing to do, and nothing written.
        std::fs::write(project.join(SHARED), "Shared.\n").unwrap();
        link(SHARED, project.join("CLAUDE.md")).unwrap();
        std::fs::write(project.join("GEMINI.md"), "@AGENTS.md\n").unwrap();
        let found = read(&project).unwrap();
        assert_eq!(found[1].link, Some(Link::To(SHARED)));
        assert!(plan(&found).is_empty());

        // A link out of the project: its text is not taken, and a change
        // planned before the link was there is refused, all of it.
        std::fs::write(outside.join("secret"), "a key\n").unwrap();
        std::fs::remove_file(project.join("CLAUDE.md")).unwrap();
        std::fs::remove_file(project.join(SHARED)).unwrap();
        let planned = plan(&read(&project).unwrap());
        link(outside.join("secret"), project.join("CLAUDE.md")).unwrap();
        let found = read(&project).unwrap();
        assert_eq!(
            (found[1].link, found[1].text.as_deref()),
            (Some(Link::Elsewhere), None)
        );
        assert!(planned.iter().any(|change| change.name == "CLAUDE.md"));
        assert!(apply(&project, &planned).is_err());
        assert!(!project.join(SHARED).exists());
        assert_eq!(
            std::fs::read_to_string(outside.join("secret")).unwrap(),
            "a key\n"
        );

        // What is planned now leaves the link alone, and a link put where
        // a file is written beside its place is not followed either.
        let planned = plan(&found);
        assert_eq!(planned.len(), 1);
        link(outside.join("secret"), project.join("AGENTS.md.partial")).unwrap();
        apply(&project, &planned).unwrap();
        assert!(!is_link(&project.join(SHARED)));
        assert_eq!(
            std::fs::read_to_string(outside.join("secret")).unwrap(),
            "a key\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn applying_writes_the_files() {
        let root = std::env::temp_dir().join(format!("ph-instructions-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "Use tabs.\n").unwrap();
        let written = apply(&root, &plan(&read(&root).unwrap())).unwrap();
        assert_eq!(written.len(), 3);
        assert_eq!(
            std::fs::read_to_string(root.join(SHARED)).unwrap(),
            "Use tabs.\n"
        );
        assert!(plan(&read(&root).unwrap()).is_empty(), "synced");
        // A file that is not UTF-8 is never taken for a missing one.
        std::fs::write(root.join("GEMINI.md"), b"caf\xe9\n").unwrap();
        assert!(read(&root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
