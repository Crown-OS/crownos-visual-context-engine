use std::path::{Path, PathBuf};

use context_core::{Episode, SegmentId, TimeRange, Timestamp};
use rusqlite::{Connection, OpenFlags};

use crate::key::{KeyError, load_or_create_key};
use crate::migrations;
use crate::queries::{self, EventRow, SegmentRow, TextHit};

pub const DATABASE_FILE: &str = "context.db";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error("cannot encode a payload: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("the writer thread is gone")]
    WriterGone,
    #[error("forget failed: {0}")]
    Forget(String),
}

/// One connection to the store. The daemon holds one for the writer thread and one or two for
/// reads; the CLI opens its own.
#[derive(Debug)]
pub struct Store {
    connection: Connection,
    path: PathBuf,
}

impl Store {
    /// Opens or creates the database under `state_dir`, with the key kept beside it.
    ///
    /// # Errors
    ///
    /// Fails when the key cannot be read or made, or the database cannot be opened or migrated.
    pub fn open_dir(state_dir: &Path) -> Result<Self, StoreError> {
        let key = load_or_create_key(state_dir)?;
        Self::open(&state_dir.join(DATABASE_FILE), &key)
    }

    /// Opens `path` with `key` (64 hex characters), creating and migrating as needed.
    ///
    /// # Errors
    ///
    /// Fails when the key is wrong for an existing database.
    pub fn open(path: &Path, key: &str) -> Result<Self, StoreError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        apply_key(&connection, key)?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        migrations::migrate(&connection)?;
        Ok(Self {
            connection,
            path: path.to_owned(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn connection(&self) -> &Connection {
        &self.connection
    }

    pub(crate) const fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    pub fn schema_version(&self) -> Result<usize, StoreError> {
        let version: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        Ok(usize::try_from(version).unwrap_or(0))
    }

    pub const fn current_schema_version() -> usize {
        migrations::CURRENT_VERSION
    }

    /// Whether the file's pages are encrypted by this build.
    pub const fn is_encrypted() -> bool {
        cfg!(feature = "sqlcipher")
    }
}

#[cfg(feature = "sqlcipher")]
fn apply_key(connection: &Connection, key: &str) -> Result<(), StoreError> {
    connection.pragma_update(None, "key", format!("\"x'{key}'\""))?;
    connection.pragma_update(None, "cipher_memory_security", "ON")?;
    // Touching the schema is what makes a wrong key fail here rather than at the first query.
    connection.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
    Ok(())
}

#[cfg(not(feature = "sqlcipher"))]
fn apply_key(_connection: &Connection, _key: &str) -> Result<(), StoreError> {
    Ok(())
}

impl Store {
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn events_between(
        &self,
        range: TimeRange,
        limit: u32,
    ) -> Result<Vec<EventRow>, StoreError> {
        queries::events_between(&self.connection, range, limit)
    }

    /// # Errors
    ///
    /// Fails on a database error.
    pub fn segments_between(
        &self,
        range: TimeRange,
        app_key: Option<&str>,
        min_importance: f32,
    ) -> Result<Vec<SegmentRow>, StoreError> {
        queries::segments_between(&self.connection, range, app_key, min_importance)
    }

    /// # Errors
    ///
    /// Fails on a database error.
    pub fn segment(&self, id: SegmentId) -> Result<Option<SegmentRow>, StoreError> {
        queries::segment(&self.connection, id)
    }

    /// # Errors
    ///
    /// Fails on a database error.
    pub fn episodes_between(&self, range: TimeRange) -> Result<Vec<Episode>, StoreError> {
        queries::episodes_between(&self.connection, range)
    }

    /// Lines whose text matches every word of `query`, best first.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn search_text(&self, query: &str, limit: u32) -> Result<Vec<TextHit>, StoreError> {
        queries::search_text(&self.connection, query, limit)
    }

    /// Segments whose url, path or title contains `query`.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn search_locators(&self, query: &str, limit: u32) -> Result<Vec<SegmentId>, StoreError> {
        queries::search_locators(&self.connection, query, limit)
    }

    /// # Errors
    ///
    /// Fails on a database error.
    pub fn load_working_memory(&self) -> Result<Option<(Vec<u8>, Timestamp)>, StoreError> {
        queries::load_working_memory(&self.connection)
    }

    /// Rows in one of the engine's tables, for status and tests.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn count(&self, table: &str) -> Result<u64, StoreError> {
        queries::count(&self.connection, table)
    }

    /// Checks the pages and the index structure.
    ///
    /// # Errors
    ///
    /// Fails when the database is damaged.
    pub fn integrity_check(&self) -> Result<(), StoreError> {
        let verdict: String = self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if verdict == "ok" {
            Ok(())
        } else {
            Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some(verdict),
            )))
        }
    }
}
