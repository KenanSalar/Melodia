pub mod atomic_file;
pub mod audio_ext;
pub mod block_move;
pub mod event_bridge;
pub mod exe;
pub mod fold;
pub mod missing_artwork;
pub mod needle;
pub mod play_counts;
pub mod redact;
pub mod reinsert;
pub mod self_writes;
pub mod text;
pub mod toast;

use std::io;
use std::path::{Path, PathBuf};

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
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
