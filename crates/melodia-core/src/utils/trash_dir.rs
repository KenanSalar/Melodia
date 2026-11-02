//! The folders file managers trash into, which a library walk prunes and the watcher ignores.
//! Only those: music kept under any other dot-folder is still walked.

use std::path::Path;

/// Whether `dir` is a trash folder: a volume trash by its name, or the home trash by its path.
/// Every volume spelling matches on every OS, since a drive can carry another OS's trash.
pub fn is_trash_dir(dir: &Path) -> bool {
    dir.file_name().is_some_and(|name| is_volume_trash_name(name.as_encoded_bytes()))
        || is_home_trash(dir)
}

/// Whether any folder above `path` is a trash folder.
pub fn in_trash_dir(path: &Path) -> bool {
    path.ancestors().skip(1).any(is_trash_dir)
}

/// freedesktop's `.Trash` and `.Trash-$uid`, macOS's `.Trashes`, and Windows' `$RECYCLE.BIN` in
/// any case, its filesystems being case-insensitive.
fn is_volume_trash_name(name: &[u8]) -> bool {
    if let Some(uid) = name.strip_prefix(b".Trash-") {
        return !uid.is_empty() && uid.iter().all(u8::is_ascii_digit);
    }
    name == b".Trash" || name == b".Trashes" || name.eq_ignore_ascii_case(b"$RECYCLE.BIN")
}

/// freedesktop's home trash, matched by path since `Trash` is a name any album folder could carry.
/// Canonical, as library roots are stored, or a symlinked `/home` would never match.
#[cfg(target_os = "linux")]
fn is_home_trash(dir: &Path) -> bool {
    use std::path::PathBuf;
    use std::sync::LazyLock;

    static HOME_TRASH: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
        let data_dir = dirs::data_dir()?;
        // A data dir that won't resolve holds nothing a walk could find.
        let data_dir = super::canonicalize_path(&data_dir).unwrap_or(data_dir);
        Some(data_dir.join("Trash"))
    });
    HOME_TRASH.as_deref() == Some(dir)
}

/// macOS's home trash, `~/.Trash`, is matched by name, and Windows keeps one bin per drive.
#[cfg(not(target_os = "linux"))]
fn is_home_trash(_dir: &Path) -> bool {
    false
}
