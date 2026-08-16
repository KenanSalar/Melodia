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
pub mod single_instance;
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

/// Read JSON from `path`, falling back to `T::default()` on a missing file or a
/// parse error. The sync variant, for startup before the runtime exists.
pub fn load_json_or_default_sync<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str::<T>(&content).unwrap_or_else(|e| {
        log::warn!("Failed to parse {}, using defaults: {e}", path.display());
        T::default()
    }))
}

/// [`load_json_or_default_sync`]'s async twin.
pub async fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Ok(T::default());
    };
    Ok(serde_json::from_str::<T>(&content).unwrap_or_else(|e| {
        log::warn!("Failed to parse {}, using defaults: {e}", path.display());
        T::default()
    }))
}

/// Write `value` as pretty JSON through a temp file in the same directory,
/// renaming on success — so a crash mid-write leaves the previous file intact,
/// and nothing allocates the whole payload as a `String` first.
pub fn write_json_atomic_sync<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value).map_err(AppError::io_source)?;
        writer.flush()?;
    }
    tmp.persist(path).map_err(|e| AppError::Io(e.error))?;
    Ok(())
}

/// Build the process-wide shared `reqwest::Client`. Kept out of any constructor
/// so the rustls stack and connection pool load only on the first real request;
/// both `OnceLock` holders init through this, so the app reuses one pool.
///
/// The deadline is **per read, not whole-body**: a legitimately slow download
/// may take minutes, but no single read should sit silent that long. It resets
/// on every byte, so it only trips on a genuinely dead socket. The build is
/// documented infallible for these options; the fallback is logged paranoia.
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

/// [`write_json_atomic_sync`]'s plain-text sibling, for M3U export. Bytes go out
/// verbatim — the caller owns line endings and the trailing newline.
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
/// from is unlinked — which an RPM/DEB upgrade does to `/usr/bin/Melodia` while
/// it runs, and cargo does to `target/debug/Melodia` on every re-uplift. The
/// suffixed path names nothing, so every consumer fails or writes nonsense.
///
/// It **resolves** rather than merely trimming, which is what makes it correct
/// rather than cosmetic: in both cases the replacement file sits at the stripped
/// path, so respawning from it relaunches the binary the user now has.
///
/// **The marker can only appear mid-session** — you cannot exec an unlinked
/// path — which is what sorts the callers. The late ones meet it: the post-exit
/// respawn, which without this dies and takes the app with it, and
/// `spawn_install`'s pre-swap [`updater::install_target`] capture.
/// `desktop_integration`'s `Exec=` line and `linux_pkg::detect`'s package-DB
/// lookup reach it too, and are why `install_target` routes through here — but
/// both run at boot, so today they are defended without it. **The second of
/// those defences is one edit away**: `detect` caches, and every later caller
/// reads that cached answer. Drop the cache and the daily check, the panic hook
/// and the staging path all ask a fresh `rpm -qf` mid-session, squarely inside
/// the window. Their failure compounds — a marked path makes `rpm -qf` miss, so
/// the updater offers a tarball to an RPM install and `desktop_integration`
/// writes the marker into the user's launcher.
///
/// Reach for this over `std::env::current_exe()` anywhere the path will be
/// executed, installed to, or written down. Inside the updater go through
/// [`updater::install_target`], which answers the `$APPIMAGE` question first.
pub fn current_exe() -> std::io::Result<PathBuf> {
    Ok(undeleted_exe(std::env::current_exe()?, Path::exists))
}

/// The pure half of [`current_exe`], with `exists` standing in for the filesystem.
///
/// The order of the three guards is the whole of it: the suffix test first, so
/// the common case costs no `stat`; a suffixed path that is itself a live file
/// wins over its sibling, a file genuinely named `… (deleted)` not being this
/// bug; and anything unresolved comes back verbatim, so the caller's error still
/// reports what the kernel said.
///
/// Deliberately not `cfg`-gated to Linux — no other platform produces the
/// marker, and the live-file guard makes it inert where a path ends that way by
/// coincidence. The strip goes through `to_str`, the kernel appending to the
/// whole path string; a non-UTF-8 path comes back unchanged rather than reaching
/// for the `unsafe` `OsStr::from_encoded_bytes_unchecked`.
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
/// The home directory comes from [`dirs::home_dir`], **not** `$HOME`: that
/// variable is a Unix convention, normally unset on Windows — exactly where this
/// earns its keep, a GUI-subsystem build having no console to have read the
/// paths from instead. The crate answers with `FOLDERID_Profile` there and reads
/// `$HOME` on Unix, so Unix is unchanged and Windows starts working.
///
/// Resolved per call rather than cached, which is a trade: four tests across
/// three files drive this through `$HOME`, so a process-wide cache would put the
/// answer out of their reach. Nor is the cost uniform — Unix reads the variable,
/// falling back to a `getpwuid_r` that behind a networked NSS module can do real
/// I/O. A bundle makes tens of these and a crash report two, so it stays per
/// call; anything hotter wants the answer passed in rather than a cache the
/// tests can't reset.
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
/// A great many `Display` impls in and under this tree are a context sentence
/// with the cause reachable only through `.source()` — `AppError`'s four
/// I/O-boundary variants by construction, and `FlexiLoggerError`'s arms because
/// they are static sentences that never interpolate the `io::Error` they hold.
/// So a bare `{e}` reports a root-owned file and a full disk in the same words.
///
/// **The other kind is what the `ends_with` skip is for, and why this is safe to
/// reach for without knowing which variant you hold.** `AppError`'s three
/// `#[from]` variants spell `#[error("… : {0}")]` over the field `#[from]` also
/// makes the source, and sqlx does the same one level down, so an unconditional
/// walk prints a constraint failure three times. A caller can't tell the two
/// shapes apart; the error can — a message already ending in its cause has
/// nothing left to add.
///
/// Reach for this in any `log::` call taking an error.
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
