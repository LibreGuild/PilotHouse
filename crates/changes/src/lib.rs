//! Reviewing uncommitted changes.
//!
//! Agents edit files directly; the Changes tab is where those edits (and the
//! user's own) are reviewed before committing: keep (stage) or discard each
//! hunk, with undo for discards. It reads the repository through `ph-git`
//! and refreshes whenever the project folder changes.

mod branch_picker;
mod changes_panel;
mod commit_box;
mod conflict_view;
mod diff_view;
mod history_view;
mod repo;
mod stash_view;
pub mod text_dialog;

/// The git operations, for callers that had them from here.
pub use ph_git as git;

pub use branch_picker::{BranchPicker, ago};
pub use changes_panel::{ChangesPanel, ChangesPanelEvent, GitView};
pub use commit_box::{Writer, clean_message, commit_prompt, cut};
pub use diff_view::file_diff;
pub use repo::{Repo, RepoEvent};
