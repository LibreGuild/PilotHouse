//! Local persistence for PilotHouse.
//!
//! Everything PilotHouse remembers lives in one SQLite file in the user's data
//! directory (no cloud, no account). [`Store`] owns the connection; each module
//! groups the records and queries for one capability:
//!
//! - [`projects`]: registered project folders and their saved UI layout.
//! - [`sessions`]: chat sessions with an agent and their transcript.
//! - [`settings`]: app-wide preferences as JSON values.
//!
//! The store is synchronous: every query is a small indexed read or write that
//! finishes in well under a millisecond, so callers may use it from the UI
//! thread. Bulk work should still go through a background task.

pub mod explanations;
mod migrations;
pub mod parallel;
pub mod paths;
pub mod projects;
pub mod reviews;
pub mod schedules;
pub mod search;
pub mod sessions;
pub mod settings;
mod time;
pub mod todos;
pub mod usage;

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context as _, Result};
use rusqlite::Connection;

pub use time::now_millis;

/// Shared handle to the PilotHouse database. Cloning is cheap.
#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    /// Settings read a moment ago (see [`settings`]).
    settings: Arc<Mutex<settings::Recent>>,
    /// A second connection for long reads (see [`Store::read`]).
    reader: Arc<Mutex<Reader>>,
}

/// The database file and, once a long read needed it, a read-only
/// connection to it.
struct Reader {
    file: Option<std::path::PathBuf>,
    connection: Option<Connection>,
}

impl Store {
    /// Opens (or creates) the database at `path` and applies pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            paths::ensure_private_dir(parent)
                .with_context(|| format!("creating data directory {}", parent.display()))?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        Self::from_connection(connection)
    }

    /// An in-memory database, used by tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Applies the migrations of an organisation's own tables, on a track of
    /// their own beside PilotHouse's (see `docs/customizing.md`). Call it once
    /// at startup with every migration of the track, oldest first; only the
    /// ones this database does not have yet run. Name tables `x_<track>_…`.
    pub fn apply_track(&self, track: &str, migrations: &[&str]) -> Result<()> {
        migrations::apply_track(&self.conn(), track, migrations)
    }

    /// The database file, or `None` for an in-memory one.
    pub fn file(&self) -> Option<std::path::PathBuf> {
        self.conn()
            .path()
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        // WAL lets the MCP bridge process read while the app writes.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        migrations::apply(&connection)?;
        let file = connection
            .path()
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from);
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            settings: Default::default(),
            reader: Arc::new(Mutex::new(Reader {
                file,
                connection: None,
            })),
        })
    }

    /// Runs a long read (one that may take more than a moment on a big
    /// database) on a second, read-only connection, opened when first
    /// needed: WAL lets it read while the main one writes, and the UI
    /// thread's next store call does not wait for it. An in-memory store
    /// (tests) has no file to open twice and uses its one connection.
    pub(crate) fn read<T>(&self, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut reader = self
            .reader
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(file) = reader.file.clone() else {
            drop(reader);
            return read(&self.conn());
        };
        let connection = match &mut reader.connection {
            Some(connection) => connection,
            empty => empty.insert(
                Connection::open_with_flags(
                    &file,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .with_context(|| format!("opening database {} to read", file.display()))?,
            ),
        };
        read(connection)
    }

    /// Locks the connection for one unit of work.
    ///
    /// A poisoned lock only means a previous query panicked mid-way; SQLite
    /// itself is still consistent, so we keep going rather than crash the app.
    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
