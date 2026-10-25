//! Posting chosen findings to GitHub, and telling GitHub what got fixed.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use ph_github::models::{DraftComment, PullFile};
use ph_github::pulls::{self, ReviewEvent};
use ph_github::{Client, Repo};
use ph_store::reviews::Finding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostMode {
    /// One review: a line comment per finding (those on lines the diff
    /// shows), the rest in the review's text.
    Inline,
    /// One comment on the conversation listing every finding.
    Conversation,
}

/// Where a finding ended up on GitHub.
#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    pub finding_id: i64,
    pub comment_id: u64,
    /// `line`, `review` (in a review's text) or `conversation`.
    pub kind: &'static str,
}

/// For each file, the lines of the new version the diff shows: GitHub only
/// accepts line comments there.
pub fn commentable_lines(files: &[PullFile]) -> HashMap<String, HashSet<u64>> {
    let mut lines = HashMap::new();
    for file in files {
        let Some(patch) = &file.patch else { continue };
        let set: &mut HashSet<u64> = lines.entry(file.filename.clone()).or_default();
        let mut line = 0u64;
        for text in patch.lines() {
            if let Some(header) = text.strip_prefix("@@") {
                // `@@ -a,b +c,d @@`: the new side starts at c.
                line = header
                    .split_whitespace()
                    .find_map(|part| part.strip_prefix('+'))
                    .and_then(|range| range.split(',').next())
                    .and_then(|start| start.parse().ok())
                    .unwrap_or(0);
                continue;
            }
            match text.chars().next() {
                Some('-') => {}
                Some('\\') => {}
                _ => {
                    set.insert(line);
                    line += 1;
                }
            }
        }
    }
    lines
}

const ICONS: [(&str, &str); 4] = [
    ("critical", "🔴"),
    ("major", "🟠"),
    ("minor", "🟡"),
    ("nit", "⚪"),
];

fn icon(severity: &str) -> &'static str {
    ICONS
        .iter()
        .find(|(name, _)| *name == severity)
        .map(|(_, icon)| *icon)
        .unwrap_or("⚪")
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A finding as a GitHub comment.
pub fn finding_markdown(finding: &Finding) -> String {
    let mut text = format!(
        "{} **{}: {}**",
        icon(&finding.severity),
        capitalized(&finding.severity),
        finding.title
    );
    if !finding.body.trim().is_empty() {
        text.push_str(&format!("\n\n**Issue:** {}", finding.body.trim()));
    }
    if !finding.evidence.trim().is_empty() {
        text.push_str(&format!("\n\n**Evidence:** {}", finding.evidence.trim()));
    }
    if !finding.suggestion.trim().is_empty() {
        text.push_str(&format!("\n\n**Fix:** {}", finding.suggestion.trim()));
    }
    text
}

/// A finding as one item of a list (with its place in the code).
fn finding_item(finding: &Finding) -> String {
    let place = match finding.line {
        Some(line) => format!("`{}:{line}`", finding.path),
        None if !finding.path.is_empty() => format!("`{}`", finding.path),
        None => String::new(),
    };
    let mut item = format!(
        "- {} **{}** {place}",
        icon(&finding.severity),
        finding.title
    );
    for (label, text) in [
        ("Issue", &finding.body),
        ("Evidence", &finding.evidence),
        ("Fix", &finding.suggestion),
    ] {
        if !text.trim().is_empty() {
            item.push_str(&format!(
                "\n  *{label}:* {}",
                text.trim().replace('\n', "\n  ")
            ));
        }
    }
    item
}

/// The pull request posts go to, and the commit they are about.
pub struct Target<'a> {
    pub client: &'a Client,
    pub repo: &'a Repo,
    pub number: u64,
    pub head: &'a str,
}

/// Posts `findings` to the target pull request, as the review of its head.
pub fn post(
    target: &Target,
    summary: &str,
    findings: &[Finding],
    files: &[PullFile],
    mode: PostMode,
) -> Result<Vec<Posted>> {
    let Target {
        client,
        repo,
        number,
        head,
    } = *target;
    let header = if summary.trim().is_empty() {
        "Review".to_string()
    } else {
        summary.trim().to_string()
    };
    match mode {
        PostMode::Conversation => {
            let body = format!(
                "{header}\n\n{}",
                findings
                    .iter()
                    .map(finding_item)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            let comment = pulls::comment(client, repo, number, &body)?;
            Ok(findings
                .iter()
                .map(|finding| Posted {
                    finding_id: finding.id,
                    comment_id: comment.id,
                    kind: "conversation",
                })
                .collect())
        }
        PostMode::Inline => {
            let lines = commentable_lines(files);
            let (inline, rest): (Vec<&Finding>, Vec<&Finding>) =
                findings.iter().partition(|finding| {
                    finding.line.is_some_and(|line| {
                        lines
                            .get(&finding.path)
                            .is_some_and(|set| set.contains(&line))
                    })
                });
            let comments: Vec<DraftComment> = inline
                .iter()
                .map(|finding| DraftComment {
                    path: finding.path.clone(),
                    line: finding.line.unwrap_or(1),
                    side: "RIGHT".into(),
                    body: finding_markdown(finding),
                })
                .collect();
            let mut body = header;
            if !rest.is_empty() {
                body.push_str("\n\nNot on a changed line:\n\n");
                body.push_str(
                    &rest
                        .iter()
                        .map(|f| finding_item(f))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            let review = pulls::submit_review(
                client,
                repo,
                number,
                head,
                ReviewEvent::Comment,
                &body,
                &comments,
            )?;
            // Match each line comment back to its finding.
            let created =
                pulls::review_comments(client, repo, number, review.id).unwrap_or_default();
            let mut posted = Vec::new();
            for finding in &inline {
                let body = finding_markdown(finding);
                let comment = created
                    .iter()
                    .find(|c| c.path == finding.path && c.body.trim() == body.trim());
                posted.push(match comment {
                    Some(comment) => Posted {
                        finding_id: finding.id,
                        comment_id: comment.id,
                        kind: "line",
                    },
                    None => Posted {
                        finding_id: finding.id,
                        comment_id: review.id,
                        kind: "review",
                    },
                });
            }
            posted.extend(rest.iter().map(|finding| Posted {
                finding_id: finding.id,
                comment_id: review.id,
                kind: "review",
            }));
            Ok(posted)
        }
    }
}

/// Tells GitHub which posted findings are now fixed: a reply on each line
/// comment thread, and one conversation comment for the rest. Returns the
/// findings it announced.
pub fn announce_fixed(target: &Target, fixed: &[Finding]) -> Result<Vec<i64>> {
    let Target {
        client,
        repo,
        number,
        head,
    } = *target;
    let short = &head[..head.len().min(8)];
    let mut done = Vec::new();
    let mut listed = Vec::new();
    for finding in fixed {
        match (finding.comment_kind.as_deref(), finding.comment_id) {
            (Some("line"), Some(comment)) => {
                pulls::reply(
                    client,
                    repo,
                    number,
                    comment,
                    &format!("✅ Fixed as of `{short}`."),
                )?;
                done.push(finding.id);
            }
            (Some(_), Some(_)) => listed.push(finding),
            _ => {}
        }
    }
    if !listed.is_empty() {
        let body = format!(
            "✅ Fixed as of `{short}`:\n\n{}",
            listed
                .iter()
                .map(|f| format!("- {}", f.title))
                .collect::<Vec<_>>()
                .join("\n")
        );
        pulls::comment(client, repo, number, &body)?;
        done.extend(listed.iter().map(|f| f.id));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, patch: &str) -> PullFile {
        PullFile {
            filename: name.into(),
            patch: Some(patch.into()),
            ..PullFile::default()
        }
    }

    #[test]
    fn finds_commentable_lines() {
        let files = [file(
            "a.rs",
            "@@ -1,3 +1,4 @@\n keep\n-old\n+new\n+more\n keep\n@@ -20,2 +21,2 @@ fn x()\n ctx\n+add\n",
        )];
        let lines = &commentable_lines(&files)["a.rs"];
        let mut sorted: Vec<_> = lines.iter().copied().collect();
        sorted.sort();
        assert_eq!(sorted, [1, 2, 3, 4, 21, 22]);
    }

    #[test]
    fn writes_findings_as_markdown() {
        let finding = Finding {
            id: 1,
            review_id: 1,
            key: "k".into(),
            severity: "major".into(),
            path: "src/a.rs".into(),
            line: Some(4),
            end_line: None,
            title: "Unbounded retry".into(),
            body: "Spins forever.".into(),
            evidence: "`loop {}` at line 4 never breaks.".into(),
            suggestion: "Cap attempts.".into(),
            status: "new".into(),
            comment_id: None,
            comment_kind: None,
            fixed_notified: false,
        };
        let text = finding_markdown(&finding);
        assert!(text.starts_with("🟠 **Major: Unbounded retry**"));
        assert!(text.contains("**Fix:** Cap attempts."));
        assert!(text.contains("**Evidence:** `loop {}`"));
        assert!(finding_item(&finding).contains("`src/a.rs:4`"));
    }
}
