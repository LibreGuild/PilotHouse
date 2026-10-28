//! The Files tree's right-click menu and the keys that do the same: open,
//! reveal, new, cut/copy/paste, rename, move to the trash, copy paths, Git,
//! compare, and handing a file to the chat.
//!
//! Each acts on the row that was right-clicked, or for a key, on the
//! selected row. File system work runs in the background; the watcher then
//! refreshes the tree, and the touched folder is listed again at once.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{IconName, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Action, App, AppContext as _, ClipboardItem, Context, WeakEntity, Window};
use ph_ui::Icons;

use crate::file_ops;
use crate::files_panel::{FilesPanel, FilesPanelEvent};
use crate::name_dialog::ask_name;

/// Files picked with Copy or Cut, waiting for Paste.
#[derive(Debug, Clone)]
pub(crate) struct Clipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

/// A row the menu (or a key) acts on.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    /// The tree's path for it (with the folder's name first when there are
    /// several folders).
    pub path: String,
    pub absolute: PathBuf,
    pub is_folder: bool,
    /// A workspace folder itself (the top rows with several folders).
    pub is_root: bool,
}

impl Target {
    fn name(&self) -> String {
        self.absolute
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.clone())
    }

    /// The folder new files go in: the folder itself, or the file's.
    fn folder(&self) -> String {
        if self.is_folder {
            self.path.clone()
        } else {
            parent_of(&self.path)
        }
    }
}

/// The tree path of the folder holding `path` (`""` at the top).
fn parent_of(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

/// A path under `old` moved to `new` (`old` itself or inside it).
fn moved_path(path: &str, old: &str, new: &str) -> Option<String> {
    if path == old {
        return Some(new.to_string());
    }
    path.strip_prefix(old)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(|rest| format!("{new}/{rest}"))
}

impl FilesPanel {
    /// The row with tree path `path`, if it is still in the workspace.
    pub(crate) fn target(&self, path: &str, is_folder: bool) -> Option<Target> {
        let absolute = self.roots.absolute(path)?;
        let is_root = self.roots.is_multi() && !path.contains('/');
        Some(Target {
            path: path.to_string(),
            absolute,
            is_folder: is_folder || is_root,
            is_root,
        })
    }

    /// The selected row, for the tree's keys.
    pub(crate) fn selected_target(&self, cx: &App) -> Option<Target> {
        let entry = self.tree_state.read(cx).selected_entry()?.clone();
        self.target(&entry.item().id, entry.is_folder())
    }

    /// The menu for `target`.
    pub(crate) fn entry_menu(&self, target: Target, menu: PopupMenu) -> PopupMenu {
        let panel = self.weak.clone();
        let run = |f: fn(&mut FilesPanel, &Target, &mut Window, &mut Context<FilesPanel>)| {
            let (panel, target) = (panel.clone(), target.clone());
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                _ = panel.update(cx, |panel, cx| f(panel, &target, window, cx));
            }
        };
        let change = self.changes.files.get(&target.path).copied();
        let in_repo = self.in_repo(&target.path);
        let tracked = in_repo
            && !self.tree.ignored().contains(&target.path)
            && !matches!(
                change,
                Some(ph_git::Change::Untracked | ph_git::Change::Added)
            );
        let mut menu = menu;

        // Open.
        if !target.is_folder {
            menu = menu
                .item(
                    PopupMenuItem::new("Open").on_click(run(|panel, target, window, cx| {
                        panel.open_file(&target.path, None, window, cx)
                    })),
                )
                .item(
                    PopupMenuItem::new("Open in New Window").on_click(run(|_, target, _, cx| {
                        cx.emit(FilesPanelEvent::OpenInWindow(target.absolute.clone()))
                    })),
                )
                .when(ph_ui::platform::opens_safely(&target.absolute), |menu| {
                    menu.item(PopupMenuItem::new("Open in Default App").on_click(run(
                        |_, target, _, cx| cx.open_with_system(&target.absolute),
                    )))
                });
        } else {
            menu = menu
                .item(
                    PopupMenuItem::new("New File…")
                        .icon(Icons::FilePlus)
                        .action(Box::new(crate::NewFile))
                        .on_click(run(|panel, target, window, cx| {
                            panel.new_entry(target.folder(), false, window, cx)
                        })),
                )
                .item(
                    PopupMenuItem::new("New Folder…")
                        .icon(Icons::FolderPlus)
                        .action(Box::new(crate::NewFolder))
                        .on_click(run(|panel, target, window, cx| {
                            panel.new_entry(target.folder(), true, window, cx)
                        })),
                );
        }
        menu = menu
            .separator()
            .item(
                PopupMenuItem::new(ph_ui::platform::reveal_label())
                    .action(Box::new(crate::RevealEntry))
                    .on_click(run(|_, target, _, cx| cx.reveal_path(&target.absolute))),
            )
            .item(
                PopupMenuItem::new("Open in Terminal")
                    .icon(Icons::Terminal)
                    .on_click(run(|_, target, _, cx| {
                        let dir = if target.is_folder {
                            target.absolute.clone()
                        } else {
                            target
                                .absolute
                                .parent()
                                .map(Path::to_path_buf)
                                .unwrap_or_default()
                        };
                        cx.emit(FilesPanelEvent::OpenInTerminal(dir))
                    })),
            );
        if target.is_folder {
            menu = menu.item(
                PopupMenuItem::new("Find in Folder…")
                    .icon(IconName::Search)
                    .on_click(run(|panel, target, window, cx| {
                        panel.find_in_folder(target.path.clone(), window, cx)
                    })),
            );
        }

        // The chat.
        menu = menu.separator();
        if target.is_folder {
            menu = menu.item(
                PopupMenuItem::new("New Chat Here")
                    .icon(Icons::MessageSquare)
                    .on_click(run(|_, target, _, cx| {
                        cx.emit(FilesPanelEvent::NewChatIn(target.absolute.clone()))
                    })),
            );
        } else {
            menu = menu
                .item(
                    PopupMenuItem::new("Add to Chat")
                        .icon(Icons::MessageSquare)
                        .on_click(run(|_, target, _, cx| {
                            cx.emit(FilesPanelEvent::AddToChat(target.path.clone()))
                        })),
                )
                .item(
                    PopupMenuItem::new("Ask Agent About This…")
                        .icon(Icons::Sparkles)
                        .on_click(run(|_, target, _, cx| {
                            cx.emit(FilesPanelEvent::AskAboutFile(target.path.clone()))
                        })),
                );
        }

        // Moving things about.
        menu = menu.separator();
        if !target.is_root {
            menu = menu
                .item(
                    PopupMenuItem::new("Cut")
                        .action(Box::new(crate::CutEntry))
                        .on_click(run(|panel, target, _, cx| {
                            panel.set_clipboard(target, true, cx)
                        })),
                )
                .item(
                    PopupMenuItem::new("Copy")
                        .action(Box::new(crate::CopyEntry))
                        .on_click(run(|panel, target, _, cx| {
                            panel.set_clipboard(target, false, cx)
                        })),
                );
        }
        menu = menu.item(
            PopupMenuItem::new("Paste")
                .action(Box::new(crate::PasteEntry))
                .disabled(self.clipboard.is_none())
                .on_click(run(|panel, target, window, cx| {
                    panel.paste_into(target.folder(), window, cx)
                })),
        );
        if !target.is_root {
            menu = menu.item(
                PopupMenuItem::new("Duplicate")
                    .action(Box::new(crate::DuplicateEntry))
                    .on_click(run(|panel, target, window, cx| {
                        panel.duplicate_entry(target, window, cx)
                    })),
            );
        }

        // Paths.
        menu = menu
            .separator()
            .item(
                PopupMenuItem::new("Copy Path")
                    .action(Box::new(crate::CopyEntryPath))
                    .on_click(run(|_, target, _, cx| {
                        copy_text(target.absolute.to_string_lossy().into_owned(), cx)
                    })),
            )
            .item(
                PopupMenuItem::new("Copy Relative Path")
                    .action(Box::new(crate::CopyRelativePath))
                    .on_click(run(|panel, target, _, cx| {
                        copy_text(panel.relative_in_folder(&target.path), cx)
                    })),
            )
            .item(
                PopupMenuItem::new("Copy Name")
                    .on_click(run(|_, target, _, cx| copy_text(target.name(), cx))),
            );

        // Changing it.
        if !target.is_root {
            menu = menu
                .separator()
                .item(
                    PopupMenuItem::new("Rename…")
                        .icon(Icons::Pencil)
                        .action(Box::new(crate::RenameEntry))
                        .on_click(run(|panel, target, window, cx| {
                            panel.rename_entry(target, window, cx)
                        })),
                )
                .item(
                    PopupMenuItem::new("Move to Trash")
                        .icon(IconName::Delete)
                        .action(Box::new(crate::TrashEntry))
                        .on_click(run(|panel, target, window, cx| {
                            panel.trash_entry(target, window, cx)
                        })),
                );
        }

        // Git.
        if in_repo && !target.is_root {
            menu = menu.separator();
            if let Some(change) = change {
                let staged = self.staged.contains(&target.path);
                menu = menu.item(if staged {
                    PopupMenuItem::new("Unstage").on_click(run(|panel, target, window, cx| {
                        panel.git_entry(target, GitOp::Unstage, window, cx)
                    }))
                } else {
                    PopupMenuItem::new("Stage").on_click(run(|panel, target, window, cx| {
                        panel.git_entry(target, GitOp::Stage, window, cx)
                    }))
                });
                if tracked && !target.is_folder {
                    menu = menu
                        .item(
                            PopupMenuItem::new("Compare with Last Commit")
                                .icon(Icons::GitCompare)
                                .on_click(run(|panel, target, _, cx| {
                                    if let Some((root, within)) = panel.repo_path(&target.path) {
                                        cx.emit(FilesPanelEvent::CompareWithHead(root, within))
                                    }
                                })),
                        )
                        .item(
                            PopupMenuItem::new("Discard Changes…")
                                .icon(Icons::Undo2)
                                .on_click(run(|panel, target, window, cx| {
                                    panel.discard_entry(target, window, cx)
                                })),
                        );
                }
                if change == ph_git::Change::Untracked {
                    menu = menu.item(PopupMenuItem::new("Add to .gitignore").on_click(run(
                        |panel, target, window, cx| {
                            panel.git_entry(target, GitOp::Ignore, window, cx)
                        },
                    )));
                }
            }
            if tracked && !target.is_folder {
                menu = menu.item(
                    PopupMenuItem::new("File History")
                        .icon(Icons::RotateCcwClock)
                        .on_click(run(|panel, target, _, cx| {
                            if let Some((root, within)) = panel.repo_path(&target.path) {
                                cx.emit(FilesPanelEvent::ShowHistory(root, within))
                            }
                        })),
                );
            }
        }

        // Comparing two files.
        if !target.is_folder {
            let picked = self
                .compare
                .as_ref()
                .filter(|picked| **picked != target.path);
            menu = menu.separator();
            if let Some(picked) = picked {
                let name = picked.rsplit('/').next().unwrap_or(picked);
                menu = menu.item(
                    PopupMenuItem::new(format!("Compare with “{name}”"))
                        .icon(Icons::GitCompareArrows)
                        .on_click(run(|panel, target, _, cx| {
                            panel.compare_with_picked(target, cx)
                        })),
                );
            }
            menu = menu.item(PopupMenuItem::new("Select for Compare").on_click(run(
                |panel, target, _, cx| {
                    panel.compare = Some(target.path.clone());
                    cx.notify();
                },
            )));
        }

        // The folders of the workspace.
        if target.is_root {
            let root = target.absolute.clone();
            let (primary, remove) = (root.clone(), root);
            menu = menu
                .separator()
                .item(
                    PopupMenuItem::new("Make primary (agents start here)").on_click({
                        let panel = panel.clone();
                        move |_, _, cx| {
                            _ = panel.update(cx, |_, cx| {
                                cx.emit(FilesPanelEvent::MakePrimary(primary.clone()))
                            });
                        }
                    }),
                )
                .item(PopupMenuItem::new("Remove from workspace").on_click({
                    let panel = panel.clone();
                    move |_, _, cx| {
                        _ = panel.update(cx, |_, cx| {
                            cx.emit(FilesPanelEvent::RemoveFolder(remove.clone()))
                        });
                    }
                }));
        }
        if target.is_folder {
            menu = menu.separator().item(
                PopupMenuItem::new("Collapse All")
                    .on_click(run(|panel, _, _, cx| panel.collapse_all(cx))),
            );
        }
        menu
    }

    /// Asks for a name, then makes the file (and opens it) or folder in `dir`.
    pub(crate) fn new_entry(
        &mut self,
        dir: String,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(base) = self.roots.absolute(&dir).filter(|_| self.can_hold(&dir)) else {
            return;
        };
        let panel = self.weak.clone();
        let title = match (folder, dir.is_empty()) {
            (true, true) => "New folder".to_string(),
            (false, true) => "New file".to_string(),
            (true, false) => format!("New folder in {dir}"),
            (false, false) => format!("New file in {dir}"),
        };
        let ok = if folder {
            "Create folder"
        } else {
            "Create file"
        };
        ask_name(
            title,
            ok,
            String::new(),
            false,
            window,
            cx,
            Rc::new(move |name, window, cx| {
                let made = if folder {
                    file_ops::create_dir(&base, &name)
                } else {
                    file_ops::create_file(&base, &name)
                };
                let made = made.map_err(|err| format!("{err:#}"))?;
                _ = panel.update(cx, |panel, cx| {
                    let Some(path) = panel.roots.path_of(&made) else {
                        return;
                    };
                    panel.show_new(&path, cx);
                    if !folder {
                        panel.open_file(&path, None, window, cx);
                    }
                });
                Ok(())
            }),
        );
    }

    /// Asks for a new name for `target` and renames it; its open tabs
    /// follow.
    pub(crate) fn rename_entry(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target.is_root {
            return;
        }
        if let Some(name) = self.unsaved_under(&target.path, cx) {
            let message = format!("Save or close {name} first: it has unsaved changes.");
            return window.push_notification(Notification::warning(message), cx);
        }
        let panel = self.weak.clone();
        let target = target.clone();
        let name = target.name();
        ask_name(
            format!("Rename {name}"),
            "Rename",
            name,
            !target.is_folder,
            window,
            cx,
            Rc::new(move |new_name, window, cx| {
                let to = file_ops::rename(&target.absolute, &new_name)
                    .map_err(|err| format!("{err:#}"))?;
                _ = panel.update(cx, |panel, cx| {
                    if let Some(new_path) = panel.roots.path_of(&to) {
                        panel.follow_move(&target.path, &new_path, window, cx);
                        panel.show_new(&new_path, cx);
                    }
                });
                Ok(())
            }),
        );
    }

    /// Moves `target` to the trash after asking; its open tabs close.
    pub(crate) fn trash_entry(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target.is_root {
            return;
        }
        let panel = self.weak.clone();
        let target = target.clone();
        let unsaved = self.unsaved_under(&target.path, cx);
        let name = target.name();
        let what = if target.is_folder {
            "The folder and everything in it go"
        } else {
            "It goes"
        };
        let mut description = format!("{what} to the Trash, where you can restore it.");
        if let Some(unsaved) = unsaved {
            description.push_str(&format!(" Unsaved changes to {unsaved} will be lost."));
        }
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (panel, target) = (panel.clone(), target.clone());
            dialog
                .confirm()
                .title(format!("Move “{name}” to the Trash?"))
                .description(description.clone())
                .button_props(
                    gpui_kit::component::dialog::DialogButtonProps::default()
                        .ok_text("Move to Trash"),
                )
                .on_ok(move |_, window, cx| {
                    _ = panel.update(cx, |panel, cx| panel.trash_now(target.clone(), window, cx));
                    true
                })
        });
    }

    fn trash_now(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.close_under(&target.path, cx);
        let paths = vec![target.absolute.clone()];
        let work = cx.background_spawn(async move { file_ops::trash(&paths) });
        let parent = parent_of(&target.path);
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.refresh_dir(parent, cx);
                if this
                    .clipboard
                    .as_ref()
                    .is_some_and(|clip| clip.paths.contains(&target.absolute))
                {
                    this.clipboard = None;
                }
                report(result, window, cx);
            });
        })
        .detach();
    }

    pub(crate) fn duplicate_entry(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target.is_root {
            return;
        }
        let path = target.absolute.clone();
        let work = cx.background_spawn(async move { file_ops::duplicate(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok(copy) => {
                    if let Some(path) = this.roots.path_of(&copy) {
                        this.show_new(&path, cx);
                    }
                }
                Err(err) => report::<()>(Err(err), window, cx),
            });
        })
        .detach();
    }

    pub(crate) fn set_clipboard(&mut self, target: &Target, cut: bool, cx: &mut Context<Self>) {
        if target.is_root {
            return;
        }
        self.clipboard = Some(crate::tree_menu::Clipboard {
            paths: vec![target.absolute.clone()],
            cut,
        });
        cx.notify();
    }

    /// Copies or moves what Copy or Cut picked into `dir`.
    pub(crate) fn paste_into(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = self.clipboard.clone() else {
            return;
        };
        let Some(into) = self.roots.absolute(&dir).filter(|_| self.can_hold(&dir)) else {
            return;
        };
        if clip.cut {
            let unsaved = clip
                .paths
                .iter()
                .filter_map(|path| self.roots.path_of(path))
                .find_map(|path| self.unsaved_under(&path, cx));
            if let Some(name) = unsaved {
                let message = format!("Save or close {name} first: it has unsaved changes.");
                return window.push_notification(Notification::warning(message), cx);
            }
        }
        let sources = clip.paths.clone();
        let work = cx.background_spawn(async move { file_ops::paste(&sources, &into, clip.cut) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok(pasted) => {
                    if clip.cut {
                        this.clipboard = None;
                        for (from, to) in clip.paths.iter().zip(&pasted) {
                            if let (Some(from), Some(to)) =
                                (this.roots.path_of(from), this.roots.path_of(to))
                            {
                                this.follow_move(&from, &to, window, cx);
                                this.refresh_dir(parent_of(&from), cx);
                            }
                        }
                    }
                    if let Some(path) = pasted.last().and_then(|to| this.roots.path_of(to)) {
                        this.show_new(&path, cx);
                    }
                }
                Err(err) => report::<()>(Err(err), window, cx),
            });
        })
        .detach();
    }

    /// Asks before putting `target` back as the last commit has it.
    pub(crate) fn discard_entry(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.weak.clone();
        let target = target.clone();
        let name = target.name();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (panel, target) = (panel.clone(), target.clone());
            dialog
                .confirm()
                .title(format!("Discard the changes to “{name}”?"))
                .description("The file goes back to how the last commit has it.")
                .button_props(
                    gpui_kit::component::dialog::DialogButtonProps::default()
                        .ok_text("Discard changes"),
                )
                .on_ok(move |_, window, cx| {
                    _ = panel.update(cx, |panel, cx| {
                        panel.git_entry(&target, GitOp::Discard, window, cx)
                    });
                    true
                })
        });
    }

    pub(crate) fn git_entry(
        &mut self,
        target: &Target,
        op: GitOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((root, path)) = self.repo_path(&target.path) else {
            return;
        };
        let is_folder = target.is_folder;
        let work = cx.background_spawn(async move {
            match op {
                GitOp::Stage => ph_git::stage_path(&root, &path),
                GitOp::Unstage => ph_git::unstage_path(&root, &path),
                GitOp::Discard => ph_git::files::discard_path(&root, &path),
                GitOp::Ignore => ph_git::files::ignore_path(&root, &path, is_folder),
            }
        });
        let open = (op == GitOp::Discard).then(|| target.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                if let Some(path) = &open
                    && let Some(ix) = this.index_of(path, cx)
                {
                    this.open_files[ix].update(cx, |file, cx| file.reload_if_clean(window, cx));
                }
                report(result, window, cx);
            });
        })
        .detach();
    }

    fn compare_with_picked(&mut self, target: &Target, cx: &mut Context<Self>) {
        let Some(picked) = self.compare.take() else {
            return;
        };
        if let Some(first) = self.roots.absolute(&picked) {
            cx.emit(FilesPanelEvent::CompareFiles(
                first,
                target.absolute.clone(),
            ));
        }
        cx.notify();
    }

    /// The repository folder `path` is in, and `path` within it.
    pub(crate) fn repo_path(&self, path: &str) -> Option<(PathBuf, String)> {
        let (root, within) = self.roots.split(path)?;
        let ix = self.roots.list().iter().position(|r| r == root)?;
        self.repos
            .get(ix)
            .copied()
            .unwrap_or_default()
            .then(|| (root.path.clone(), within.to_string()))
    }

    fn in_repo(&self, path: &str) -> bool {
        self.repo_path(path).is_some()
    }

    /// `path` within its folder (without the folder's name in front).
    fn relative_in_folder(&self, path: &str) -> String {
        self.roots
            .split(path)
            .map(|(root, within)| {
                if within.is_empty() {
                    root.name.clone()
                } else {
                    within.to_string()
                }
            })
            .unwrap_or_else(|| path.to_string())
    }

    /// Whether files can go in tree folder `dir`: the top of a workspace
    /// with several folders lists only the folders.
    fn can_hold(&self, dir: &str) -> bool {
        !(self.roots.is_multi() && dir.is_empty())
    }

    /// An open file under `path` with unsaved changes, by name.
    fn unsaved_under(&self, path: &str, cx: &App) -> Option<String> {
        self.open_files.iter().find_map(|file| {
            let file = file.read(cx);
            (file.is_dirty() && moved_path(file.relative_path(), path, path).is_some())
                .then(|| file.relative_path().to_string())
        })
    }

    /// Closes the open files under `path` (it went to the trash).
    fn close_under(&mut self, path: &str, cx: &mut Context<Self>) {
        while let Some(ix) = self
            .open_files
            .iter()
            .position(|file| moved_path(file.read(cx).relative_path(), path, path).is_some())
        {
            self.close_file(ix, cx);
        }
    }

    /// `old` moved to `new`: its open files close and open again there.
    fn follow_move(&mut self, old: &str, new: &str, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_file_path(cx);
        let mut reopen = Vec::new();
        while let Some((ix, to)) = self.open_files.iter().enumerate().find_map(|(ix, file)| {
            moved_path(file.read(cx).relative_path(), old, new).map(|to| (ix, to))
        }) {
            self.close_file(ix, cx);
            reopen.push(to);
        }
        for path in &reopen {
            self.open_file(path, None, window, cx);
        }
        if let Some(active) =
            active.and_then(|active| moved_path(&active, old, new).or(Some(active)))
            && let Some(ix) = self.index_of(&active, cx)
        {
            self.active = Some(ix);
        }
        if let Some(picked) = self
            .compare
            .as_deref()
            .and_then(|picked| moved_path(picked, old, new))
        {
            self.compare = Some(picked);
        }
        self.refresh_dir(parent_of(old), cx);
    }

    /// Lists `path`'s folder again, opens the folders down to it and
    /// selects it once listed.
    pub(crate) fn show_new(&mut self, path: &str, cx: &mut Context<Self>) {
        let mut dir = parent_of(path);
        let mut dirs = Vec::new();
        loop {
            self.tree.set_expanded(&dir, true);
            dirs.push(dir.clone());
            if dir.is_empty() {
                break;
            }
            dir = parent_of(&dir);
        }
        self.pending_select = Some(path.to_string());
        self.load_dirs(dirs, cx);
    }

    fn refresh_dir(&mut self, dir: String, cx: &mut Context<Self>) {
        if self.tree.is_listed(&dir) {
            self.load_dirs(vec![dir], cx);
        }
    }

    /// New file (or folder) beside the selected row, or at the top (in
    /// the first folder, with several).
    pub fn new_in_selection(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        let dir = match self.selected_target(cx) {
            Some(target) => target.folder(),
            None if self.roots.is_multi() => self.roots.list()[0].name.clone(),
            None => String::new(),
        };
        self.new_entry(dir, folder, window, cx);
    }

    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.tree.collapse_all();
        self.rebuild_tree(cx);
    }

    /// Workspace search, within tree folder `dir`.
    pub(crate) fn find_in_folder(
        &mut self,
        dir: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_scope = (!dir.is_empty()).then_some(dir);
        self.find_in_project(window, cx);
        self.search_again(window, cx);
    }

    /// Runs a tree key's action on the selected row.
    pub(crate) fn on_tree_key(
        &mut self,
        action: &dyn Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.selected_target(cx);
        let folder = match &target {
            Some(target) => target.folder(),
            None if self.roots.is_multi() => self.roots.list()[0].name.clone(),
            None => String::new(),
        };
        let any = action.as_any();
        if any.is::<crate::NewFile>() {
            return self.new_entry(folder, false, window, cx);
        }
        if any.is::<crate::NewFolder>() {
            return self.new_entry(folder, true, window, cx);
        }
        if any.is::<crate::PasteEntry>() {
            return self.paste_into(folder, window, cx);
        }
        let Some(target) = target else {
            return;
        };
        if any.is::<crate::RenameEntry>() {
            self.rename_entry(&target, window, cx)
        } else if any.is::<crate::TrashEntry>() {
            self.trash_entry(&target, window, cx)
        } else if any.is::<crate::CopyEntry>() {
            self.set_clipboard(&target, false, cx)
        } else if any.is::<crate::CutEntry>() {
            self.set_clipboard(&target, true, cx)
        } else if any.is::<crate::DuplicateEntry>() {
            self.duplicate_entry(&target, window, cx)
        } else if any.is::<crate::RevealEntry>() {
            cx.reveal_path(&target.absolute)
        } else if any.is::<crate::CopyEntryPath>() {
            copy_text(target.absolute.to_string_lossy().into_owned(), cx)
        } else if any.is::<crate::CopyRelativePath>() {
            copy_text(self.relative_in_folder(&target.path), cx)
        }
    }
}

/// A Git change from the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitOp {
    Stage,
    Unstage,
    Discard,
    Ignore,
}

fn copy_text(text: String, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

/// Shows why a file operation failed, if it did.
fn report<T>(result: anyhow::Result<T>, window: &mut Window, cx: &mut App) {
    if let Err(err) = result {
        window.push_notification(Notification::error(format!("{err:#}")), cx);
    }
}

/// For the tree's builder, which runs inside the tree's state.
pub(crate) fn build(
    panel: &WeakEntity<FilesPanel>,
    path: &str,
    is_folder: bool,
    menu: PopupMenu,
    cx: &App,
) -> PopupMenu {
    let Some(panel) = panel.upgrade() else {
        return menu;
    };
    let panel = panel.read(cx);
    match panel.target(path, is_folder) {
        Some(target) => panel.entry_menu(target, menu),
        None => menu,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_carry_paths_inside() {
        assert_eq!(
            moved_path("a/b.rs", "a/b.rs", "a/c.rs").as_deref(),
            Some("a/c.rs")
        );
        assert_eq!(
            moved_path("a/x/y.rs", "a/x", "a/z").as_deref(),
            Some("a/z/y.rs")
        );
        assert_eq!(moved_path("a/xy.rs", "a/x", "a/z"), None);
        assert_eq!(parent_of("a/b/c"), "a/b");
        assert_eq!(parent_of("c"), "");
    }
}
