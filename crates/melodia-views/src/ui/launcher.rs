//! Handing a URL or a path to the desktop's own launcher.
//!
//! **Detached, not `open::that`**, which `waitpid`s the launcher: where `xdg-open` falls
//! through to `$BROWSER` rather than handing off over D-Bus, that is the browser's whole
//! lifetime, and `scrobbling_settings` awaits this call before it can offer the
//! paste-token field. The trade is the late failure: `that_detached` watches each launcher
//! for a short window, falling through to the next on a failed exit, and a launcher that
//! fails after it goes unseen.
//!
//! It still forks, execs and waits out that window, so callers get off the UI thread first.
//! Both failures are logged and swallowed; `library::tracks::reveal_in_file_manager` needs a
//! typed error for a vanished folder and stays on its own path.

use std::ffi::OsStr;

use melodia_core::error::describe;

/// Open `target` with the desktop's default handler, off the current thread. `label`
/// prefixes both failure logs; name the action rather than the module.
pub async fn open_target<T>(target: T, label: &'static str)
where
    T: AsRef<OsStr> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || open::that_detached(target)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::warn!("{label}: open::that_detached failed: {}", describe(&e)),
        Err(e) => log::warn!("{label}: launch task join failed: {}", describe(&e)),
    }
}
