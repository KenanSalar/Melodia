use std::fs;
use std::path::Path;

use tempfile::tempdir;

use crate::services::updater::install::download::{
    ResumeState, capture_strong_etag, exceeds_size_bound, plan_resume,
};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use crate::services::updater::install::old_path;
#[cfg(target_os = "windows")]
use crate::services::updater::install::staging::{
    InstallMethod, resolve_install_method, staged_msi_path,
};
use crate::services::updater::install::staging::{
    StagedMeta, discard_staging_if_sidecar_mismatches, read_staged_meta, sidecar_meta_path,
    staged_path, write_staged_meta,
};
use crate::services::updater::install::swap_in_place;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn size_bound_allows_exact_declared_size() {
    // A response exactly matching the manifest's `size` is the happy
    // path and must not trip the bound.
    assert!(!exceeds_size_bound(80 * 1024 * 1024, 80 * 1024 * 1024));
}

#[test]
fn size_bound_allows_small_overshoot_within_slack() {
    // 3 % over — within the 5 % slack for chunked-transfer accounting
    // differences. Must not trip.
    let expected = 100_000_000;
    let downloaded = expected + (expected * 3 / 100);
    assert!(!exceeds_size_bound(downloaded, expected));
}

#[test]
fn size_bound_rejects_overshoot_past_slack() {
    // 6 % over — past the 5 % slack. Must trip.
    let expected = 100_000_000;
    let downloaded = expected + (expected * 6 / 100);
    assert!(exceeds_size_bound(downloaded, expected));
}

#[test]
fn size_bound_rejects_gigantic_overshoot() {
    // Compromised-manifest scenario: 80 MiB declared, 10 GiB served.
    // Must trip at the very first 80 MiB + slack worth of bytes.
    let expected = 80 * 1024 * 1024;
    let downloaded = expected + (expected * 6 / 100);
    assert!(exceeds_size_bound(downloaded, expected));
    // And of course the full 10 GiB trips it many times over.
    assert!(exceeds_size_bound(10u64 * 1024 * 1024 * 1024, expected));
}

#[test]
fn size_bound_handles_zero_declared() {
    // A manifest reporting `size: 0` is malformed, but the bound check
    // must not panic. Anything > 0 must trip immediately.
    assert!(exceeds_size_bound(1, 0));
    assert!(!exceeds_size_bound(0, 0));
}

#[test]
fn size_bound_saturates_on_overflow() {
    // expected_size * 105 would wrap on u64::MAX. The saturating mul
    // pins the bound at u64::MAX / 100, which is still larger than
    // any realistic downloaded value, so the check biases toward
    // "not exceeded" (i.e. we don't accidentally reject a sane
    // download because of a bogus huge manifest size). The opposite
    // bias would also be defensible; the choice here matches the
    // "always saturating-add" pattern used elsewhere.
    assert!(!exceeds_size_bound(1_000_000_000, u64::MAX));
}

#[test]
fn staged_path_lives_next_to_target() {
    let target = std::path::PathBuf::from("/opt/melodia/melodia");
    let staged = staged_path(&target);
    assert_eq!(staged, std::path::PathBuf::from("/opt/melodia/melodia.new"));
    assert_eq!(staged.parent(), target.parent());
}

#[test]
fn staged_path_preserves_extension_for_windows_exe() {
    let target = std::path::PathBuf::from("C:/Program Files/Melodia/Melodia.exe");
    let staged = staged_path(&target);
    assert_eq!(staged, std::path::PathBuf::from("C:/Program Files/Melodia/Melodia.exe.new"));
}

/// `staged_msi_path` returns a `.msi`-suffixed path under the per-user
/// cache dir so `msiexec /i` recognises the package. Caller depends on
/// the extension being preserved — msiexec dispatches on extension and
/// silently rejects a renamed `.new` file with error 1620.
#[cfg(target_os = "windows")]
#[test]
fn staged_msi_path_preserves_msi_extension_under_cache_dir() -> TestResult {
    let staged = staged_msi_path("https://example.test/releases/melodia-0.5.0-x86_64.msi")?;
    assert_eq!(
        staged.extension().and_then(|s| s.to_str()),
        Some("msi"),
        "msiexec dispatches on .msi extension; staging must preserve it"
    );
    // File should live under <cache>/Melodia/update-staging/.
    let cache = dirs::cache_dir().ok_or("could not resolve cache dir for assert")?;
    let staging = cache.join("Melodia").join("update-staging");
    assert!(
        staged.starts_with(&staging),
        "staged MSI must live under {} (got {})",
        staging.display(),
        staged.display(),
    );
    Ok(())
}

/// Synthetic-filename fallback when the asset URL has no usable basename
/// (or doesn't end in `.msi`). Without this, a manifest with a query-
/// string-stripped URL could land in the cache as something msiexec
/// rejects.
#[cfg(target_os = "windows")]
#[test]
fn staged_msi_path_falls_back_to_synthetic_name_when_url_lacks_msi_suffix() -> TestResult {
    let staged = staged_msi_path("https://example.test/some-redirect-without-suffix")?;
    assert_eq!(staged.file_name().and_then(|s| s.to_str()), Some("melodia-update.msi"),);
    Ok(())
}

/// On Windows, the resolver must pick `WindowsMsi` so
/// `download_and_install` routes through `install_via_msiexec` instead
/// of the atomic-swap path (which would fail with `PermissionDenied`
/// against `C:\Program Files\Melodia\bin\`).
#[cfg(target_os = "windows")]
#[test]
fn resolve_install_method_picks_windows_msi() {
    assert_eq!(resolve_install_method(), InstallMethod::WindowsMsi);
}

#[test]
fn same_dir_swap_succeeds() -> TestResult {
    // Regression for the cross-filesystem `$TEMP` → exe-dir rename bug
    // from the v1 plan. By staying inside one `tempdir()`, both files
    // share a filesystem and the rename must succeed.
    let dir = tempdir()?;
    let target = dir.path().join("melodia");
    let staged = staged_path(&target);
    fs::write(&target, b"OLD BINARY BYTES")?;
    fs::write(&staged, b"NEW BINARY BYTES")?;

    swap_in_place(&target, &staged)?;

    let contents = fs::read(&target)?;
    assert_eq!(contents, b"NEW BINARY BYTES");
    assert!(!staged.exists(), "staged file should be consumed by rename");
    Ok(())
}

#[test]
fn swap_replaces_existing_target_bytes() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia.exe");
    let staged = staged_path(&target);
    fs::write(&target, b"v0.1.0")?;
    fs::write(&staged, b"v0.2.0")?;
    swap_in_place(&target, &staged)?;
    assert_eq!(fs::read(&target)?, b"v0.2.0");
    Ok(())
}

/// `swap_in_place` (Linux + Windows branches) keeps a `.old` snapshot of
/// the previously-running binary so a failed post-swap smoke test can
/// roll back, and so `main()`'s startup remove on first successful boot
/// has something to reap. The same-fs atomic rename path is what the
/// per-user tarball / `AppImage` / Windows-zip cases all hit.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn swap_retains_old_snapshot_for_rollback() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    let staged = staged_path(&target);
    let old = old_path(&target);
    fs::write(&target, b"v0.1.0 OLD")?;
    fs::write(&staged, b"v0.2.0 NEW")?;
    assert!(!old.exists(), "test setup: .old must not pre-exist");

    swap_in_place(&target, &staged)?;

    assert_eq!(fs::read(&target)?, b"v0.2.0 NEW", "target carries new bytes");
    assert!(!staged.exists(), "staged is consumed by rename");
    assert!(old.exists(), ".old snapshot is retained for rollback");
    assert_eq!(fs::read(&old)?, b"v0.1.0 OLD", ".old snapshot carries previous-binary bytes");
    Ok(())
}

#[test]
fn plan_resume_fresh_for_missing_file() {
    assert_eq!(plan_resume(0, 100), ResumeState::Fresh);
}

#[test]
fn plan_resume_skip_for_fully_matching_file() {
    // The "retention on failure" hot path: a previously-verified
    // .rpm/.deb sits on disk at the expected size. Re-downloading
    // would waste bandwidth; verify will catch any corruption.
    assert_eq!(plan_resume(100, 100), ResumeState::Skip);
    assert_eq!(plan_resume(80 * 1024 * 1024, 80 * 1024 * 1024), ResumeState::Skip);
}

#[test]
fn plan_resume_resume_for_partial_file() {
    assert_eq!(plan_resume(60, 100), ResumeState::Resume(60));
    assert_eq!(plan_resume(1, 100), ResumeState::Resume(1));
}

#[test]
fn plan_resume_fresh_for_oversized_existing() {
    // Existing > expected means leftover from a different release,
    // or a downward manifest-size shift. Safest is to discard.
    assert_eq!(plan_resume(200, 100), ResumeState::Fresh);
}

#[test]
fn plan_resume_fresh_when_manifest_size_is_zero() {
    // Malformed manifest. Force fresh so the C2 bound check at the
    // streaming layer catches whatever the server actually serves.
    assert_eq!(plan_resume(0, 0), ResumeState::Fresh);
    assert_eq!(plan_resume(100, 0), ResumeState::Fresh);
}

#[test]
fn sidecar_meta_path_appends_meta_json_to_basename() {
    // `.new` + `.meta.json` = `.new.meta.json` (per-user tarball case).
    let p = Path::new("/opt/melodia/Melodia.new");
    assert_eq!(sidecar_meta_path(p), Path::new("/opt/melodia/Melodia.new.meta.json").to_path_buf());
    // Multi-suffix RPM staging path — appending preserves the trailing
    // `.rpm` so dnf/apt still recognise the original (the sidecar is
    // strictly additive).
    let q = Path::new("/home/u/.cache/Melodia/update-staging/melodia-0.2.0-1.fc41.x86_64.rpm");
    assert_eq!(
        sidecar_meta_path(q),
        Path::new(
            "/home/u/.cache/Melodia/update-staging/melodia-0.2.0-1.fc41.x86_64.rpm.meta.json",
        )
        .to_path_buf()
    );
}

#[test]
fn staged_meta_round_trips_through_disk() -> TestResult {
    let dir = tempdir()?;
    let path = dir.path().join("Melodia.new.meta.json");
    let meta = StagedMeta {
        version: "0.2.0".into(),
        size: 52_428_800,
        asset_url: "https://example.test/melodia-0.2.0-x86_64.AppImage".into(),
        etag: Some(r#"W/"abc123""#.into()),
    };
    write_staged_meta(&path, &meta)?;
    assert_eq!(read_staged_meta(&path), Some(meta));
    Ok(())
}

#[test]
fn staged_meta_matches_requires_all_fields() {
    let meta = StagedMeta {
        version: "0.2.0".into(),
        size: 100,
        asset_url: "https://u".into(),
        etag: Some("W/\"abc\"".into()),
    };
    assert!(meta.matches("0.2.0", 100, "https://u"));
    assert!(!meta.matches("0.3.0", 100, "https://u"), "version drift must fail");
    assert!(!meta.matches("0.2.0", 101, "https://u"), "size drift must fail");
    assert!(!meta.matches("0.2.0", 100, "https://v"), "url drift must fail");
    // Etag deliberately is NOT consulted by matches() — it's freshness
    // metadata for the resume protocol, not part of the content fingerprint.
    let meta_no_tag = StagedMeta {
        version: "0.2.0".into(),
        size: 100,
        asset_url: "https://u".into(),
        etag: None,
    };
    assert!(
        meta_no_tag.matches("0.2.0", 100, "https://u"),
        "matches() must ignore etag so a pre-etag sidecar still validates"
    );
}

/// Backward-compat: a sidecar written by a pre-etag client (no `etag`
/// field on disk) must still parse — the `#[serde(default)]` on the
/// field makes the missing key deserialize as `None` rather than failing
/// the JSON parse and triggering an unnecessary discard.
#[test]
fn staged_meta_parses_legacy_sidecar_without_etag_field() -> TestResult {
    let dir = tempdir()?;
    let path = dir.path().join("legacy.meta.json");
    fs::write(&path, br#"{"version":"0.2.0","size":1024,"asset_url":"https://u"}"#)?;
    let parsed = read_staged_meta(&path).ok_or("legacy sidecar must still parse")?;
    assert_eq!(parsed.version, "0.2.0");
    assert_eq!(parsed.size, 1024);
    assert_eq!(parsed.asset_url, "https://u");
    assert_eq!(parsed.etag, None);
    Ok(())
}

/// RFC 9110 §13.1.5 forbids `If-Range` with a weak entity-tag, and
/// §8.8.3.2 strong comparison would make a server silently force a full
/// re-download on every resume if we sent one. `capture_strong_etag`
/// drops weak tags at the HTTP boundary so they never reach the sidecar.
///
/// Regression guard against accidental removal of the filter — without
/// it, a future origin switch to a weak-ETag CDN would defeat resume
/// without any visible symptom in CI.
#[test]
fn capture_strong_etag_drops_weak_tags_and_keeps_strong() {
    // Weak ETags (`W/"..."`) — must be dropped.
    assert_eq!(capture_strong_etag(Some(r#"W/"abc123""#)), None);
    assert_eq!(capture_strong_etag(Some(r#"W/"""#)), None);

    // Strong ETags — must pass through unchanged. Sample shapes pulled
    // from real Azure-Blob / S3 / nginx origins.
    let azure_blob = r#""0x8DBF736055F8CFD""#;
    assert_eq!(capture_strong_etag(Some(azure_blob)), Some(azure_blob.to_owned()));
    let s3_md5 = r#""d41d8cd98f00b204e9800998ecf8427e""#;
    assert_eq!(capture_strong_etag(Some(s3_md5)), Some(s3_md5.to_owned()));

    // Missing header — `None` passes straight through.
    assert_eq!(capture_strong_etag(None), None);
}

#[test]
fn read_staged_meta_returns_none_for_missing_file() -> TestResult {
    let dir = tempdir()?;
    let path = dir.path().join("nonexistent.meta.json");
    assert!(read_staged_meta(&path).is_none());
    Ok(())
}

#[test]
fn read_staged_meta_returns_none_for_corrupted_json() -> TestResult {
    // A partial / truncated sidecar must not panic and must not be
    // treated as a valid fingerprint — caller treats `None` as
    // "discard the staged bytes alongside".
    let dir = tempdir()?;
    let path = dir.path().join("corrupted.meta.json");
    fs::write(&path, b"this is not json")?;
    assert!(read_staged_meta(&path).is_none());
    Ok(())
}

/// Backward-compat path: a partial download staged by pre-sidecar code
/// has bytes on disk but no sidecar at all. `download_to_file` must
/// discard those bytes on first retry rather than gluing them onto a
/// resumed `Range:` request.
#[test]
fn discard_drops_both_when_sidecar_missing() -> TestResult {
    let dir = tempdir()?;
    let staged = dir.path().join("melodia-0.2.0.rpm");
    let sidecar = sidecar_meta_path(&staged);
    fs::write(&staged, vec![0u8; 1024])?;
    assert!(!sidecar.exists(), "test setup: sidecar must not pre-exist");

    discard_staging_if_sidecar_mismatches(&staged, "0.2.0", 1024, "https://example.test/rpm");

    assert!(!staged.exists(), "staged bytes must be discarded when no sidecar is present");
    assert!(!sidecar.exists());
    Ok(())
}

/// The most common drift case: user bumped Cargo.toml, CI re-cut the
/// release at a new version, the stale partial belongs to the previous
/// version. Sidecar fingerprint catches it and both files get dropped.
#[test]
fn discard_drops_both_when_sidecar_fingerprint_mismatches() -> TestResult {
    let dir = tempdir()?;
    let staged = dir.path().join("Melodia.new");
    let sidecar = sidecar_meta_path(&staged);
    fs::write(&staged, vec![0u8; 1024])?;
    write_staged_meta(
        &sidecar,
        &StagedMeta {
            version: "0.1.9".into(),
            size: 1024,
            asset_url: "https://example.test/old".into(),
            etag: None,
        },
    )?;

    discard_staging_if_sidecar_mismatches(&staged, "0.2.0", 1024, "https://example.test/new");

    assert!(!staged.exists(), "stale-version partial must be discarded");
    assert!(!sidecar.exists(), "stale sidecar must be dropped alongside");
    Ok(())
}

/// The happy retry path: prior failed attempt left a verified partial
/// with a matching fingerprint. Both files survive so the next attempt
/// can `Skip` or `Resume` rather than re-downloading.
#[test]
fn discard_keeps_both_when_fingerprint_matches() -> TestResult {
    let dir = tempdir()?;
    let staged = dir.path().join("Melodia.new");
    let sidecar = sidecar_meta_path(&staged);
    fs::write(&staged, vec![0u8; 1024])?;
    write_staged_meta(
        &sidecar,
        &StagedMeta {
            version: "0.2.0".into(),
            size: 1024,
            asset_url: "https://example.test/asset".into(),
            etag: None,
        },
    )?;

    discard_staging_if_sidecar_mismatches(&staged, "0.2.0", 1024, "https://example.test/asset");

    assert!(staged.exists(), "matching-fingerprint partial must survive");
    assert!(sidecar.exists(), "matching sidecar must survive");
    Ok(())
}

/// Orphan sidecar (no staged file alongside) — e.g. previous run's
/// staged file got swept by the pruner but the sidecar's mtime kept it
/// inside the 7d window. Function clears the orphan so it doesn't
/// outlive the next prune cycle as misleading state. Confirms the
/// `existing_size == 0` early-return branch.
#[test]
fn discard_clears_orphan_sidecar_when_staged_missing() -> TestResult {
    let dir = tempdir()?;
    let staged = dir.path().join("Melodia.new");
    let sidecar = sidecar_meta_path(&staged);
    write_staged_meta(
        &sidecar,
        &StagedMeta {
            version: "0.2.0".into(),
            size: 1024,
            asset_url: "https://example.test/asset".into(),
            etag: None,
        },
    )?;
    assert!(!staged.exists(), "test setup: no staged file");

    discard_staging_if_sidecar_mismatches(&staged, "0.2.0", 1024, "https://example.test/asset");

    assert!(!sidecar.exists(), "orphan sidecar must be reaped");
    Ok(())
}

/// A stale `.old` from a prior incomplete swap shouldn't block a fresh
/// swap; `linux_swap` / `windows_swap` both clear it best-effort before
/// the new two-step rename.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn swap_clears_stale_old_before_retaining_fresh_snapshot() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    let staged = staged_path(&target);
    let old = old_path(&target);
    fs::write(&target, b"v0.2.0")?;
    fs::write(&staged, b"v0.3.0")?;
    fs::write(&old, b"ANCIENT v0.0.1 LEFTOVER")?;

    swap_in_place(&target, &staged)?;

    assert_eq!(fs::read(&target)?, b"v0.3.0");
    assert_eq!(
        fs::read(&old)?,
        b"v0.2.0",
        ".old carries the previous-step bytes, not the ancient leftover"
    );
    Ok(())
}
