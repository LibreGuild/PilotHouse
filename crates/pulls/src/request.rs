//! Work for an agent about a pull request, asked from its view and done
//! where chats run (the GitHub view, or a task for fixes).

use ph_github::models::PullRequest;

#[derive(Debug, Clone)]
pub enum AgentRequest {
    /// A review round: `full`, `followup` (check the fixes), `second` (by
    /// another agent) or `double_check` (try to disprove the open findings).
    Review {
        pull: Box<PullRequest>,
        kind: &'static str,
    },
    /// Whether the author's answer on finding `finding_id` holds up.
    Claim { finding_id: i64 },
    /// A walkthrough of the change.
    Walkthrough { pull: Box<PullRequest> },
    /// A question about lines `start..=end` of changed file `path`.
    AskLines {
        pull: Box<PullRequest>,
        path: String,
        start: u64,
        end: u64,
        question: String,
    },
    /// Fix `findings` in a task on the pull request's branch.
    Fix {
        pull: Box<PullRequest>,
        findings: Vec<ph_store::reviews::Finding>,
    },
    /// Work through the review comments on the user's own pull request, in
    /// a task on its branch.
    Address { pull: Box<PullRequest> },
    /// Make a failing check pass, in a task on the pull request's branch:
    /// the check's name, what it points at (place, message) and its log
    /// around the first error.
    FixCheck {
        pull: Box<PullRequest>,
        name: String,
        annotations: Vec<(String, String)>,
        log: String,
    },
    /// Fix an issue in a task: the issue, and it with its comments as
    /// Markdown.
    FixIssue {
        issue: Box<ph_github::models::Issue>,
        text: String,
    },
}
