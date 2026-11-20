//! Explanations of changes that an agent wrote (see `ph_review::explanation`),
//! kept on this computer until the user shares one as a file in the
//! repository.
//!
//! An explanation is filed under the content it explains: each file's path
//! and the Git blob id of its content then. So the explanation of a commit
//! is found by what the commit put in the repository, and no commit id is
//! kept that a rebase or a squash would break.

use anyhow::Result;
use rusqlite::{OptionalExtension as _, Row, params, params_from_iter};

use crate::Store;
use crate::sessions::SessionId;
use crate::time::now_millis;

#[derive(Debug, Clone, PartialEq)]
pub struct Explanation {
    pub id: i64,
    /// The folder of the repository it was written in.
    pub root: String,
    /// What was explained: `changes` (the work not committed then) or a
    /// commit's id.
    pub subject: String,
    pub title: String,
    /// Markdown; empty until the agent finished.
    pub markdown: String,
    /// The agent that wrote it (its name).
    pub agent: String,
    /// The chat it was written in.
    pub session_id: Option<SessionId>,
    /// What was asked in the chats that made the change, as JSON owned by
    /// the review crate.
    pub asked: String,
    pub finished: bool,
    pub created_at: i64,
    /// The files it explains, each with the blob id of its content.
    pub files: Vec<(String, String)>,
}

impl Explanation {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            root: row.get("root")?,
            subject: row.get("subject")?,
            title: row.get("title")?,
            markdown: row.get("markdown")?,
            agent: row.get("agent")?,
            session_id: row.get("session_id")?,
            asked: row.get("asked")?,
            finished: row.get::<_, i64>("finished")? != 0,
            created_at: row.get("created_at")?,
            files: Vec::new(),
        })
    }
}

impl Store {
    /// Starts an explanation of `subject` in `root`, to be written by
    /// `agent`: of `files` (path and blob id), with what was asked as
    /// `asked`. [`Store::finish_explanation`] gives it its text.
    pub fn create_explanation(
        &self,
        root: &str,
        subject: &str,
        agent: &str,
        files: &[(String, String)],
        asked: &str,
    ) -> Result<i64> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO explanations (root, subject, agent, asked, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![root, subject, agent, asked, now_millis()],
        )?;
        let id = tx.last_insert_rowid();
        for (path, blob) in files {
            tx.execute(
                "INSERT INTO explanation_files (explanation_id, path, blob) VALUES (?1, ?2, ?3)",
                params![id, path, blob],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// The agent wrote explanation `id`, in chat `session_id`.
    pub fn finish_explanation(
        &self,
        id: i64,
        title: &str,
        markdown: &str,
        session_id: Option<SessionId>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE explanations SET title = ?2, markdown = ?3, session_id = ?4, finished = 1
             WHERE id = ?1",
            params![id, title, markdown, session_id],
        )?;
        Ok(())
    }

    pub fn explanation(&self, id: i64) -> Result<Option<Explanation>> {
        let found = self
            .conn()
            .query_row(
                "SELECT * FROM explanations WHERE id = ?1",
                params![id],
                Explanation::from_row,
            )
            .optional()?;
        found.map(|found| self.with_files(found)).transpose()
    }

    /// The newest explanation of `subject` asked for in `root`, finished or
    /// still being written.
    pub fn latest_explanation(&self, root: &str, subject: &str) -> Result<Option<Explanation>> {
        let found = self
            .conn()
            .query_row(
                "SELECT * FROM explanations WHERE root = ?1 AND subject = ?2
                 ORDER BY id DESC LIMIT 1",
                params![root, subject],
                Explanation::from_row,
            )
            .optional()?;
        found.map(|found| self.with_files(found)).transpose()
    }

    /// The finished explanations that explain any of the content with blob
    /// ids `blobs`, newest first.
    pub fn explanations_of(&self, blobs: &[String]) -> Result<Vec<Explanation>> {
        if blobs.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; blobs.len()].join(", ");
        let found = {
            let conn = self.conn();
            let mut statement = conn.prepare(&format!(
                "SELECT * FROM explanations WHERE finished = 1 AND id IN
                    (SELECT explanation_id FROM explanation_files WHERE blob IN ({marks}))
                 ORDER BY id DESC"
            ))?;
            statement
                .query_map(params_from_iter(blobs.iter()), Explanation::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        found
            .into_iter()
            .map(|found| self.with_files(found))
            .collect()
    }

    /// The finished explanations that explain file `path` in any state,
    /// newest first.
    pub fn explanations_naming(&self, path: &str) -> Result<Vec<Explanation>> {
        let found = {
            let conn = self.conn();
            let mut statement = conn.prepare(
                "SELECT * FROM explanations WHERE finished = 1 AND id IN
                    (SELECT explanation_id FROM explanation_files WHERE path = ?1)
                 ORDER BY id DESC LIMIT 20",
            )?;
            statement
                .query_map(params![path], Explanation::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        found
            .into_iter()
            .map(|found| self.with_files(found))
            .collect()
    }

    pub fn delete_explanation(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM explanations WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn with_files(&self, mut explanation: Explanation) -> Result<Explanation> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT path, blob FROM explanation_files WHERE explanation_id = ?1 ORDER BY path",
        )?;
        explanation.files = statement
            .query_map(params![explanation.id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(explanation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(path, blob)| (path.to_string(), blob.to_string()))
            .collect()
    }

    #[test]
    fn an_explanation_is_found_by_the_content_it_explains() {
        let store = Store::open_in_memory().unwrap();
        let first = store
            .create_explanation(
                "/repo",
                "changes",
                "Claude Code",
                &files(&[("src/a.rs", "aaa"), ("src/b.rs", "bbb")]),
                "[]",
            )
            .unwrap();
        // Not finished yet: the newest asked for, but nothing to find.
        let latest = store
            .latest_explanation("/repo", "changes")
            .unwrap()
            .unwrap();
        assert_eq!((latest.id, latest.finished), (first, false));
        assert!(store.explanations_of(&["aaa".into()]).unwrap().is_empty());

        store
            .finish_explanation(first, "Reset emails", "# Reset emails", None)
            .unwrap();
        let second = store
            .create_explanation(
                "/repo",
                "changes",
                "Codex",
                &files(&[("src/b.rs", "bbb"), ("src/c.rs", "ccc")]),
                "[]",
            )
            .unwrap();
        store
            .finish_explanation(second, "Settings page", "# Settings page", None)
            .unwrap();

        let of_b = store.explanations_of(&["bbb".into()]).unwrap();
        assert_eq!(
            of_b.iter().map(|e| e.id).collect::<Vec<_>>(),
            [second, first]
        );
        assert_eq!(
            of_b[1].files,
            files(&[("src/a.rs", "aaa"), ("src/b.rs", "bbb")])
        );
        assert_eq!(
            store
                .explanations_of(&["aaa".into(), "zzz".into()])
                .unwrap()
                .len(),
            1
        );
        assert!(store.explanations_of(&[]).unwrap().is_empty());
        assert_eq!(store.explanations_naming("src/c.rs").unwrap()[0].id, second);
        assert_eq!(
            store
                .latest_explanation("/repo", "changes")
                .unwrap()
                .unwrap()
                .title,
            "Settings page"
        );
        assert!(
            store
                .latest_explanation("/other", "changes")
                .unwrap()
                .is_none()
        );

        // Its files go with it.
        store.delete_explanation(second).unwrap();
        assert!(store.explanation(second).unwrap().is_none());
        assert!(store.explanations_naming("src/c.rs").unwrap().is_empty());
    }
}
