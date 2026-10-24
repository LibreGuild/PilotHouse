//! Local persistence for PilotHouse.
//!
//! Everything PilotHouse remembers lives in one SQLite file in the user's data
//! directory (no cloud, no account). [`Store`] owns the connection; each module
//! groups the records and queries for one capability:
//!
//! - [`projects`]: registered project folders and their saved UI layout.
//! - [`sessions`]: chat sessions with an agent and their transcript.
//! - [`board`]: Kanban cards and their activity log.
//! - [`settings`]: app-wide preferences as JSON values.
//!
//! The store is synchronous: every query is a small indexed read or write that
//! finishes in well under a millisecond, so callers may use it from the UI
//! thread. Bulk work should still go through a background task.

pub mod board;
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
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
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
