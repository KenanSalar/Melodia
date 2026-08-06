//! Handing a URL or a path to the desktop's own launcher.
//!
//! `open::that` spawns and waits on a child process (`xdg-open` / `open` /
//! `explorer`), so every caller has to get off the UI thread first — and every
//! caller then has to distinguish a launcher that refused from a blocking task
//! that died, because the two say different things about what went wrong.
//! That pair is the whole of this module.
//!
//! Failures are logged and swallowed: nothing here is load-bearing enough to
//! surface, and a user whose `xdg-open` is broken already knows. The one caller
//! that *does* need a typed error is `library::tracks::reveal_in_file_manager`,
//! which reports a vanished folder back to the UI — it stays on its own path.

use std::ffi::OsStr;

/// Open `target` with the desktop's default handler, off the current thread.
///
/// `label` prefixes both failure logs; make it name the action rather than the
/// module, since that is what a reader grepping the log has.
pub async fn open_target<T>(target: T, label: &'static str)
where
    T: AsRef<OsStr> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || open::that(target)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::warn!("{label}: open::that failed: {e}"),
        Err(e) => log::warn!("{label}: launch task join failed: {e}"),
    }
}
