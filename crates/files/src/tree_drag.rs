//! Dragging in the Files tree, as in VS Code:
//!
//! - **Rows** drag to a folder (or a file: its folder) to move there; with
//!   ⌥ (Ctrl off macOS) held at the drop they are copied. A folder the drag
//!   rests on opens. Moving asks first, unless the user said not to.
//! - **Files from the system** (Finder, Explorer) dropped on the tree are
//!   copied in.
//! - A dragged row ([`DraggedFiles`]) also opens in the editor when dropped
//!   there, and attaches to a chat's message when dropped on the chat.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex};
use gpui_kit::{
    AppContext as _, Context, ExternalPaths, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, px,
};

use crate::files_panel::FilesPanel;
use crate::tree_menu::{Target, outermost};

/// The setting that turns off the question before a drag moves files.
const CONFIRM_MOVE_KEY: &str = "files.confirm_drag_move";
/// How long a drag rests on a closed folder before it opens.
const OPEN_AFTER: Duration = Duration::from_millis(700);

/// What a drag from the Files tree carries: the rows it moves (the one
/// dragged, or all the picked ones when it is among them).
#[derive(Debug, Clone)]
pub struct DraggedFiles {
    /// The tree's paths (the folder's name first, with several folders):
    /// what a chat attaches. Shared, since every picked row carries them.
    pub paths: Rc<[String]>,
    /// Whether the first one is a folder (for the drag's picture).
    pub folder: bool,
}

impl DraggedFiles {
    fn label(&self) -> SharedString {
        match &*self.paths {
            [one] => one.rsplit('/').next().unwrap_or(one).to_string().into(),
            many => format!("{} items", many.len()).into(),
        }
    }
}

/// The picture under the pointer while rows are dragged.
pub struct DragPreview {
    label: SharedString,
    folder: bool,
}

impl DragPreview {
    pub fn new(dragged: &DraggedFiles) -> Self {
        Self {
            label: dragged.label(),
            folder: dragged.folder,
        }
    }
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_md()
            .text_sm()
            .child(if self.folder {
                ph_ui::folder_icon_element(&self.label, px(14.))
            } else {
                ph_ui::file_icon_element(&self.label, px(14.))
            })
            .child(self.label.clone())
    }
}

impl FilesPanel {
    /// Rows dropped on row `onto` (into it when a folder, else into its
    /// folder): moved, or copied with ⌥ (Ctrl off macOS) held.
    pub(crate) fn drop_files(
        &mut self,
        dragged: &DraggedFiles,
        onto: &str,
        onto_folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drag_hover = None;
        let modifiers = window.modifiers();
        let copy = if cfg!(target_os = "macos") {
            modifiers.alt
        } else {
            modifiers.control
        };
        let dir = if onto_folder {
            onto.to_string()
        } else {
            crate::tree_menu::parent_of(onto)
        };
        let Some(into) = self.drop_folder(&dir, window, cx) else {
            return;
        };
        // As Cut does: never a workspace folder itself, and a folder brings
        // what is picked inside it. A folder dropped on itself or inside
        // itself stays, and so does what is already there (unless copied).
        let targets = outermost(
            dragged
                .paths
                .iter()
                .filter_map(|path| self.target(path, self.tree.is_dir(path)))
                .filter(|target| !target.is_root)
                .collect(),
        );
        let targets: Vec<Target> = targets
            .into_iter()
            .filter(|target| !into.starts_with(&target.absolute))
            .filter(|target| copy || target.absolute.parent() != Some(into.as_path()))
            .collect();
        if targets.is_empty() {
            return;
        }
        let sources: Vec<PathBuf> = targets
            .iter()
            .map(|target| target.absolute.clone())
            .collect();
        if copy {
            return self.transfer(sources, into, false, false, window, cx);
        }
        if let Some(name) = targets
            .iter()
            .find_map(|target| self.unsaved_under(&target.path, cx))
        {
            let message = format!("Save or close {name} first: it has unsaved changes.");
            return window.push_notification(
                gpui_kit::component::notification::Notification::warning(message),
                cx,
            );
        }
        let ask: bool = self
            .store
            .as_ref()
            .and_then(|store| store.setting::<Option<bool>>(CONFIRM_MOVE_KEY))
            .unwrap_or(true);
        if !ask {
            return self.transfer(sources, into, true, false, window, cx);
        }
        let what = match sources.as_slice() {
            [one] => format!(
                "“{}”",
                one.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            many => format!("{} items", many.len()),
        };
        let place = into
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| into.display().to_string());
        let title = format!("Move {what} into “{place}”?");
        let again = Rc::new(std::cell::Cell::new(false));
        let panel = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (panel, sources, into, again) =
                (panel.clone(), sources.clone(), into.clone(), again.clone());
            let checked = again.get();
            let toggle = again.clone();
            dialog
                .confirm()
                .title(title.clone())
                .description(
                    gpui_kit::component::v_flex()
                        .gap_2()
                        .child("Open files move with it. ⌥-drag (Ctrl off macOS) copies instead.")
                        .child(
                            gpui_kit::component::checkbox::Checkbox::new("files-move-ask-again")
                                .label("Don't ask again")
                                .checked(checked)
                                .on_click(move |checked, window, _| {
                                    toggle.set(*checked);
                                    window.refresh();
                                }),
                        ),
                )
                .button_props(
                    gpui_kit::component::dialog::DialogButtonProps::default().ok_text("Move"),
                )
                .on_ok(move |_, window, cx| {
                    let (sources, into, again) = (sources.clone(), into.clone(), again.get());
                    _ = panel.update(cx, |panel, cx| {
                        if again && let Some(store) = &panel.store {
                            let _ = store.set_setting(CONFIRM_MOVE_KEY, &Some(false));
                        }
                        panel.transfer(sources, into, true, false, window, cx);
                    });
                    true
                })
        });
    }

    /// Files dragged from the system onto row `onto` (or its folder): copied
    /// in.
    pub(crate) fn drop_external(
        &mut self,
        paths: &ExternalPaths,
        onto: &str,
        onto_folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drag_hover = None;
        let dir = if onto_folder {
            onto.to_string()
        } else {
            crate::tree_menu::parent_of(onto)
        };
        let Some(into) = self.drop_folder(&dir, window, cx) else {
            return;
        };
        let sources: Vec<PathBuf> = paths.paths().to_vec();
        if !sources.is_empty() {
            self.transfer(sources, into, false, false, window, cx);
        }
    }

    /// The folder tree path `dir` names, when files can go in it.
    fn drop_folder(
        &self,
        dir: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        if !self.can_hold(dir) {
            window.push_notification(
                gpui_kit::component::notification::Notification::info(
                    "Drop on one of the workspace's folders.",
                ),
                cx,
            );
            return None;
        }
        self.roots.absolute(dir)
    }

    /// A drag rests on closed folder `dir`: it opens after a moment, so the
    /// drag can go deeper.
    pub(crate) fn drag_rest(&mut self, dir: &str, cx: &mut Context<Self>) {
        if self
            .drag_hover
            .as_ref()
            .is_some_and(|(hovered, _)| hovered == dir)
        {
            return;
        }
        let path = dir.to_string();
        let opening: Task<()> = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OPEN_AFTER).await;
            _ = this.update(cx, |this, cx| {
                // Still resting there, and still dragging (not dropped or
                // cancelled meanwhile).
                let resting = this
                    .drag_hover
                    .as_ref()
                    .is_some_and(|(hovered, _)| *hovered == path);
                if resting {
                    if cx.has_active_drag() {
                        this.expand_dir(path.clone(), cx);
                    }
                    this.drag_hover = None;
                }
            });
        });
        self.drag_hover = Some((dir.to_string(), opening));
    }

    /// The drag left folder `dir` before it opened: it stays closed.
    pub(crate) fn drag_left(&mut self, dir: &str) {
        if self
            .drag_hover
            .as_ref()
            .is_some_and(|(hovered, _)| hovered == dir)
        {
            self.drag_hover = None;
        }
    }
}

/// The colour a row takes while something can drop on it.
pub(crate) fn drop_highlight(cx: &gpui_kit::App) -> gpui_kit::Hsla {
    cx.theme().drop_target
}

/// The drag's picture, made when a row starts dragging.
pub(crate) fn preview(
    dragged: &DraggedFiles,
    cx: &mut gpui_kit::App,
) -> gpui_kit::Entity<DragPreview> {
    cx.new(|_| DragPreview::new(dragged))
}
