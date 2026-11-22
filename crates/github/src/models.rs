//! The parts of GitHub's REST replies PilotHouse uses. Every field has a
//! default, so API additions and Enterprise differences do not break it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: String,
    /// `User`, `Bot`, `Organization`…
    #[serde(rename = "type")]
    pub kind: String,
}

impl User {
    /// An app or a bot account (Dependabot, Renovate…), not a person.
    pub fn is_bot(&self) -> bool {
        self.kind.eq_ignore_ascii_case("bot") || self.login.ends_with("[bot]")
    }
}

/// A team asked to review.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Team {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Repository {
    /// `owner/name`, as GitHub spells it.
    pub full_name: String,
    pub default_branch: String,
    pub private: bool,
    pub html_url: String,
    /// The HTTPS clone URL.
    pub clone_url: String,
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
    pub requested_teams: Vec<Team>,
    /// Only on a single pull request: `None` while GitHub computes it.
    pub mergeable: Option<bool>,
    /// Only on a single pull request: `clean`, `blocked` (a required
    /// review or check is missing), `behind`, `dirty` (conflicts),
    /// `unstable` (a check fails), `draft`, `unknown`.
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
    /// The file's content, as git names it: it changes when the file does.
    pub sha: String,
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
    /// The author's standing in the repository (`OWNER`, `MEMBER`,
    /// `COLLABORATOR`, `CONTRIBUTOR`, `NONE` …).
    pub author_association: String,
}

/// A comment on a line of the diff, possibly a reply in a thread.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewComment {
    pub id: u64,
    pub user: User,
    /// How the author relates to the repository (`OWNER`, `MEMBER`,
    /// `COLLABORATOR`, `CONTRIBUTOR`, `NONE`…).
    #[serde(default)]
    pub author_association: String,
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
    /// The commit it was made on.
    pub commit_id: Option<String>,
}

/// One CI check (a check run, or a commit status).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Check {
    pub name: String,
    /// `success`, `failure`, `pending`, `neutral`, `skipped`, `cancelled`…
    pub outcome: String,
    pub url: String,
    /// A check run's id (a commit status has none): its annotations, its
    /// log and running it again go by it.
    pub id: Option<u64>,
    /// What the check said of itself (a title, a short summary).
    pub summary: String,
    /// A GitHub Actions job: its log can be read and the job run again.
    pub actions: bool,
}

impl Check {
    pub fn failed(&self) -> bool {
        matches!(
            self.outcome.as_str(),
            "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure"
        )
    }
}

/// A place a check points at: a compiler error, a failed test.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Annotation {
    pub path: String,
    pub start_line: u64,
    /// `failure`, `warning` or `notice`.
    pub annotation_level: String,
    pub title: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CheckRuns {
    pub check_runs: Vec<CheckRun>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CheckRun {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: String,
    pub output: CheckOutput,
    pub app: CheckApp,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CheckOutput {
    pub title: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CheckApp {
    pub slug: String,
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
    /// The line in the new file (`RIGHT`) the comment is on (the last one
    /// of several).
    pub line: u64,
    pub side: String,
    pub body: String,
    /// The first line, for a comment on several lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_side: Option<String>,
}

/// A pull request's line-comment thread (GraphQL), by its first comment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Thread {
    /// GraphQL node id, to resolve it.
    pub id: String,
    pub resolved: bool,
    /// The REST id of its first comment.
    pub first_comment: u64,
}

/// An issue.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    /// `open` or `closed`.
    pub state: String,
    pub html_url: String,
    pub body: Option<String>,
    pub user: User,
    pub labels: Vec<Label>,
    pub assignees: Vec<User>,
    /// How many comments it has.
    pub comments: u64,
    pub created_at: String,
    pub updated_at: String,
    /// Set on a pull request, which GitHub also lists as an issue.
    pub pull_request: Option<serde_json::Value>,
}
