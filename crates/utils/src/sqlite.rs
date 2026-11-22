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
    let sql = input.second.trim();
    if sql.is_empty() {
        let tables = tables_at(file)?;
        if tables.is_empty() {
            return Ok("No tables.".into());
        }
        return Ok(tables
            .iter()
            .filter(|table| !table.view)
            .map(|table| format!("{} ({} rows)", table.name, table.rows.unwrap_or(-1)))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    Ok(text_of(&rows_at(file, sql)?))
}

/// The file opened read-only, one file only, with no functions from its
/// own schema.
fn open(file: &Path) -> Result<Connection> {
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
    Ok(db)
}

/// A table or view of a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    pub view: bool,
    /// Rows in a table (views are not counted: that runs their query).
    pub rows: Option<i64>,
    /// Each column's name and declared type.
    pub columns: Vec<(String, String)>,
}

/// The tables and views of the database at `file` (opened as [`query_at`]
/// does), tables first.
pub fn tables_at(file: &Path) -> Result<Vec<TableInfo>> {
    let db = open(file)?;
    within_limit(&db, |db| {
        let mut statement = db.prepare(
            "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )?;
        let listed = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)? == "view"))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        listed
            .into_iter()
            .map(|(name, view)| {
                let quoted = name.replace('"', "\"\"");
                let rows = (!view).then(|| {
                    db.query_row(&format!("SELECT COUNT(*) FROM \"{quoted}\""), [], |row| {
                        row.get(0)
                    })
                    .unwrap_or(-1)
                });
                let mut info = db.prepare(&format!("PRAGMA table_info(\"{quoted}\")"))?;
                let columns = info
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(TableInfo {
                    name,
                    view,
                    rows,
                    columns,
                })
            })
            .collect()
    })
}

/// A value read from a database.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    /// A blob: only its size is kept.
    Blob(usize),
}

impl std::fmt::Display for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cell::Null => f.write_str("NULL"),
            Cell::Integer(value) => write!(f, "{value}"),
            Cell::Real(value) => write!(f, "{value}"),
            Cell::Text(text) => f.write_str(text),
            Cell::Blob(size) => write!(f, "<{size} bytes>"),
        }
    }
}

/// What a query gave: its columns (name, declared type when it comes from
/// a table's column) and up to [`MAX_ROWS`] rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows {
    pub columns: Vec<(String, Option<String>)>,
    pub rows: Vec<Vec<Cell>>,
    /// There were more rows than shown.
    pub more: bool,
}

/// The rows a query that only reads gives, from the database at `file`
/// (opened as [`query_at`] does).
pub fn rows_at(file: &Path, sql: &str) -> Result<Rows> {
    let db = open(file)?;
    within_limit(&db, |db| {
        let mut statement = db.prepare(sql).context("The query is not valid")?;
        // Reading only (`VACUUM INTO` would write a file anywhere).
        if !statement.readonly() {
            bail!("Only queries that read are run here.");
        }
        let columns: Vec<(String, Option<String>)> = statement
            .columns()
            .iter()
            .map(|column| {
                (
                    column.name().to_string(),
                    column.decl_type().map(str::to_string),
                )
            })
            .collect();
        let count = columns.len();
        let mut rows = statement.query([])?;
        let mut read = Vec::new();
        let mut more = false;
        while let Some(row) = rows.next()? {
            if read.len() == MAX_ROWS {
                more = true;
                break;
            }
            read.push(
                (0..count)
                    .map(|ix| match row.get_ref(ix) {
                        Ok(ValueRef::Null) | Err(_) => Cell::Null,
                        Ok(ValueRef::Integer(value)) => Cell::Integer(value),
                        Ok(ValueRef::Real(value)) => Cell::Real(value),
                        Ok(ValueRef::Text(text)) => {
                            Cell::Text(String::from_utf8_lossy(text).into_owned())
                        }
                        Ok(ValueRef::Blob(blob)) => Cell::Blob(blob.len()),
                    })
                    .collect(),
            );
        }
        Ok(Rows {
            columns,
            rows: read,
            more,
        })
    })
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

/// The rows as an aligned table, for agents.
fn text_of(rows: &Rows) -> String {
    let mut table: Vec<Vec<String>> =
        vec![rows.columns.iter().map(|(name, _)| name.clone()).collect()];
    table.extend(
        rows.rows
            .iter()
            .map(|row| row.iter().map(ToString::to_string).collect()),
    );
    let widths: Vec<usize> = (0..rows.columns.len())
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
    out.push(if rows.more {
        format!("(the first {MAX_ROWS} rows)")
    } else {
        format!("({} rows)", rows.rows.len())
    });
    out.join("\n")
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
        let tables = tables_at(&dunce::canonicalize(&path).unwrap()).unwrap();
        assert_eq!(tables[0].rows, Some(2));
        assert_eq!(
            tables[0].columns[1],
            ("age".to_string(), "INTEGER".to_string())
        );
        let rows = rows_at(
            &dunce::canonicalize(&path).unwrap(),
            "SELECT age FROM people ORDER BY name",
        )
        .unwrap();
        assert_eq!(rows.columns[0].1.as_deref(), Some("INTEGER"));
        assert_eq!(rows.rows, vec![vec![Cell::Integer(36)], vec![Cell::Null]]);
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
