//! Staging-path selection, plus the sidecar that fingerprints a partially
//! downloaded file so cross-release `Range:` resume can't glue mismatched bytes
//! together.
//!
//! The path branches on the resolved [`InstallMethod`]. An
//! [`InstallMethod::AtomicSwap`] stages *beside* the install target where that
//! directory is user-writable, keeping the final `rename` inside one filesystem;
//! otherwise it falls back to the user cache dir and a `pkexec mv`. The two
//! package methods always stage in the cache dir, keeping the original suffix so
//! `dnf` / `apt` / `msiexec` recognise the local file — writable without
//! elevation, which is deferred to install time.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use melodia_core::error::{AppError, AppResult};
use melodia_platform::services::platform::install_kind::linux_pkg::LinuxPackageFormat;
use melodia_platform::services::platform::install_kind::probe::dir_is_writable;
use melodia_platform::services::platform::install_kind::target::current_target_key;

/// Install strategy resolved from the current `latest.json` target key, driving
/// both where the artifact lands and how it is installed. One enum for both, so
/// the asset choice and the install path can't drift — picking an `.rpm` and
/// then atomic-swapping it as a raw binary is the bug class that prevents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    /// Direct rename over the live binary, keeping a `.old` snapshot for
    /// post-smoke-test rollback.
    AtomicSwap,
    /// Elevated `dnf install` / `apt install` of a local package.
    LinuxPackage(LinuxPackageFormat),
    /// `msiexec /i` — elevation comes from the per-machine MSI's UAC prompt, and
    /// `wix/main.wxs`'s `MajorUpgrade` + `util:RestartResource` replace the
    /// running app.
    WindowsMsi,
}

/// Maps the resolved target key onto the install strategy.
pub(crate) fn resolve_install_method() -> InstallMethod {
    install_method_for_key(current_target_key())
}

/// The mapping itself, split from the host lookup the way `format_key` is split from
/// `current_target_key` next door and for the same reason: a typo in one of the six literals
/// falls through to `AtomicSwap` and renames a package over the live binary, and on a host that
/// answers with one key the other five are unreachable from a test.
pub(crate) fn install_method_for_key(key: Option<&str>) -> InstallMethod {
    match key {
        Some("linux-x86_64-rpm" | "linux-aarch64-rpm") => {
            InstallMethod::LinuxPackage(LinuxPackageFormat::Rpm)
        }
        Some("linux-x86_64-deb" | "linux-aarch64-deb") => {
            InstallMethod::LinuxPackage(LinuxPackageFormat::Deb)
        }
        Some("windows-x86_64-msi" | "windows-aarch64-msi") => InstallMethod::WindowsMsi,
        // AppImage, tarball and unknown all fall through to the swap. An
        // unsupported platform is caught earlier, by the manifest's
        // `NoAssetForTarget` outcome.
        _ => InstallMethod::AtomicSwap,
    }
}

/// Stage an `.rpm` / `.deb` in the cache dir under the original asset filename,
/// so the package manager recognises it as a local package. A URL with no usable
/// basename falls back to a synthetic name.
pub(super) fn staged_package_path(
    asset_url: &str,
    format: LinuxPackageFormat,
) -> AppResult<PathBuf> {
    let cache = dirs::cache_dir().ok_or_else(|| {
        AppError::Settings("could not resolve user cache dir for update staging".into())
    })?;
    let dir = cache.join("Melodia").join("update-staging");
    std::fs::create_dir_all(&dir)?;
    let fallback = match format {
        LinuxPackageFormat::Rpm => "melodia-update.rpm",
        LinuxPackageFormat::Deb => "melodia-update.deb",
    };
    let suffix = match format {
        LinuxPackageFormat::Rpm => ".rpm",
        LinuxPackageFormat::Deb => ".deb",
    };
    let name = asset_basename(asset_url)
        .filter(|n| n.ends_with(suffix))
        .unwrap_or_else(|| fallback.to_string());
    Ok(dir.join(name))
}

/// [`staged_package_path`]'s Windows twin. msiexec dispatches on the extension,
/// so a file renamed to `.new` fails with error 1620, "invalid package".
pub(crate) fn staged_msi_path(asset_url: &str) -> AppResult<PathBuf> {
    let cache = dirs::cache_dir().ok_or_else(|| {
        AppError::Settings("could not resolve user cache dir for update staging".into())
    })?;
    let dir = cache.join("Melodia").join("update-staging");
    std::fs::create_dir_all(&dir)?;
    let name = asset_basename(asset_url)
        .filter(|n| {
            // Case-insensitively: Windows filesystems are, and a CDN redirect
            // or hand-renamed asset can hand back an uppercase `.MSI`.
            std::path::Path::new(n).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("msi"))
        })
        .unwrap_or_else(|| "melodia-update.msi".to_string());
    Ok(dir.join(name))
}

/// Reap the staging dir every method shares. Runs at the start of each install
/// attempt *and* as a one-shot at boot, so a user who dismisses every prompt
/// still gets stale artifacts collected.
///
/// A failed or cancelled install deliberately leaves its verified bytes for
/// retry-without-redownload; this gathers them once the retention window
/// closes. Silent on every error path — the only observable effect is disk
/// space. Filesystem work goes on the blocking pool.
pub async fn prune_stale_staging() {
    let _ = tokio::task::spawn_blocking(|| {
        let Some(cache) = dirs::cache_dir() else {
            return;
        };
        let Some(cutoff) = std::time::SystemTime::now().checked_sub(STAGING_TTL) else {
            return;
        };
        prune_dir(&cache.join("Melodia").join("update-staging"), cutoff);
    })
    .await;
}

/// How long a verified artifact is kept for a retry that never came. Long enough to span the
/// week a user might leave a prompt dismissed, short enough that an abandoned install isn't a
/// permanent tenant of the cache dir.
const STAGING_TTL: std::time::Duration = std::time::Duration::from_hours(7 * 24);

/// The sweep itself, over a directory it is handed. Split from the cache-dir lookup so the
/// cutoff can be driven from both sides, this being the only thing in the module that deletes.
fn prune_dir(dir: &Path, cutoff: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        // Directories are nobody's staged artifact, and recursing would put a user-chosen cache
        // subtree within reach of a sweep that only understands flat files.
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < cutoff {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Best-effort URL basename: strip fragment and query, take the last segment.
/// `None` on an empty result, so the caller can fall back to a synthetic name.
fn asset_basename(url: &str) -> Option<String> {
    let no_fragment = url.split_once('#').map_or(url, |(head, _)| head);
    let no_query = no_fragment.split_once('?').map_or(no_fragment, |(head, _)| head);
    let last = no_query.rsplit('/').next()?;
    if last.is_empty() {
        return None;
    }
    Some(last.to_string())
}

/// `<target>.new`, beside the install target so the final rename stays inside
/// one filesystem. Appends rather than replaces, so `Melodia.exe` keeps its
/// extension and the two names read as siblings mid-swap.
///
/// Used directly by tests; production goes through [`resolve_staged_path`].
pub(crate) fn staged_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".new");
    target.with_file_name(name)
}

/// A writable staging path for the downloaded artifact: beside the install
/// target on a per-user install, so the swap is an atomic same-filesystem
/// rename needing no elevation; in the cache dir when that parent is root-owned,
/// where the swap becomes a `pkexec mv`. The polkit prompt therefore comes at
/// swap time rather than download time, so a mid-download network failure
/// doesn't waste the user's authentication.
pub(super) fn resolve_staged_path(target: &Path) -> AppResult<PathBuf> {
    let primary = staged_path(target);
    if primary.parent().is_some_and(dir_is_writable) {
        return Ok(primary);
    }
    let cache = dirs::cache_dir().ok_or_else(|| {
        AppError::Settings("could not resolve user cache dir for update staging".into())
    })?;
    let dir = cache.join("Melodia").join("update-staging");
    std::fs::create_dir_all(&dir)?;
    let mut name = target.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".new");
    Ok(dir.join(name))
}

/// Metadata sidecar that fingerprints a staged file so a partial download
/// from a previous release can't be glued onto a current-release fetch via
/// `Range:` resume. Written next to the staged file at download start;
/// validated on every later attempt before deciding `plan_resume`.
///
/// The first three fields are the fingerprint [`StagedMeta::matches`] checks,
/// and each catches a different drift: `version` a re-cut release, `size` a
/// re-cut at the *same* version with a different file, `asset_url` a flipped
/// target key.
///
/// `etag` is freshness metadata for the resume protocol rather than part of the
/// fingerprint — captured from the original GET and replayed as `If-Range`, so a
/// resource that changed between attempts comes back as a full 200 rather than a
/// 206 append. **Strong tags only**: RFC 9110 §13.1.5 forbids `If-Range` with a
/// weak entity-tag, and under §8.8.3.2 a server receiving one always evaluates
/// it false, silently forcing a full re-download on every resume. Weak tags are
/// filtered at capture, so this is a strong tag or `None`.
///
/// `None` also covers pre-etag sidecars and responses without the header.
/// Correctness doesn't depend on it — the size bound and the post-download
/// signature verify catch a concatenation accident regardless; the etag only
/// shrinks the wasted bandwidth to one zero-byte round trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StagedMeta {
    pub(crate) version: String,
    pub(crate) size: u64,
    pub(crate) asset_url: String,
    #[serde(default)]
    pub(crate) etag: Option<String>,
}

impl StagedMeta {
    pub(crate) fn matches(&self, version: &str, size: u64, asset_url: &str) -> bool {
        self.version == version && self.size == size && self.asset_url == asset_url
    }
}

/// `<staged>.meta.json`, beside the staged file. Appends rather than replacing
/// the extension, so a multi-suffix name keeps its format suffix.
pub(crate) fn sidecar_meta_path(staged: &Path) -> PathBuf {
    let mut name: OsString =
        staged.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".meta.json");
    staged.with_file_name(name)
}

/// The sidecar if it exists and parses. `None` means the caller discards the
/// staged file too — bytes with no fingerprint aren't safe to resume.
pub(crate) fn read_staged_meta(path: &Path) -> Option<StagedMeta> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn write_staged_meta(path: &Path, meta: &StagedMeta) -> AppResult<()> {
    let bytes = serde_json::to_vec(meta)
        .map_err(|e| AppError::Settings(format!("failed to serialise staging sidecar: {e}")))?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Drop both files when `dest` carries bytes their sidecar can't fingerprint;
/// the caller then re-reads a size of 0 and `plan_resume` answers `Fresh`.
///
/// Hands back the surviving sidecar so the caller can take its `ETag` without
/// re-reading the file — purely an optimisation, so tests discard it freely.
pub(crate) fn discard_staging_if_sidecar_mismatches(
    dest: &Path,
    expected_version: &str,
    expected_size: u64,
    expected_url: &str,
) -> Option<StagedMeta> {
    let sidecar = sidecar_meta_path(dest);
    let existing_size = std::fs::metadata(dest).map_or(0, |m| m.len());
    if existing_size == 0 {
        // Nothing to validate, but an orphan sidecar left here outlives the next
        // prune cycle as misleading state.
        let _ = std::fs::remove_file(&sidecar);
        return None;
    }
    let meta = read_staged_meta(&sidecar);
    let valid =
        meta.as_ref().is_some_and(|m| m.matches(expected_version, expected_size, expected_url));
    if valid {
        return meta;
    }
    log::info!(
        "updater: discarding staged file at {} — sidecar missing or fingerprint mismatch \
         (a different release was being resumed)",
        dest.display()
    );
    let _ = std::fs::remove_file(dest);
    let _ = std::fs::remove_file(&sidecar);
    None
}

#[cfg(test)]
#[path = "../tests/staging_tests.rs"]
mod tests;
