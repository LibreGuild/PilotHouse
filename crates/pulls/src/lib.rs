//! Pull requests in PilotHouse, for GitHub and GitHub Enterprise.
//!
//! - [`AccountsView`]: Settings → GitHub (accounts per host, tokens in the
//!   system keychain).
//! - [`PullsPanel`]: a repository's open PRs with CI state, filters (mine,
//!   to review), in a workspace's PRs tool a form to open one for the
//!   current branch, and the repository's issues beside them.
//! - [`PullView`]: one PR in full: what an agent found, a briefing, the
//!   files with what is said on their lines, the conversation with the
//!   checks and the merge under it; a review is finished in one post.
//! - [`TemplatesView`]: Settings → Review templates, what agents are asked to do
//!   with a pull request.
//! - [`clones`]: the GitHub view's own clones of repositories, where agent
//!   reviews run.
//!
//! The GitHub API lives in `ph-github`; this crate is the UI.

mod accounts_view;
pub mod clones;
mod findings_view;
mod finish_view;
pub mod hosting;
mod issue_form;
mod issues_view;
mod pull_view;
mod pulls_panel;
mod request;
mod review_settings;
mod templates_view;
mod walkthrough_view;

pub use accounts_view::AccountsView;
pub use hosting::Hosting;
pub use pull_view::{PullView, PullViewEvent};
pub use pulls_panel::{PullsPanel, PullsPanelEvent};
pub use request::AgentRequest;
pub use review_settings::ReviewSettings;
pub use templates_view::TemplatesView;

/// The result of a GitHub call made on a scoped thread, so several can run
/// at once.
pub(crate) fn joined<T>(
    handle: std::thread::ScopedJoinHandle<'_, anyhow::Result<T>>,
) -> anyhow::Result<T> {
    handle
        .join()
        .unwrap_or_else(|_| Err(anyhow::anyhow!("reading from GitHub failed")))
}
