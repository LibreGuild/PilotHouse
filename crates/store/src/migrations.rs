//! Schema migrations, tracked with SQLite's `user_version` pragma.
//!
//! Append new migrations to [`MIGRATIONS`]; never edit one that has shipped.

use anyhow::{Context as _, Result};
use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    // 1: initial schema.
    r#"
    CREATE TABLE projects (
        id              INTEGER PRIMARY KEY,
        name            TEXT NOT NULL,
        path            TEXT NOT NULL UNIQUE,
        created_at      INTEGER NOT NULL,
        last_opened_at  INTEGER NOT NULL,
        ui_state        TEXT NOT NULL DEFAULT '{}'
    );

    CREATE TABLE sessions (
        id              INTEGER PRIMARY KEY,
        project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        title           TEXT NOT NULL,
        agent_id        TEXT NOT NULL,
        created_at      INTEGER NOT NULL,
        updated_at      INTEGER NOT NULL
    );
    CREATE INDEX sessions_by_project ON sessions(project_id, updated_at DESC);

    CREATE TABLE messages (
        id              INTEGER PRIMARY KEY,
        session_id      INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        role            TEXT NOT NULL,
        body            TEXT NOT NULL,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX messages_by_session ON messages(session_id, id);

    CREATE TABLE cards (
        id              INTEGER PRIMARY KEY,
        project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        title           TEXT NOT NULL,
        description     TEXT NOT NULL DEFAULT '',
        acceptance      TEXT NOT NULL DEFAULT '',
        status          TEXT NOT NULL,
        position        REAL NOT NULL,
        assignee        TEXT,
        created_at      INTEGER NOT NULL,
        updated_at      INTEGER NOT NULL
    );
    CREATE INDEX cards_by_project ON cards(project_id, status, position);

    CREATE TABLE card_events (
        id              INTEGER PRIMARY KEY,
        card_id         INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
        author          TEXT NOT NULL,
        text            TEXT NOT NULL,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX card_events_by_card ON card_events(card_id, id);

    CREATE TABLE settings (
        key             TEXT PRIMARY KEY,
        value           TEXT NOT NULL
    );
    "#,
    // 2: the agent's own session id, so a chat can be resumed after a restart.
    r#"
    ALTER TABLE sessions ADD COLUMN acp_session_id TEXT;
    "#,
    // 3: chats that run elsewhere (a PR review worktree), and PR reviews
    // with their findings, kept so a re-review can say what got fixed.
    r#"
    ALTER TABLE sessions ADD COLUMN cwd TEXT;
    ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'chat';

    CREATE TABLE pr_reviews (
        id              INTEGER PRIMARY KEY,
        project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        host            TEXT NOT NULL,
        owner           TEXT NOT NULL,
        repo            TEXT NOT NULL,
        number          INTEGER NOT NULL,
        title           TEXT NOT NULL DEFAULT '',
        head_sha        TEXT NOT NULL,
        session_id      INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
        summary         TEXT NOT NULL DEFAULT '',
        verdict         TEXT NOT NULL DEFAULT '',
        finished        INTEGER NOT NULL DEFAULT 0,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX pr_reviews_by_pull ON pr_reviews(host, owner, repo, number, id DESC);

    CREATE TABLE pr_findings (
        id              INTEGER PRIMARY KEY,
        review_id       INTEGER NOT NULL REFERENCES pr_reviews(id) ON DELETE CASCADE,
        key             TEXT NOT NULL,
        severity        TEXT NOT NULL,
        path            TEXT NOT NULL,
        line            INTEGER,
        end_line        INTEGER,
        title           TEXT NOT NULL,
        body            TEXT NOT NULL DEFAULT '',
        suggestion      TEXT NOT NULL DEFAULT '',
        status          TEXT NOT NULL,
        comment_id      INTEGER,
        comment_kind    TEXT,
        fixed_notified  INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX pr_findings_by_review ON pr_findings(review_id, id);
    "#,
    // 4: full-text search over chat messages. The text is the message's
    // `text` (user, agent, notices) or `title` (tool calls), kept in step by
    // triggers; existing messages are indexed once here.
    r#"
    CREATE VIRTUAL TABLE message_search USING fts5(
        text,
        session_id UNINDEXED,
        tokenize = 'unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER message_search_insert AFTER INSERT ON messages BEGIN
        INSERT INTO message_search (rowid, text, session_id)
        VALUES (new.id, CASE WHEN json_valid(new.body)
            THEN coalesce(json_extract(new.body, '$.text'), json_extract(new.body, '$.title'), '')
            ELSE '' END, new.session_id);
    END;
    CREATE TRIGGER message_search_update AFTER UPDATE OF body ON messages BEGIN
        DELETE FROM message_search WHERE rowid = old.id;
        INSERT INTO message_search (rowid, text, session_id)
        VALUES (new.id, CASE WHEN json_valid(new.body)
            THEN coalesce(json_extract(new.body, '$.text'), json_extract(new.body, '$.title'), '')
            ELSE '' END, new.session_id);
    END;
    CREATE TRIGGER message_search_delete AFTER DELETE ON messages BEGIN
        DELETE FROM message_search WHERE rowid = old.id;
    END;
    INSERT INTO message_search (rowid, text, session_id)
        SELECT id, CASE WHEN json_valid(body)
            THEN coalesce(json_extract(body, '$.text'), json_extract(body, '$.title'), '')
            ELSE '' END, session_id
        FROM messages;
    "#,
    // 5: what each chat used: tokens and cost added up across the agent's
    // sessions, and the context window as last reported. Agents report
    // running totals per agent session; `base_*` is the last report, so only
    // the difference is added (and a resumed session is not counted twice).
    r#"
    CREATE TABLE session_usage (
        session_id      INTEGER PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        input_tokens    INTEGER NOT NULL DEFAULT 0,
        output_tokens   INTEGER NOT NULL DEFAULT 0,
        cached_tokens   INTEGER NOT NULL DEFAULT 0,
        cost            REAL NOT NULL DEFAULT 0,
        currency        TEXT,
        context_used    INTEGER,
        context_size    INTEGER,
        base_session    TEXT,
        base_input      INTEGER NOT NULL DEFAULT 0,
        base_output     INTEGER NOT NULL DEFAULT 0,
        base_cached     INTEGER NOT NULL DEFAULT 0,
        base_cost       REAL NOT NULL DEFAULT 0,
        updated_at      INTEGER NOT NULL
    );
    "#,
    // 6: parallel runs: one prompt given to several agents, each in its own
    // worktree from the same snapshot, to compare and keep one.
    r#"
    CREATE TABLE parallel_runs (
        id              INTEGER PRIMARY KEY,
        project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        prompt          TEXT NOT NULL,
        base            TEXT NOT NULL,
        created_at      INTEGER NOT NULL
    );
    CREATE TABLE parallel_lanes (
        id              INTEGER PRIMARY KEY,
        run_id          INTEGER NOT NULL REFERENCES parallel_runs(id) ON DELETE CASCADE,
        agent_id        TEXT NOT NULL,
        worktree        TEXT NOT NULL,
        session_id      INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
        state           TEXT NOT NULL DEFAULT 'open'
    );
    CREATE INDEX parallel_lanes_by_run ON parallel_lanes(run_id, id);
    "#,
    // 7: workspaces with several folders, or none. `projects.path` stays the
    // primary folder (the agents' working directory); `kind = 'empty'` marks
    // a workspace without folders, whose path is a private scratch folder.
    r#"
    ALTER TABLE projects ADD COLUMN kind TEXT NOT NULL DEFAULT 'folder';
    CREATE TABLE workspace_folders (
        project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        path            TEXT NOT NULL,
        position        INTEGER NOT NULL,
        PRIMARY KEY (project_id, path)
    );
    "#,
    // 8: UI tests a card needs to pass before it can be Done, by the test's
    // file name (`sign-in-works` for `.pilothouse/tests/sign-in-works.json`).
    r#"
    CREATE TABLE card_tests (
        card_id         INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
        test            TEXT NOT NULL,
        PRIMARY KEY (card_id, test)
    );
    "#,
    // 9: schedules: an agent prompt or a UI test run at set times. `job` and
    // `timing` are JSON (`schedules.rs`); times are milliseconds.
    r#"
    CREATE TABLE schedules (
        id              INTEGER PRIMARY KEY,
        project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        name            TEXT NOT NULL,
        job             TEXT NOT NULL,
        timing          TEXT NOT NULL,
        enabled         INTEGER NOT NULL DEFAULT 1,
        next_run        INTEGER NOT NULL,
        last_run        INTEGER,
        last_result     TEXT,
        last_session    INTEGER,
        created_at      INTEGER NOT NULL
    );
    CREATE INDEX schedules_due ON schedules(enabled, next_run);
    "#,
    // 10: the user's to-dos, with optional reminders (`todos.rs`).
    r#"
    CREATE TABLE todos (
        id              INTEGER PRIMARY KEY,
        project_id      INTEGER REFERENCES projects(id) ON DELETE CASCADE,
        text            TEXT NOT NULL,
        done            INTEGER NOT NULL DEFAULT 0,
        remind_at       INTEGER,
        reminded        INTEGER NOT NULL DEFAULT 0,
        created_at      INTEGER NOT NULL,
        done_at         INTEGER
    );
    CREATE INDEX todos_by_project ON todos(project_id, done);
    "#,
    // 11: a workspace's colour, chosen by the user (an index into the
    // palette); empty means the one picked from its id.
    r#"
    ALTER TABLE projects ADD COLUMN color INTEGER;
    "#,
];

/// Backups kept of a database file, newest first; older ones are deleted.
const BACKUPS_KEPT: usize = 3;

pub fn apply(connection: &Connection) -> Result<()> {
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    // An existing database about to change gets a copy first, so a
    // migration that goes wrong cannot cost the user their data.
    if current > 0 && (current as usize) < MIGRATIONS.len() {
        backup(connection, current).context("backing up the database before updating it")?;
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current.max(0) as usize) {
        let version = index + 1;
        connection
            .execute_batch(&format!(
                "BEGIN; {sql}; PRAGMA user_version = {version}; COMMIT;"
            ))
            .with_context(|| format!("applying database migration {version}"))?;
        tracing::info!("applied database migration {version}");
    }
    Ok(())
}

/// Copies the database (consistently, even while open in WAL mode) to
/// `<file>.v<version>.bak` beside it, keeping the newest [`BACKUPS_KEPT`].
/// An in-memory database has nothing to back up.
fn backup(connection: &Connection, version: i64) -> Result<()> {
    let Some(file) = connection.path().filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    let file = std::path::Path::new(file);
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let target = file.with_file_name(format!("{name}.v{version}.bak"));
    let _ = std::fs::remove_file(&target);
    connection.execute("VACUUM INTO ?1", [target.to_string_lossy()])?;
    tracing::info!("backed up the database to {}", target.display());
    // The oldest backups go.
    let folder = file.parent().unwrap_or(std::path::Path::new("."));
    let mut backups: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(folder)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|other| other.to_string_lossy())
                .is_some_and(|other| {
                    other.starts_with(&format!("{name}.v")) && other.ends_with(".bak")
                })
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.0));
    for (_, old) in backups.into_iter().skip(BACKUPS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_database_is_backed_up_before_it_changes() {
        let dir = std::env::temp_dir().join(format!("ph-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pilothouse.db");
        {
            // An old database: the first migration only, with a row in it.
            let connection = Connection::open(&file).unwrap();
            connection
                .execute_batch(&format!("{}; PRAGMA user_version = 1;", MIGRATIONS[0]))
                .unwrap();
            connection
                .execute(
                    "INSERT INTO projects (name, path, created_at, last_opened_at) VALUES ('old', '/tmp/old', 0, 0)",
                    [],
                )
                .unwrap();
        }
        let connection = Connection::open(&file).unwrap();
        apply(&connection).unwrap();
        let backup = dir.join("pilothouse.db.v1.bak");
        assert!(backup.is_file(), "a copy before the update");
        let copy = Connection::open(&backup).unwrap();
        let (version, rows): (i64, i64) = (
            copy.pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap(),
            copy.query_row("SELECT count(*) FROM projects", [], |row| row.get(0))
                .unwrap(),
        );
        assert_eq!((version, rows), (1, 1), "the copy is the old database");
        // Up to date: nothing more is copied.
        apply(&connection).unwrap();
        let backups = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
            .count();
        assert_eq!(backups, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
