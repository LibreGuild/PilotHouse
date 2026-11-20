//! A shared explanation: one Markdown file under [`FOLDER`](super::FOLDER)
//! in the repository, which the user adds to a commit when they want the
//! team to have it. Nothing is ever written there without being asked.
//!
//! The file is the explanation as it reads, so it is as readable on the
//! repository's web page as in the app. What it explains stands in a comment
//! at its end ([`NOTE`]), which a Markdown viewer does not show: who had it
//! written and when, and each file's path with the blob id of the content
//! explained. One file per explanation, each under a name of its own, so two
//! people's never conflict.

use std::path::Path;

use anyhow::{Context as _, Result};

use super::FOLDER;

/// The first line of the comment that says what a shared explanation is of.
const NOTE: &str = "<!-- pilothouse-explanation-of";

/// An explanation as a file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shared {
    pub markdown: String,
    /// When it was written, in Unix seconds.
    pub written: i64,
    /// Who had it written (their Git name).
    pub author: String,
    /// The agent that wrote it.
    pub agent: String,
    /// The files it explains, each with the blob id of its content.
    pub files: Vec<(String, String)>,
}

/// The file's text: the explanation, then the note of what it explains.
pub fn text(shared: &Shared) -> String {
    let mut text = format!("{}\n\n{NOTE}\n", shared.markdown.trim());
    text.push_str(&format!("written: {}\n", shared.written));
    for (key, value) in [("author", &shared.author), ("agent", &shared.agent)] {
        let value = one_line(value);
        if !value.is_empty() {
            text.push_str(&format!("{key}: {value}\n"));
        }
    }
    for (path, blob) in &shared.files {
        text.push_str(&format!("file: {blob} {path}\n"));
    }
    text.push_str("-->\n");
    text
}

/// Reads a file's text back; `None` when it carries no note (a Markdown
/// file someone put in the folder by hand).
pub fn read(text: &str) -> Option<Shared> {
    let at = text.rfind(NOTE)?;
    let mut shared = Shared {
        markdown: text[..at].trim().to_string(),
        ..Shared::default()
    };
    for line in text[at + NOTE.len()..].lines() {
        let line = line.trim();
        if line.starts_with("-->") {
            break;
        }
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        match key {
            "written" => shared.written = value.trim().parse().unwrap_or(0),
            "author" => shared.author = value.trim().to_string(),
            "agent" => shared.agent = value.trim().to_string(),
            "file" => {
                if let Some((blob, path)) = value.split_once(' ') {
                    shared.files.push((path.to_string(), blob.to_string()));
                }
            }
            // A key a later version writes.
            _ => {}
        }
    }
    (!shared.markdown.is_empty()).then_some(shared)
}

/// The file name for an explanation titled `title`, written at `written`
/// (Unix seconds): its day and the title in plain letters.
pub fn name(title: &str, written: i64) -> String {
    let day = jiff::Timestamp::from_second(written)
        .map(|at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| "0000-00-00".to_string());
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 60 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("{day}-explanation.md")
    } else {
        format!("{day}-{slug}.md")
    }
}

/// Writes `shared` as a new file in `root`'s folder of explanations and
/// returns its path from `root`. A name taken gets a number.
pub fn write(root: &Path, shared: &Shared) -> Result<String> {
    let folder = root.join(FOLDER);
    std::fs::create_dir_all(&folder).with_context(|| format!("making {}", folder.display()))?;
    let name = name(&super::title_of(&shared.markdown), shared.written);
    let stem = name.trim_end_matches(".md");
    let mut file = name.clone();
    let mut n = 2;
    while folder.join(&file).exists() {
        file = format!("{stem}-{n}.md");
        n += 1;
    }
    std::fs::write(folder.join(&file), text(shared)).with_context(|| format!("writing {file}"))?;
    Ok(format!("{FOLDER}/{file}"))
}

/// Every shared explanation in `root`'s working tree, with its path from
/// `root`. Files without a note, or that cannot be read, are passed over.
pub fn all(root: &Path) -> Vec<(String, Shared)> {
    let Ok(entries) = std::fs::read_dir(root.join(FOLDER)) else {
        return Vec::new();
    };
    let mut found: Vec<(String, Shared)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .filter_map(|entry| {
            let shared = read(&std::fs::read_to_string(entry.path()).ok()?)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            Some((format!("{FOLDER}/{name}"), shared))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// `text` on one line (a name is never more).
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Shared {
            markdown: "# Forgot-password emails\n\n## In short\n\nA reset link by email.\n\n\
                       ```mermaid\nflowchart TD\n  A[\"Sign in\"] --> B[\"Email\"]\n```"
                .into(),
            written: 1_763_640_000,
            author: "Jane  Doe".into(),
            agent: "Claude Code".into(),
            files: vec![
                ("src/auth/reset.rs".into(), "a".repeat(40)),
                ("docs/my notes.md".into(), "b".repeat(40)),
            ],
        }
    }

    #[test]
    fn a_file_reads_back_as_it_was_written() {
        let text = text(&shared());
        // The explanation is the file; the note is a comment at its end.
        assert!(text.starts_with("# Forgot-password emails\n"));
        assert!(text.ends_with("-->\n"));
        let read = read(&text).unwrap();
        assert_eq!(read.author, "Jane Doe");
        assert_eq!(
            Shared {
                author: "Jane  Doe".into(),
                ..read
            },
            shared()
        );
        // A diagram's arrows end no comment, and a file by hand has no note.
        assert!(super::read("# Notes\n\nA --> B").is_none());
    }

    #[test]
    fn a_name_is_the_day_and_the_title() {
        let name = name("Forgot-password emails: the “link”!", 1_763_640_000);
        assert!(
            name.ends_with("-forgot-password-emails-the-link.md"),
            "{name}"
        );
        assert_eq!(
            name.len(),
            "2025-11-20-".len() + "forgot-password-emails-the-link.md".len()
        );
        assert!(super::name("…", 1_763_640_000).ends_with("-explanation.md"));
        assert!(super::name(&"long word ".repeat(20), 0).len() <= 11 + 60 + 3);
    }

    #[test]
    fn files_are_written_under_names_of_their_own() {
        let root =
            std::env::temp_dir().join(format!("ph-explanation-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let first = write(&root, &shared()).unwrap();
        let second = write(&root, &shared()).unwrap();
        assert!(first.starts_with(".pilothouse/explanations/"));
        assert_ne!(first, second);
        assert!(second.ends_with("-2.md"));
        // A file someone wrote by hand is not one of them.
        std::fs::write(root.join(FOLDER).join("README.md"), "# About this folder").unwrap();
        let all = all(&root);
        let mut written = [first, second];
        written.sort();
        assert_eq!(
            all.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>(),
            written
        );
        assert_eq!(all[0].1.files.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
