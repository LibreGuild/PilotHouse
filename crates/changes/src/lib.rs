//! Reviewing uncommitted changes.
//!
//! Agents edit files directly; the Git tab is where those edits (and the
//! user's own) are reviewed before committing: tick each file as looked at,
//! keep (stage) or discard it or one of its hunks, with undo for discards,
//! and leave notes on lines for the agent. It reads the repository through
//! `ph-git` and refreshes whenever the project folder changes.

mod branch_picker;
mod change_tree;
mod changes_panel;
mod commit_box;
mod conflict_view;
pub mod file_dialogs;
mod history_view;
pub mod notes;
mod repo;
mod stash_view;
pub mod text_dialog;

/// The git operations, for callers that had them from here.
pub use ph_git as git;

pub use branch_picker::{BranchPicker, ago};
pub use changes_panel::{
    Attention, ChangesPanel, ChangesPanelEvent, Evidence, EvidenceKind, GitView,
};
pub use commit_box::{Writer, clean_message, commit_prompt, cut};
pub use ph_preview::diff::file_diff;
pub use repo::{Repo, RepoEvent};
