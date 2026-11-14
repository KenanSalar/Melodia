//! Whether a file whose read failed may read later. One out of reach is tried again at the next
//! launch, while one still there that won't read would fail the same way, so its failure is kept.

use std::error::Error;
use std::path::Path;

/// Whether `error`, failing a read of `path`, may lift: the file has gone, or another program
/// holds it locked.
pub fn is_out_of_reach(path: &Path, error: &(dyn Error + 'static)) -> bool {
    is_held_elsewhere(error) || is_gone(path)
}

/// Whether `path` has gone, as on an unmounted drive. Where that can't be told it counts as gone,
/// a retry costing less than a failure kept for good.
pub fn is_gone(path: &Path) -> bool {
    !matches!(path.try_exists(), Ok(true))
}

/// Windows refuses an open another handle's sharing excludes, and std answers such a file as
/// existing, so the refusal is read off the error.
#[cfg(target_os = "windows")]
fn is_held_elsewhere(error: &(dyn Error + 'static)) -> bool {
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

    std::iter::successors(Some(error), |&cause| cause.source())
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|cause| cause.raw_os_error() == Some(ERROR_SHARING_VIOLATION.cast_signed()))
}

/// Elsewhere a lock is advisory and refuses no open.
#[cfg(not(target_os = "windows"))]
fn is_held_elsewhere(_: &(dyn Error + 'static)) -> bool {
    false
}
