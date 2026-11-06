//! Opening a connection for a role: its flags, its pragmas, and the setup every role shares.

use std::path::Path;
use std::time::Duration;

use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags};

use super::super::collation;

/// What a lane opens its connections as.
#[derive(Clone, Copy)]
pub(super) enum Role {
    Writer,
    Reader,
}

/// How long any connection waits on a lock, the checkpointer's among them, before `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Prepared statements each connection keeps, the least recently used evicted first. Sized with
/// headroom over a scan's working set, the largest a connection cycles through: an LRU smaller
/// than a loop's statements evicts each before its next use, so the loop re-prepares every one.
const STATEMENT_CACHE_CAPACITY: usize = 100;

/// Small for both file roles: a desktop library's working set fits, and [`MMAP_SIZE`] keeps cold
/// pages file-backed rather than in each connection's heap. Negative, so in KiB rather than pages.
const CACHE_SIZE_KIB: i64 = -16000;

/// The map every file connection reads through, so a library-sized read stays in the file-backed
/// page cache instead of being copied into that connection's heap. The writer reads whole tables
/// too: a bulk scan's stats recalc walks every track.
const MMAP_SIZE: i64 = 268_435_456;

pub(super) fn open(path: &Path, role: Role) -> rusqlite::Result<Connection> {
    let flags = match role {
        Role::Writer => {
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
        }
        Role::Reader => OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    };
    let conn = Connection::open_with_flags(path, flags)?;
    configure(&conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    if matches!(role, Role::Writer) {
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    conn.pragma_update(None, "cache_size", CACHE_SIZE_KIB)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "mmap_size", MMAP_SIZE)?;
    Ok(conn)
}

/// The database [`DbPool::test_pool`](super::DbPool::test_pool) lives in, gone when it closes.
pub(super) fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(conn)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
    reject_double_quoted_strings(conn)?;
    collation::register(conn)
}

/// `SQLite` otherwise reads a double-quoted name matching no column as a string literal, so a
/// misspelled identifier queries a constant rather than failing. A schema holding one still loads.
fn reject_double_quoted_strings(conn: &Connection) -> rusqlite::Result<()> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)?;
    Ok(())
}
