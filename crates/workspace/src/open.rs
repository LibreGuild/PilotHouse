//! Opening folders and files from outside the app: `pilothouse <path>`,
//! Finder's or Explorer's "Open with PilotHouse", or a second launch that
//! forwards its paths to the running app.
//!
//! - A folder opens as a workspace, reusing a workspace that has it as one
//!   of its folders.
//! - A file inside a workspace's folder opens in that workspace's Files tab.
//! - Any other file opens on its own in a small window
//!   ([`file_window`](crate::file_window)), like a notepad.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{AnyWindowHandle, App, Entity, Global, WeakEntity};
use ph_store::paths;
use ph_store::projects::ProjectId;

use crate::workspace::Workspace;
use crate::{AppState, file_window};

/// The main window and its workspace.
struct MainWindow {
    handle: AnyWindowHandle,
    workspace: WeakEntity<Workspace>,
}

impl Global for MainWindow {}

/// Records the main window, once it is open.
pub fn set_main_window(handle: AnyWindowHandle, workspace: &Entity<Workspace>, cx: &mut App) {
    cx.set_global(MainWindow {
        handle,
        workspace: workspace.downgrade(),
    });
}

/// The main window's workspace; once the main window has closed, another
/// open workspace window stands in for it.
pub fn main_workspace(cx: &App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    cx.try_global::<MainWindow>()
        .and_then(|main| Some((main.handle, main.workspace.upgrade()?)))
        .or_else(|| crate::windows::first_open(cx))
}

/// What a path is to PilotHouse.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// A folder that is not in a workspace yet: open it as one.
    Folder(PathBuf),
    /// A workspace that has the folder.
    Workspace(ProjectId),
    /// A file in a workspace's folder.
    ProjectFile { project: ProjectId, path: PathBuf },
    /// A file outside every workspace.
    LoneFile(PathBuf),
}

/// Sorts `path` (canonical) against the workspaces' folders (`projects`:
/// each workspace's folders, most recently opened first). The deepest folder
/// holding a file wins; a folder must be one of a workspace's own.
pub fn classify(path: &Path, projects: &[(ProjectId, Vec<PathBuf>)]) -> Target {
    if path.is_dir() {
        return projects
            .iter()
            .find(|(_, folders)| folders.iter().any(|folder| folder == path))
            .map_or_else(
                || Target::Folder(path.to_path_buf()),
                |(id, _)| Target::Workspace(*id),
            );
    }
    let owner = projects
        .iter()
        .filter_map(|(id, folders)| {
            let depth = folders
                .iter()
                .filter(|folder| path.starts_with(folder))
                .map(|folder| folder.as_os_str().len())
                .max()?;
            Some((depth, *id))
        })
        .max_by_key(|(depth, _)| *depth);
    match owner {
        Some((_, project)) => Target::ProjectFile {
            project,
            path: path.to_path_buf(),
        },
        None => Target::LoneFile(path.to_path_buf()),
    }
}

/// Opens each path (see the module docs) and brings PilotHouse forward.
pub fn open_paths(paths: &[PathBuf], cx: &mut App) -> Result<()> {
    let mut failures = Vec::new();
    for path in paths {
        // Read for each path: an earlier one may have added a project.
        let projects: Vec<(ProjectId, Vec<PathBuf>)> = AppState::store(cx)
            .projects()?
            .into_iter()
            .filter(|project| project.is_workspace())
            .map(|project| (project.id, project.folders()))
            .collect();
        let Ok(path) = paths::canonical(path) else {
            failures.push(format!("{} does not exist", path.display()));
            continue;
        };
        let result = match classify(&path, &projects) {
            Target::LoneFile(path) => file_window::open(path, cx),
            target => in_main_window(target, cx),
        };
        if let Err(err) = result {
            failures.push(format!("{err:#}"));
        }
    }
    cx.activate(true);
    match failures.is_empty() {
        true => Ok(()),
        false => Err(anyhow!(failures.join("; "))),
    }
}

fn in_main_window(target: Target, cx: &mut App) -> Result<()> {
    let (handle, workspace) = main_workspace(cx).context("the main window is not open")?;
    handle
        .update(cx, |_, window, cx| {
            window.activate_window();
            workspace.update(cx, |workspace, cx| match target {
                Target::Folder(path) => workspace.add_project_path(&path, window, cx),
                Target::Workspace(project) => workspace.open_project(project, window, cx),
                Target::ProjectFile { project, path } => {
                    workspace.open_project_file(project, &path, window, cx)
                }
                Target::LoneFile(_) => unreachable!("lone files get their own window"),
            })
        })
        .map_err(|err| anyhow!("{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_open_in_the_workspace_that_has_them() {
        let root = std::env::temp_dir().join(format!("ph-open-{}", std::process::id()));
        let (client, server) = (root.join("client"), root.join("server"));
        std::fs::create_dir_all(server.join("src")).unwrap();
        std::fs::create_dir_all(&client).unwrap();
        std::fs::write(server.join("src/main.rs"), "").unwrap();
        let elsewhere =
            std::env::temp_dir().join(format!("ph-open-lone-{}.log", std::process::id()));
        std::fs::write(&elsewhere, "").unwrap();
        let projects = vec![
            (1, vec![client.clone(), server.clone()]),
            (2, vec![server.join("src")]),
        ];

        assert_eq!(
            classify(&server, &projects),
            Target::Workspace(1),
            "an added folder opens its workspace"
        );
        assert_eq!(classify(&root, &projects), Target::Folder(root.clone()));
        assert_eq!(
            classify(&server.join("src/main.rs"), &projects),
            Target::ProjectFile {
                project: 2,
                path: server.join("src/main.rs")
            },
            "the deepest folder wins"
        );
        assert_eq!(
            classify(&elsewhere, &projects),
            Target::LoneFile(elsewhere.clone())
        );
    }
}
