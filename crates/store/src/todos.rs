//! To-dos: the user's own short list, with optional reminders. A to-do
//! belongs to a workspace, or to none (shown everywhere). A reminder that
//! comes due is announced once; it stays in the inbox until the to-do is
//! done or the reminder moved.

use anyhow::{Context as _, Result, bail};
use rusqlite::{OptionalExtension as _, Row, params};

use crate::Store;
use crate::projects::ProjectId;
use crate::time::now_millis;

pub type TodoId = i64;

#[derive(Debug, Clone, PartialEq)]
pub struct Todo {
    pub id: TodoId,
    pub project_id: Option<ProjectId>,
    pub text: String,
    pub done: bool,
    /// When to remind, in milliseconds.
    pub remind_at: Option<i64>,
    /// The reminder was announced.
    pub reminded: bool,
    pub created_at: i64,
}

impl Todo {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            text: row.get("text")?,
            done: row.get("done")?,
            remind_at: row.get("remind_at")?,
            reminded: row.get("reminded")?,
            created_at: row.get("created_at")?,
        })
    }

    /// Its reminder time has come (and it is not done).
    pub fn is_due(&self, now: i64) -> bool {
        !self.done && self.remind_at.is_some_and(|at| at <= now)
    }
}

const COLUMNS: &str = "id, project_id, text, done, remind_at, reminded, created_at";

impl Store {
    /// The open to-dos of `project_id` and those of no workspace, oldest
    /// first, then the ten done most lately.
    pub fn todos(&self, project_id: Option<ProjectId>) -> Result<Vec<Todo>> {
        self.listed_todos("(project_id IS NULL OR project_id = ?1)", &[&project_id])
    }

    /// Every to-do, whichever workspace it was made for (the app's list).
    pub fn all_todos(&self) -> Result<Vec<Todo>> {
        self.listed_todos("1", &[])
    }

    /// The open to-dos matching `filter`, oldest first, then the latest
    /// finished ones.
    fn listed_todos(&self, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Todo>> {
        let conn = self.conn();
        let mut open = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM todos WHERE done = 0 AND {filter} ORDER BY created_at, id"
        ))?;
        let mut todos = open
            .query_map(args, Todo::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut done = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM todos WHERE done = 1 AND {filter} ORDER BY done_at DESC LIMIT 10"
        ))?;
        todos.extend(
            done.query_map(args, Todo::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        Ok(todos)
    }

    pub fn todo(&self, id: TodoId) -> Result<Option<Todo>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM todos WHERE id = ?1"),
                params![id],
                Todo::from_row,
            )
            .optional()?)
    }

    pub fn add_todo(
        &self,
        project_id: Option<ProjectId>,
        text: &str,
        remind_at: Option<i64>,
    ) -> Result<Todo> {
        let text = text.trim();
        if text.is_empty() {
            bail!("Write what to do.");
        }
        // One lock for both: another thread's insert in between would
        // change the last row id.
        let id = {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO todos (project_id, text, remind_at, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![project_id, text, remind_at, now_millis()],
            )?;
            conn.last_insert_rowid()
        };
        self.todo(id)?.context("the to-do vanished")
    }

    pub fn set_todo_done(&self, id: TodoId, done: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE todos SET done = ?2, done_at = CASE WHEN ?2 THEN ?3 ELSE NULL END WHERE id = ?1",
            params![id, done, now_millis()],
        )?;
        Ok(())
    }

    /// Moves (or with `None`, clears) the reminder; it will be announced
    /// again.
    pub fn set_todo_reminder(&self, id: TodoId, remind_at: Option<i64>) -> Result<()> {
        self.conn().execute(
            "UPDATE todos SET remind_at = ?2, reminded = 0 WHERE id = ?1",
            params![id, remind_at],
        )?;
        Ok(())
    }

    pub fn delete_todo(&self, id: TodoId) -> Result<()> {
        self.conn()
            .execute("DELETE FROM todos WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Open to-dos of every workspace whose reminder has come by `now`.
    pub fn due_todos(&self, now: i64) -> Result<Vec<Todo>> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM todos WHERE done = 0 AND remind_at <= ?1 ORDER BY remind_at"
        ))?;
        let todos = statement
            .query_map(params![now], Todo::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(todos)
    }

    /// Notes that a to-do's reminder was announced.
    pub fn todo_reminded(&self, id: TodoId) -> Result<()> {
        self.conn()
            .execute("UPDATE todos SET reminded = 1 WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todos_remind_once_and_list_by_workspace() {
        let store = Store::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("ph-todos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store.add_project(&dir).unwrap();
        let mine = store
            .add_todo(Some(project.id), "Review the PR", Some(1_000))
            .unwrap();
        let everywhere = store.add_todo(None, "Call the team", None).unwrap();
        let elsewhere = store.add_todo(Some(project.id + 1), "Other workspace", None);
        assert!(elsewhere.is_err(), "an unknown workspace is refused");
        assert!(store.add_todo(None, "  ", None).is_err());

        let listed: Vec<String> = store
            .todos(Some(project.id))
            .unwrap()
            .into_iter()
            .map(|todo| todo.text)
            .collect();
        assert_eq!(listed, ["Review the PR", "Call the team"]);
        assert_eq!(
            store.todos(None).unwrap().len(),
            1,
            "no workspace: only the shared ones"
        );
        assert_eq!(store.all_todos().unwrap().len(), 2, "the app's list: all");

        let due = store.due_todos(2_000).unwrap();
        assert_eq!(due.len(), 1);
        assert!(due[0].is_due(2_000) && !due[0].reminded);
        store.todo_reminded(mine.id).unwrap();
        assert!(store.todo(mine.id).unwrap().unwrap().reminded);
        store.set_todo_reminder(mine.id, Some(5_000)).unwrap();
        assert!(
            !store.todo(mine.id).unwrap().unwrap().reminded,
            "moved: announced again"
        );

        store.set_todo_done(everywhere.id, true).unwrap();
        let after = store.todos(Some(project.id)).unwrap();
        assert!(after.last().unwrap().done, "done ones come last");
        store.delete_todo(everywhere.id).unwrap();
        assert_eq!(store.todos(Some(project.id)).unwrap().len(), 1);
    }
}
