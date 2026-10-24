//! Pull request reviews by a coding agent.
//!
//! 1. [`prompt`]: what the agent is asked, with the PR, its checked-out
//!    worktree and the previous run's findings to re-check.
//! 2. [`parse`]: the agent answers in a fixed template (a fenced JSON block)
//!    with a summary, a verdict and findings: severity, file, line, title,
//!    details, suggestion.
//! 3. [`merge`]: findings are matched to the previous run by key, so each is
//!    new, still open, fixed or dismissed, and keeps the GitHub comment it
//!    was posted as.
//! 4. [`post`]: the user picks findings and posts them as line comments in
//!    one review, or as one conversation comment; fixed ones can be marked on
//!    GitHub. Nothing is posted without the user asking.

pub mod merge;
pub mod parse;
pub mod post;
pub mod prompt;

pub use parse::{Report, ReportedFinding, parse};

/// The severities, most serious first.
pub const SEVERITIES: [&str; 4] = ["critical", "major", "minor", "nit"];

/// Ordering for display (critical first).
pub fn severity_rank(severity: &str) -> usize {
    SEVERITIES
        .iter()
        .position(|known| *known == severity)
        .unwrap_or(SEVERITIES.len())
}
