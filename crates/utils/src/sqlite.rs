//! Looking into a SQLite file: its tables, or a query's rows. The file is
//! opened read-only, so nothing in it can change.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rusqlite::{Connection, OpenFlags, types::ValueRef};

use crate::Input;

/// Rows shown at most.
const MAX_ROWS: usize = 200;

/// The tables (with their row counts), or the rows of a query.
pub fn query(input: &Input) -> Result<String> {
    query_at(&file_of(&input.text)?, input)
}

/// The path a SQLite input names, without the spaces and quotes around it.
/// Everything that checks the path and everything that opens it start from
/// this one form, so a check cannot pass on one file while another opens.
pub fn path_of(text: &str) -> &str {
    text.trim().trim_matches('"')
}

/// The file `text` names, absolute and free of links.
pub fn file_of(text: &str) -> Result<PathBuf> {
    let path = path_of(text);
    if path.is_empty() {
        bail!("Give the path of a SQLite file.");
    }
    if is_network_path(path) {
        bail!("{path} is on another computer");
    }
    dunce::canonicalize(path).with_context(|| format!("no such file: {path}"))
}

/// Like [`query`], on `file` exactly as given: a caller that checked where
/// `file` is opens that same path, and a link put in its place (or in any
/// folder on its way) after the check is refused, not followed.
pub fn query_at(file: &Path, input: &Input) -> Result<String> {
    // An absolute path never starts with `file:`, which this SQLite (built
    // with `SQLITE_USE_URI`) reads as a URI whatever the open flags say.
    if !file.is_absolute() {
        bail!("{} is not a full path", file.display());
    }
    if is_network_path(&file.to_string_lossy()) {
        bail!("{} is on another computer", file.display());
    }
    let db = Connection::open_with_flags(
        file,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .with_context(|| format!("could not open {}", file.display()))?;
    // One file, read only: no other databases attached (they could be any
    // file), no functions from the file's own schema, and a time limit.
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    db.pragma_update(None, "trusted_schema", false)?;
    within_limit(&db, |db| run(db, input))
}

/// A network path (`\\server\share`, `//server/share`): opening one on
/// Windows sends the user's sign-in to that computer (as
/// `ph_ui::platform::is_network_path`, which this crate does not use).
fn is_network_path(path: &str) -> bool {
    let path = path.trim_start();
    path.starts_with("\\\\")
        || path.starts_with("//")
        || path.starts_with("\\/")
        || path.starts_with("/\\")
}

/// How long a query may run before it is stopped.
const TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Runs `read` on `db`, interrupting it after [`TIME_LIMIT`].
fn within_limit<T>(db: &Connection, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let interrupt = db.get_interrupt_handle();
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let watch = std::thread::spawn(move || {
        if finished.recv_timeout(TIME_LIMIT) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            interrupt.interrupt();
        }
    });
    let result = read(db);
    drop(done);
    let _ = watch.join();
    result.map_err(|err| {
        if err.to_string().contains("interrupted") {
            anyhow::anyhow!(
                "The query took longer than {} seconds and was stopped.",
                TIME_LIMIT.as_secs()
            )
        } else {
            err
        }
    })
}

fn run(db: &Connection, input: &Input) -> Result<String> {
    let sql = input.second.trim();
    if sql.is_empty() {
        let mut statement =
            db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if names.is_empty() {
            return Ok("No tables.".into());
        }
        let lines: Vec<String> = names
            .iter()
            .map(|name| {
                let count: i64 = db
                    .query_row(
                        &format!("SELECT COUNT(*) FROM \"{}\"", name.replace('"', "\"\"")),
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(-1);
                format!("{name} ({count} rows)")
            })
            .collect();
        return Ok(lines.join("\n"));
    }
    let mut statement = db.prepare(sql).context("The query is not valid")?;
    // Reading only (`VACUUM INTO` would write a file anywhere).
    if !statement.readonly() {
        bail!("Only queries that read are run here.");
    }
    let columns: Vec<String> = statement
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let mut rows = statement.query([])?;
    let mut table = vec![columns.clone()];
    let mut more = false;
    while let Some(row) = rows.next()? {
        if table.len() > MAX_ROWS {
            more = true;
            break;
        }
        table.push(
            (0..columns.len())
                .map(|ix| match row.get_ref(ix) {
                    Ok(ValueRef::Null) => "NULL".to_string(),
                    Ok(ValueRef::Integer(value)) => value.to_string(),
                    Ok(ValueRef::Real(value)) => value.to_string(),
                    Ok(ValueRef::Text(text)) => String::from_utf8_lossy(text).into_owned(),
                    Ok(ValueRef::Blob(blob)) => format!("<{} bytes>", blob.len()),
                    Err(_) => "?".to_string(),
                })
                .collect(),
        );
    }
    let widths: Vec<usize> = (0..columns.len())
        .map(|ix| {
            table
                .iter()
                .map(|row| row[ix].chars().count().min(60))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |row: &Vec<String>| {
        row.iter()
            .zip(&widths)
            .map(|(cell, width)| {
                let cell: String = cell.chars().take(60).collect();
                format!("{cell:<width$}")
            })
            .collect::<Vec<_>>()
            .join(" | ")
            .trim_end()
            .to_string()
    };
    let mut out = vec![line(&table[0])];
    out.push(
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("-+-"),
    );
    out.extend(table[1..].iter().map(line));
    out.push(if more {
        format!("(the first {MAX_ROWS} rows)")
    } else {
        format!("({} rows)", table.len() - 1)
    });
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_tables_and_runs_queries_read_only() {
        let path = std::env::temp_dir().join(format!("ph-utils-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE people (name TEXT, age INTEGER); INSERT INTO people VALUES ('Ada', 36), ('Bob', NULL);")
            .unwrap();
        drop(db);
        let file = path.display().to_string();
        assert_eq!(query(&Input::new(file.clone())).unwrap(), "people (2 rows)");
        let rows = query(&Input {
            second: "SELECT * FROM people ORDER BY name".into(),
            ..Input::new(file.clone())
        })
        .unwrap();
        assert!(rows.contains("Ada  | 36") && rows.contains("NULL") && rows.ends_with("(2 rows)"));
        let write = query(&Input {
            second: "DELETE FROM people".into(),
            ..Input::new(file)
        });
        assert!(write.is_err(), "read only");
    }

    #[test]
    fn one_path_is_checked_and_opened() {
        assert_eq!(path_of(" \"/ws/a.db\" "), "/ws/a.db");
        assert_eq!(path_of("/ws/a.db\""), "/ws/a.db");
        // `file:` text is a path like any other here, never a URI.
        let uri = query_at(Path::new("file:/etc/x.db"), &Input::default()).unwrap_err();
        assert!(uri.to_string().contains("not a full path"), "{uri}");
        assert!(file_of("//server/share/x.db").is_err());
        assert!(file_of(" \"\" ").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed_when_opened() {
        let dir = std::env::temp_dir().join(format!("ph-utils-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.db");
        Connection::open(&real)
            .unwrap()
            .execute_batch("CREATE TABLE t (x);")
            .unwrap();
        let link = dir.join("link.db");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let canonical_dir = dunce::canonicalize(&dir).unwrap();
        // Checked as the real file, then swapped for a link: refused.
        assert!(query_at(&canonical_dir.join("link.db"), &Input::default()).is_err());
        assert_eq!(
            query_at(&canonical_dir.join("real.db"), &Input::default()).unwrap(),
            "t (0 rows)"
        );
        // Through `query`, the link is resolved once and the target opened.
        assert_eq!(
            file_of(&link.display().to_string()).unwrap(),
            canonical_dir.join("real.db")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
