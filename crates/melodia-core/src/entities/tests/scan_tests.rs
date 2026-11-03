//! When a file is the one the store last read, and so isn't read again.

use super::*;

const READ_AT: &str = "2024-01-01T00:00:00+00:00";

fn stamp(file_size: Option<i64>, date_modified: Option<&str>) -> FileStamp {
    FileStamp { file_size, date_modified: date_modified.map(str::to_owned) }
}

/// A match leaves the file unread, so every doubt has to read as changed: an unstamped exclusion
/// is how the next scan learns a removed spare's hash.
#[test]
fn only_the_size_and_mtime_stamped_match() {
    let rows = [
        ("unchanged", stamp(Some(5), Some(READ_AT)), (5, Some(READ_AT)), true),
        ("resized", stamp(Some(5), Some(READ_AT)), (6, Some(READ_AT)), false),
        ("touched", stamp(Some(5), Some(READ_AT)), (5, Some("2025-01-01T00:00:00+00:00")), false),
        ("no mtime either side", stamp(Some(5), None), (5, None), false),
        ("unstamped", stamp(None, None), (5, Some(READ_AT)), false),
    ];
    for (label, stamped, (file_size, date_modified), expected) in rows {
        assert_eq!(stamped.matches(file_size, date_modified), expected, "{label}");
    }
}
