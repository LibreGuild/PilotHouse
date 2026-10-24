//! Tidying PilotHouse's data folder at startup, in the background, so copies
//! made for agents do not fill the disk:
//!
//! - a pull request's review copy (`worktrees/<workspace>-pr-<n>`) unused
//!   for [`UNUSED`]; reviewing again makes a new one;
//! - a parallel run's copy that was kept or discarded and is older than
//!   [`UNUSED`] (open ones stay until the user decides);
//! - any copy whose workspace no longer exists.
//!
//! Copies are removed with `git worktree remove`, so their repository
//! forgets them too; one whose repository is gone is deleted directly. Only
//! folders inside `<data dir>/worktrees` are ever removed.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ph_store::Store;

/// How long a finished copy is kept.
const UNUSED: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// What one copy is, from its folder's name.
#[derive(Debug, PartialEq, Eq)]
enum Copy {
    Review {
        project: i64,
    },
    Lane {
        project: i64,
        run: i64,
        agent: String,
    },
}

fn classify(name: &str) -> Option<Copy> {
    let (project, rest) = name.split_once('-')?;
    let project = project.parse().ok()?;
    if let Some(number) = rest.strip_prefix("pr-") {
        number.parse::<u64>().ok()?;
        return Some(Copy::Review { project });
    }
    let rest = rest.strip_prefix("run")?;
    let (run, agent) = rest.split_once('-')?;
    Some(Copy::Lane {
        project,
        run: run.parse().ok()?,
        agent: agent.to_string(),
    })
}

/// Removes what is no longer needed; returns how many copies went.
///
/// Only with the data folder's own database: with any other (an in-memory
/// one in tests), every copy would look like it had lost its workspace.
pub fn tidy(store: &Store) -> usize {
    let data = ph_store::paths::data_dir();
    let own = store
        .file()
        .and_then(|file| ph_store::paths::canonical(&file).ok())
        .zip(ph_store::paths::canonical(&data).ok())
        .is_some_and(|(file, data)| file.parent() == Some(data.as_path()));
    if !own {
        return 0;
    }
    let folder = data.join("worktrees");
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return 0;
    };
    let projects = store.projects().unwrap_or_default();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(copy) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(classify)
        else {
            continue;
        };
        let age = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .unwrap_or_default();
        let project_id = match &copy {
            Copy::Review { project } | Copy::Lane { project, .. } => *project,
        };
        let root = projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| project.path.clone());
        let stale = match (&copy, &root) {
            (_, None) => true,
            (Copy::Review { .. }, Some(_)) => age > UNUSED,
            (
                Copy::Lane {
                    project,
                    run,
                    agent,
                },
                Some(_),
            ) => age > UNUSED && lane_finished(store, *project, *run, agent, &path),
        };
        if stale && remove(&folder, root.as_deref(), &path) {
            removed += 1;
        }
    }
    removed
}

/// The lane of `run` (in `project`) for `agent` at `path` was kept or
/// discarded, or its run is gone.
fn lane_finished(store: &Store, project: i64, run: i64, agent: &str, path: &Path) -> bool {
    store
        .parallel_runs(project)
        .unwrap_or_default()
        .into_iter()
        .find(|known| known.id == run)
        .and_then(|known| {
            known
                .lanes
                .into_iter()
                .find(|lane| lane.agent_id == agent || Path::new(&lane.worktree) == path)
        })
        .is_none_or(|lane| lane.state != "open")
}

/// Removes the copy at `path` (inside `folder`), through its repository at
/// `root` when there is one.
fn remove(folder: &Path, root: Option<&Path>, path: &PathBuf) -> bool {
    if !path.starts_with(folder) || path == folder {
        return false;
    }
    if let Some(root) = root
        && ph_git::worktree::remove(root, path, true).is_ok()
    {
        return true;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!("could not remove {}: {err}", path.display());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_known_by_their_names() {
        assert_eq!(classify("2-pr-12"), Some(Copy::Review { project: 2 }));
        assert_eq!(
            classify("3-run7-codex"),
            Some(Copy::Lane {
                project: 3,
                run: 7,
                agent: "codex".into()
            })
        );
        assert_eq!(classify("notes"), None);
        assert_eq!(classify("2-pr-x"), None);
        assert!(
            !remove(
                Path::new("/data/worktrees"),
                None,
                &PathBuf::from("/elsewhere/x")
            ),
            "only inside the folder"
        );
        let memory = Store::open_in_memory().unwrap();
        assert_eq!(
            tidy(&memory),
            0,
            "never with a database that is not the data folder's"
        );
    }
}
