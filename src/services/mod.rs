pub mod always_on_top;
pub mod artist_images;
pub mod crash_report;
#[cfg(target_os = "linux")]
pub mod desktop_integration;
pub mod diagnostics;
pub mod discord;
#[cfg(target_os = "windows")]
pub mod dwm_titlebar;
pub mod logging;
pub mod material_you;
pub mod media_controls;
pub mod scrobble;
pub mod search_history;
pub mod settings;
#[cfg(target_os = "linux")]
pub mod system_theme;
pub mod toast;
pub mod tray;
pub mod updater;
pub mod view_state;

use std::borrow::Cow;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{AppError, AppResult};

/// Read JSON from `path` and deserialize to `T`. Missing file or parse error
/// fall back to `T::default()` (parse errors are logged at warn level). Sync
/// variant — used at startup before the tokio runtime exists.
pub fn load_json_or_default_sync<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str::<T>(&content).unwrap_or_else(|e| {
        log::warn!(
            "Failed to parse {}, using defaults: {e}",
            path.display()
        );
        T::default()
    }))
}

/// Async variant of [`load_json_or_default_sync`]. Both missing-file and
/// parse-error paths fall back to `T::default()`.
pub async fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Ok(T::default());
    };
    Ok(serde_json::from_str::<T>(&content).unwrap_or_else(|e| {
        log::warn!(
            "Failed to parse {}, using defaults: {e}",
            path.display()
        );
        T::default()
    }))
}

/// Atomically write `value` as pretty JSON to `path` by streaming through a
/// temp file in the same directory and renaming on success. Avoids the
/// allocate-entire-payload-as-String pattern of `to_string_pretty` + `write`,
/// and a crash mid-write leaves the previous file intact.
pub fn write_json_atomic_sync<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value)
            .map_err(AppError::io_source)?;
        writer.flush()?;
    }
    tmp.persist(path).map_err(|e| AppError::Io(e.error))?;
    Ok(())
}

/// Build the process-wide shared `reqwest::Client`. Kept out of any `new`/
/// constructor so the rustls TLS stack and connection pool only load the first
/// time something actually makes a request (the updater, the Deezer artist-image
/// fetch, or a scrobble). Both [`crate::state::AppState::http_client`] and
/// [`crate::services::scrobble::ScrobbleService`] init a single shared
/// `OnceLock` with this, so the whole app reuses one connection pool.
///
/// Timeouts use a per-read (not whole-body) deadline: a legitimately slow large
/// download may take minutes, but no single read should sit silent for a minute
/// on a wedged socket. `read_timeout` resets on every byte, so it only trips
/// when the socket is genuinely dead. The pool cap and descriptive User-Agent
/// bound idle memory and make server logs useful. The build is documented
/// infallible for these options; the fallback is logged paranoia (losing the
/// timeouts if it ever fires).
pub(crate) fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_mins(1))
        .pool_max_idle_per_host(4)
        .user_agent(concat!("Melodia/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|e| {
            log::warn!(
                "reqwest::Client::builder().build() failed unexpectedly ({e}); falling back to \
                 default client without timeouts — downloads may hang on a wedged socket"
            );
            reqwest::Client::new()
        })
}

/// Atomically write plain `text` to `path` via a temp file in the same
/// directory + rename on success. Plain-text sibling of
/// [`write_json_atomic_sync`] (used by M3U playlist export). Bytes are
/// written verbatim — the caller owns line endings and trailing newline.
pub fn write_text_atomic_sync(path: &Path, text: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        writer.write_all(text.as_bytes())?;
        writer.flush()?;
    }
    tmp.persist(path).map_err(|e| AppError::Io(e.error))?;
    Ok(())
}

/// The running binary's path, with Linux's `" (deleted)"` marker resolved.
///
/// `std::env::current_exe()` is a bare `readlink("/proc/self/exe")` on Linux, and
/// the kernel appends that literal suffix once the dentry the process was exec'd
/// from has been unlinked — which is what an RPM/DEB upgrade does to
/// `/usr/bin/Melodia` while it is running, and what cargo does to
/// `target/debug/Melodia` every time it re-uplifts it from `deps/` (unlink, then
/// hardlink, so the inode's mtime never moves and the file looks untouched). The
/// suffixed path names nothing, so every consumer of it fails or writes nonsense.
///
/// It **resolves** rather than merely trimming, and that is what makes it correct
/// rather than cosmetic: in both cases above the replacement file sits at the
/// stripped path, so respawning from it relaunches the binary the user now has.
///
/// **The marker can only appear mid-session** — you cannot exec an unlinked path,
/// so at boot there is nothing to strip. That is what sorts the callers. The ones
/// that meet a marked path are the late ones: the post-exit respawn (through
/// [`crate::ui::window_chrome::respawn_target`]), which without this dies and takes
/// the app with it, and `spawn_install`'s pre-swap [`updater::install_target`]
/// capture. `desktop_integration`'s `Exec=` line and `linux_pkg::detect`'s
/// package-DB lookup reach it too, and are the reason `install_target` routes
/// through here rather than only the respawn — but both run at boot, so today they
/// are defended without it. **By two things, and the second is the one that can be
/// edited away.** `detect` runs at boot, and it also caches: its `OnceLock` is
/// primed by `is_system_install()` on a root-owned install and by
/// `desktop_integration` on a writable one, and every *later* caller reads that
/// answer — `check_for_update` on the daily task and on the user's Check button,
/// the panic hook through `current_target_key()`, and the install staging path.
/// Drop the cache and those three are asking a fresh `rpm -qf` mid-session, which
/// is squarely inside the window. Ordering and memoization are both incidental;
/// the defence is worth keeping. Their failure compounds, which is what it buys: a
/// marked path makes `rpm -qf` miss, so `detect` returns `None`, so the updater
/// offers a tarball to an RPM install and `desktop_integration` stops skipping and
/// writes the marker into the user's launcher.
///
/// Reach for this over `std::env::current_exe()` anywhere the path is going to be
/// executed, installed to, or written down. Inside the updater, go through
/// [`updater::install_target`] instead — it answers the `$APPIMAGE` question first.
pub fn current_exe() -> std::io::Result<PathBuf> {
    Ok(undeleted_exe(std::env::current_exe()?, Path::exists))
}

/// The pure half of [`current_exe`], with `exists` standing in for the filesystem.
///
/// The order of the three guards is the whole of it. The suffix test comes first,
/// so the overwhelmingly common case costs no `stat`; a suffixed path that is
/// itself a live file then wins over its sibling, because a file genuinely named
/// `… (deleted)` is not this bug; and anything left unresolved comes back verbatim,
/// so the caller's error still reports what the kernel said.
///
/// Deliberately not `cfg`-gated to Linux — no other platform produces the marker,
/// and the live-file guard makes it inert where a path ends that way by
/// coincidence. The strip goes through `to_str` because the kernel appends to the
/// whole path string; a non-UTF-8 path is returned unchanged rather than reaching
/// for `OsStr::from_encoded_bytes_unchecked`, which is `unsafe`.
fn undeleted_exe(exe: PathBuf, exists: impl Fn(&Path) -> bool) -> PathBuf {
    const DELETED_MARKER: &str = " (deleted)";

    let Some(base) = exe.to_str().and_then(|p| p.strip_suffix(DELETED_MARKER)) else {
        return exe;
    };
    let base = PathBuf::from(base);
    if exists(&exe) || !exists(&base) {
        return exe;
    }
    base
}

/// Replace the user's home directory with `~` throughout `text`.
///
/// Everything a crash report or diagnostics bundle carries goes through this
/// before reaching a file the user is asked to attach to a public issue — a
/// home directory usually holds a real name.
///
/// The home directory comes from [`dirs::home_dir`], **not** from `$HOME`:
/// that variable is a Unix convention and is normally unset on Windows, which
/// is exactly where this feature earns its keep (a GUI-subsystem build has no
/// console to have read the paths from instead). The crate answers with
/// `FOLDERID_Profile` there and reads `$HOME` on Unix, so the Unix behaviour is
/// unchanged and the Windows one starts existing.
///
/// Resolved per call rather than cached, and that is a trade rather than a free
/// choice — four tests across three files drive this through `$HOME`, so a
/// process-wide cache would put the answer out of their reach. The cost isn't
/// uniform either: Unix reads the variable (falling back to `getpwuid_r`, which
/// behind a networked NSS module can do real I/O), Windows calls
/// `SHGetKnownFolderPath`. A bundle makes on the order of twenty of these and a
/// crash report two, so it stays per call; anything hotter wants the answer
/// passed in rather than a cache the tests can't reset.
pub fn redact_home(text: &str) -> Cow<'_, str> {
    let Some(home) = home_dir_string() else {
        return Cow::Borrowed(text);
    };
    redact_prefix(text, &home)
}

/// The home directory as a string, or `None` when there isn't one to redact.
fn home_dir_string() -> Option<String> {
    let home = dirs::home_dir()?;
    let home = home.to_str()?;
    (!home.is_empty()).then(|| home.to_owned())
}

/// Flatten an error and its causes onto one line.
///
/// A great many `Display` impls in and under this tree are a context sentence with
/// the cause reachable only through `.source()` — `AppError`'s four I/O-boundary
/// variants by construction (`#[source]` on the boxed cause), and
/// `FlexiLoggerError`'s arms because they are static sentences that never
/// interpolate the `io::Error` they hold. So a bare `{e}` reports a root-owned
/// file and a full disk in the same words, and a log line is left saying which
/// operation failed but not why.
///
/// **The other kind is what the `ends_with` skip is for, and it is why this is
/// safe to reach for without knowing which variant you hold.** `AppError`'s three
/// `#[from]` variants spell `#[error("… : {0}")]` over the field `#[from]` also
/// makes the source, so `Display` has already printed what the walk is about to
/// append — and sqlx does the same thing one level down, so a plain walk prints a
/// constraint failure three times. A caller handed a bare `AppError` cannot tell
/// the two shapes apart, and the one that can is the error itself: a message
/// already ending in its cause has nothing left to add.
///
/// Reach for this in any `log::` call taking an error. A variant carrying only a
/// message flattens to itself, and one that interpolates its source flattens to
/// exactly what `{e}` would have printed.
pub(crate) fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        let message = source.to_string();
        if !text.ends_with(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        cause = source.source();
    }
    text
}

/// The pure half of [`redact_home`]. Borrows when there is nothing to replace,
/// which is the common case.
fn redact_prefix<'a>(text: &'a str, home: &str) -> Cow<'a, str> {
    if !text.contains(home) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace(home, "~"))
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
