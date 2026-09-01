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
pub mod radio_blocklist;
pub mod radio_browser;
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

/// Read JSON from `path`, falling back to `T::default()` on a missing file or a parse error. The
/// sync variant, for startup before the runtime exists.
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

/// Write `value` as pretty JSON through a temp file in the same directory, renaming on success —
/// so a crash mid-write leaves the previous file intact, and nothing allocates the whole payload
/// as a `String` first.
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

/// Build the process-wide shared `reqwest::Client`. Kept out of any constructor so the rustls
/// stack and connection pool load only on the first real request; both `OnceLock` holders init
/// through this, so the app reuses one pool.
///
/// The deadline is **per read, not whole-body**: a legitimately slow download may take minutes,
/// but no single read should sit silent that long. The build is documented infallible for these
/// options; the fallback is logged paranoia.
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

/// `candidate` as an absolute `http`/`https` URL that names a host, or `None`.
///
/// **The parse is the check, and that is the whole point.** A `starts_with("http://")` test admits
/// the bare scheme, which names nothing and is not a fetch anything can make — two of the four
/// spellings this replaced did exactly that, and one of them was on the station-import path, so a
/// line reading `http://` became a row. It also gets case for free, `Url` lowercasing the scheme
/// where a prefix test has to remember to.
///
/// Everything that takes a URL from outside the app goes through here: a station's website field,
/// its logo URL, and the lines of a `.pls`/`.m3u`/`.asx` pointer.
pub(crate) fn http_url(candidate: &str) -> Option<reqwest::Url> {
    let parsed = reqwest::Url::parse(candidate.trim()).ok()?;
    is_http(&parsed).then_some(parsed)
}

/// [`http_url`] where only the verdict is wanted. Two callers wrote this line out for themselves.
pub(crate) fn is_http_url(candidate: &str) -> bool {
    http_url(candidate).is_some()
}

/// The rule itself, asked of a URL already parsed. [`http_url`] is this plus the parse.
///
/// `Url::join` returns an absolute URI unchanged, so a playlist line reading `file:///etc/passwd`
/// or `data:…` comes back out of it as a `Url` like any other. Nothing downstream re-asks, and the
/// text form is gone by then, so the check has to be reachable on the parsed value too.
pub(crate) fn is_http(url: &reqwest::Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.has_host()
}

/// GET `url` and read at most `max_bytes` of what comes back.
///
/// [`read_capped`] is the half that holds; this is the request around it, plus the two cheap
/// refusals that come before a byte is read — a non-success status, and a `Content-Length` already
/// over the cap. The header check is a courtesy a host can omit or lie about, which is why it sits
/// here rather than instead of the streamed bound.
///
/// `what` is a noun phrase, capitalized as every [`read_capped`] caller passes one: it is the
/// subject of all four messages the two halves can raise, so a refusal points at the right half of
/// a two-request fetch. Timeout and cap are the caller's, a station's playlist and one of its
/// segments being two orders of magnitude apart on the cap.
pub(crate) async fn get_capped(
    client: &reqwest::Client,
    url: &reqwest::Url,
    what: &str,
    timeout: std::time::Duration,
    max_bytes: u64,
) -> Result<Vec<u8>, AppError> {
    let response = client
        .get(url.clone())
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| AppError::network(format!("{what} could not be fetched"), e))?;
    if !response.status().is_success() {
        return Err(AppError::network_msg(format!(
            "{what} request returned HTTP {}",
            response.status().as_u16()
        )));
    }
    if response.content_length().is_some_and(|len| len > max_bytes) {
        // Worded as `read_capped` words it, the two refusing the same thing from either side of
        // the download.
        return Err(AppError::network_msg(format!("{what} is larger than {max_bytes} bytes")));
    }
    read_capped(response, what, max_bytes).await
}

/// [`get_capped`] for a body that is text.
///
/// **The fallback arm is the whole reason this is not one line at each call site.**
/// `from_utf8` *moves* the bytes it was handed, so a well-formed body costs nothing, where
/// `from_utf8_lossy` on an owned `Vec` borrows and then copies the lot, which is a copy of every
/// playlist a station reloads for the life of it. The lossy path stays on the error arm rather
/// than being dropped for the cheaper spelling: a mount serving one Latin-1 byte in a track title
/// should get its replacement character, not a refusal that takes the station off the air.
pub(crate) async fn get_capped_text(
    client: &reqwest::Client,
    url: &reqwest::Url,
    what: &str,
    timeout: std::time::Duration,
    max_bytes: u64,
) -> Result<String, AppError> {
    let body = get_capped(client, url, what, timeout, max_bytes).await?;
    Ok(String::from_utf8(body)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// Ceiling on the capacity a `Content-Length` may claim before a byte has arrived. High enough to
/// skip the cheap end of the growth chain on every body here, low enough that a host overstating
/// its length buys one hint rather than the caller's whole cap, which for the largest of them is
/// two orders of magnitude more.
const READ_HINT_MAX_BYTES: u64 = 64 * 1024;

/// Read at most `max_bytes` of `response`, refusing as soon as the body crosses the cap.
///
/// **Streamed rather than `bytes()`-ed**, and that is the whole point: a `Content-Length` check
/// ahead of the call is a courtesy a host can omit or lie about, so a cap enforced only after
/// `bytes()` has returned has already allocated whatever was sent. **Every response body in the
/// tree is read here**, the updater's streamed-to-disk download aside, and a `.json::<T>()` is not
/// the exemption it looks like: it allocates the whole body before serde sees a byte, so a typed
/// decode bounds the *shape* and nothing about the size. Each caller brings its own `max_bytes` and
/// its own `what`, which names the thing in the error so a refusal points at the right half of a
/// two-request fetch.
pub(crate) async fn read_capped(
    response: reqwest::Response,
    what: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, AppError> {
    use futures_util::StreamExt;

    // The same header the cap deliberately doesn't trust is still a fine allocation hint, clamped
    // by [`READ_HINT_MAX_BYTES`] because it is a claim. It buys the reallocations up to the clamp,
    // not the ones past it: a body larger than the hint still grows the rest of the way, which for
    // an HLS segment arriving every few seconds is the point worth being honest about.
    let hint = response.content_length().unwrap_or(0).min(max_bytes).min(READ_HINT_MAX_BYTES);
    let mut body = Vec::with_capacity(usize::try_from(hint).unwrap_or(0));
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| AppError::network(format!("{what} could not be read"), e))?;
        if body.len().saturating_add(chunk.len()) as u64 > max_bytes {
            return Err(AppError::network_msg(format!("{what} is larger than {max_bytes} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// [`write_json_atomic_sync`]'s plain-text sibling, for M3U export. Bytes go out verbatim — the
/// caller owns line endings and the trailing newline.
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
/// `std::env::current_exe()` is a bare `readlink("/proc/self/exe")` on Linux, and the kernel
/// appends that literal suffix once the dentry the process was exec'd from is unlinked. It
/// **resolves** rather than merely trimming, which is what makes it correct rather than cosmetic:
/// the replacement file sits at the stripped path, so respawning from it relaunches the binary the
/// user now has.
///
/// **The marker can only appear mid-session** — you cannot exec an unlinked path — which is what
/// sorts the callers; `.claude/rules/updater.md` walks that list and what their failure compounds
/// into.
///
/// Reach for this over `std::env::current_exe()` anywhere the path will be executed, installed to,
/// or written down. Inside the updater go through [`updater::install_target`], which answers the
/// `$APPIMAGE` question first.
pub fn current_exe() -> std::io::Result<PathBuf> {
    Ok(undeleted_exe(std::env::current_exe()?, Path::exists))
}

/// The pure half of [`current_exe`], with `exists` standing in for the filesystem.
///
/// The order of the three guards is the whole of it: the suffix test first, so the common case
/// costs no `stat`; a suffixed path that is itself a live file wins over its sibling, a file
/// genuinely named `… (deleted)` not being this bug; and anything unresolved comes back verbatim,
/// so the caller's error still reports what the kernel said.
///
/// Deliberately not `cfg`-gated to Linux — no other platform produces the marker, and the
/// live-file guard makes it inert where a path ends that way by coincidence. The strip goes
/// through `to_str`; a non-UTF-8 path comes back unchanged rather than reaching for the `unsafe`
/// `OsStr::from_encoded_bytes_unchecked`.
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

/// Whether the running binary came out of the source tree rather than an install.
///
/// A `cfg!(debug_assertions)` alone would miss `cargo build --release`, which is a real way to run
/// this tree and produces a binary indistinguishable from a shipped one except for where it sits —
/// hence the second, path-shaped answer.
///
/// The raw `std::env::current_exe()` is deliberate where [`current_exe`] is otherwise the rule: the
/// `" (deleted)"` marker lands on the file name, which nothing below looks at.
#[must_use]
pub fn is_dev_build() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // .../target/<profile>/<binary>  →  parent = <profile>, grandparent = target
    exe.parent()
        .and_then(Path::parent)
        .is_some_and(|p| p.file_name().is_some_and(|n| n == "target"))
}

/// Replace the user's home directory with `~` throughout `text`.
///
/// Everything a crash report or diagnostics bundle carries goes through this before reaching a
/// file the user is asked to attach to a public issue — a home directory usually holds a real name.
///
/// The home directory comes from [`dirs::home_dir`], **not** `$HOME`; the root `CLAUDE.md` argues
/// why, and the short of it is that the variable is normally unset on Windows, exactly where this
/// earns its keep.
///
/// Resolved per call rather than cached, which is a trade: four tests across three files drive
/// this through `$HOME`, so a process-wide cache would put the answer out of their reach. A bundle
/// makes tens of these and a crash report two; anything hotter wants the answer passed in rather
/// than a cache the tests can't reset.
pub fn redact_home(text: &str) -> Cow<'_, str> {
    let Some(home) = home_dir_string() else {
        return Cow::Borrowed(text);
    };
    redact_prefix(text, &home)
}

/// The home directory as a string, or `None` when there isn't one to redact.
///
/// Reachable from `test_support::resolved_home` so a redaction fixture is built from the same
/// answer the redaction reads, rather than from a second guess at it.
pub(crate) fn home_dir_string() -> Option<String> {
    let home = dirs::home_dir()?;
    let home = home.to_str()?;
    (!home.is_empty()).then(|| home.to_owned())
}

/// Flatten an error and its causes onto one line.
///
/// A great many `Display` impls in and under this tree are a context sentence with the cause
/// reachable only through `.source()`, so a bare `{e}` reports a root-owned file and a full disk
/// in the same words.
///
/// **The other kind is what the `ends_with` skip is for, and why this is safe to reach for without
/// knowing which variant you hold.** `AppError`'s three `#[from]` variants spell
/// `#[error("… : {0}")]` over the field `#[from]` also makes the source, and sqlx does the same
/// one level down, so an unconditional walk prints a constraint failure three times. A caller
/// can't tell the two shapes apart; the error can — a message already ending in its cause has
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
