//! What the windows of a deck (`slides_window`) and of a whiteboard
//! (`board_window`) share. Both show the artifact's page in the system's
//! web view beside a chat, and both may work from a workspace: the chat is
//! then that workspace's, whose agent has the workspace's files to make
//! the deck or the board from.

use gpui_kit::component::{
    Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    menu::PopupMenuItem,
};
use gpui_kit::{App, Context, IntoElement, SharedString, Window};
use ph_artifacts::Artifact;
use ph_store::projects::Project;
use ph_ui::Icons;
use ph_ui::MenuButton as _;

/// The workspace an artifact belongs to, while there is such a workspace:
/// its chats are the artifact's.
pub(crate) fn workspace_of(artifact: &Artifact, cx: &App) -> Option<Project> {
    let source = artifact.manifest.source.as_ref()?;
    crate::AppState::store(cx)
        .project(source.project)
        .ok()
        .flatten()
        .filter(Project::is_workspace)
}

/// Whether something of the app's is open that may be drawn where the
/// artifact's page is (the system draws a web view over everything the app
/// draws): a dialog or sheet, a note at the window's edge, a menu or
/// pop-up of the chat (those hold the app's focus while open).
///
/// `drawn`: the window has drawn a frame; before that there is no focus to
/// ask about.
pub(crate) fn overlay_open(drawn: bool, window: &mut Window, cx: &mut App) -> bool {
    window.has_active_dialog(cx)
        || window.has_active_sheet(cx)
        || !window.notifications(cx).is_empty()
        || (drawn
            && window
                .context_stack()
                .iter()
                .any(|context| context.contains("PopupMenu") || context.contains("Popover")))
}

/// Which workspace the `noun` ("deck", "board") of window `V` works from
/// (`current`: its id and name), and the menu that changes it through
/// `work_from`.
pub(crate) fn workspace_button<V: 'static>(
    noun: &'static str,
    current: Option<&(i64, String)>,
    work_from: fn(&mut V, Option<Project>, &mut Window, &mut Context<V>),
    cx: &mut Context<V>,
) -> impl IntoElement + use<V> {
    let chosen = current.map(|(id, _)| *id);
    let this = cx.entity().downgrade();
    let button = Button::new(SharedString::from(format!("{noun}-workspace")))
        .ghost()
        .small()
        .icon(Icons::FolderOpen)
        .accessibility_label(format!("The workspace this {noun} works from"))
        .tooltip(ph_ui::tip(match current {
            Some(_) => format!(
                "The chat is this workspace's: its agent works in the workspace's folders and on this {noun}"
            ),
            None => format!(
                "Work from a workspace: its agent gets the workspace's files to make this {noun} from"
            ),
        }));
    let button = match current {
        Some((_, name)) => {
            let short: String = name.chars().take(28).collect();
            button.label(if short.len() < name.len() {
                format!("{short}…")
            } else {
                short
            })
        }
        None => button,
    };
    button.menu(gpui_kit::Anchor::TopRight, move |menu, _, cx| {
        /// Workspaces offered: the ones used last.
        const OFFERED: usize = 12;
        // Read when the menu opens, not at every frame of the window.
        let mut workspaces: Vec<Project> = crate::AppState::store(cx)
            .projects()
            .unwrap_or_default()
            .into_iter()
            .filter(Project::is_workspace)
            .collect();
        workspaces.sort_by_key(|project| std::cmp::Reverse(project.last_opened_at));
        // The artifact's own is among them however long ago it was used.
        let mut offered: Vec<Project> = workspaces.iter().take(OFFERED).cloned().collect();
        if let Some(own) = workspaces
            .iter()
            .find(|project| Some(project.id) == chosen)
            .filter(|own| !offered.iter().any(|project| project.id == own.id))
        {
            offered.push(own.clone());
        }
        let pick = |to: Option<Project>| {
            let this = this.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                let to = to.clone();
                _ = this.update(cx, |this, cx| work_from(this, to, window, cx));
            }
        };
        let mut menu = menu.label("The agent works from");
        for workspace in &offered {
            // No icon: its place is where the tick of the one chosen is.
            menu = menu.item(
                PopupMenuItem::new(workspace.name.clone())
                    .checked(Some(workspace.id) == chosen)
                    .on_click(pick(Some(workspace.clone()))),
            );
        }
        menu.separator().item(
            PopupMenuItem::new(format!("No workspace: this {noun} only"))
                .checked(chosen.is_none())
                .on_click(pick(None)),
        )
    })
}
