use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use melodia_platform::services::platform::install_kind::linux_pkg::LinuxPackageFormat;
use tempfile::tempdir;

use super::{
    InstallMethod, StagedMeta, discard_staging_if_sidecar_mismatches, install_method_for_key,
    prune_dir, read_staged_meta, sidecar_meta_path, staged_path, write_staged_meta,
};
// The package path and the MSI path are the same function on two platforms, and each is only
// asked for on the one whose installer dispatches on that suffix.
#[cfg(target_os = "linux")]
use super::staged_package_path;
#[cfg(target_os = "windows")]
use super::{resolve_install_method, staged_msi_path};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Backdates a file so the retention boundary can be driven from both sides. Files only: Windows
/// hands out no directory handle without `FILE_FLAG_BACKUP_SEMANTICS`, and nothing here wants one.
fn set_mtime(path: &Path, at: SystemTime) -> std::io::Result<()> {
    let handle = fs::File::options().write(true).open(path)?;
    handle.set_times(fs::FileTimes::new().set_modified(at))
}

/// Every key the manifest can name, against the method it must install by. A typo in one of the
/// six package literals falls through to `AtomicSwap`, which renames an `.rpm` over the live
/// binary — the bug the single enum exists to prevent, and the majority of the Linux matrix.
#[test]
fn every_target_key_maps_to_the_method_that_can_install_it() {
    let rpm = InstallMethod::LinuxPackage(LinuxPackageFormat::Rpm);
    let deb = InstallMethod::LinuxPackage(LinuxPackageFormat::Deb);
    let cases = [
        (Some("linux-x86_64-rpm"), rpm),
        (Some("linux-aarch64-rpm"), rpm),
        (Some("linux-x86_64-deb"), deb),
        (Some("linux-aarch64-deb"), deb),
        (Some("windows-x86_64-msi"), InstallMethod::WindowsMsi),
        (Some("windows-aarch64-msi"), InstallMethod::WindowsMsi),
        (Some("linux-x86_64-appimage"), InstallMethod::AtomicSwap),
        (Some("linux-aarch64-appimage"), InstallMethod::AtomicSwap),
        (Some("linux-x86_64-tarball"), InstallMethod::AtomicSwap),
        (Some("linux-aarch64-tarball"), InstallMethod::AtomicSwap),
    ];
    for (key, expected) in cases {
        assert_eq!(install_method_for_key(key), expected, "wrong method for {key:?}");
    }
}

/// An unpackaged host and a key from a future manifest both land on the swap. Neither reaches
/// install: `check_for_update` refuses first with `NoAssetForTarget`, and the fall-through is
/// what keeps a key this binary doesn't know from being installed by guesswork.
#[test]
fn an_unknown_key_falls_through_to_the_swap() {
    assert_eq!(install_method_for_key(None), InstallMethod::AtomicSwap);
    assert_eq!(install_method_for_key(Some("macos-aarch64-dmg")), InstallMethod::AtomicSwap);
    assert_eq!(install_method_for_key(Some("")), InstallMethod::AtomicSwap);
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

/// Both sides of the retention boundary, since the cutoff is exactly the kind of constant that
/// is written once and never exercised. A file one second either side of it decides whether a
/// user who left a prompt dismissed for a week re-downloads the artifact.
#[test]
fn the_pruner_takes_what_is_past_the_cutoff_and_nothing_else() -> TestResult {
    let dir = tempdir()?;
    let cutoff = SystemTime::now();

    let stale = dir.path().join("melodia-0.1.0.rpm");
    let fresh = dir.path().join("melodia-0.2.0.rpm");
    let at_the_boundary = dir.path().join("melodia-0.3.0.rpm");
    for path in [&stale, &fresh, &at_the_boundary] {
        fs::write(path, b"artifact")?;
    }
    set_mtime(&stale, cutoff - Duration::from_secs(1))?;
    set_mtime(&fresh, cutoff + Duration::from_secs(1))?;
    set_mtime(&at_the_boundary, cutoff)?;

    prune_dir(dir.path(), cutoff);

    assert!(!stale.exists(), "a file older than the cutoff is what the sweep is for");
    assert!(fresh.exists(), "a newer one must survive");
    // The comparison is strictly-less, so a file whose mtime is the cutoff itself is kept. The
    // direction matters more than the tie: erring toward keeping costs disk, erring the other
    // way costs the user a re-download.
    assert!(at_the_boundary.exists(), "the boundary itself is kept");
    Ok(())
}

/// The staging dir is under the user's cache root, which is not exclusively ours. A directory
/// there belongs to whoever made it, and recursing would put its contents in reach of a sweep
/// that only understands flat artifacts.
///
/// Driven from inside the directory rather than on the directory itself, which is what makes it
/// an assertion: the entry is skipped by `is_file`, but a sweep that fell through to `remove_file`
/// on it would fail there anyway and swallow the error, so the directory surviving says nothing.
/// A stale file under it is only kept by the walk stopping.
#[test]
fn the_pruner_does_not_descend_into_a_directory_it_finds() -> TestResult {
    let dir = tempdir()?;
    let cutoff = SystemTime::now();
    let nested = dir.path().join("someone-elses-cache");
    fs::create_dir(&nested)?;

    let buried = nested.join("melodia-0.1.0.rpm");
    fs::write(&buried, b"artifact")?;
    set_mtime(&buried, cutoff - Duration::from_secs(1))?;

    prune_dir(dir.path(), cutoff);

    assert!(nested.is_dir(), "the directory is not the sweep's to remove");
    assert!(buried.exists(), "and neither is anything under it, however far past the cutoff");
    Ok(())
}

#[test]
fn the_pruner_is_silent_about_a_directory_that_is_not_there() {
    let missing = Path::new("melodia-no-such-staging-dir");
    assert!(!missing.exists(), "test setup");
    prune_dir(missing, SystemTime::now());
}

/// `dnf` and `apt` dispatch on the suffix, so a redirect that hands back a basename without one
/// has to fall back rather than stage a file the package manager will refuse. The Windows twin
/// has had these two since it was written; this is the same function on the other platform.
#[cfg(target_os = "linux")]
#[test]
fn a_staged_package_keeps_the_suffix_its_installer_dispatches_on() -> TestResult {
    let cache = tempdir()?;
    let cache_root = cache.path().to_string_lossy().into_owned();
    melodia_testkit::with_env_set(&["XDG_CACHE_HOME"], &[("XDG_CACHE_HOME", &cache_root)], || {
        let rpm = staged_package_path(
            "https://example.test/rel/melodia-0.3.0-1.fc44.x86_64.rpm",
            LinuxPackageFormat::Rpm,
        )?;
        assert_eq!(
            rpm.file_name().and_then(|n| n.to_str()),
            Some("melodia-0.3.0-1.fc44.x86_64.rpm")
        );
        assert!(rpm.starts_with(cache.path().join("Melodia").join("update-staging")));

        // The query string is stripped before the suffix is read, so a signed URL still stages
        // under the name the package manager will see.
        let deb = staged_package_path(
            "https://example.test/rel/melodia-0.3.0.deb?sig=x",
            LinuxPackageFormat::Deb,
        )?;
        assert_eq!(deb.file_name().and_then(|n| n.to_str()), Some("melodia-0.3.0.deb"));
        Ok(())
    })
}

#[cfg(target_os = "linux")]
#[test]
fn a_package_url_without_a_usable_suffix_falls_back_to_a_synthetic_name() -> TestResult {
    let cache = tempdir()?;
    let cache_root = cache.path().to_string_lossy().into_owned();
    melodia_testkit::with_env_set(&["XDG_CACHE_HOME"], &[("XDG_CACHE_HOME", &cache_root)], || {
        let cases = [
            ("https://example.test/some-redirect", LinuxPackageFormat::Rpm, "melodia-update.rpm"),
            ("https://example.test/", LinuxPackageFormat::Deb, "melodia-update.deb"),
            // The suffix match is case-sensitive where the MSI twin's is not, and stays that way:
            // no Linux filesystem folds case, and dnf/apt read the name as literally as this does.
            (
                "https://example.test/melodia-0.3.0.RPM",
                LinuxPackageFormat::Rpm,
                "melodia-update.rpm",
            ),
            // A `.deb` asked for as an RPM is the mismatch the single `InstallMethod` exists to
            // prevent; the suffix filter is the second place it cannot get through.
            (
                "https://example.test/melodia-0.3.0.deb",
                LinuxPackageFormat::Rpm,
                "melodia-update.rpm",
            ),
        ];
        for (url, format, expected) in cases {
            let staged = staged_package_path(url, format)?;
            assert_eq!(staged.file_name().and_then(|n| n.to_str()), Some(expected), "for {url}");
        }
        Ok(())
    })
}
