//! Pull requests in PilotHouse, for GitHub and GitHub Enterprise.
//!
//! - [`AccountsView`]: Settings → GitHub (accounts per host, tokens in the
//!   system keychain).
//! - [`PullsPanel`]: the Pull requests tab: open PRs with CI state, filters
//!   (mine, to review), and a form to open one for the current branch.
//! - [`PullView`]: one PR in full: description, conversation, files with
//!   line comments, commits, checks; comment, approve, request changes,
//!   merge, check out, review with an agent.
//!
//! The GitHub API lives in `ph-github`; this crate is the UI.

mod accounts_view;
pub mod hosting;
mod pull_view;
mod pulls_panel;

pub use accounts_view::AccountsView;
pub use hosting::Hosting;
pub use pull_view::{PullView, PullViewEvent};
pub use pulls_panel::{PullsPanel, PullsPanelEvent};
