//! SQLite databases, read-only: the tables and views down the side, the
//! picked one's rows in a table.

use std::path::{Path, PathBuf};

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, px,
};
use rusqlite::{Connection, OpenFlags, types::ValueRef};

use crate::byte_size;
use crate::table::{TableData, TableGrid};

/// Rows read from a table at most.
const MAX_ROWS: usize = 5_000;
/// Characters of a text value shown.
const MAX_CELL: usize = 2_000;
/// How long reading a table may take: a view is a query the database's
/// author wrote, and one can run for ever.
#[cfg(not(test))]
const TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);
#[cfg(test)]
const TIME_LIMIT: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub name: String,
    /// `table` or `view`.
    pub kind: String,
}

pub struct SqliteView {
    path: PathBuf,
    objects: Option<Result<Vec<Object>, SharedString>>,
    selected: Option<String>,
    grid: Entity<TableGrid>,
    error: Option<SharedString>,
    _load: Option<Task<()>>,
    _rows: Option<Task<()>>,
}

impl SqliteView {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            objects: None,
            selected: None,
            grid: cx.new(|cx| TableGrid::new(window, cx)),
            error: None,
            _load: None,
            _rows: None,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let read = cx.background_spawn(async move { objects(&path) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read
                .await
                .map_err(|err| SharedString::from(format!("{err:#}")));
            _ = this.update(cx, |this, cx| {
                let first = result
                    .as_ref()
                    .ok()
                    .and_then(|objects| objects.first().map(|object| object.name.clone()));
                let keep = this.selected.clone().filter(|name| {
                    result
                        .as_ref()
                        .is_ok_and(|objects| objects.iter().any(|object| &object.name == name))
                });
                this.objects = Some(result);
                if let Some(name) = keep.or(first) {
                    this.select(name, cx);
                }
                cx.notify();
            });
        }));
    }

    fn select(&mut self, name: String, cx: &mut Context<Self>) {
        self.selected = Some(name.clone());
        let path = self.path.clone();
        let read = cx.background_spawn(async move { rows(&path, &name) });
        self._rows = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok((data, note)) => {
                        this.error = None;
                        this.grid
                            .update(cx, |grid, cx| grid.set_data(data, note.map(Into::into), cx));
                    }
                    Err(err) => this.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    // Read-only: looking never changes the database, and a database another
    // program is writing is read as it stands.
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // The schema is the file's author's: views and triggers it holds may
    // not call functions with effects outside the query.
    connection.pragma_update(None, "trusted_schema", false)?;
    Ok(connection)
}

/// Runs `read` on `connection`, interrupting it after [`TIME_LIMIT`].
fn within_limit<T>(
    connection: &Connection,
    read: impl FnOnce(&Connection) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let interrupt = connection.get_interrupt_handle();
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let watch = std::thread::spawn(move || {
        if finished.recv_timeout(TIME_LIMIT) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            interrupt.interrupt();
        }
    });
    let result = read(connection);
    drop(done);
    _ = watch.join();
    result.map_err(|err| match err.downcast_ref::<rusqlite::Error>() {
        Some(rusqlite::Error::SqliteFailure(failure, _))
            if failure.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            anyhow::anyhow!(
                "Reading it took too long (over {} s).",
                TIME_LIMIT.as_secs()
            )
        }
        _ => err,
    })
}

/// The database's tables and views, by name.
pub fn objects(path: &Path) -> anyhow::Result<Vec<Object>> {
    let connection = open(path)?;
    let mut statement = connection.prepare(
        "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') \
         AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok(Object {
                name: row.get(0)?,
                kind: row.get(1)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(objects)
}

/// The first rows of table (or view) `name`, and a note when there are more.
pub fn rows(path: &Path, name: &str) -> anyhow::Result<(TableData, Option<String>)> {
    let connection = open(path)?;
    let quoted = format!("\"{}\"", name.replace('"', "\"\""));
    let data = within_limit(&connection, |connection| read_rows(connection, &quoted))?;
    // Counting is only for the note: a slow count is left out.
    let total: i64 = within_limit(&connection, |connection| {
        Ok(
            connection.query_row(&format!("SELECT COUNT(*) FROM {quoted}"), [], |row| {
                row.get(0)
            })?,
        )
    })
    .unwrap_or(-1);
    let note = (total > MAX_ROWS as i64).then(|| format!("first {MAX_ROWS} of {total} rows"));
    Ok((data, note))
}

fn read_rows(connection: &Connection, quoted: &str) -> anyhow::Result<TableData> {
    let mut statement = connection.prepare(&format!("SELECT * FROM {quoted} LIMIT {MAX_ROWS}"))?;
    let columns: Vec<SharedString> = statement
        .column_names()
        .into_iter()
        .map(|name| SharedString::from(name.to_string()))
        .collect();
    let count = columns.len();
    let rows = statement
        .query_map([], |row| {
            (0..count)
                .map(|ix| Ok(SharedString::from(cell(row.get_ref(ix)?))))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TableData { columns, rows })
}

fn cell(value: ValueRef) -> String {
    match value {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => value.to_string(),
        ValueRef::Text(text) => {
            let text = String::from_utf8_lossy(text);
            match text.char_indices().nth(MAX_CELL) {
                Some((end, _)) => format!("{}…", &text[..end]),
                None => text.into_owned(),
            }
        }
        ValueRef::Blob(bytes) => format!("<blob, {}>", byte_size(bytes.len() as u64)),
    }
}

impl Render for SqliteView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let objects = match &self.objects {
            None => return crate::notice("Opening the database…", cx),
            Some(Err(err)) => return crate::notice(format!("Not a readable database: {err}"), cx),
            Some(Ok(objects)) if objects.is_empty() => {
                return crate::notice("This database has no tables.", cx);
            }
            Some(Ok(objects)) => objects.clone(),
        };
        let theme = cx.theme();
        let list = objects.into_iter().enumerate().map(|(ix, object)| {
            let selected = self.selected.as_deref() == Some(object.name.as_str());
            let name = object.name.clone();
            h_flex()
                .id(("db-object", ix))
                .px_2()
                .py_1()
                .gap_1()
                .rounded(theme.radius)
                .text_sm()
                .cursor_pointer()
                .when(selected, |this| this.bg(theme.list_active))
                .when(!selected, |this| {
                    this.hover(|this| this.bg(theme.list_hover))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(object.name.clone()),
                )
                .when(object.kind == "view", |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("view"),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| this.select(name.clone(), cx)))
        });
        h_flex()
            .size_full()
            .bg(theme.background)
            .child(
                v_flex()
                    .id("db-objects")
                    .w(px(200.))
                    .flex_none()
                    .h_full()
                    .overflow_y_scroll()
                    .p_1()
                    .gap_0p5()
                    .border_r_1()
                    .border_color(theme.border)
                    .children(list),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .map(|this| match &self.error {
                        Some(error) => this.child(crate::notice(error.clone(), cx)),
                        None => this.child(self.grid.clone()),
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_tables_and_reads_rows() {
        let path = std::env::temp_dir().join(format!("ph-sqlite-{}.db", std::process::id()));
        _ = std::fs::remove_file(&path);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE \"my \"\"items\" (id INTEGER, name TEXT, data BLOB);
                 INSERT INTO \"my \"\"items\" VALUES (1, 'a', x'0102'), (2, NULL, NULL);
                 CREATE VIEW names AS SELECT name FROM \"my \"\"items\";",
            )
            .unwrap();
        drop(connection);
        let found = objects(&path).unwrap();
        assert_eq!(found[0].name, "my \"items");
        assert_eq!(found[1].kind, "view");
        let (data, note) = rows(&path, "my \"items").unwrap();
        assert_eq!(data.columns, ["id", "name", "data"]);
        assert_eq!(data.rows[0], ["1", "a", "<blob, 2 bytes>"]);
        assert_eq!(data.rows[1][1], "NULL");
        assert!(note.is_none());
    }

    #[test]
    fn a_view_that_never_ends_is_stopped() {
        let path = std::env::temp_dir().join(format!("ph-sqlite-loop-{}.db", std::process::id()));
        _ = std::fs::remove_file(&path);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE VIEW forever AS WITH RECURSIVE n(i) AS \
                 (SELECT 1 UNION ALL SELECT i + 1 FROM n) SELECT i FROM n WHERE i < 0;",
            )
            .unwrap();
        drop(connection);
        let err = rows(&path, "forever").unwrap_err();
        assert!(format!("{err:#}").contains("too long"), "{err:#}");
        _ = std::fs::remove_file(&path);
    }
}
