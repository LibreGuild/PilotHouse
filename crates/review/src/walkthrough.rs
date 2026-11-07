//! A pull request explained for whoever reviews it: a tech lead or a
//! manager who has to understand the change, often without knowing this
//! part of the code. An agent writes a briefing in Markdown, in sections
//! that go from plain words to detail ([`SECTIONS`]): the change in short,
//! why, how the system works today, what changes (with diagrams), how it is
//! done, what it affects, where to look closely, how it is tested, questions
//! for the author, and the terms used.
//!
//! The reply carries it between two marker lines ([`START`], [`END`]), so
//! a diagram's or a code block's fences inside it stay as they are.
//! References to code (`` `path:12-20` ``) become links when it is shown
//! ([`Walkthrough::linked`]): a changed file opens in the Files tab, any
//! other file on the web at the commit the briefing read.

use serde::{Deserialize, Serialize};

/// The line before the briefing in the agent's reply.
pub const START: &str = "<!-- pilothouse-walkthrough -->";
/// The line after it.
pub const END: &str = "<!-- /pilothouse-walkthrough -->";

/// The briefing's sections, in order, with what each says (the prompt asks
/// for them; one with nothing to say may be left out, but the first and
/// What changes always come).
pub const SECTIONS: &[(&str, &str)] = &[
    (
        "In short",
        "two or three sentences anyone on the team understands: what changes, for users or \
         for the system, and why it matters",
    ),
    (
        "Why",
        "the problem or goal, and where that comes from: the description, the commits, an \
         issue it names, or your reading of the code (then say it is a guess)",
    ),
    (
        "How it works today",
        "the part of the system this touches, before this change: the pieces involved, what \
         each does, how a request or data flows through them, for someone new to this code; \
         with a Mermaid diagram when more than two pieces are involved",
    ),
    (
        "What changes",
        "the change step by step in plain words: what is new, what is different, what is \
         removed; with a Mermaid diagram of the new flow when it differs from today's (mark \
         the new parts, e.g. \"(new)\")",
    ),
    (
        "How it is done",
        "the main implementation choices, by area rather than file by file: what was done, \
         where (code references), and why that way when the code or the description says",
    ),
    (
        "What it affects",
        "only what applies: behaviour users see, APIs and contracts, data and migrations, \
         configuration and feature flags, permissions and security, performance, \
         compatibility, rollout and rollback",
    ),
    (
        "Where to look closely",
        "the three to six places that most deserve a reviewer's attention (tricky logic, edge \
         cases, concurrency, error handling, security-sensitive code), each with a code \
         reference and one sentence on what to check; then what is safe to skim (renames, \
         moved or generated code, formatting)",
    ),
    (
        "How it is tested",
        "tests added or changed and what they cover, what is not covered, and how to try it \
         by hand",
    ),
    (
        "Questions for the author",
        "points the change leaves unclear that are worth asking",
    ),
    (
        "Terms",
        "short definitions of the domain words and abbreviations used above",
    ),
];

/// A briefing, as stored with its run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Walkthrough {
    /// The briefing, Markdown, as the agent wrote it.
    pub markdown: String,
}

/// The briefing in `reply`: what is between the last [`START`] and the
/// [`END`] after it (or the end, when the agent left that line out).
pub fn parse(reply: &str) -> Option<Walkthrough> {
    let start = reply.rfind(START)? + START.len();
    let rest = &reply[start..];
    let body = match rest.find(END) {
        Some(end) => &rest[..end],
        None => rest,
    };
    let markdown = body.trim();
    (!markdown.is_empty()).then(|| Walkthrough {
        markdown: markdown.to_string(),
    })
}

impl Walkthrough {
    /// The briefing as stored ([`Self::to_data`]); `None` for data an older
    /// version wrote (in another shape), which is asked for again.
    pub fn from_data(data: &str) -> Option<Self> {
        serde_json::from_str::<Self>(data)
            .ok()
            .filter(|walkthrough| !walkthrough.markdown.trim().is_empty())
    }

    pub fn to_data(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// One line for lists and notices: the title (`# …`), else the start of
    /// *In short*, else the first line.
    pub fn summary(&self) -> String {
        let lines: Vec<&str> = self.markdown.lines().map(str::trim).collect();
        if let Some(title) = lines
            .iter()
            .find_map(|line| line.strip_prefix("# "))
            .filter(|title| !title.trim().is_empty())
        {
            return title.trim().to_string();
        }
        let short = lines
            .iter()
            .position(|line| {
                line.trim_start_matches('#')
                    .trim()
                    .eq_ignore_ascii_case("in short")
            })
            .and_then(|at| lines[at + 1..].iter().find(|line| !line.is_empty()));
        short
            .or_else(|| lines.iter().find(|line| !line.is_empty()))
            .map(|line| line.trim_start_matches('#').trim().to_string())
            .unwrap_or_default()
    }

    /// The briefing with its code references made links: one to a file in
    /// `changed` opens it in the Files tab ([`file_link`]); one to any other
    /// file opens `<web>/blob/<sha>/<path>#L<line>` (`web`: the repository's
    /// page). Code blocks and diagrams are left as they are.
    pub fn linked(&self, changed: &[String], web: Option<&str>, sha: &str) -> String {
        let mut out = String::with_capacity(self.markdown.len() + 256);
        let mut fence: Option<&str> = None;
        for line in self.markdown.split_inclusive('\n') {
            let trimmed = line.trim_start();
            match fence {
                Some(open) => {
                    if trimmed.trim_end() == open {
                        fence = None;
                    }
                    out.push_str(line);
                    continue;
                }
                None if trimmed.starts_with("```") || trimmed.starts_with("~~~") => {
                    let marks = trimmed
                        .chars()
                        .take_while(|c| *c == '`' || *c == '~')
                        .count();
                    fence = Some(&trimmed[..marks]);
                    out.push_str(line);
                    continue;
                }
                None => {}
            }
            out.push_str(&link_spans(line, changed, web, sha));
        }
        out
    }
}

/// The scheme of a link to a changed file, opened in the Files tab.
const FILE_SCHEME: &str = "pilothouse-file:";

/// A link to `path` (at `line`) in the Files tab.
pub fn file_link(path: &str, line: Option<u64>) -> String {
    match line {
        Some(line) => format!("{FILE_SCHEME}{path}#{line}"),
        None => format!("{FILE_SCHEME}{path}"),
    }
}

/// The file and line a link made by [`file_link`] names.
pub fn parse_file_link(url: &str) -> Option<(String, Option<u64>)> {
    let rest = url.strip_prefix(FILE_SCHEME)?;
    let (path, line) = match rest.rsplit_once('#') {
        Some((path, line)) => (path, line.parse().ok()),
        None => (rest, None),
    };
    (!path.is_empty()).then(|| (path.to_string(), line))
}

/// `line` with each inline code span that names a file made a link.
fn link_spans(line: &str, changed: &[String], web: Option<&str>, sha: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            break;
        };
        let code = &after[..close];
        // Already a link's text (`[`…`](…)`): left alone.
        let in_link = rest[..open].ends_with('[') && after[close + 1..].starts_with("](");
        out.push_str(&rest[..open]);
        match (!in_link).then(|| reference(code)).flatten() {
            Some((path, start, end)) => {
                let target = if changed.iter().any(|changed| changed == path) {
                    Some(file_link(path, start))
                } else {
                    web.filter(|_| !sha.is_empty()).map(|web| {
                        let lines = match (start, end) {
                            (Some(start), Some(end)) if end > start => {
                                format!("#L{start}-L{end}")
                            }
                            (Some(start), _) => format!("#L{start}"),
                            _ => String::new(),
                        };
                        format!("{}/blob/{sha}/{path}{lines}", web.trim_end_matches('/'))
                    })
                };
                match target {
                    Some(target) => out.push_str(&format!("[`{code}`]({target})")),
                    None => out.push_str(&format!("`{code}`")),
                }
            }
            None => out.push_str(&format!("`{code}`")),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The file (and lines) an inline code span names: `src/a.rs`,
/// `src/a.rs:12`, `src/a.rs:12-20`, `src/a.rs#L12`. A path has a `/` or an
/// extension and no spaces; anything else is just code.
fn reference(code: &str) -> Option<(&str, Option<u64>, Option<u64>)> {
    let code = code.trim().trim_start_matches("./");
    let (path, lines) = match code.rsplit_once(':') {
        Some((path, lines))
            if !lines.is_empty() && lines.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            (path, Some(lines))
        }
        _ => match code.rsplit_once("#L") {
            Some((path, lines)) => (path, Some(lines)),
            None => (code, None),
        },
    };
    let file = path.rsplit('/').next().unwrap_or(path);
    let looks_like_path = !path.is_empty()
        && !path.contains(char::is_whitespace)
        && !path.contains("://")
        && !path.starts_with('-')
        && (path.contains('/') || file.contains('.'))
        && !file.ends_with('.')
        && path
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '@' | '+'));
    if !looks_like_path {
        return None;
    }
    let (start, end) = match lines {
        Some(lines) => {
            let mut parts = lines.split(['-', 'L']).filter(|part| !part.is_empty());
            let start = parts.next().and_then(|n| n.parse().ok());
            let end = parts.next().and_then(|n| n.parse().ok());
            (start, end)
        }
        None => (None, None),
    };
    Some((path, start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "I read the change and the session code around it.\n\n\
<!-- pilothouse-walkthrough -->\n\
# Users stay signed in across restarts\n\n\
## In short\n\nSessions are kept on disk, so a restart no longer signs everyone out.\n\n\
## What changes\n\n`SessionStore` now writes to `src/session/store.rs:40-58`, read by `src/app.rs:12`.\n\n\
```mermaid\nflowchart LR\n  A[`src/app.rs`] --> B\n```\n\n\
Run `cargo test` and see `docs/sessions.md` and [`src/app.rs`](https://example.com).\n\
<!-- /pilothouse-walkthrough -->\n\nAnything else?";

    #[test]
    fn reads_the_briefing_between_its_markers() {
        let walkthrough = parse(REPLY).unwrap();
        assert!(walkthrough.markdown.starts_with("# Users stay signed in"));
        assert!(walkthrough.markdown.ends_with("(https://example.com)."));
        assert_eq!(
            walkthrough.summary(),
            "Users stay signed in across restarts"
        );
        // Without the closing line, to the end; without the opening one, none.
        let open = parse(&format!("{START}\n## In short\n\nIt caches.")).unwrap();
        assert_eq!(open.summary(), "It caches.");
        assert_eq!(parse("no briefing here"), None);
        assert_eq!(parse(&format!("{START}\n  \n{END}")), None);
        // Stored and read back; an older version's data is not a briefing.
        assert_eq!(
            Walkthrough::from_data(&walkthrough.to_data()),
            Some(walkthrough)
        );
        assert_eq!(
            Walkthrough::from_data(r#"{"gist":{"what":"x"},"groups":[],"tour":[]}"#),
            None
        );
    }

    #[test]
    fn code_references_become_links() {
        let walkthrough = parse(REPLY).unwrap();
        let changed = vec!["src/session/store.rs".to_string()];
        let linked = walkthrough.linked(&changed, Some("https://github.com/acme/app"), "abc123");
        // A changed file opens in the Files tab, at its first line.
        assert!(
            linked.contains(
                "[`src/session/store.rs:40-58`](pilothouse-file:src/session/store.rs#40)"
            )
        );
        // Another file opens on the web at the commit read.
        assert!(
            linked.contains(
                "[`src/app.rs:12`](https://github.com/acme/app/blob/abc123/src/app.rs#L12)"
            )
        );
        assert!(linked.contains(
            "[`docs/sessions.md`](https://github.com/acme/app/blob/abc123/docs/sessions.md)"
        ));
        // Code that is not a path, a diagram, and an existing link stay.
        assert!(linked.contains("Run `cargo test`"));
        assert!(linked.contains("`SessionStore`"));
        assert!(linked.contains("A[`src/app.rs`] --> B"));
        assert!(linked.contains("[`src/app.rs`](https://example.com)"));
        // Without the repository's page, other files stay code.
        let local = walkthrough.linked(&changed, None, "abc123");
        assert!(local.contains("read by `src/app.rs:12`."));
        assert_eq!(
            parse_file_link("pilothouse-file:src/session/store.rs#40"),
            Some(("src/session/store.rs".to_string(), Some(40)))
        );
        assert_eq!(
            parse_file_link("pilothouse-file:Cargo.toml"),
            Some(("Cargo.toml".to_string(), None))
        );
        assert_eq!(parse_file_link("https://example.com"), None);
    }

    #[test]
    fn tells_paths_from_other_code() {
        assert_eq!(reference("src/a.rs"), Some(("src/a.rs", None, None)));
        assert_eq!(reference("src/a.rs:7"), Some(("src/a.rs", Some(7), None)));
        assert_eq!(
            reference("./lib/b.ts:3-9"),
            Some(("lib/b.ts", Some(3), Some(9)))
        );
        assert_eq!(
            reference("web/c.go#L4-L8"),
            Some(("web/c.go", Some(4), Some(8)))
        );
        assert_eq!(reference("Cargo.toml"), Some(("Cargo.toml", None, None)));
        for code in [
            "cargo test",
            "fn main()",
            "SessionStore",
            "https://x.io/a.rs",
            "a.b()",
            "--flag",
            "x::y",
        ] {
            assert_eq!(reference(code), None, "{code}");
        }
    }
}
