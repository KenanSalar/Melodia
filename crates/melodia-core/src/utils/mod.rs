pub mod atomic_file;
pub mod audio_ext;
pub mod block_move;
pub mod event_bridge;
pub mod exe;
pub mod fold;
pub mod missing_artwork;
pub mod needle;
pub mod play_counts;
pub mod reach;
pub mod redact;
pub mod reinsert;
pub mod self_writes;
pub mod text;
pub mod toast;
pub mod trash_dir;

use std::io;
use std::path::{Path, PathBuf};

use jiff::tz::Offset;
use jiff::{SignedDuration, Timestamp};

pub fn now_rfc3339() -> String {
    rfc3339(Timestamp::now())
}

/// `at` in UTC as RFC 3339, the spelling every stored stamp shares.
///
/// `+00:00`, never `Z`: `+` sorts below `.` and the digits, so stamps order as text whatever the
/// fraction's width. It keeps 0, 3, 6 or 9 digits because `scanner::file_is_current` compares a
/// stored `date_modified` byte for byte, and another width reads every file as changed.
pub fn rfc3339(at: Timestamp) -> String {
    let digits = fraction_digits(at.subsec_nanosecond());
    format!("{:.digits$}", at.display_with_offset(Offset::UTC))
}

fn fraction_digits(nanos: i32) -> usize {
    if nanos == 0 {
        0
    } else if nanos % 1_000_000 == 0 {
        3
    } else if nanos % 1_000 == 0 {
        6
    } else {
        9
    }
}

/// `days` whole days before `at`, or `at` itself where that falls outside `Timestamp`'s range.
pub fn days_before(at: Timestamp, days: i64) -> Timestamp {
    SignedDuration::try_from_hours(days.saturating_mul(24))
        .and_then(|duration| at.checked_sub(duration).ok())
        .unwrap_or(at)
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> i64 {
    Timestamp::now().as_second()
}

/// `canonicalize` without Windows' verbatim `\\?\` form, which `dunce` strips wherever the path
/// fits `MAX_PATH`, so a stored path equality-matches what `read_dir` yields. For every path that
/// is persisted, shown or used as a DB key.
#[cfg(windows)]
pub fn canonicalize_path<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

#[cfg(not(windows))]
pub fn canonicalize_path<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Whether the directory at `dir` has anything to read, which tells an offline library folder from
/// one online: an unmounted drive's mount point is there but empty.
pub fn holds_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}
