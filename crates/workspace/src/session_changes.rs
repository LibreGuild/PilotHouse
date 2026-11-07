//! Workspaces whose list of chats changed away from their screen (a
//! terminal agent's chat saved, a quick chat started): each window reads
//! them again on its next look (`Workspace` each second). Changes are
//! numbered, so every window sees each one, whichever looks first.

use gpui_kit::{App, Global};
use ph_store::projects::ProjectId;

/// How many changes are kept for windows that have not looked yet.
const KEPT: usize = 256;

#[derive(Default)]
struct Changed {
    /// The number the next change gets.
    next: u64,
    /// The latest changes, oldest first.
    changes: std::collections::VecDeque<(u64, ProjectId)>,
}

impl Global for Changed {}

/// What a window has seen of the changes.
#[derive(Default, Clone, Copy)]
pub(crate) struct Seen(u64);

/// What a window opened now has seen: everything so far.
pub(crate) fn current(cx: &App) -> Seen {
    Seen(cx.try_global::<Changed>().map_or(0, |changed| changed.next))
}

/// Workspace `project`'s chats changed.
pub(crate) fn note(project: ProjectId, cx: &mut App) {
    let changed = cx.default_global::<Changed>();
    if changed.changes.len() == KEPT {
        changed.changes.pop_front();
    }
    changed.changes.push_back((changed.next, project));
    changed.next += 1;
}

/// The workspaces whose chats changed since `seen`, each once, and `seen`
/// moved on; `None` when more changed than are kept (read them all again).
/// Reads only: the windows drawn from the list are not marked changed.
pub(crate) fn since(seen: &mut Seen, cx: &App) -> Option<Vec<ProjectId>> {
    let Some(changed) = cx.try_global::<Changed>() else {
        return Some(Vec::new());
    };
    let from = std::mem::replace(&mut seen.0, changed.next);
    if changed
        .changes
        .front()
        .is_some_and(|(first, _)| *first > from)
    {
        return None;
    }
    let mut projects: Vec<ProjectId> = changed
        .changes
        .iter()
        .filter(|(at, _)| *at >= from)
        .map(|(_, project)| *project)
        .collect();
    projects.sort_unstable();
    projects.dedup();
    Some(projects)
}
