//! Self-deploy the `.desktop` launcher entry, icon and `AppStream`
//! metainfo for per-user tarball installs.
//!
//! RPM/DEB re-deploy their launcher files on every upgrade via the package
//! manager, and an `AppImage` carries its own inside the bundle. The gap is
//! the **tarball**: `install-linux.sh` deploys `.desktop` + icon on first
//! install, but the in-app updater's atomic binary swap never refreshes them,
//! so an updated tarball install would keep the old entry indefinitely.
//!
//! So the `.desktop` template, icon and `AppStream` metainfo are compiled in
//! as `include_str!` / `include_bytes!` payloads, and every boot BLAKE3s the
//! on-disk copies against them — rewrite on mismatch, no-op when current.
//!
//! Skipped on:
//!
//! - RPM/DEB installs — the package manager owns those files under
//!   `/usr/share/`, and user-scoped copies that diverge from them would make
//!   which one wins depend on `XDG_DATA_DIRS` order.
//! - `AppImage` (`$APPIMAGE` set) — its `.desktop` lives inside the bundle.
//! - Development builds — an `Exec=` pointing into `target/` would hijack the
//!   user's installed launcher entry.
//! - `macOS` / Windows — the module is Linux-only.
//!
//! The template's `@EXEC@` placeholder is substituted with the running
//! binary's absolute path; a bare `Exec=Melodia` would assume a tarball
//! install is on `$PATH`.

use std::path::Path;

use crate::error::{AppError, AppResult};

use super::updater::{install_target, linux_pkg};

/// `.desktop` launcher template. `@EXEC@` placeholder is replaced at
/// write time with the absolute path of the running binary.
const DESKTOP_TEMPLATE: &str = include_str!("../../assets/desktop/Melodia.desktop.tmpl");

/// SVG launcher icon. The "with background" variant — the disc behind
/// the glyph belongs in taskbar / launcher contexts (the in-app custom
/// titlebar uses the without-background variant because the window
/// mantle already provides the disc). Matches the file shipped by
/// `scripts/install-linux.sh` and `scripts/build-rpm.sh`.
const ICON_SVG: &[u8] = include_bytes!("../../ui/assets/icons/logo-with-background.svg");

/// `AppStream` metainfo. Deployed to `~/.local/share/metainfo/` so a
/// per-user tarball install shows up in software centres (KDE Discover
/// / GNOME Software) with the right name, developer and licence — the
/// same file the RPM/DEB ship system-wide. Shipped verbatim.
const METAINFO_XML: &str =
    include_str!("../../packaging/com.github.kenansalar.melodia.metainfo.xml");

/// Idempotently deploy `.desktop` + icon to the user's `XDG_DATA_HOME`.
///
/// No-op when:
/// - the binary is running from an `AppImage` (`$APPIMAGE` set),
/// - the binary is a development build (`cargo run`, or running out of
///   a Cargo `target/{debug,release}` directory) — its `Exec=` path
///   would clobber the user's installed launcher entry, or
/// - the binary is owned by `rpm`/`dpkg` (the package manager already
///   deploys the system-wide copies via its own manifest).
///
/// Errors from any single write are logged and the function continues
/// to the next file — a half-deployed launcher (e.g. desktop entry
/// written, icon not) is still better than a fully-failed deploy.
/// Returns `Ok(())` if at least the gate evaluation succeeded; the
/// only `Err` path is "couldn't resolve `$XDG_DATA_HOME`", which on a
/// real Linux session shouldn't happen.
pub fn refresh_user_install() -> AppResult<()> {
    // `$APPIMAGE` gate first — it's free and short-circuits before we
    // spawn an `rpm -qf` subprocess. linux_pkg::detect()'s probe is
    // memoised after first call, so the second access from the
    // gating below is essentially free.
    if std::env::var("APPIMAGE").is_ok_and(|p| !p.is_empty()) {
        log::info!(
            "desktop_integration: skipping — running from AppImage (bundled .desktop/icon)"
        );
        return Ok(());
    }

    // Dev-build gate, before the `rpm -qf`/`dpkg -S` probe — it's free
    // (a `cfg!` check plus a path inspection), so by the same reasoning
    // as the `$APPIMAGE` gate above it short-circuits ahead of the
    // subprocess spawn. When launched via `cargo run` the binary lives
    // at `…/target/debug/Melodia` (or `…/target/release/` for `cargo run
    // --release`). Deploying a `.desktop` whose `Exec=` points there
    // would hijack the user's installed launcher entry. An installed
    // tarball / RPM / DEB binary never matches.
    if is_dev_build() {
        log::info!(
            "desktop_integration: skipping — running a development build \
             (would clobber the installed .desktop Exec= path)"
        );
        return Ok(());
    }

    if linux_pkg::detect().is_some() {
        log::info!(
            "desktop_integration: skipping — binary owned by package manager \
             (system-wide .desktop/icon already deployed)"
        );
        return Ok(());
    }

    let data_home = dirs::data_dir().ok_or_else(|| {
        AppError::Settings(
            "could not resolve $XDG_DATA_HOME for .desktop / icon self-install".into(),
        )
    })?;

    let exec_path = install_target()?;
    let apps_dir = data_home.join("applications");
    // Desktop entry named after the reverse-DNS app id so its
    // desktop-id matches the AppStream component
    // `com.github.kenansalar.melodia` — software centres need that
    // match to merge the launcher with the MetaInfo component.
    let desktop_path = apps_dir.join("com.github.kenansalar.melodia.desktop");
    // Legacy launcher filenames from earlier releases: `Melodia.desktop`
    // (capital M), then `melodia.desktop`. XDG only overrides a system
    // entry with a user entry of the *same* filename, so a stale
    // sibling shows up as a duplicate launcher tile. Best-effort
    // cleanup; a missing file is not an error.
    for legacy in ["Melodia.desktop", "melodia.desktop"] {
        let legacy_path = apps_dir.join(legacy);
        if let Err(e) = std::fs::remove_file(&legacy_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::debug!(
                "desktop_integration: legacy {} remove failed: {e}",
                legacy_path.display()
            );
        }
    }
    let icon_path = data_home
        .join("icons")
        .join("hicolor")
        .join("scalable")
        .join("apps")
        .join("melodia.svg");
    // AppStream MetaInfo — software centres read it from the per-user
    // metainfo dir, mirroring the system-wide copy the RPM/DEB land in
    // `/usr/share/metainfo/`.
    let metainfo_path = data_home
        .join("metainfo")
        .join("com.github.kenansalar.melodia.metainfo.xml");

    let desktop_body = render_desktop(DESKTOP_TEMPLATE, &exec_path);

    // Each write is independent: a failure on one payload shouldn't
    // suppress the others. Errors are logged and the function continues
    // — per the docstring. `None` ⇒ this write errored.
    let write = |path: &Path, payload: &[u8]| -> Option<bool> {
        match write_if_changed(path, payload) {
            Ok(changed) => Some(changed),
            Err(e) => {
                log::warn!("desktop_integration: write {} failed: {e}", path.display());
                None
            }
        }
    };
    let desktop_changed = write(&desktop_path, desktop_body.as_bytes());
    let icon_changed = write(&icon_path, ICON_SVG);
    let metainfo_changed = write(&metainfo_path, METAINFO_XML.as_bytes());

    let any_changed = desktop_changed.unwrap_or(false)
        || icon_changed.unwrap_or(false)
        || metainfo_changed.unwrap_or(false);
    if any_changed {
        log::info!(
            "desktop_integration: refreshed (desktop={desktop_changed:?}, \
             icon={icon_changed:?}, metainfo={metainfo_changed:?}); refreshing caches"
        );
        refresh_caches(&data_home);
    } else if desktop_changed == Some(false)
        && icon_changed == Some(false)
        && metainfo_changed == Some(false)
    {
        log::info!("desktop_integration: on-disk copies match compiled-in payloads");
    }
    Ok(())
}

/// True when the running binary is a development build — either compiled
/// with debug assertions, or located inside a Cargo `target/{debug,release}`
/// directory (covers `cargo run --release`).
fn is_dev_build() -> bool {
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

/// Substitute `@EXEC@` in the template with the binary's absolute path.
/// The placeholder is a literal substring, not regex — no escaping
/// concerns. Path is rendered via `Path::display()` which preserves
/// UTF-8 paths verbatim (the canonical case on every distro Melodia
/// targets).
pub(crate) fn render_desktop(template: &str, exec: &Path) -> String {
    template.replace("@EXEC@", &exec.display().to_string())
}

/// Write `payload` to `path` if and only if the on-disk content's
/// BLAKE3 hash differs from the payload's. Returns whether a write
/// happened.
///
/// Creates parent dirs on demand. Uses [`crate::services::write_atomic`]'s
/// temp-file-then-rename pattern by virtue of going through
/// `tempfile::NamedTempFile::persist` to dodge a half-written file
/// outliving a crash mid-write.
fn write_if_changed(path: &Path, payload: &[u8]) -> AppResult<bool> {
    if let Ok(existing) = std::fs::read(path)
        && blake3::hash(&existing) == blake3::hash(payload)
    {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        use std::io::Write;
        tmp.as_file_mut().write_all(payload)?;
        tmp.as_file_mut().flush()?;
    }
    tmp.persist(path).map_err(|e| AppError::Io(e.error))?;
    Ok(true)
}

/// Best-effort `update-desktop-database` + `gtk-update-icon-cache`
/// invocations so the new files show up in the user's launcher
/// without a logout. Both binaries may be absent on minimal installs;
/// errors are silenced because the underlying file writes succeeded
/// and the user's session will pick them up on next start regardless.
fn refresh_caches(data_home: &Path) {
    let apps = data_home.join("applications");
    let icons = data_home.join("icons").join("hicolor");
    let _ = std::process::Command::new("update-desktop-database")
        .arg("-q")
        .arg(&apps)
        .output();
    let _ = std::process::Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg("-t")
        .arg(&icons)
        .output();
}

#[cfg(test)]
#[path = "tests/desktop_integration_tests.rs"]
mod tests;

/// Re-export for tests that need to assert against the bundled payloads
/// without going through the runtime probe gating.
#[cfg(test)]
pub(crate) const TEST_DESKTOP_TEMPLATE: &str = DESKTOP_TEMPLATE;
#[cfg(test)]
pub(crate) const TEST_ICON_SVG: &[u8] = ICON_SVG;
#[cfg(test)]
pub(crate) const TEST_METAINFO: &str = METAINFO_XML;

/// Re-export `write_if_changed` for tests.
#[cfg(test)]
pub(crate) fn test_write_if_changed(path: &Path, payload: &[u8]) -> AppResult<bool> {
    write_if_changed(path, payload)
}
