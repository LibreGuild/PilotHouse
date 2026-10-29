//! Settings → Storage: what PilotHouse keeps on disk and how much, with a
//! way to remove what is no longer wanted.
//!
//! Everything is under the data folder, except copies made for agents when
//! another folder was chosen for them (a bigger disk; see
//! [`ph_store::paths::copies_dir`]): GitHub clones and their pull request
//! copies, tasks' worktrees, and review and parallel-run copies. Copies are
//! removed with `git worktree remove`, so their repository forgets them; a
//! task's removal also takes its workspace out of PilotHouse. Sizes are
//! measured off the UI thread.

use std::path::{Path, PathBuf};

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, PathPromptOptions, Render,
    Styled as _, Task, Window, div,
};
use ph_files::large::format_size;
use ph_store::projects::ProjectId;
use ph_ui::Icons;

use crate::AppState;

/// How an item goes.
#[derive(Clone, Debug, PartialEq)]
enum Removal {
    /// A copy or clone: its folder (through Git for a worktree).
    Folder,
    /// A task: its worktree, and its workspace.
    Task(ProjectId),
    /// What the folder holds (logs, recordings…); the folder stays.
    Contents,
    /// Kept: PilotHouse's own data.
    Never,
}

#[derive(Clone, Debug)]
struct Item {
    label: String,
    detail: String,
    path: PathBuf,
    size: u64,
    removal: Removal,
}

#[derive(Clone, Debug)]
struct Group {
    title: &'static str,
    note: &'static str,
    items: Vec<Item>,
}

impl Group {
    fn size(&self) -> u64 {
        self.items.iter().map(|item| item.size).sum()
    }
}

pub struct StorageView {
    groups: Option<Vec<Group>>,
    measuring: bool,
    /// Items being removed, by folder.
    removing: Vec<PathBuf>,
    _measure: Option<Task<()>>,
}

impl StorageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            groups: None,
            measuring: false,
            removing: Vec::new(),
            _measure: None,
        };
        view.measure(cx);
        view
    }

    fn measure(&mut self, cx: &mut Context<Self>) {
        let store = AppState::store(cx).clone();
        self.measuring = true;
        let work = cx.background_spawn(async move { measure(&store) });
        self._measure = Some(cx.spawn(async move |this, cx| {
            let groups = work.await;
            _ = this.update(cx, |this, cx| {
                this.groups = Some(groups);
                this.measuring = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn remove(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        if self.removing.contains(&item.path) {
            return;
        }
        self.removing.push(item.path.clone());
        cx.notify();
        let store = AppState::store(cx).clone();
        let path = item.path.clone();
        let removal = item.removal.clone();
        let work = cx.background_spawn(async move { remove(&store, &path, &removal) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.removing.retain(|path| *path != item.path);
                match result {
                    Ok(()) => {
                        // A task's workspace goes with it.
                        if let Removal::Task(project) = item.removal
                            && let Some((_, workspace)) = crate::main_workspace(cx)
                        {
                            workspace.update(cx, |workspace, cx| {
                                workspace.remove_project(project, window, cx)
                            });
                        }
                        window.push_notification(
                            Notification::success(format!(
                                "Removed {} ({}).",
                                item.label,
                                format_size(item.size)
                            )),
                            cx,
                        );
                    }
                    Err(err) => window.push_notification(
                        Notification::error(format!("Could not remove {}: {err:#}", item.label)),
                        cx,
                    ),
                }
                this.measure(cx);
            });
        })
        .detach();
    }

    /// Picks the folder new copies go to.
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use for copies".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            _ = this.update_in(cx, |this, window, cx| {
                this.set_folder(Some(folder), window, cx)
            });
        })
        .detach();
    }

    fn set_folder(&mut self, folder: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        match ph_store::paths::set_copies_dir(folder.as_deref()) {
            Ok(()) => window.push_notification(
                Notification::info(
                    "New clones and copies go there. Those made before stay where they are until removed.",
                ),
                cx,
            ),
            Err(err) => window.push_notification(
                Notification::error(format!("Could not use that folder: {err}")),
                cx,
            ),
        }
        self.measure(cx);
    }

    fn render_item(&self, ix: usize, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.removing.contains(&item.path);
        let action = match item.removal {
            Removal::Folder | Removal::Task(_) => Some("Remove"),
            Removal::Contents if item.size > 0 => Some("Clear"),
            _ => None,
        };
        let entry = item.clone();
        h_flex()
            .gap_2()
            .py_1()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(item.label.clone()))
                    .when(!item.detail.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(cx.theme().muted_foreground)
                                .child(item.detail.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_size(item.size)),
            )
            .child(
                Button::new(("storage-open", ix))
                    .ghost()
                    .xsmall()
                    .icon(Icons::FolderOpen)
                    .tooltip(ph_ui::platform::reveal_label())
                    .accessibility_label("Show the folder")
                    .on_click({
                        let path = item.path.clone();
                        move |_, _, cx| cx.reveal_path(&path)
                    }),
            )
            .children(action.map(|label| {
                Button::new(("storage-remove", ix))
                    .xsmall()
                    .ghost()
                    .when(busy, |this| this.loading(true))
                    .label(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_remove(entry.clone(), window, cx)
                    }))
            }))
    }

    /// Asks before removing: a task's or copy's changes that were not
    /// pushed go with it.
    fn confirm_remove(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        if item.removal == Removal::Contents {
            self.remove(item, window, cx);
            return;
        }
        let message = match item.removal {
            Removal::Task(_) => format!(
                "Remove the task “{}”? Its folder is deleted with any changes not committed and pushed, and its workspace leaves PilotHouse. Its branch stays in the repository.",
                item.label
            ),
            _ => format!(
                "Remove {}? Its folder is deleted, with any changes not pushed. A review or task needing it again makes a new one.",
                item.label
            ),
        };
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (view, item) = (view.clone(), item.clone());
            dialog
                .title("Remove from disk")
                .child(div().text_sm().child(message.clone()))
                .footer(ph_ui::dialog_buttons("Remove", true))
                .on_ok(move |_, window, cx| {
                    _ = view.update(cx, |view, cx| view.remove(item.clone(), window, cx));
                    true
                })
        });
    }
}

impl Render for StorageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let data = ph_store::paths::data_dir();
        let copies = ph_store::paths::copies_dir();
        let total: u64 = self.groups.iter().flatten().map(Group::size).sum();
        let mut ix = 0;
        let groups: Vec<_> = self
            .groups
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter(|group| !group.items.is_empty())
            .map(|group| {
                let rows: Vec<_> = group
                    .items
                    .iter()
                    .map(|item| {
                        ix += 1;
                        self.render_item(ix, item, cx).into_any_element()
                    })
                    .collect();
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .child(group.title),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format_size(group.size())),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(group.note),
                    )
                    .children(rows)
            })
            .collect();
        v_flex()
            .w_full()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .child(format!("PilotHouse uses {} on disk", format_size(total))),
                            )
                            .when(self.measuring, |this| this.child(Spinner::new().xsmall()))
                            .child(
                                Button::new("storage-refresh")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icons::RotateCw)
                                    .label("Measure again")
                                    .on_click(cx.listener(|this, _, _, cx| this.measure(cx))),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Data: {}", ph_ui::platform::display_path(&data))),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(Icons::FolderInput).small())
                            .child(div().flex_1().text_sm().child("Folder for copies"))
                            .when(copies != data, |this| {
                                this.child(
                                    Button::new("copies-default")
                                        .ghost()
                                        .xsmall()
                                        .label("Use the data folder")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.set_folder(None, window, cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new("copies-choose")
                                    .xsmall()
                                    .label("Change…")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_folder(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "GitHub clones, pull request copies, tasks and review copies go to {}. Choose a folder on a bigger disk to keep them off this one.",
                                ph_ui::platform::display_path(&copies)
                            )),
                    ),
            )
            .when(self.groups.is_none(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Measuring…"),
                )
            })
            .children(groups)
    }
}

/// What PilotHouse keeps, measured. Blocking.
fn measure(store: &ph_store::Store) -> Vec<Group> {
    let data = ph_store::paths::data_dir();
    let mut clones = Vec::new();
    let mut pull_copies = Vec::new();
    for root in ph_pulls::clones::roots() {
        for repo in subfolders(&root)
            .into_iter()
            .flat_map(|host| subfolders(&host))
            .flat_map(|owner| subfolders(&owner))
        {
            let name = repo_name(&root, &repo);
            let clone = repo.join("repo");
            if clone.is_dir() {
                clones.push(Item {
                    label: name.clone(),
                    detail: "Clone, for reviews and fixes".into(),
                    size: size_of(&clone),
                    path: repo.clone(),
                    removal: Removal::Folder,
                });
            }
            for copy in subfolders(&repo) {
                let folder = copy.file_name().map(|n| n.to_string_lossy().into_owned());
                if let Some(number) = folder.as_deref().and_then(|n| n.strip_prefix("pr-")) {
                    pull_copies.push(Item {
                        label: format!("{name} #{number}"),
                        detail: "Pull request copy".into(),
                        size: size_of(&copy),
                        path: copy,
                        removal: Removal::Folder,
                    });
                }
            }
        }
    }
    // A clone's folder holds its pull request copies too: removing it
    // takes them with it.
    for clone in &mut clones {
        let copies = pull_copies
            .iter()
            .filter(|copy| copy.path.starts_with(&clone.path))
            .count();
        if copies > 0 {
            clone.detail = format!(
                "Clone, for reviews and fixes · removing it also removes its {copies} pull request {}",
                if copies == 1 { "copy" } else { "copies" }
            );
        }
    }
    let tasks: Vec<Item> = crate::tasks::all(store)
        .into_iter()
        .filter(|task| task.path.is_dir())
        .map(|task| {
            let name = store
                .project(task.project)
                .ok()
                .flatten()
                .map(|project| project.name)
                .unwrap_or_else(|| task.branch.clone());
            Item {
                label: name,
                detail: format!("Task worktree · branch {}", task.branch),
                size: size_of(&task.path),
                path: task.path,
                removal: Removal::Task(task.project),
            }
        })
        .collect();
    let task_paths: Vec<PathBuf> = tasks.iter().map(|task| task.path.clone()).collect();
    let mut copies = Vec::new();
    for folder in crate::copies::all_folders(store).into_iter().chain(
        ph_store::paths::copies_dirs()
            .into_iter()
            .map(|dir| dir.join("tasks")),
    ) {
        for copy in subfolders(&folder) {
            if task_paths.contains(&copy) {
                continue;
            }
            let name = copy
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            copies.push(Item {
                label: name,
                detail: if folder.ends_with("tasks") {
                    "A task's worktree whose workspace is gone".into()
                } else {
                    "Review or parallel-run copy".into()
                },
                size: size_of(&copy),
                path: copy,
                removal: Removal::Folder,
            });
        }
    }
    let other = |name: &str, label: &str, removal: Removal| {
        let path = data.join(name);
        path.exists().then(|| Item {
            label: label.into(),
            detail: String::new(),
            size: size_of(&path),
            path,
            removal,
        })
    };
    let kept = [
        other("recordings", "Screen recordings", Removal::Contents),
        other("captures", "Screenshots sent to chats", Removal::Contents),
        other("logs", "Logs", Removal::Contents),
        other(
            "browser",
            "Browser data (cookies, cache)",
            Removal::Contents,
        ),
        other("unsaved", "Unsaved edits kept after a quit", Removal::Never),
        other(
            "pilothouse.db",
            "Chats, boards and settings",
            Removal::Never,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    let sort = |mut items: Vec<Item>| {
        items.sort_by_key(|item| std::cmp::Reverse(item.size));
        items
    };
    vec![
        Group {
            title: "GitHub clones",
            note: "One per repository you reviewed in the GitHub view.",
            items: sort(clones),
        },
        Group {
            title: "Pull request copies",
            note: "Each reviewed or fixed pull request, checked out.",
            items: sort(pull_copies),
        },
        Group {
            title: "Tasks",
            note: "Worktrees of tasks; removing one also takes its workspace out of PilotHouse.",
            items: sort(tasks),
        },
        Group {
            title: "Review and run copies",
            note: "Copies agents worked in; old ones are tidied away on their own.",
            items: sort(copies),
        },
        Group {
            title: "Other data",
            note: "Recordings, screenshots, logs and the browser's data can be cleared.",
            items: kept,
        },
    ]
}

/// `host/owner/name` of a clone folder.
fn repo_name(root: &Path, repo: &Path) -> String {
    repo.strip_prefix(root)
        .map(|rest| {
            let parts: Vec<String> = rest
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            match parts.as_slice() {
                [host, owner, name] if host == "github.com" => format!("{owner}/{name}"),
                _ => parts.join("/"),
            }
        })
        .unwrap_or_default()
}

fn subfolders(folder: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// The bytes of `path` and all under it (links not followed).
fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Removes `path` as `removal` says. Blocking. Only folders under the
/// copies' and data folders are ever touched.
fn remove(store: &ph_store::Store, path: &Path, removal: &Removal) -> anyhow::Result<()> {
    let ours = ph_store::paths::copies_dirs()
        .iter()
        .chain([&ph_store::paths::data_dir()])
        .any(|folder| path.starts_with(folder) && path != folder);
    anyhow::ensure!(ours, "{} is not PilotHouse's", path.display());
    match removal {
        Removal::Never => anyhow::bail!("PilotHouse needs it"),
        Removal::Contents => {
            for entry in std::fs::read_dir(path)?.flatten() {
                let entry = entry.path();
                if entry.is_dir() {
                    std::fs::remove_dir_all(&entry)?;
                } else {
                    std::fs::remove_file(&entry)?;
                }
            }
            Ok(())
        }
        Removal::Folder => remove_copy(path),
        Removal::Task(project) => {
            remove_copy(path)?;
            crate::tasks::forget(store, *project)
        }
    }
}

/// A worktree through its repository (which then forgets it), anything
/// else (a clone) by deleting it.
fn remove_copy(path: &Path) -> anyhow::Result<()> {
    if let Some(entry) = crate::housekeeping::admin_entry(path) {
        // `<repo>/.git/worktrees/<name>`.
        if let Some(repo) = entry.parent().and_then(Path::parent).and_then(Path::parent)
            && ph_git::worktree::remove(repo, path, true).is_ok()
        {
            return Ok(());
        }
        std::fs::remove_dir_all(path)?;
        let _ = std::fs::remove_dir_all(entry);
        return Ok(());
    }
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// The space GitHub clones and their copies take (for the GitHub view).
/// Blocking.
pub fn clones_size() -> u64 {
    ph_pulls::clones::roots()
        .iter()
        .map(|root| size_of(root))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_add_up_and_names_read_well() {
        let dir = std::env::temp_dir().join(format!("ph-storage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/one"), [0u8; 100]).unwrap();
        std::fs::write(dir.join("a/b/two"), [0u8; 50]).unwrap();
        assert_eq!(size_of(&dir), 150);
        let root = Path::new("/data/github");
        assert_eq!(
            repo_name(root, &root.join("github.com/acme/app")),
            "acme/app"
        );
        assert_eq!(
            repo_name(root, &root.join("ghe.example.net/acme/app")),
            "ghe.example.net/acme/app"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_pilothouses_folders_are_removed() {
        let store = ph_store::Store::open_in_memory().unwrap();
        let outside = std::env::temp_dir().join("not-pilothouse");
        assert!(remove(&store, &outside, &Removal::Folder).is_err());
    }
}
