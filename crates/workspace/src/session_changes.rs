//! Workspaces whose list of chats changed away from their screen (a
//! terminal agent's chat saved, a quick chat started): the window reads
//! them again on its next look (`Workspace` each second).

use gpui_kit::{App, Global};
use ph_store::projects::ProjectId;

#[derive(Default)]
struct Changed(Vec<ProjectId>);

impl Global for Changed {}

/// Workspace `project`'s chats changed.
pub(crate) fn note(project: ProjectId, cx: &mut App) {
    cx.default_global::<Changed>().0.push(project);
}

/// The workspaces whose chats changed since last asked, each once.
pub(crate) fn take(cx: &mut App) -> Vec<ProjectId> {
    let mut changed = std::mem::take(&mut cx.default_global::<Changed>().0);
    changed.sort_unstable();
    changed.dedup();
    changed
}
