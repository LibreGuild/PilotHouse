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
}
