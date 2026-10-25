//! Pull requests in PilotHouse, for GitHub and GitHub Enterprise.
//!
//! - [`AccountsView`]: Settings → GitHub (accounts per host, tokens in the
//!   system keychain).
//! - [`PullsPanel`]: a repository's open PRs with CI state, filters (mine,
//!   to review), and in a workspace's PRs tool a form to open one for the
//!   current branch.
//! - [`PullView`]: one PR in full: description, conversation, files with
//!   line comments, commits, checks; comment, approve, request changes,
//!   merge, check out, review with an agent.
//! - [`TemplatesView`]: Settings → Templates, what agents are asked to do
//!   with a pull request.
//! - [`clones`]: the GitHub view's own clones of repositories, where agent
//!   reviews run.
//!
//! The GitHub API lives in `ph-github`; this crate is the UI.

mod accounts_view;
pub mod clones;
pub mod hosting;
mod pull_view;
mod pulls_panel;
mod templates_view;

pub use accounts_view::AccountsView;
pub use hosting::Hosting;
pub use pull_view::{PullView, PullViewEvent, short_date};
pub use pulls_panel::{PullsPanel, PullsPanelEvent};
pub use templates_view::TemplatesView;
