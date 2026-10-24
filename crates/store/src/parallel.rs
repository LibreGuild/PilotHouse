//! Parallel runs: one prompt given to several agents at once, each working
//! in its own worktree from the same snapshot of the project.

use anyhow::Result;
use rusqlite::params;

use crate::Store;
use crate::projects::ProjectId;
use crate::sessions::SessionId;
use crate::time::now_millis;

/// One agent's part of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    pub id: i64,
    pub agent_id: String,
    pub worktree: String,
    pub session_id: Option<SessionId>,
    /// `open`, `applied` (its changes were brought into the project) or
    /// `discarded` (its worktree was removed).
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParallelRun {
    pub id: i64,
    pub project_id: ProjectId,
    pub prompt: String,
    /// The snapshot commit every lane started from.
    pub base: String,
    pub created_at: i64,
    pub lanes: Vec<Lane>,
}

impl Store {
    pub fn create_parallel_run(
        &self,
        project_id: ProjectId,
        prompt: &str,
        base: &str,
    ) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO parallel_runs (project_id, prompt, base, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![project_id, prompt, base, now_millis()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn add_parallel_lane(&self, run_id: i64, agent_id: &str, worktree: &str) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO parallel_lanes (run_id, agent_id, worktree) VALUES (?1, ?2, ?3)",
            params![run_id, agent_id, worktree],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn set_lane_session(&self, lane_id: i64, session_id: SessionId) -> Result<()> {
        self.conn().execute(
            "UPDATE parallel_lanes SET session_id = ?2 WHERE id = ?1",
            params![lane_id, session_id],
        )?;
        Ok(())
    }

    pub fn set_lane_state(&self, lane_id: i64, state: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE parallel_lanes SET state = ?2 WHERE id = ?1",
            params![lane_id, state],
        )?;
        Ok(())
    }

    /// A project's runs, newest first, with their lanes.
    pub fn parallel_runs(&self, project_id: ProjectId) -> Result<Vec<ParallelRun>> {
        let conn = self.conn();
        let mut runs = conn
            .prepare(
                "SELECT id, project_id, prompt, base, created_at FROM parallel_runs
                 WHERE project_id = ?1 ORDER BY id DESC",
            )?
            .query_map(params![project_id], |row| {
                Ok(ParallelRun {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    prompt: row.get(2)?,
                    base: row.get(3)?,
                    created_at: row.get(4)?,
                    lanes: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut lanes = conn.prepare(
            "SELECT id, agent_id, worktree, session_id, state FROM parallel_lanes WHERE run_id = ?1 ORDER BY id",
        )?;
        for run in &mut runs {
            run.lanes = lanes
                .query_map(params![run.id], |row| {
                    Ok(Lane {
                        id: row.get(0)?,
                        agent_id: row.get(1)?,
                        worktree: row.get(2)?,
                        session_id: row.get(3)?,
                        state: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }
        Ok(runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_keep_their_lanes() {
        let store = Store::open_in_memory().unwrap();
        let folder = std::env::temp_dir().join(format!("ph-parallel-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let project = store.add_project(&folder).unwrap();
        let run = store
            .create_parallel_run(project.id, "Add a cache", "abc123")
            .unwrap();
        let claude = store.add_parallel_lane(run, "claude", "/tmp/w1").unwrap();
        store.add_parallel_lane(run, "codex", "/tmp/w2").unwrap();
        let session = store
            .create_session(project.id, "Add a cache · Claude", "claude")
            .unwrap();
        store.set_lane_session(claude, session.id).unwrap();
        store.set_lane_state(claude, "applied").unwrap();

        let runs = store.parallel_runs(project.id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            (runs[0].prompt.as_str(), runs[0].base.as_str()),
            ("Add a cache", "abc123")
        );
        let lanes = &runs[0].lanes;
        assert_eq!(lanes.len(), 2);
        assert_eq!(
            (lanes[0].session_id, lanes[0].state.as_str()),
            (Some(session.id), "applied")
        );
        assert_eq!(
            (lanes[1].agent_id.as_str(), lanes[1].state.as_str()),
            ("codex", "open")
        );

        store.delete_session(session.id).unwrap();
        assert_eq!(
            store.parallel_runs(project.id).unwrap()[0].lanes[0].session_id,
            None
        );
    }
}
