//! Chat sessions and their transcripts.
//!
//! The store keeps transcript entries opaque: `role` is a short tag and `body`
//! is JSON owned by the agent crate. That keeps the chat model free to evolve
//! without schema changes here.

use anyhow::Result;
use rusqlite::{OptionalExtension as _, Row, params};

use crate::projects::ProjectId;
use crate::{Store, now_millis};

pub type SessionId = i64;

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub title: String,
    /// Which agent the session talks to, e.g. `claude`, `codex`, `gemini`.
    pub agent_id: String,
    /// The agent's own session id, for resuming (ACP `session/load`).
    pub acp_session_id: Option<String>,
    pub updated_at: i64,
    /// Where the agent works when not the project folder (a review worktree).
    pub cwd: Option<String>,
    /// `chat`, or `review` for a pull request review.
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    pub id: i64,
    pub role: String,
    pub body: String,
    pub created_at: i64,
}

impl Session {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            title: row.get("title")?,
            agent_id: row.get("agent_id")?,
            acp_session_id: row.get("acp_session_id")?,
            updated_at: row.get("updated_at")?,
            cwd: row.get("cwd")?,
            kind: row.get("kind")?,
        })
    }
}

impl Store {
    pub fn create_session(
        &self,
        project_id: ProjectId,
        title: &str,
        agent_id: &str,
    ) -> Result<Session> {
        let now = now_millis();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO sessions (project_id, title, agent_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![project_id, title, agent_id, now],
        )?;
        let id = conn.last_insert_rowid();
        Ok(Session {
            id,
            project_id,
            title: title.to_string(),
            agent_id: agent_id.to_string(),
            acp_session_id: None,
            updated_at: now,
            cwd: None,
            kind: "chat".into(),
        })
    }

    /// Marks a session as running in `cwd` (e.g. a review worktree).
    pub fn set_session_place(&self, id: SessionId, kind: &str, cwd: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET kind = ?2, cwd = ?3 WHERE id = ?1",
            params![id, kind, cwd],
        )?;
        Ok(())
    }

    /// A project's sessions, most recently active first.
    pub fn sessions(&self, project_id: ProjectId) -> Result<Vec<Session>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT * FROM sessions WHERE project_id = ?1 ORDER BY updated_at DESC, id DESC",
        )?;
        let sessions = statement
            .query_map(params![project_id], Session::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(sessions)
    }

    pub fn session(&self, id: SessionId) -> Result<Option<Session>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM sessions WHERE id = ?1",
                params![id],
                Session::from_row,
            )
            .optional()?)
    }

    pub fn rename_session(&self, id: SessionId, title: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        Ok(())
    }

    /// Remembers the agent-side session id so the chat can be resumed.
    pub fn set_session_acp_id(&self, id: SessionId, acp_session_id: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET acp_session_id = ?2 WHERE id = ?1",
            params![id, acp_session_id],
        )?;
        Ok(())
    }

    /// Switches the agent a session talks to (before its first message).
    pub fn set_session_agent(&self, id: SessionId, agent_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET agent_id = ?2 WHERE id = ?1",
            params![id, agent_id],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, id: SessionId) -> Result<()> {
        self.conn()
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Appends one transcript entry and marks the session as recently active.
    pub fn append_message(&self, session_id: SessionId, role: &str, body: &str) -> Result<i64> {
        let now = now_millis();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO messages (session_id, role, body, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, role, body, now],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
            params![session_id, now],
        )?;
        Ok(id)
    }

    /// Replaces the body of an entry that was still streaming when first saved.
    pub fn update_message(&self, id: i64, body: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE messages SET body = ?2 WHERE id = ?1",
            params![id, body],
        )?;
        Ok(())
    }

    pub fn messages(&self, session_id: SessionId) -> Result<Vec<StoredMessage>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT id, role, body, created_at FROM messages WHERE session_id = ?1 ORDER BY id",
        )?;
        let messages = statement
            .query_map(params![session_id], |row| {
                Ok(StoredMessage {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(store: &Store) -> ProjectId {
        let dir = std::env::temp_dir().join(format!("ph-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        store.add_project(&dir).unwrap().id
    }

    #[test]
    fn transcript_keeps_order_and_bumps_session() {
        let store = Store::open_in_memory().unwrap();
        let session = store
            .create_session(project(&store), "Fix login", "claude")
            .unwrap();
        let first = store
            .append_message(session.id, "user", r#"{"text":"hi"}"#)
            .unwrap();
        store
            .append_message(session.id, "agent", r#"{"text":"hello"}"#)
            .unwrap();
        store
            .update_message(first, r#"{"text":"hi there"}"#)
            .unwrap();

        let messages = store.messages(session.id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].body, r#"{"text":"hi there"}"#);
        assert!(store.session(session.id).unwrap().unwrap().updated_at >= session.updated_at);
    }

    #[test]
    fn deleting_a_project_removes_its_sessions() {
        let store = Store::open_in_memory().unwrap();
        let project_id = project(&store);
        let session = store.create_session(project_id, "Chat", "codex").unwrap();
        store.append_message(session.id, "user", "{}").unwrap();
        store.remove_project(project_id).unwrap();
        assert!(store.session(session.id).unwrap().is_none());
        assert!(store.messages(session.id).unwrap().is_empty());
    }
}
