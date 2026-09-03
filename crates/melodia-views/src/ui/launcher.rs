//! Handing a URL or a path to the desktop's own launcher.
//!
//! **Detached, not `open::that`**, which `waitpid`s the launcher: where `xdg-open` falls
//! through to `$BROWSER` rather than handing off over D-Bus, that is the browser's whole
//! lifetime, and `scrobbling_settings` awaits this call before it can offer the
//! paste-token field. The trade is one class of failure — a launcher that *starts* and
//! then exits non-zero has a status only `that` is around to read, so `xdg-open`'s "no
//! application found" is now silent.
//!
//! It still forks and execs, so callers get off the UI thread first. Both failures are
//! logged and swallowed; `library::tracks::reveal_in_file_manager` needs a typed error for
//! a vanished folder and stays on its own path.

use std::ffi::OsStr;

/// Open `target` with the desktop's default handler, off the current thread. `label`
/// prefixes both failure logs; name the action rather than the module.
pub async fn open_target<T>(target: T, label: &'static str)
where
    T: AsRef<OsStr> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || open::that_detached(target)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::warn!("{label}: open::that_detached failed: {e}"),
        Err(e) => log::warn!("{label}: launch task join failed: {e}"),
    }
}
