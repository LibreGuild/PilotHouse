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
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("could not open {path}"))?;
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
