//! Putting a review on GitHub, and reading back what people did there.
//!
//! - **Publish**: open findings not yet on the code become line comments
//!   (one GitHub review, neither approving nor requesting changes), each on
//!   the part of its lines the diff shows, with a one-click suggestion when
//!   the agent's replacement covers exactly those lines; then the pull
//!   request's one summary comment is written, or rewritten in place; then
//!   the threads of findings no longer open are resolved.
//! - **Read back**: ticks in the summary comment, and replies on findings'
//!   lines or naming them in the conversation (see [`crate::replies`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context as _, Result};
use ph_github::models::{DraftComment, IssueComment, PullFile, ReviewComment};
use ph_github::pulls::{self, ReviewEvent};
use ph_github::{Client, Repo};
use ph_store::reviews::Finding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostMode {
    /// The summary comment, and each open finding on its lines.
    Inline,
    /// The summary comment only.
    Conversation,
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

/// The marker naming the finding a line comment is about.
fn marker(label: &str) -> String {
    format!("<!-- ph:{label} -->")
}

/// The finding a line comment of ours is about.
pub fn finding_of(body: &str) -> Option<&str> {
    let start = body.find("<!-- ph:")? + "<!-- ph:".len();
    let rest = &body[start..];
    let end = rest.find("-->")?;
    let label = rest[..end].trim();
    (!label.contains(':')).then_some(label)
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A finding as a line comment: `exact` when the comment covers exactly
/// the quoted lines, so the replacement code can be a suggestion.
pub fn finding_markdown(finding: &Finding, exact: bool) -> String {
    let why = crate::severity::reasoning(finding);
    let mut text = format!(
        "**{} · {}: {}**{}",
        finding.label,
        capitalized(&finding.severity),
        finding.title.trim(),
        if why.is_empty() {
            String::new()
        } else {
            format!(" <sub>{why}</sub>")
        }
    );
    if !finding.body.trim().is_empty() {
        text.push_str(&format!("\n\n{}", finding.body.trim()));
    }
    let fix = crate::comment::fix_markdown(finding, if exact { "suggestion" } else { "" });
    if !fix.is_empty() {
        text.push_str(&format!("\n\n{fix}"));
    }
    text.push_str(
        "\n\n<sub>Reply starting with *Fixed*, *Not a problem* or *Won't fix* (and why).</sub>",
    );
    text.push_str(&format!("\n{}", marker(&finding.label)));
    text
}

/// Where a finding goes on the diff: the part of its lines inside a hunk
/// (first and last), and whether that is all of them.
fn place(finding: &Finding, lines: &HashMap<String, HashSet<u64>>) -> Option<(u64, u64, bool)> {
    let shown = lines.get(&finding.path)?;
    let start = finding.line?;
    let end = finding.end_line.unwrap_or(start).max(start);
    let inside: Vec<u64> = (start..=end).filter(|line| shown.contains(line)).collect();
    let (first, last) = (*inside.first()?, *inside.last()?);
    let exact = first == start && last == end && inside.len() as u64 == end - start + 1;
    Some((first, last, exact))
}

/// The pull request posts go to, and the commit they are about.
pub struct Target<'a> {
    pub client: &'a Client,
    pub repo: &'a Repo,
    pub number: u64,
    pub head: &'a str,
}

/// What publishing did.
#[derive(Debug, Default)]
pub struct Published {
    /// Findings (by label) now on the code, with their comment ids.
    pub on_code: Vec<(String, u64)>,
    /// The summary comment.
    pub summary_comment: u64,
    /// Findings (by label) whose threads were resolved.
    pub resolved: Vec<String>,
    /// Findings that could not go on the code (their lines are not in the
    /// diff); they are in full in the summary.
    pub off_code: Vec<String>,
}

/// Publishes the review: line comments for `findings` open and not yet on
/// the code (when `mode` is inline), the summary comment `summary` (a
/// rewrite of `existing` when given), and resolved threads for findings no
/// longer open.
pub fn publish(
    target: &Target,
    findings: &[Finding],
    files: &[PullFile],
    mode: PostMode,
    summary: &dyn Fn(&[Finding]) -> String,
    existing: Option<u64>,
) -> Result<Published> {
    let Target {
        client,
        repo,
        number,
        head,
    } = *target;
    let mut published = Published::default();
    let mut findings = findings.to_vec();

    if mode == PostMode::Inline {
        let lines = commentable_lines(files);
        let mut drafts: Vec<(String, DraftComment)> = Vec::new();
        for finding in findings.iter().filter(|f| {
            f.status == "open" && f.comment_kind.as_deref() != Some("line") && f.verified
        }) {
            match place(finding, &lines) {
                Some((first, last, exact)) => drafts.push((
                    finding.label.clone(),
                    DraftComment {
                        path: finding.path.clone(),
                        line: last,
                        side: "RIGHT".into(),
                        body: finding_markdown(finding, exact),
                        start_line: (last > first).then_some(first),
                        start_side: (last > first).then(|| "RIGHT".into()),
                    },
                )),
                None => published.off_code.push(finding.label.clone()),
            }
        }
        if !drafts.is_empty() {
            let comments: Vec<DraftComment> = drafts.iter().map(|(_, d)| d.clone()).collect();
            let created: Vec<ReviewComment> = match pulls::submit_review(
                client,
                repo,
                number,
                head,
                ReviewEvent::Comment,
                "",
                &comments,
            ) {
                Ok(review) => pulls::review_comments(client, repo, number, review.id)?,
                // One line GitHub refuses sinks the whole review: post them
                // one by one, keeping the ones it takes.
                Err(_) => drafts
                    .iter()
                    .filter_map(|(label, draft)| {
                        pulls::line_comment(client, repo, number, head, draft)
                            .map_err(|err| {
                                tracing::info!("finding {label} not posted on the code: {err:#}")
                            })
                            .ok()
                    })
                    .collect(),
            };
            for comment in &created {
                if let Some(label) = finding_of(&comment.body) {
                    published.on_code.push((label.to_string(), comment.id));
                }
            }
            for (label, _) in &drafts {
                if !published.on_code.iter().any(|(done, _)| done == label) {
                    published.off_code.push(label.clone());
                }
            }
            for finding in &mut findings {
                if let Some((_, id)) = published.on_code.iter().find(|(l, _)| *l == finding.label) {
                    finding.comment_id = Some(*id);
                    finding.comment_kind = Some("line".into());
                }
            }
        }
    }

    let body = summary(&findings);
    let rewritten = existing.and_then(|id| pulls::edit_comment(client, repo, id, &body).ok());
    let comment = match rewritten {
        Some(comment) => comment,
        None => pulls::comment(client, repo, number, &body)?,
    };
    published.summary_comment = comment.id;

    // Threads of findings dealt with are resolved (once).
    let done: Vec<&Finding> = findings
        .iter()
        .filter(|f| {
            f.status != "open" && f.comment_kind.as_deref() == Some("line") && !f.fixed_notified
        })
        .collect();
    if !done.is_empty() {
        let threads = pulls::threads(client, repo, number).unwrap_or_default();
        for finding in done {
            let Some(thread) = threads
                .iter()
                .find(|t| Some(t.first_comment) == finding.comment_id)
            else {
                continue;
            };
            if thread.resolved || pulls::resolve_thread(client, &thread.id).is_ok() {
                published.resolved.push(finding.label.clone());
            }
        }
    }
    Ok(published)
}

/// The summary comment, if it is still there.
pub fn read_summary(target: &Target, comment: u64) -> Option<IssueComment> {
    pulls::issue_comment(target.client, target.repo, comment).ok()
}

/// What people said about findings on GitHub since they were last read.
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    pub label: String,
    /// Their login.
    pub who: String,
    pub text: String,
    /// The claim it makes, if any.
    pub answer: Option<crate::replies::Answer>,
    /// The reply's id (to read only newer ones next time).
    pub id: u64,
    /// Where: `line` (in the finding's thread) or `conversation`.
    pub place: &'static str,
}

/// Replies on findings' line threads newer than each finding's
/// `last_reply_id`, and conversation comments naming a finding (`F3: …`)
/// newer than `conversation_after`, by anyone but `me`, oldest first. (Line
/// comments and conversation comments are numbered apart, so each has its
/// own mark.)
pub fn read_replies(
    findings: &[Finding],
    line_comments: &[ReviewComment],
    conversation: &[IssueComment],
    conversation_after: u64,
    me: &str,
) -> Vec<Said> {
    let mut said = Vec::new();
    for finding in findings {
        let after = finding.last_reply_id.unwrap_or(0);
        if let (Some("line"), Some(root)) = (finding.comment_kind.as_deref(), finding.comment_id) {
            for reply in line_comments
                .iter()
                .filter(|c| c.in_reply_to_id == Some(root) && c.id > after && c.user.login != me)
            {
                said.push(Said {
                    label: finding.label.clone(),
                    who: reply.user.login.clone(),
                    text: reply.body.clone(),
                    answer: crate::replies::classify(&reply.body),
                    id: reply.id,
                    place: "line",
                });
            }
        }
        for comment in conversation
            .iter()
            .filter(|c| c.id > conversation_after && c.user.login != me)
        {
            let Some((label, _)) = crate::replies::label_of(&comment.body) else {
                continue;
            };
            if label == finding.label {
                said.push(Said {
                    label: label.clone(),
                    who: comment.user.login.clone(),
                    text: comment.body.clone(),
                    answer: crate::replies::classify(&comment.body),
                    id: comment.id,
                    place: "conversation",
                });
            }
        }
    }
    said.sort_by_key(|s| s.id);
    said
}

/// Ticks in the summary comment `body` that differ from the findings'
/// statuses: label → status ticked.
pub fn new_ticks(body: &str, findings: &[Finding]) -> BTreeMap<String, String> {
    crate::comment::read_ticks(body)
        .into_iter()
        .filter(|(label, status)| {
            findings
                .iter()
                .any(|f| &f.label == label && &f.status != status)
        })
        .collect()
}

/// Posts a reply on finding `finding`: in its line thread when it has one,
/// else in the conversation, starting with its label.
pub fn reply_to(target: &Target, finding: &Finding, text: &str) -> Result<u64> {
    let Target {
        client,
        repo,
        number,
        ..
    } = *target;
    match (finding.comment_kind.as_deref(), finding.comment_id) {
        (Some("line"), Some(root)) => Ok(pulls::reply(client, repo, number, root, text)
            .context("replying on the finding's line")?
            .id),
        _ => Ok(pulls::comment(
            client,
            repo,
            number,
            &format!("**{}**: {}", finding.label, text.trim()),
        )?
        .id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_github::models::User;

    fn file(name: &str, patch: &str) -> PullFile {
        PullFile {
            filename: name.into(),
            patch: Some(patch.into()),
            ..PullFile::default()
        }
    }

    #[test]
    fn places_findings_on_the_lines_the_diff_shows() {
        let files = [file("a.rs", "@@ -1,2 +10,4 @@\n ctx\n+one\n+two\n ctx\n")];
        let lines = commentable_lines(&files);
        let finding = |line: u64, end: Option<u64>| Finding {
            path: "a.rs".into(),
            line: Some(line),
            end_line: end,
            ..Finding::default()
        };
        assert_eq!(place(&finding(11, Some(12)), &lines), Some((11, 12, true)));
        assert_eq!(place(&finding(12, Some(20)), &lines), Some((12, 13, false)));
        assert_eq!(place(&finding(30, None), &lines), None);
    }

    #[test]
    fn suggestions_only_for_exact_lines_and_markers_find_the_finding() {
        let finding = Finding {
            label: "F2".into(),
            severity: "major".into(),
            title: "Off by one".into(),
            suggested_code: "for i in 0..n {".into(),
            ..Finding::default()
        };
        let exact = finding_markdown(&finding, true);
        assert!(exact.contains("```suggestion\nfor i in 0..n {\n```"));
        assert!(!finding_markdown(&finding, false).contains("suggestion\n"));
        assert_eq!(finding_of(&exact), Some("F2"));
        assert_eq!(
            finding_of("<!-- ph:F2:fixed -->"),
            None,
            "a box, not a comment"
        );
    }

    #[test]
    fn reads_replies_by_others_after_the_last_one_seen() {
        let user = |login: &str| User {
            login: login.into(),
            ..User::default()
        };
        let finding = Finding {
            label: "F1".into(),
            comment_id: Some(10),
            comment_kind: Some("line".into()),
            last_reply_id: Some(11),
            ..Finding::default()
        };
        let lines = vec![
            ReviewComment {
                id: 11,
                in_reply_to_id: Some(10),
                user: user("dev"),
                body: "Fixed".into(),
                ..ReviewComment::default()
            },
            ReviewComment {
                id: 12,
                in_reply_to_id: Some(10),
                user: user("me"),
                body: "Thanks".into(),
                ..ReviewComment::default()
            },
            ReviewComment {
                id: 13,
                in_reply_to_id: Some(10),
                user: user("dev"),
                body: "Not a problem: validated upstream".into(),
                ..ReviewComment::default()
            },
        ];
        let conversation = vec![IssueComment {
            id: 14,
            user: user("dev"),
            body: "F1: won't fix, later".into(),
            ..IssueComment::default()
        }];
        let findings = [finding];
        let said = read_replies(&findings, &lines, &conversation, 0, "me");
        assert_eq!(
            read_replies(&findings, &lines, &conversation, 14, "me").len(),
            1,
            "the conversation has its own mark"
        );
        assert_eq!(said.len(), 2);
        assert_eq!(said[0].answer.as_ref().unwrap().status, "not_problem");
        assert_eq!(said[1].place, "conversation");
        assert_eq!(said[1].answer.as_ref().unwrap().status, "wont_fix");
    }
}
