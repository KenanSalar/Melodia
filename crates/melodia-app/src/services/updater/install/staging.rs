//! Where a download is staged, and the sidecar that fingerprints a partial one so a `Range:`
//! resume can't glue two releases' bytes together.
//!
//! A swap stages beside its target where that directory is writable, keeping the final rename on
//! one filesystem, and in the user cache dir otherwise. The package methods always stage in the
//! cache dir under the asset's own name, which dnf, apt and msiexec dispatch on.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use melodia_core::error::{AppError, AppResult};
use melodia_platform::services::platform::install_kind::linux_pkg::LinuxPackageFormat;
use melodia_platform::services::platform::install_kind::probe::dir_is_writable;
use melodia_platform::services::platform::install_kind::target::current_target_key;

/// How this install is updated, deciding both where the artifact lands and how it goes in, so an
/// `.rpm` can't be staged by one and swapped in raw by the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    /// A rename over the live binary, keeping a `.old` for rollback.
    AtomicSwap,
    /// Elevated `dnf install` / `apt install` of a local package.
    LinuxPackage(LinuxPackageFormat),
    /// `msiexec /i` of the per-machine MSI, which replaces and reopens the running app itself.
    WindowsMsi,
}

pub(crate) fn resolve_install_method() -> InstallMethod {
    install_method_for_key(current_target_key())
}

/// Split from the host lookup so all six keys are reachable from a test: a typo in one falls
/// through to `AtomicSwap` and renames a package over the live binary.
pub(crate) fn install_method_for_key(key: Option<&str>) -> InstallMethod {
    match key {
        Some("linux-x86_64-rpm" | "linux-aarch64-rpm") => {
            InstallMethod::LinuxPackage(LinuxPackageFormat::Rpm)
        }
        Some("linux-x86_64-deb" | "linux-aarch64-deb") => {
            InstallMethod::LinuxPackage(LinuxPackageFormat::Deb)
        }
        Some("windows-x86_64-msi" | "windows-aarch64-msi") => InstallMethod::WindowsMsi,
        // AppImage and tarball; an unknown platform stopped earlier, at `NoAssetForTarget`.
        _ => InstallMethod::AtomicSwap,
    }
}

/// The cache-dir path for an `.rpm` or `.deb`, under the asset's own name where the URL has one.
pub(super) fn staged_package_path(
    asset_url: &str,
    format: LinuxPackageFormat,
) -> AppResult<PathBuf> {
    let dir = staging_dir()?;
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
    let dir = staging_dir()?;
    let name = asset_basename(asset_url)
        .filter(|n| {
            // Windows filesystems are case-insensitive, so an `.MSI` counts.
            std::path::Path::new(n).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("msi"))
        })
        .unwrap_or_else(|| "melodia-update.msi".to_string());
    Ok(dir.join(name))
}

/// Reaps the staging dir at each install attempt and once at boot, so the bytes a failed or
/// cancelled install kept for a retry go once `STAGING_TTL` has passed. Silent: the only
/// observable effect is disk space.
pub async fn prune_stale_staging() {
    let _ = tokio::task::spawn_blocking(|| {
        let Some(dir) = staging_root() else {
            return;
        };
        let Some(cutoff) = std::time::SystemTime::now().checked_sub(STAGING_TTL) else {
            return;
        };
        prune_dir(&dir, cutoff);
    })
    .await;
}

/// How long a kept artifact waits for a retry: about the week a prompt might sit dismissed.
const STAGING_TTL: std::time::Duration = std::time::Duration::from_hours(7 * 24);

/// Split from the cache-dir lookup so a test can drive the cutoff from both sides.
fn prune_dir(dir: &Path, cutoff: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        // Flat files only: a subdirectory is nobody's staged artifact.
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

/// The URL's last path segment, query and fragment dropped.
fn asset_basename(url: &str) -> Option<String> {
    let no_fragment = url.split_once('#').map_or(url, |(head, _)| head);
    let no_query = no_fragment.split_once('?').map_or(no_fragment, |(head, _)| head);
    let last = no_query.rsplit('/').next()?;
    if last.is_empty() {
        return None;
    }
    Some(last.to_string())
}

/// `<target>.new`, appended so `Melodia.exe` keeps its extension.
pub(crate) fn staged_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".new");
    target.with_file_name(name)
}

/// Beside the target when its parent is writable, else in the cache dir for a `pkexec mv`, so the
/// polkit prompt comes at swap time and a failed download doesn't waste it.
pub(super) fn resolve_staged_path(target: &Path) -> AppResult<PathBuf> {
    let primary = staged_path(target);
    if primary.parent().is_some_and(dir_is_writable) {
        return Ok(primary);
    }
    let dir = staging_dir()?;
    let mut name = target.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".new");
    Ok(dir.join(name))
}

fn staging_root() -> Option<PathBuf> {
    dirs::cache_dir().map(|cache| cache.join("Melodia").join("update-staging"))
}

fn staging_dir() -> AppResult<PathBuf> {
    let dir = staging_root().ok_or_else(|| {
        AppError::updater_msg("could not resolve user cache dir for update staging")
    })?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The sidecar beside a staged file, checked before any resume.
///
/// [`StagedMeta::matches`] compares the first three: `version` catches a re-cut release, `size` a
/// re-cut at the same version, `asset_url` a flipped target key. `etag` is not part of the
/// fingerprint but is replayed as `If-Range`, so a resource that changed between attempts comes
/// back whole rather than appended. It is a strong tag or `None`, and correctness doesn't rest on
/// it: the size bound and the signature check catch a bad concatenation regardless.
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

/// `<staged>.meta.json`, appended so the staged name keeps its own suffix.
pub(crate) fn sidecar_meta_path(staged: &Path) -> PathBuf {
    let mut name: OsString =
        staged.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".meta.json");
    staged.with_file_name(name)
}

/// The sidecar, or `None`, which costs the staged bytes too: unfingerprinted, they can't resume.
pub(crate) fn read_staged_meta(path: &Path) -> Option<StagedMeta> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn write_staged_meta(path: &Path, meta: &StagedMeta) -> AppResult<()> {
    let bytes = serde_json::to_vec(meta)
        .map_err(|e| AppError::updater("failed to serialise staging sidecar", e))?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Drops both files when `dest` holds bytes their sidecar can't vouch for, so `plan_resume`
/// answers `Fresh`. Hands back a sidecar that survives, for its `ETag`.
pub(crate) fn discard_staging_if_sidecar_mismatches(
    dest: &Path,
    expected_version: &str,
    expected_size: u64,
    expected_url: &str,
) -> Option<StagedMeta> {
    let sidecar = sidecar_meta_path(dest);
    let existing_size = std::fs::metadata(dest).map_or(0, |m| m.len());
    if existing_size == 0 {
        // No bytes, so a sidecar left here would only mislead.
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
