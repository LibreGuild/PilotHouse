//! Workspaces (called projects in the code): a primary folder, more
//! folders added to it, or none at all.
//!
//! The primary folder (`path`) is where agents work and what the Git,
//! terminal and run tools start from; the other folders are added roots
//! (shown in Files, given to agents as extra directories, each with its own
//! Git tab). A workspace without folders works in a private scratch folder
//! in PilotHouse's data folder, so chat, the browser and devices still work.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rusqlite::{OptionalExtension as _, Row, params};
use serde::{Deserialize, Serialize};

use crate::{Store, now_millis};

pub type ProjectId = i64;

/// A workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// The primary folder, or the scratch folder of a workspace without
    /// folders.
    pub path: PathBuf,
    pub last_opened_at: i64,
    /// The folders added after the primary one, in order.
    pub extra_folders: Vec<PathBuf>,
    /// No folders: `path` is a scratch folder.
    pub empty: bool,
    /// The colour the user chose (an index into the UI's palette); `None`
    /// means one picked from the id, see [`Project::shade`].
    pub color: Option<u8>,
}

/// Per-project layout restored when the project is reopened.
///
/// Unknown fields are ignored and missing ones take their defaults, so the
/// shape can grow without a migration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectUiState {
    /// The tool that was open (`changes`, `board`, `terminal`, …).
    pub context_tab: Option<String>,
    /// Whether the context panel was visible.
    pub context_panel_open: Option<bool>,
    /// The chat session that was selected.
    pub session_id: Option<i64>,
    /// Files open in the viewer, relative to the project root.
    pub open_files: Vec<String>,
    /// The file that was focused in the viewer.
    pub active_file: Option<String>,
    /// The sidebar's Files section is folded.
    pub files_folded: bool,
    /// The sidebar's Chats section is folded.
    pub chats_folded: bool,
    /// The chats PilotHouse started (schedules, tests, reviews) are listed.
    pub runs_open: bool,
    /// Height of the sidebar's Files section (pixels) when Chats is open
    /// below it, as last dragged.
    pub files_height: Option<f32>,
}

impl Project {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            path: PathBuf::from(row.get::<_, String>("path")?),
            last_opened_at: row.get("last_opened_at")?,
            extra_folders: Vec::new(),
            empty: row.get::<_, String>("kind")? == "empty",
            color: row.get("color")?,
        })
    }

    /// The workspace's colour as an index below `palette_len`: the chosen
    /// one, else one from the id (so neighbours differ).
    pub fn shade(&self, palette_len: usize) -> usize {
        let len = palette_len.max(1);
        match self.color {
            Some(color) => usize::from(color) % len,
            None => (self.id.unsigned_abs() as usize).wrapping_sub(1) % len,
        }
    }

    /// Every folder shown: the primary (or scratch) folder, then the others.
    pub fn roots(&self) -> Vec<PathBuf> {
        std::iter::once(self.path.clone())
            .chain(self.extra_folders.iter().cloned())
            .collect()
    }

    /// The folders the user added (no scratch folder).
    pub fn folders(&self) -> Vec<PathBuf> {
        if self.empty {
            self.extra_folders.clone()
        } else {
            self.roots()
        }
    }

    /// Whether `path` is one of the workspace's folders or inside one.
    pub fn contains(&self, path: &Path) -> bool {
        self.folders().iter().any(|folder| path.starts_with(folder))
    }
}

/// A new, private, empty folder under `root`.
fn new_scratch(root: &Path) -> Result<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let scratch = root.join(format!(
        "{}-{}",
        now_millis(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    crate::paths::ensure_private_dir(&scratch)?;
    Ok(crate::paths::canonical(&scratch)?)
}

impl Store {
    /// Reads a workspace's extra folders into it.
    fn with_folders(&self, mut project: Project) -> Result<Project> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT path FROM workspace_folders WHERE project_id = ?1 ORDER BY position, path",
        )?;
        project.extra_folders = statement
            .query_map(params![project.id], |row| {
                row.get::<_, String>(0).map(PathBuf::from)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(project)
    }

    /// A workspace without folders yet, named `name`; it works in a scratch
    /// folder of its own, made under `scratch_root` (the data folder's
    /// `workspaces/`, see [`crate::paths::scratch_root`]).
    pub fn create_empty_workspace(&self, name: &str, scratch_root: &Path) -> Result<Project> {
        let now = now_millis();
        let scratch = new_scratch(scratch_root)?;
        let id = {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO projects (name, path, created_at, last_opened_at, kind)
                 VALUES (?1, ?2, ?3, ?3, 'empty')",
                params![name, scratch.to_string_lossy(), now],
            )?;
            conn.last_insert_rowid()
        };
        self.project(id)?.context("just created")
    }

    /// Adds a folder to a workspace. A workspace without folders makes it
    /// its primary folder (unless another workspace has it as its primary,
    /// in which case it is added beside the scratch folder).
    pub fn add_workspace_folder(&self, id: ProjectId, path: &Path) -> Result<Project> {
        let path = crate::paths::canonical(path)
            .with_context(|| format!("resolving {}", path.display()))?;
        if !path.is_dir() {
            bail!("{} is not a folder", path.display());
        }
        let project = self.project(id)?.context("no such workspace")?;
        if project.roots().contains(&path) {
            return Ok(project);
        }
        let text = path.to_string_lossy().to_string();
        let taken: bool = self.conn().query_row(
            "SELECT count(*) > 0 FROM projects WHERE path = ?1",
            params![text],
            |row| row.get(0),
        )?;
        if project.empty && !taken {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| text.clone());
            // A workspace still called by its default name takes the folder's.
            let rename = project.name.starts_with("Workspace");
            self.conn().execute(
                "UPDATE projects SET path = ?2, kind = 'folder', name = CASE WHEN ?4 THEN ?3 ELSE name END
                 WHERE id = ?1",
                params![id, text, name, rename],
            )?;
        } else {
            self.conn().execute(
                "INSERT INTO workspace_folders (project_id, path, position)
                 VALUES (?1, ?2, (SELECT coalesce(max(position), 0) + 1 FROM workspace_folders WHERE project_id = ?1))",
                params![id, text],
            )?;
        }
        self.project(id)?.context("no such workspace")
    }

    /// Takes a folder out of a workspace (the folder on disk is untouched).
    /// Removing the primary folder makes the next one primary, or leaves
    /// the workspace without folders.
    pub fn remove_workspace_folder(
        &self,
        id: ProjectId,
        path: &Path,
        scratch_root: &Path,
    ) -> Result<Project> {
        let project = self.project(id)?.context("no such workspace")?;
        let text = path.to_string_lossy().to_string();
        if project.path == path && !project.empty {
            match project.extra_folders.first() {
                Some(next) => {
                    let next = next.to_string_lossy().to_string();
                    let conn = self.conn();
                    conn.execute(
                        "DELETE FROM workspace_folders WHERE project_id = ?1 AND path = ?2",
                        params![id, next],
                    )?;
                    conn.execute(
                        "UPDATE projects SET path = ?2 WHERE id = ?1",
                        params![id, next],
                    )?;
                }
                None => {
                    let scratch = new_scratch(scratch_root)?;
                    self.conn().execute(
                        "UPDATE projects SET path = ?2, kind = 'empty' WHERE id = ?1",
                        params![id, scratch.to_string_lossy()],
                    )?;
                }
            }
        } else {
            self.conn().execute(
                "DELETE FROM workspace_folders WHERE project_id = ?1 AND path = ?2",
                params![id, text],
            )?;
        }
        self.project(id)?.context("no such workspace")
    }

    /// Makes `path`, one of the workspace's other folders, its primary one
    /// (where agents and terminals start); the old primary takes its place.
    pub fn make_primary_folder(&self, id: ProjectId, path: &Path) -> Result<Project> {
        let project = self.project(id)?.context("no such workspace")?;
        if !project.extra_folders.iter().any(|folder| folder == path) {
            bail!(
                "{} is not one of this workspace's other folders",
                path.display()
            );
        }
        if project.empty {
            bail!("a workspace without folders has no primary folder to swap");
        }
        if self
            .projects()?
            .iter()
            .any(|other| other.id != id && other.path == path)
        {
            bail!(
                "{} is another workspace's primary folder, so it cannot be this one's too",
                path.display()
            );
        }
        let (new, old) = (
            path.to_string_lossy().to_string(),
            project.path.to_string_lossy().to_string(),
        );
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        // The old primary takes the new one's place in the order.
        let position: i64 = tx.query_row(
            "SELECT position FROM workspace_folders WHERE project_id = ?1 AND path = ?2",
            params![id, new],
            |row| row.get(0),
        )?;
        tx.execute(
            "DELETE FROM workspace_folders WHERE project_id = ?1 AND path = ?2",
            params![id, new],
        )?;
        tx.execute(
            "UPDATE projects SET path = ?2 WHERE id = ?1",
            params![id, new],
        )?;
        tx.execute(
            "INSERT INTO workspace_folders (project_id, path, position) VALUES (?1, ?2, ?3)",
            params![id, old, position],
        )?;
        tx.commit()?;
        drop(conn);
        self.project(id)?.context("no such workspace")
    }

    /// The workspaces that have `path` as one of their folders (or inside
    /// one), the deepest folder first, then the most recently opened.
    pub fn workspaces_containing(&self, path: &Path) -> Result<Vec<Project>> {
        let mut found: Vec<(usize, Project)> = self
            .projects()?
            .into_iter()
            .filter_map(|project| {
                let depth = project
                    .folders()
                    .iter()
                    .filter(|folder| path.starts_with(folder))
                    .map(|folder| folder.as_os_str().len())
                    .max()?;
                Some((depth, project))
            })
            .collect();
        found.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.last_opened_at.cmp(&a.1.last_opened_at))
        });
        Ok(found.into_iter().map(|(_, project)| project).collect())
    }

    /// Registers `path` as a project, or returns the existing one for it.
    pub fn add_project(&self, path: &Path) -> Result<Project> {
        let path = crate::paths::canonical(path)
            .with_context(|| format!("resolving {}", path.display()))?;
        if !path.is_dir() {
            bail!("{} is not a folder", path.display());
        }
        let path_text = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path_text.clone());
        let now = now_millis();

        let conn = self.conn();
        conn.execute(
            "INSERT INTO projects (name, path, created_at, last_opened_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(path) DO UPDATE SET last_opened_at = excluded.last_opened_at",
            params![name, path_text, now],
        )?;
        let project = conn.query_row(
            "SELECT * FROM projects WHERE path = ?1",
            params![path_text],
            Project::from_row,
        )?;
        drop(conn);
        self.with_folders(project)
    }

    /// All projects, most recently opened first.
    pub fn projects(&self) -> Result<Vec<Project>> {
        let conn = self.conn();
        let mut statement =
            conn.prepare("SELECT * FROM projects ORDER BY last_opened_at DESC, id DESC")?;
        let projects = statement
            .query_map([], Project::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(conn);
        projects
            .into_iter()
            .map(|project| self.with_folders(project))
            .collect()
    }

    pub fn project(&self, id: ProjectId) -> Result<Option<Project>> {
        let project = self
            .conn()
            .query_row(
                "SELECT * FROM projects WHERE id = ?1",
                params![id],
                Project::from_row,
            )
            .optional()?;
        project
            .map(|project| self.with_folders(project))
            .transpose()
    }

    pub fn touch_project(&self, id: ProjectId) -> Result<()> {
        self.conn().execute(
            "UPDATE projects SET last_opened_at = ?2 WHERE id = ?1",
            params![id, now_millis()],
        )?;
        Ok(())
    }

    pub fn rename_project(&self, id: ProjectId, name: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE projects SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    /// Sets the workspace's colour (`None`: the one from its id).
    pub fn set_project_color(&self, id: ProjectId, color: Option<u8>) -> Result<()> {
        self.conn().execute(
            "UPDATE projects SET color = ?2 WHERE id = ?1",
            params![id, color],
        )?;
        Ok(())
    }

    /// Removes the project from PilotHouse. The folder on disk is untouched.
    pub fn remove_project(&self, id: ProjectId) -> Result<()> {
        self.conn()
            .execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn project_ui_state(&self, id: ProjectId) -> Result<ProjectUiState> {
        let raw: Option<String> = self
            .conn()
            .query_row(
                "SELECT ui_state FROM projects WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        // A corrupt layout must never stop a project from opening.
        Ok(raw
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default())
    }

    pub fn save_project_ui_state(&self, id: ProjectId, state: &ProjectUiState) -> Result<()> {
        let raw = serde_json::to_string(state)?;
        self.conn().execute(
            "UPDATE projects SET ui_state = ?2 WHERE id = ?1",
            params![id, raw],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ph-store-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn adding_the_same_folder_twice_returns_one_project() {
        let store = Store::open_in_memory().unwrap();
        let folder = temp_folder("dup");
        let first = store.add_project(&folder).unwrap();
        let second = store.add_project(&folder).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(store.projects().unwrap().len(), 1);
    }

    #[test]
    fn workspaces_keep_a_colour() {
        let store = Store::open_in_memory().unwrap();
        let first = store.add_project(&temp_folder("colour-a")).unwrap();
        let second = store.add_project(&temp_folder("colour-b")).unwrap();
        assert_ne!(first.shade(10), second.shade(10), "neighbours differ");
        store.set_project_color(first.id, Some(13)).unwrap();
        let first = store.project(first.id).unwrap().unwrap();
        assert_eq!(first.color, Some(13));
        assert_eq!(first.shade(10), 3, "wraps into the palette");
        store.set_project_color(first.id, None).unwrap();
        assert_eq!(store.project(first.id).unwrap().unwrap().color, None);
    }

    #[test]
    fn workspaces_gain_and_lose_folders() {
        let store = Store::open_in_memory().unwrap();
        let client = temp_folder("ws-client");
        let server = temp_folder("ws-server");

        let empty = store
            .create_empty_workspace("Workspace 1", &temp_folder("ws-scratch"))
            .unwrap();
        assert!(empty.empty && empty.path.is_dir());
        assert!(empty.folders().is_empty());

        // The first folder becomes the primary one and names the workspace.
        let ws = store.add_workspace_folder(empty.id, &client).unwrap();
        assert!(!ws.empty);
        assert_eq!(ws.path, crate::paths::canonical(&client).unwrap());
        assert_eq!(ws.name, ws.path.file_name().unwrap().to_string_lossy());
        let ws = store.add_workspace_folder(ws.id, &server).unwrap();
        assert_eq!(ws.roots().len(), 2);
        assert_eq!(
            store
                .add_workspace_folder(ws.id, &server)
                .unwrap()
                .roots()
                .len(),
            2,
            "no duplicates"
        );
        assert!(ws.contains(&crate::paths::canonical(&server).unwrap().join("src")));
        assert_eq!(
            store
                .workspaces_containing(&crate::paths::canonical(&server).unwrap())
                .unwrap()[0]
                .id,
            ws.id
        );

        // Removing the primary promotes the next; removing the last leaves none.
        let ws = store
            .remove_workspace_folder(ws.id, &ws.path.clone(), &temp_folder("ws-scratch"))
            .unwrap();
        assert_eq!(ws.path, crate::paths::canonical(&server).unwrap());
        assert!(ws.extra_folders.is_empty());
        let ws = store
            .remove_workspace_folder(ws.id, &ws.path.clone(), &temp_folder("ws-scratch"))
            .unwrap();
        assert!(ws.empty && ws.folders().is_empty());
        assert!(
            client.is_dir() && server.is_dir(),
            "folders on disk are untouched"
        );
    }

    #[test]
    fn a_folder_that_is_another_workspace_is_added_beside() {
        let store = Store::open_in_memory().unwrap();
        let shared = temp_folder("ws-shared");
        store.add_project(&shared).unwrap();
        let empty = store
            .create_empty_workspace("Workspace 2", &temp_folder("ws-scratch"))
            .unwrap();
        let ws = store.add_workspace_folder(empty.id, &shared).unwrap();
        assert!(
            ws.empty,
            "the other workspace keeps it as its primary folder"
        );
        assert_eq!(
            ws.folders(),
            vec![crate::paths::canonical(&shared).unwrap()]
        );
    }

    #[test]
    fn another_folder_can_become_the_primary_one() {
        let store = Store::open_in_memory().unwrap();
        let (client, server) = (temp_folder("mp-client"), temp_folder("mp-server"));
        let ws = store.add_project(&client).unwrap();
        let ws = store.add_workspace_folder(ws.id, &server).unwrap();
        let server = crate::paths::canonical(&server).unwrap();
        let client = crate::paths::canonical(&client).unwrap();
        let swapped = store.make_primary_folder(ws.id, &server).unwrap();
        assert_eq!(swapped.path, server);
        assert_eq!(swapped.extra_folders, vec![client.clone()]);
        assert!(
            store.make_primary_folder(ws.id, &server).is_err(),
            "already primary"
        );
        let other = store.add_project(&temp_folder("mp-other")).unwrap();
        let ws = store.add_workspace_folder(swapped.id, &other.path).unwrap();
        assert!(
            store.make_primary_folder(ws.id, &other.path).is_err() || ws.extra_folders.is_empty(),
            "another workspace's primary folder stays its own"
        );
    }

    #[test]
    fn ui_state_round_trips_and_tolerates_garbage() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&temp_folder("ui")).unwrap();
        assert_eq!(
            store.project_ui_state(project.id).unwrap(),
            ProjectUiState::default()
        );

        let state = ProjectUiState {
            context_tab: Some("board".into()),
            open_files: vec!["src/main.rs".into()],
            ..Default::default()
        };
        store.save_project_ui_state(project.id, &state).unwrap();
        assert_eq!(store.project_ui_state(project.id).unwrap(), state);

        store
            .conn()
            .execute(
                "UPDATE projects SET ui_state = 'not json' WHERE id = ?1",
                params![project.id],
            )
            .unwrap();
        assert_eq!(
            store.project_ui_state(project.id).unwrap(),
            ProjectUiState::default()
        );
    }

    #[test]
    fn adding_a_file_is_rejected() {
        let store = Store::open_in_memory().unwrap();
        let file = temp_folder("file").join("note.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(store.add_project(&file).is_err());
    }
}
