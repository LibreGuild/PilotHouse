//! Schema migrations, tracked with SQLite's `user_version` pragma.
//!
//! Append new migrations to [`MIGRATIONS`]; never edit one that has shipped.
//!
//! An organisation's build that keeps tables of its own puts their
//! migrations on a track of its own ([`apply_track`]), counted in the
//! `schema_tracks` table, never in [`MIGRATIONS`]: a migration it appended
//! there would take the number of PilotHouse's next one, and a database that
//! already had it would skip PilotHouse's.

use anyhow::{Context as _, Result, bail};
use rusqlite::{Connection, OptionalExtension as _};

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
    // 12: a review finding's evidence (what shows the problem), beside its
    // details and fix.
    r#"
    ALTER TABLE pr_findings ADD COLUMN evidence TEXT NOT NULL DEFAULT '';
    "#,
    // 13: reviews in rounds with a findings ledger. A finding keeps a label
    // (`F3`) across rounds; its severity comes from what goes wrong (impact)
    // and how often (likelihood); its status says who set it and why; a
    // round knows its number and kind; the PR keeps one summary comment.
    // Statuses become open, fixed, not_problem and wont_fix.
    r#"
    ALTER TABLE pr_findings ADD COLUMN label TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN impact TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN likelihood TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN status_source TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN status_by TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN reason TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN first_round INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE pr_findings ADD COLUMN found_late INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE pr_findings ADD COLUMN verified INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE pr_findings ADD COLUMN agents TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN note TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN issue_number INTEGER;
    ALTER TABLE pr_findings ADD COLUMN last_reply_id INTEGER;
    ALTER TABLE pr_findings ADD COLUMN suggested_code TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN claim TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_findings ADD COLUMN draft_reply TEXT NOT NULL DEFAULT '';
    UPDATE pr_findings SET status = 'open' WHERE status = 'new';
    UPDATE pr_findings SET status = 'not_problem' WHERE status = 'dismissed';
    ALTER TABLE pr_reviews ADD COLUMN round INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE pr_reviews ADD COLUMN kind TEXT NOT NULL DEFAULT 'full';
    ALTER TABLE pr_reviews ADD COLUMN agent TEXT NOT NULL DEFAULT '';
    ALTER TABLE pr_reviews ADD COLUMN summary_comment_id INTEGER;
    ALTER TABLE pr_reviews ADD COLUMN data TEXT NOT NULL DEFAULT '';
    "#,
    // 14: usage by day and model, for the usage dashboard: what each report
    // added to a chat, on the (local) day it came, under the model the chat
    // used then. Chats recorded before this keep their totals on the day
    // they were last used, with no model.
    r#"
    CREATE TABLE usage_days (
        session_id      INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        day             TEXT NOT NULL,
        model           TEXT NOT NULL DEFAULT '',
        input_tokens    INTEGER NOT NULL DEFAULT 0,
        output_tokens   INTEGER NOT NULL DEFAULT 0,
        cached_tokens   INTEGER NOT NULL DEFAULT 0,
        cost            REAL NOT NULL DEFAULT 0,
        currency        TEXT,
        PRIMARY KEY (session_id, day, model)
    );
    INSERT INTO usage_days (session_id, day, input_tokens, output_tokens, cached_tokens, cost, currency)
        SELECT session_id, date(updated_at / 1000, 'unixepoch', 'localtime'),
               input_tokens, output_tokens, cached_tokens, cost, currency
        FROM session_usage;
    "#,
    // 15: a chat's last line, for chats whose messages are not kept here
    // (an agent CLI's in a terminal, shown under its title once closed).
    "ALTER TABLE sessions ADD COLUMN last_line TEXT;",
];

/// Backups kept of a database file, newest first; older ones are deleted.
const BACKUPS_KEPT: usize = 3;

pub fn apply(connection: &Connection) -> Result<()> {
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current as usize > MIGRATIONS.len() {
        // A newer build updated this database. Its migrations only add, so
        // this build still works with it; say so in the log for support.
        tracing::warn!(
            "the database is at schema {current}, newer than this build's {}",
            MIGRATIONS.len()
        );
    }
    // An existing database about to change gets a copy first, so a
    // migration that goes wrong cannot cost the user their data.
    if current > 0 && (current as usize) < MIGRATIONS.len() {
        backup(connection, &format!("v{current}"))
            .context("backing up the database before updating it")?;
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

/// Applies `migrations` of the track `track` that this database does not
/// have yet, each in a transaction, in order. The track's version (how many
/// it has) is kept in `schema_tracks`; the core's stays in `user_version`.
/// Like the core's, a track's migrations are append-only. `track` is a short
/// name of lowercase letters, digits, `-` and `_` (an organisation's name);
/// `core` is taken.
pub fn apply_track(connection: &Connection, track: &str, migrations: &[&str]) -> Result<()> {
    let valid = !track.is_empty()
        && track.len() <= 64
        && track != "core"
        && track
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        bail!(
            "`{track}` is not a migration track name (lowercase letters, digits, - and _; not `core`)"
        );
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_tracks (
            track   TEXT PRIMARY KEY,
            version INTEGER NOT NULL
        );",
    )?;
    let current: i64 = connection
        .query_row(
            "SELECT version FROM schema_tracks WHERE track = ?1",
            [track],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    if current as usize > migrations.len() {
        tracing::warn!(
            "the database's `{track}` track is at {current}, newer than this build's {}",
            migrations.len()
        );
    }
    if current > 0 && (current as usize) < migrations.len() {
        backup(connection, &format!("{track}-v{current}"))
            .with_context(|| format!("backing up the database before updating `{track}`"))?;
    }
    for (index, sql) in migrations.iter().enumerate().skip(current.max(0) as usize) {
        let version = index as i64 + 1;
        let transaction = connection.unchecked_transaction()?;
        transaction
            .execute_batch(sql)
            .and_then(|()| {
                transaction.execute(
                    "INSERT INTO schema_tracks (track, version) VALUES (?1, ?2)
                     ON CONFLICT(track) DO UPDATE SET version = excluded.version",
                    rusqlite::params![track, version],
                )
            })
            .and_then(|_| transaction.commit())
            .with_context(|| format!("applying database migration {version} of `{track}`"))?;
        tracing::info!("applied database migration {version} of `{track}`");
    }
    Ok(())
}

/// Copies the database (consistently, even while open in WAL mode) to
/// `<file>.<label>.bak` beside it (`pilothouse.db.v13.bak`), keeping the
/// newest [`BACKUPS_KEPT`]. An in-memory database has nothing to back up.
fn backup(connection: &Connection, label: &str) -> Result<()> {
    let Some(file) = connection.path().filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    let file = std::path::Path::new(file);
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let target = file.with_file_name(format!("{name}.{label}.bak"));
    let _ = std::fs::remove_file(&target);
    connection.execute("VACUUM INTO ?1", [target.to_string_lossy()])?;
    tracing::info!("backed up the database to {}", target.display());
    // The oldest backups go: of this one's kind only (the core's, or this
    // track's), so a track's updates never take the core's backups.
    let folder = file.parent().unwrap_or(std::path::Path::new("."));
    let kind = backup_kind(label);
    let mut backups: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(folder)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|other| other.to_string_lossy().into_owned())
                .and_then(|other| {
                    other
                        .strip_prefix(&format!("{name}."))?
                        .strip_suffix(".bak")
                        .map(|other| backup_kind(other) == kind)
                })
                .unwrap_or(false)
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.0));
    for (_, old) in backups.into_iter().skip(BACKUPS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// Whose backup a label is: `v13` the core's (``), `acme-v2` track
/// `acme`'s (`acme-`).
fn backup_kind(label: &str) -> &str {
    label
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .strip_suffix('v')
        .unwrap_or(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups_are_kept_by_kind() {
        assert_eq!(backup_kind("v14"), "");
        assert_eq!(backup_kind("acme-v2"), "acme-");
        assert_ne!(backup_kind("v3"), backup_kind("acme-v3"));
    }

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

    fn track_version(connection: &Connection, track: &str) -> i64 {
        connection
            .query_row(
                "SELECT version FROM schema_tracks WHERE track = ?1",
                [track],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn a_track_applies_its_own_migrations_beside_the_core() {
        let connection = Connection::open_in_memory().unwrap();
        apply(&connection).unwrap();
        let core: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let first = ["CREATE TABLE x_acme_notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)"];
        apply_track(&connection, "acme", &first).unwrap();
        assert_eq!(track_version(&connection, "acme"), 1);
        // Again: nothing to do.
        apply_track(&connection, "acme", &first).unwrap();
        // A later build appends one; only that one runs.
        let later = [
            first[0],
            "ALTER TABLE x_acme_notes ADD COLUMN done INTEGER NOT NULL DEFAULT 0",
        ];
        apply_track(&connection, "acme", &later).unwrap();
        assert_eq!(track_version(&connection, "acme"), 2);
        connection
            .execute("INSERT INTO x_acme_notes (text, done) VALUES ('hi', 1)", [])
            .unwrap();
        // The core's version is untouched, and so is a later core update.
        let after: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(after, core);
        apply(&connection).unwrap();
    }

    #[test]
    fn a_failing_track_migration_changes_nothing() {
        let connection = Connection::open_in_memory().unwrap();
        apply(&connection).unwrap();
        let broken = [
            "CREATE TABLE x_acme_a (id INTEGER PRIMARY KEY)",
            "CREATE TABLE x_acme_b (id INTEGER PRIMARY KEY); NOT SQL",
        ];
        assert!(apply_track(&connection, "acme", &broken).is_err());
        // The first one stays applied; the broken one left no table behind.
        assert_eq!(track_version(&connection, "acme"), 1);
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'x_acme_b'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
    }

    #[test]
    fn track_names_are_checked() {
        let connection = Connection::open_in_memory().unwrap();
        for name in ["", "core", "Acme", "acme corp", "a/b"] {
            assert!(apply_track(&connection, name, &[]).is_err(), "{name:?}");
        }
        apply_track(&connection, "acme-tools_2", &[]).unwrap();
    }

    #[test]
    fn a_track_update_is_backed_up_first() {
        let dir = std::env::temp_dir().join(format!("ph-track-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pilothouse.db");
        let first = ["CREATE TABLE x_acme_a (id INTEGER PRIMARY KEY)"];
        {
            let connection = Connection::open(&file).unwrap();
            apply(&connection).unwrap();
            apply_track(&connection, "acme", &first).unwrap();
        }
        let connection = Connection::open(&file).unwrap();
        apply_track(
            &connection,
            "acme",
            &[first[0], "CREATE TABLE x_acme_b (id INTEGER PRIMARY KEY)"],
        )
        .unwrap();
        assert!(dir.join("pilothouse.db.acme-v1.bak").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_kept_before_days_moves_to_its_last_day() {
        let connection = Connection::open_in_memory().unwrap();
        // A database just before usage by day, with a chat that used some.
        // Migration 14 made usage by day.
        let before = 13;
        for sql in &MIGRATIONS[..before] {
            connection.execute_batch(sql).unwrap();
        }
        connection
            .execute_batch(&format!(
                "PRAGMA user_version = {before};
                 INSERT INTO projects (id, name, path, created_at, last_opened_at) VALUES (1, 'p', '/tmp/p', 0, 0);
                 INSERT INTO sessions (id, project_id, title, agent_id, created_at, updated_at)
                    VALUES (7, 1, 'One', 'codex', 0, 0);
                 INSERT INTO session_usage (session_id, input_tokens, output_tokens, cached_tokens, cost, updated_at)
                    VALUES (7, 900, 40, 300, 0.5, 1767225600000);"
            ))
            .unwrap();
        apply(&connection).unwrap();
        let (day, input, cost): (String, i64, f64) = connection
            .query_row(
                "SELECT day, input_tokens, cost FROM usage_days WHERE session_id = 7",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        // 1 January 2026, 00:00 UTC: that day or the one before, by time zone.
        assert!(day == "2026-01-01" || day == "2025-12-31", "{day}");
        assert_eq!((input, cost), (900, 0.5));
    }
}
