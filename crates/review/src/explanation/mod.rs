//! A change explained in plain words, for the person about to commit it and
//! for whoever meets the code later.
//!
//! Agents write the code and agents review it; what gets lost is the
//! developer's own picture of what happened. An explanation gives it back:
//! how this part worked before, what changed (in the order that is easiest
//! to follow), why it was done this way (from the chats that made the
//! change, which the code cannot say), what was checked, and what is worth a
//! word with the team.
//!
//! - [`prompt`]: what the agent is asked, and how it is told to write.
//! - [`parse`]: its reply carries the explanation between two marker lines
//!   ([`START`], [`END`]).
//! - [`file`]: an explanation the user chose to share is one Markdown file
//!   under [`FOLDER`] in the repository, committed like any other.
//! - [`find`]: an explanation knows the content it explains (each file's
//!   path and Git blob id), so the one for a commit or a line is found by
//!   what that commit put in the repository: no commit id is kept, and a
//!   rebase, a cherry-pick or a squash changes nothing.

pub mod file;
pub mod find;
pub mod prompt;

/// The line before the explanation in the agent's reply.
pub const START: &str = "<!-- pilothouse-explanation -->";
/// The line after it.
pub const END: &str = "<!-- /pilothouse-explanation -->";

/// Where shared explanations live in a repository, from its root.
pub const FOLDER: &str = ".pilothouse/explanations";

/// The blob id Git gives a file that is not there (a deleted one).
pub const GONE: &str = "0000000000000000000000000000000000000000";

/// Whether `path` (from the repository's root) is a shared explanation:
/// those are not part of the work an explanation is about.
pub fn is_explanation_file(path: &str) -> bool {
    path.strip_prefix(FOLDER)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// An explanation, as the agent wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Explanation {
    /// Markdown.
    pub markdown: String,
}

/// The explanation in `reply`: what is between the last [`START`] line and
/// the [`END`] line after it (or the end, when the agent left that line
/// out). The marker lines are read loosely, since agents do not all copy
/// them to the letter (`<!-- /pilothouse-explanation-->`).
pub fn parse(reply: &str) -> Option<Explanation> {
    let is = |line: &str, marker: &str| {
        let squeeze = |text: &str| text.split_whitespace().collect::<String>();
        squeeze(line).eq_ignore_ascii_case(&squeeze(marker))
    };
    let lines: Vec<&str> = reply.lines().collect();
    let start = lines.iter().rposition(|line| is(line, START))? + 1;
    let end = lines[start..]
        .iter()
        .position(|line| is(line, END))
        .map_or(lines.len(), |at| start + at);
    let markdown = lines[start..end].join("\n");
    let markdown = markdown.trim();
    (!markdown.is_empty()).then(|| Explanation {
        markdown: markdown.to_string(),
    })
}

impl Explanation {
    /// Its title: the `# ` line, else the first line of *In short*.
    pub fn title(&self) -> String {
        title_of(&self.markdown)
    }

    /// *In short*, as one paragraph: what a pop-up shows.
    pub fn short(&self) -> String {
        short_of(&self.markdown)
    }
}

/// The title of explanation `markdown` (see [`Explanation::title`]).
pub fn title_of(markdown: &str) -> String {
    let title = markdown
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(str::trim)
        .filter(|title| !title.is_empty());
    match title {
        Some(title) => title.to_string(),
        None => {
            let short = short_of(markdown);
            let first = short.split(". ").next().unwrap_or_default();
            first.chars().take(80).collect()
        }
    }
}

/// The text under the *In short* heading of `markdown`, up to the next
/// heading; without that heading, the first paragraph that is not one.
pub fn short_of(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().map(str::trim).collect();
    let is_heading = |line: &str| line.starts_with('#');
    let under_heading = lines.iter().position(|line| {
        is_heading(line)
            && line
                .trim_start_matches('#')
                .trim()
                .eq_ignore_ascii_case("in short")
    });
    let from = under_heading
        .map(|at| at + 1)
        .or_else(|| {
            lines
                .iter()
                .position(|line| !line.is_empty() && !is_heading(line))
        })
        .unwrap_or(lines.len());
    lines[from..]
        .iter()
        .skip_while(|line| line.is_empty())
        // Under its heading, all of it; without one, the first paragraph.
        .take_while(|line| !is_heading(line) && (under_heading.is_some() || !line.is_empty()))
        .filter(|line| !line.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "I read the change.\n\n\
<!-- pilothouse-explanation -->\n\
# Forgot-password emails\n\n\
## In short\n\nPeople who forget their password can get a reset link by email.\n\
The link works once.\n\n\
## What changed\n\nThe sign-in page has a link now (`web/signin.tsx:40`).\n\
<!-- /pilothouse-explanation -->\n\nThe explanation is ready.";

    #[test]
    fn reads_the_explanation_between_its_markers() {
        let explanation = parse(REPLY).unwrap();
        assert!(explanation.markdown.starts_with("# Forgot-password emails"));
        assert!(explanation.markdown.ends_with("(`web/signin.tsx:40`)."));
        assert_eq!(explanation.title(), "Forgot-password emails");
        assert_eq!(
            explanation.short(),
            "People who forget their password can get a reset link by email. The link works once."
        );
        // Without the closing line, to the end; without the opening one, none.
        assert!(parse(&format!("{START}\n## In short\n\nIt caches.")).is_some());
        assert_eq!(parse("no explanation here"), None);
        assert_eq!(parse(&format!("{START}\n  \n{END}")), None);
        // Markers not copied to the letter still mark.
        let loose =
            parse("<!--pilothouse-explanation -->\n# A\n<!-- /pilothouse-explanation-->\nDone.");
        assert_eq!(loose.unwrap().markdown, "# A");
        // A marker named inside a sentence is not a marker line.
        assert_eq!(
            parse(&format!("I will write it between `{START}` and `{END}`.")),
            None
        );
    }

    #[test]
    fn a_title_is_found_without_its_line() {
        let markdown = "## In short\n\nSessions are kept on disk. A restart signs nobody out.";
        assert_eq!(title_of(markdown), "Sessions are kept on disk");
        assert_eq!(
            short_of("Just a paragraph.\n\nAnd more."),
            "Just a paragraph."
        );
    }

    #[test]
    fn knows_its_own_files() {
        assert!(is_explanation_file(
            ".pilothouse/explanations/2026-11-20-reset.md"
        ));
        assert!(!is_explanation_file(".pilothouse/review.md"));
        assert!(!is_explanation_file(".pilothouse/explanations-old/a.md"));
    }
}
