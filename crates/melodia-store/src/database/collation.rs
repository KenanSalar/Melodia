//! The collation a detail page's title tie-break and the card grids order their text by.
//!
//! `ui::track_sort` orders by `str::to_lowercase()`, a Unicode fold, where `SQLite`'s `NOCASE`
//! folds ASCII alone, so an `ORDER BY` reproducing it needs Rust's own lowercase. The track lists'
//! artist, album and genre orders read that fold stored instead (`queries::track::order_keys`),
//! which an index can carry. **Never name it in schema**: an index, view or trigger using it leaves
//! the table unreadable to any connection without it, the `sqlite3` CLI included.

use std::cmp::Ordering;

use rusqlite::Connection;

/// Spelled in a query as `COLLATE UNICODE_LOWER`.
pub(crate) const UNICODE_LOWER: &str = "UNICODE_LOWER";

/// Registers every collation the queries name, on every connection the pool opens: a connection
/// without one fails any statement that names it.
pub(crate) fn register(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_collation(UNICODE_LOWER, compare_lowercase)
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
