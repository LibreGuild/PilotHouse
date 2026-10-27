//! Finishing a review round from the agent's reply: reading its answer,
//! building the pull request's findings from the previous round's (see
//! [`crate::ledger`]), and saving them. Blocking (git); the reply comes
//! from the review chat's last turn.

use std::path::Path;

use anyhow::{Context as _, Result};
use ph_store::Store;
use ph_store::reviews::PrReview;

use crate::ledger::{self, Round};
use crate::linemap::LineMap;

/// What a finished round found, for the chat to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Newly found, fixed this round, and still open in all.
    pub new: usize,
    pub fixed: usize,
    pub open: usize,
    /// New findings whose quoted code was not found.
    pub unverified: usize,
}

/// Reads `reply` as round `review`'s answer and saves the findings it
/// leads to; `worktree` is the reviewed commit's checkout, and `agent` the
/// name of the agent that answered. `None` when the reply has no answer.
pub fn finish(
    store: &Store,
    review: &PrReview,
    reply: &str,
    worktree: &Path,
    agent: &str,
) -> Result<Option<Outcome>> {
    let Some(report) = crate::parse(reply) else {
        return Ok(None);
    };
    let before = store.latest_pr_review(&review.pull, Some(review.id))?;
    let previous = match &before {
        Some(earlier) => store.pr_findings(earlier.id)?,
        None => Vec::new(),
    };
    // How lines moved since the commit the last round read.
    let lines = before
        .as_ref()
        .filter(|earlier| earlier.head_sha != review.head_sha && !earlier.head_sha.is_empty())
        .and_then(|earlier| {
            ph_git::git(
                worktree,
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-U0",
                    "-M",
                    &earlier.head_sha,
                    &review.head_sha,
                ],
            )
            .ok()
        })
        .map(|diff| LineMap::parse(&diff));
    let read_file = |path: &str| -> Option<String> {
        // Only files inside the checkout.
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return None;
        }
        // A pull request's files as of the reviewed commit (the checkout
        // lacks the agent settings taken out of it); the user's own
        // changes as they are on disk.
        if review.kind != "self"
            && let Ok(text) = ph_git::git(
                worktree,
                &[
                    "show",
                    "--no-textconv",
                    &format!("{}:{path}", review.head_sha),
                ],
            )
        {
            return (text.len() < 4 << 20).then_some(text);
        }
        let bytes = std::fs::read(worktree.join(relative)).ok()?;
        (bytes.len() < 4 << 20).then(|| String::from_utf8_lossy(&bytes).into_owned())
    };
    let findings = ledger::build(Round {
        review_id: review.id,
        round: review.round,
        kind: &review.kind,
        agent,
        previous: &previous,
        report: &report,
        lines: lines.as_ref(),
        read_file: &read_file,
    });
    let verdict = crate::severity::verdict(&findings);
    let summary = if report.summary.trim().is_empty() {
        before.map(|earlier| earlier.summary).unwrap_or_default()
    } else {
        report.summary.trim().to_string()
    };
    store
        .finish_pr_review(review.id, &summary, verdict, &findings)
        .context("saving the review")?;
    let outcome = Outcome {
        new: findings
            .iter()
            .filter(|f| f.first_round == review.round && f.status == "open")
            .count(),
        fixed: findings
            .iter()
            .filter(|f| {
                f.status == "fixed"
                    && previous
                        .iter()
                        .any(|old| old.label == f.label && old.status != "fixed")
            })
            .count(),
        open: findings.iter().filter(|f| f.status == "open").count(),
        unverified: findings
            .iter()
            .filter(|f| f.first_round == review.round && !f.verified)
            .count(),
    };
    Ok(Some(outcome))
}
