//! Full-text search over chat messages (SQLite FTS5, see migration 4).

use anyhow::Result;
use rusqlite::params;

use crate::Store;
use crate::projects::ProjectId;
use crate::sessions::SessionId;

/// Marks around matched words in [`SearchHit::snippet`].
pub const MATCH_START: char = '\u{1}';
pub const MATCH_END: char = '\u{2}';

/// One message that matched.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub session_id: SessionId,
    pub session_title: String,
    pub project_id: ProjectId,
    pub message_id: i64,
    /// `user` or `agent`.
    pub role: String,
    /// A few words around the match, matched words between
    /// [`MATCH_START`] and [`MATCH_END`].
    pub snippet: String,
    pub created_at: i64,
}

/// What people type, as an FTS5 query: each word must appear, the last one
/// as a prefix (so results come while typing). Quotes keep FTS5 syntax
/// (`AND`, `-`, `:`…) from being read as operators.
pub fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect();
    let (last, rest) = words.split_last()?;
    let mut query = rest.join(" ");
    if !query.is_empty() {
        query.push(' ');
    }
    query.push_str(last);
    query.push('*');
    Some(query)
}

impl Store {
    /// The best matches for `text` in one project's chats (or all), newest
    /// sessions first among equals.
    pub fn search_messages(
        &self,
        project_id: Option<ProjectId>,
        text: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let Some(query) = fts_query(text) else {
            return Ok(Vec::new());
        };
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT m.session_id, s.title, s.project_id, m.id, m.role,
                    snippet(message_search, 0, ?3, ?4, '…', 14), m.created_at
             FROM message_search
             JOIN messages m ON m.id = message_search.rowid
             JOIN sessions s ON s.id = m.session_id
             WHERE message_search MATCH ?1 AND (?2 IS NULL OR s.project_id = ?2)
             ORDER BY bm25(message_search), m.created_at DESC
             LIMIT ?5",
        )?;
        let hits = statement
            .query_map(
                params![
                    query,
                    project_id,
                    MATCH_START.to_string(),
                    MATCH_END.to_string(),
                    limit as i64
                ],
                |row| {
                    Ok(SearchHit {
                        session_id: row.get(0)?,
                        session_title: row.get(1)?,
                        project_id: row.get(2)?,
                        message_id: row.get(3)?,
                        role: row.get(4)?,
                        snippet: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(hits)
    }

    /// Where message `message_id` sits among its session's messages
    /// (0-based), to scroll to it.
    pub fn message_position(&self, session_id: SessionId, message_id: i64) -> Result<usize> {
        let position: i64 = self.conn().query_row(
            "SELECT count(*) FROM messages WHERE session_id = ?1 AND id < ?2",
            params![session_id, message_id],
            |row| row.get(0),
        )?;
        Ok(position as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_becomes_a_safe_prefix_query() {
        assert_eq!(
            fts_query("retry login"),
            Some("\"retry\" \"login\"*".into())
        );
        assert_eq!(
            fts_query("  NOT -x \"y\" "),
            Some("\"NOT\" \"-x\" \"\"\"y\"\"\"*".into())
        );
        assert_eq!(fts_query("   "), None);
    }

    #[test]
    fn finds_messages_by_words_and_keeps_up_with_changes() {
        let store = Store::open_in_memory().unwrap();
        let folder = std::env::temp_dir().join(format!("ph-search-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let project = store.add_project(&folder).unwrap();
        let session = store
            .create_session(project.id, "Login bug", "claude")
            .unwrap();
        let user = store
            .append_message(
                session.id,
                "user",
                r#"{"entry":"user","text":"Why does the login retry forever?"}"#,
            )
            .unwrap();
        store
            .append_message(
                session.id,
                "agent",
                r#"{"entry":"agent","text":"The backoff never grows."}"#,
            )
            .unwrap();
        store
            .append_message(
                session.id,
                "agent",
                r#"{"entry":"tool","title":"Read file src/retry.rs"}"#,
            )
            .unwrap();
        store
            .append_message(session.id, "agent", "not json")
            .unwrap();

        let hits = store.search_messages(Some(project.id), "retr", 10).unwrap();
        assert_eq!(
            hits.len(),
            2,
            "prefix match in the question and the tool title"
        );
        assert!(
            hits.iter()
                .any(|hit| hit.snippet.contains("\u{1}retry\u{2}"))
        );
        assert_eq!(hits[0].session_title, "Login bug");
        assert_eq!(
            store
                .search_messages(Some(project.id), "backoff grows", 10)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .search_messages(Some(project.id + 1), "backoff", 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.message_position(session.id, user).unwrap(), 0);

        store
            .update_message(user, r#"{"entry":"user","text":"Something else entirely"}"#)
            .unwrap();
        assert_eq!(store.search_messages(None, "login", 10).unwrap().len(), 0);
        assert_eq!(
            store.search_messages(None, "entirely", 10).unwrap().len(),
            1
        );
    }
}
