//! The collation the track lists order their text columns by.
//!
//! `ui::track_sort` orders artist, album, genre and a detail's title tie-break by
//! `str::to_lowercase()`, a Unicode fold, where `SQLite`'s `NOCASE` folds ASCII alone, so an
//! `ORDER BY` reproducing those arms needs Rust's own lowercase. **Never name it in schema**: an
//! index, view or trigger using it leaves the table unreadable to any connection without it, the
//! `sqlite3` CLI included.

use std::cmp::Ordering;

use sqlx::sqlite::SqliteConnectOptions;

/// Spelled in a query as `COLLATE UNICODE_LOWER`.
pub(crate) const UNICODE_LOWER: &str = "UNICODE_LOWER";

/// Registers every collation the queries name, on every pool's options: a connection without one
/// fails any statement that names it.
pub(crate) fn register(options: SqliteConnectOptions) -> SqliteConnectOptions {
    options.collation(UNICODE_LOWER, compare_lowercase)
}

/// `a.to_lowercase().cmp(&b.to_lowercase())`, without allocating when both sides are ASCII, where
/// the two folds agree. Anything else goes through `to_lowercase` itself, which is what keeps a
/// final sigma (`Σ` closing a word lowers to `ς`) in the order the comparator puts it.
fn compare_lowercase(a: &str, b: &str) -> Ordering {
    if a.is_ascii() && b.is_ascii() {
        return ascii_lowercase_bytes(a).cmp(ascii_lowercase_bytes(b));
    }
    a.to_lowercase().cmp(&b.to_lowercase())
}

fn ascii_lowercase_bytes(s: &str) -> impl Iterator<Item = u8> + '_ {
    s.bytes().map(|byte| byte.to_ascii_lowercase())
}
