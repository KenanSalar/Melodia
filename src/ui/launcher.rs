//! Handing a URL or a path to the desktop's own launcher.
//!
//! `open::that_detached` still forks and execs a child process (`xdg-open` /
//! `open` / `explorer`), so every caller has to get off the UI thread first —
//! and every caller then has to distinguish a launcher that refused from a
//! blocking task that died, because the two say different things about what
//! went wrong. That pair is the whole of this module.
//!
//! **Detached, not `open::that`**, which `waitpid`s the launcher: on a setup
//! where `xdg-open` falls through to `$BROWSER` rather than handing off over
//! D-Bus, that is the browser's whole lifetime. `scrobbling_settings` awaits
//! this call before it can offer the paste-token field, so there the wait is a
//! panel stuck on its spinner until the user closes their browser.
//!
//! The trade is one class of failure: a launcher that *starts* and then exits
//! non-zero — `xdg-open`'s "no application found" — has an exit status only
//! `that` is around to read, so it is now silent. A launcher that can't be
//! spawned at all still errors and is still logged.
//!
//! Failures are logged and swallowed either way: nothing here is load-bearing
//! enough to surface, and a user whose `xdg-open` is broken already knows. The
//! one caller that *does* need a typed error is
//! `library::tracks::reveal_in_file_manager`, which reports a vanished folder
//! back to the UI — it stays on its own path.

use std::ffi::OsStr;

/// Open `target` with the desktop's default handler, off the current thread.
///
/// `label` prefixes both failure logs; make it name the action rather than the
/// module, since that is what a reader grepping the log has.
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
