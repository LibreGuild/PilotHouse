//! The parts of GitHub's REST replies PilotHouse uses. Every field has a
//! default, so API additions and Enterprise differences do not break it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Label {
    pub name: String,
    /// Hex without `#`.
    pub color: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Branch {
    #[serde(rename = "ref")]
    pub name: String,
    pub sha: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    /// `open` or `closed`.
    pub state: String,
    pub draft: bool,
    pub merged: bool,
    pub user: User,
    pub html_url: String,
    pub created_at: String,
    pub updated_at: String,
    pub head: Branch,
    pub base: Branch,
    pub labels: Vec<Label>,
    pub requested_reviewers: Vec<User>,
    /// Only on a single pull request: `None` while GitHub computes it.
    pub mergeable: Option<bool>,
    pub mergeable_state: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub commits: u64,
    pub comments: u64,
    pub review_comments: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PullFile {
    pub filename: String,
    /// `added`, `removed`, `modified`, `renamed`…
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    /// The unified diff for the file (absent for binary or huge files).
    pub patch: Option<String>,
    pub previous_filename: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommitAuthor {
    pub name: String,
    pub date: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommitInfo {
    pub message: String,
    pub author: CommitAuthor,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Commit {
    pub sha: String,
    pub commit: CommitInfo,
}

/// A comment on the pull request's conversation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssueComment {
    pub id: u64,
    pub user: User,
    pub body: String,
    pub created_at: String,
    pub html_url: String,
}

/// A comment on a line of the diff, possibly a reply in a thread.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewComment {
    pub id: u64,
    pub user: User,
    pub body: String,
    pub path: String,
    pub line: Option<u64>,
    pub original_line: Option<u64>,
    pub side: Option<String>,
    pub in_reply_to_id: Option<u64>,
    pub diff_hunk: String,
    pub created_at: String,
    pub html_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Review {
    pub id: u64,
    pub user: User,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED`, `PENDING`.
    pub state: String,
    pub body: Option<String>,
    pub submitted_at: Option<String>,
}

/// One CI check (a check run, or a commit status).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    /// `success`, `failure`, `pending`, `neutral`, `skipped`, `cancelled`…
    pub outcome: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CheckRuns {
    pub check_runs: Vec<CheckRun>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CombinedStatus {
    pub statuses: Vec<CommitStatus>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CommitStatus {
    pub context: String,
    pub state: String,
    pub target_url: Option<String>,
}

/// A line comment for a new review.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DraftComment {
    pub path: String,
    /// The line in the new file (`RIGHT`) the comment is on.
    pub line: u64,
    pub side: String,
    pub body: String,
}
