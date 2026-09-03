pub mod atomic_file;

use std::io;
use std::path::{Path, PathBuf};

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Cross-platform `canonicalize` that avoids the Windows verbatim
/// (`\\?\C:\...`) form. On Windows we route through `dunce::canonicalize`
/// — it strips the `\\?\` prefix whenever the resolved path fits in
/// `MAX_PATH`, so the returned `PathBuf` round-trips through string
/// storage (`folders.path`, `tracks.file_path`) and equality-matches
/// what `std::fs::read_dir` yields. On non-Windows it forwards to
/// `std::fs::canonicalize`. Use this everywhere a canonicalized path
/// is persisted, displayed to the user, or used as a DB key.
#[cfg(windows)]
pub fn canonicalize_path<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

#[cfg(not(windows))]
pub fn canonicalize_path<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}
