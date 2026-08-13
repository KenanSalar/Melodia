//! Staging-path selection + the `(version, size, url, etag?)` sidecar
//! that fingerprints a partially-downloaded file so cross-release
//! `Range:` resume can't glue mismatched bytes together.
//!
//! Staging path selection branches on the resolved [`InstallMethod`]:
//!
//!   - **[`InstallMethod::AtomicSwap`]** (`AppImage` / tarball) — the
//!     downloaded file goes next to the install target when that
//!     directory is user-writable (`std::fs::rename` stays inside one
//!     filesystem and is atomic). Otherwise the staged file lands
//!     under the user cache dir and the final swap goes through
//!     `pkexec mv` (Linux only).
//!   - **[`InstallMethod::LinuxPackage`]** (RPM / .deb installs
//!     detected via [`super::super::linux_pkg::detect`]) — staged under
//!     `$XDG_CACHE_HOME/Melodia/update-staging/` with the original
//!     `.rpm` / `.deb` suffix preserved so `dnf` / `apt` recognise the
//!     local file.
//!   - **[`InstallMethod::WindowsMsi`]** — staged under
//!     `%LocalAppData%\Melodia\update-staging\` with a `.msi` suffix
//!     so `msiexec /i` recognises the package. Lands in the per-user
//!     cache (writable without admin) — the elevation prompt comes
//!     from msiexec, not the download.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::services::updater::linux_pkg::LinuxPackageFormat;
use crate::services::updater::probe::dir_is_writable;
use crate::services::updater::target::current_target_key;

/// Install strategy resolved from the current `latest.json` target key.
/// Drives both the staging-path choice (where the downloaded artifact
/// lands) and the install method (atomic rename vs. package-manager
/// subprocess vs. msiexec). Keeping the two coupled to one enum
/// ensures the `latest.json` asset choice and the install path stay in
/// lockstep — picking an `.rpm` asset and then trying to atomic-swap
/// it as a raw binary is a bug class this prevents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    /// Direct rename over the live binary (`AppImage` / tarball).
    /// Keeps a `.old` snapshot for post-swap-smoke-test rollback.
    AtomicSwap,
    /// Elevated `dnf install` / `apt install` of a local package.
    LinuxPackage(LinuxPackageFormat),
    /// `msiexec /i <staged>.msi` — elevation comes from the per-machine
    /// MSI's `Scope="perMachine"` (UAC) rather than polkit; the
    /// `MajorUpgrade` element + `util:RestartResource` in `wix/main.wxs`
    /// handle replacing the running app.
    WindowsMsi,
}

/// Maps the resolved target key onto the install strategy.
pub(crate) fn resolve_install_method() -> InstallMethod {
    match current_target_key() {
        Some("linux-x86_64-rpm" | "linux-aarch64-rpm") => {
            InstallMethod::LinuxPackage(LinuxPackageFormat::Rpm)
        }
        Some("linux-x86_64-deb" | "linux-aarch64-deb") => {
            InstallMethod::LinuxPackage(LinuxPackageFormat::Deb)
        }
        Some("windows-x86_64-msi" | "windows-aarch64-msi") => InstallMethod::WindowsMsi,
        // AppImage / tarball / unknown / non-packaged platform fall
        // through to the atomic-swap path. The caller catches
        // unsupported platforms separately via the manifest's
        // `NoAssetForTarget` outcome before we reach the install step.
        _ => InstallMethod::AtomicSwap,
    }
}

/// Stages an `.rpm` / `.deb` under `$XDG_CACHE_HOME/Melodia/update-staging/`
/// with the original asset filename so `dnf install` / `apt install`
/// recognise it as a local package. Falls back to a synthetic name
/// (`melodia-update.rpm` / `melodia-update.deb`) if the URL has no
/// usable basename.
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

/// Stages a Windows `.msi` under `%LocalAppData%\Melodia\update-staging\`
/// with the original asset filename so `msiexec /i` reads the `.msi`
/// extension correctly (msiexec dispatches on extension; renaming to
/// `.new` would silently fail with error 1620 "invalid package"). Falls
/// back to a synthetic name (`melodia-update.msi`) if the URL has no
/// usable basename. The per-user cache dir is writable without
/// elevation — the UAC prompt is deferred to msiexec at install time.
pub(crate) fn staged_msi_path(asset_url: &str) -> AppResult<PathBuf> {
    let cache = dirs::cache_dir().ok_or_else(|| {
        AppError::Settings("could not resolve user cache dir for update staging".into())
    })?;
    let dir = cache.join("Melodia").join("update-staging");
    std::fs::create_dir_all(&dir)?;
    let name = asset_basename(asset_url)
        .filter(|n| {
            // Case-insensitive `.msi` extension check — Windows
            // filesystems are case-insensitive by default, and a
            // manifest with an uppercase `.MSI` URL is plausible
            // (cargo-wix output is lowercase, but a CDN redirect or
            // a hand-renamed asset could differ).
            std::path::Path::new(n).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("msi"))
        })
        .unwrap_or_else(|| "melodia-update.msi".to_string());
    Ok(dir.join(name))
}

/// Removes files older than 7 days from
/// `$XDG_CACHE_HOME/Melodia/update-staging/` — the directory shared by
/// both [`staged_package_path`] (RPM/.deb) and [`resolve_staged_path`]'s
/// cross-fs fallback (tarball / per-user `AppImage` cases). Called from
/// (a) the start of every install attempt, and (b) a fire-and-forget
/// one-shot spawned at boot so a user who dismisses every prompt still
/// gets stale artifacts reaped.
///
/// Failed / auth-cancelled installs deliberately leave their verified
/// bytes on disk for retry-without-redownload; this gathers them up
/// after the 7d retention window. Silent on every error path — the
/// only observable effect is freed disk space.
///
/// All filesystem work runs on Tokio's blocking pool so a pathologically
/// large staging dir (shouldn't happen, but cheap insurance) can't
/// stall the async worker.
pub async fn prune_stale_staging() {
    let _ = tokio::task::spawn_blocking(|| {
        let Some(cache) = dirs::cache_dir() else {
            return;
        };
        let dir = cache.join("Melodia").join("update-staging");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let Some(cutoff) =
            std::time::SystemTime::now().checked_sub(std::time::Duration::from_hours(7 * 24))
        else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
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
    })
    .await;
}

/// Best-effort URL basename extraction. Strips query string and
/// fragment, then takes the last `/`-segment. Returns `None` for an
/// empty result so the caller can fall back to a synthetic name.
fn asset_basename(url: &str) -> Option<String> {
    let no_fragment = url.split_once('#').map_or(url, |(head, _)| head);
    let no_query = no_fragment.split_once('?').map_or(no_fragment, |(head, _)| head);
    let last = no_query.rsplit('/').next()?;
    if last.is_empty() {
        return None;
    }
    Some(last.to_string())
}

/// The download target normally sits next to the install target so
/// the final rename stays inside one filesystem (avoids `EXDEV` on
/// `/tmp` vs the user's home / `/opt` mount). The suffix carries the
/// original extension where possible — `Melodia.exe` ⇒
/// `Melodia.exe.new` — so the rename produces clean side-by-side
/// names mid-swap.
///
/// Used directly in tests; production callers go through
/// [`resolve_staged_path`] which falls back to a user-writable cache
/// dir when the target's parent isn't writable (RPM/.deb installs).
pub(crate) fn staged_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".new");
    target.with_file_name(name)
}

/// Resolves a writable staging path for the downloaded artifact.
///
/// Happy path (per-user installs): the staged file lives next to the
/// install target, the final swap is a same-filesystem
/// `std::fs::rename` — atomic and no elevation needed.
///
/// Fallback (RPM/.deb to `/usr/bin/`): the target dir is root-owned,
/// so we stage under `$XDG_CACHE_HOME/Melodia/update-staging/`
/// instead. The final swap will need a cross-filesystem move via
/// `pkexec mv` (handled in [`super::swap::swap_in_place`] on Linux).
/// The user sees the polkit prompt at swap time, not download time, so
/// a network failure mid-download doesn't waste their authentication.
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
/// The first three fields are the fingerprint [`StagedMeta::matches`] checks:
/// * `version` — the manifest version, and the usual drift signal: CI re-cut
///   the release and the partial belongs to the previous one.
/// * `size` — the manifest-declared byte count, which catches a release
///   re-cut at the *same* version with a different file (`--clobber`).
/// * `asset_url` — catches a flipped target key (partial RPM bytes don't
///   apply once the resolved key is the tarball).
///
/// `etag` is decoupled freshness metadata for the resume protocol, not part
/// of the fingerprint. Captured from the CDN's `ETag` on the original GET and
/// replayed as `If-Range`, so a resource that changed between attempts comes
/// back as a 200 full body rather than a 206 append — closing the hole where
/// two different artifacts get glued together. **Strong tags only**: RFC 9110
/// §13.1.5 forbids `If-Range` with a weak entity-tag, and under §8.8.3.2
/// strong comparison a server receiving one always evaluates it false,
/// silently forcing a full re-download on every resume. Weak tags are
/// filtered at capture in [`super::download::download_to_file`], so this is
/// either a strong tag or `None`.
///
/// `None` covers pre-etag sidecars (hence `#[serde(default)]`), responses
/// without the header, and those filtered weak tags. Correctness doesn't
/// depend on it — the size bound and the post-download signature verify catch
/// a concatenation accident regardless; the etag just shrinks the wasted
/// bandwidth to one zero-byte roundtrip.
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

/// `<staged>.meta.json` — sits next to the staged file (`.new`, `.rpm`,
/// `.deb`, …). Appends to the basename rather than replacing the
/// extension so multi-suffix names like `melodia-0.2.0.x86_64.rpm`
/// don't lose their format suffix.
pub(crate) fn sidecar_meta_path(staged: &Path) -> PathBuf {
    let mut name: OsString =
        staged.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".meta.json");
    staged.with_file_name(name)
}

/// Returns the sidecar if it exists and parses; `None` for missing,
/// unreadable, or corrupted files. The caller's policy on `None` is to
/// discard the staged file alongside (it's not safe to resume bytes
/// without a fingerprint).
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

/// If `dest` carries bytes but the sidecar is missing / corrupted /
/// fingerprint-mismatched, drop both. Caller then re-reads `dest`'s size
/// (will be 0) and `plan_resume` returns `Fresh`. No-op when `dest`
/// doesn't exist or is empty.
///
/// Returns the surviving sidecar (if any) so the caller can extract the
/// cached `ETag` for `If-Range` resume without re-reading the file. `None`
/// means the sidecar was absent, corrupted, fingerprint-mismatched, or
/// orphan-cleaned because `dest` was missing/empty. Tests discard the
/// return value freely — it's purely a hot-path optimisation.
pub(crate) fn discard_staging_if_sidecar_mismatches(
    dest: &Path,
    expected_version: &str,
    expected_size: u64,
    expected_url: &str,
) -> Option<StagedMeta> {
    let sidecar = sidecar_meta_path(dest);
    let existing_size = std::fs::metadata(dest).map_or(0, |m| m.len());
    if existing_size == 0 {
        // No bytes to validate, but clear any orphan sidecar so it
        // doesn't outlive the next prune cycle as misleading state.
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
