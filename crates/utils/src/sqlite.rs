//! Looking into a SQLite file: its tables, or a query's rows. The file is
//! opened read-only, so nothing in it can change.

use anyhow::{Context as _, Result, bail};
use rusqlite::{Connection, OpenFlags, types::ValueRef};

use crate::Input;

/// Rows shown at most.
const MAX_ROWS: usize = 200;

/// The tables (with their row counts), or the rows of a query.
pub fn query(input: &Input) -> Result<String> {
    let path = input.text.trim().trim_matches('"');
    if path.is_empty() {
        bail!("Give the path of a SQLite file.");
    }
    if is_network_path(path) {
        bail!("{path} is on another computer");
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("could not open {path}"))?;
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
}
