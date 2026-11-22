//! Folders outside a workspace that its chats may change without asking:
//! the files of the decks made from it. A deck is kept with the artifacts,
//! not in the workspace, and what an agent changes there shows at once and
//! is undone in the deck; so the chat that made the deck works on it as the
//! deck's own chat does. Whoever knows the decks (the window shell) says
//! which folders these are.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::{App, Global};
use ph_store::projects::ProjectId;

#[derive(Default)]
struct OwnFiles(HashMap<ProjectId, Vec<PathBuf>>);

impl Global for OwnFiles {}

/// The folders workspace `project`'s chats change without asking are
/// `folders` from here on (none: it has no deck any more).
pub fn set(project: ProjectId, folders: Vec<PathBuf>, cx: &mut App) {
    let known = cx.default_global::<OwnFiles>();
    if folders.is_empty() {
        known.0.remove(&project);
    } else {
        known.0.insert(project, folders);
    }
}

/// Forgets every workspace's folders but those in `kept`.
pub fn keep_only(kept: &[ProjectId], cx: &mut App) {
    if let Some(known) = cx.try_global::<OwnFiles>()
        && known.0.keys().any(|project| !kept.contains(project))
    {
        cx.global_mut::<OwnFiles>()
            .0
            .retain(|project, _| kept.contains(project));
    }
}

/// The folders workspace `project`'s chats change without asking.
pub(crate) fn of(project: ProjectId, cx: &App) -> &[PathBuf] {
    cx.try_global::<OwnFiles>()
        .and_then(|known| known.0.get(&project))
        .map(Vec::as_slice)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn a_workspaces_folders_are_set_replaced_and_forgotten(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            assert!(of(3, cx).is_empty());
            set(3, vec![PathBuf::from("/decks/a/site")], cx);
            set(
                4,
                vec![
                    PathBuf::from("/decks/b/site"),
                    PathBuf::from("/decks/c/site"),
                ],
                cx,
            );
            assert_eq!(of(3, cx), [PathBuf::from("/decks/a/site")]);
            assert_eq!(of(4, cx).len(), 2);
            // Its decks are others now.
            set(3, vec![PathBuf::from("/decks/d/site")], cx);
            assert_eq!(of(3, cx), [PathBuf::from("/decks/d/site")]);
            // Workspace 4 has no deck any more.
            keep_only(&[3], cx);
            assert!(of(4, cx).is_empty());
            assert_eq!(of(3, cx).len(), 1);
            set(3, Vec::new(), cx);
            assert!(of(3, cx).is_empty());
        });
    }
}
