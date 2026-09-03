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

use super::install_kind::{install_target, linux_pkg};

/// `.desktop` launcher template. `@EXEC@` placeholder is replaced at
/// write time with the absolute path of the running binary.
const DESKTOP_TEMPLATE: &str = include_str!("../../../assets/desktop/Melodia.desktop.tmpl");

/// SVG launcher icon. The "with background" variant — the disc behind
/// the glyph belongs in taskbar / launcher contexts (the in-app custom
/// titlebar uses the without-background variant because the window
/// mantle already provides the disc). Matches the file shipped by
/// `scripts/install-linux.sh` and `scripts/build-rpm.sh`.
const ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/logo-with-background.svg");

/// `AppStream` metainfo. Deployed to `~/.local/share/metainfo/` so a
/// per-user tarball install shows up in software centres (KDE Discover
/// / GNOME Software) with the right name, developer and licence — the
/// same file the RPM/DEB ship system-wide. Shipped verbatim.
const METAINFO_XML: &str =
    include_str!("../../../packaging/com.github.kenansalar.melodia.metainfo.xml");

/// Idempotently deploy `.desktop` + icon to the user's `XDG_DATA_HOME`. No-op
/// on `AppImage`, a development build, or a package-manager-owned binary — each
/// argued at its gate below.
///
/// A failed write is logged and the next file still attempted: a desktop entry
/// without its icon beats neither. The one `Err` is an unresolvable
/// `$XDG_DATA_HOME`, which a real session doesn't have.
pub fn refresh_user_install() -> AppResult<()> {
    // First because it is free, short-circuiting ahead of the `rpm -qf`
    // subprocess below (whose probe memoises after the first call anyway).
    if std::env::var("APPIMAGE").is_ok_and(|p| !p.is_empty()) {
        log::info!("desktop_integration: skipping — running from AppImage (bundled .desktop/icon)");
        return Ok(());
    }

    // Also free (a `cfg!` and a path inspection), so also ahead of the probe.
    // Under `cargo run` the binary sits in `target/{debug,release}/`, and a
    // `.desktop` pointing `Exec=` there would hijack the installed entry. No
    // installed tarball / RPM / DEB binary matches.
    if crate::utils::exe::is_dev_build() {
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
    // Reverse-DNS name so the desktop-id matches the AppStream component id,
    // which is what makes software centres merge the two rather than list them
    // separately.
    let desktop_path = apps_dir.join("com.github.kenansalar.melodia.desktop");
    // Earlier releases' filenames. XDG overrides a system entry only with a user
    // entry of the *same* name, so a stale sibling shows as a duplicate tile.
    // Best-effort; a missing file is not an error.
    for legacy in ["Melodia.desktop", "melodia.desktop"] {
        let legacy_path = apps_dir.join(legacy);
        if let Err(e) = std::fs::remove_file(&legacy_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::debug!("desktop_integration: legacy {} remove failed: {e}", legacy_path.display());
        }
    }
    let icon_path =
        data_home.join("icons").join("hicolor").join("scalable").join("apps").join("melodia.svg");
    // Per-user mirror of the copy RPM/DEB land in `/usr/share/metainfo/`.
    let metainfo_path =
        data_home.join("metainfo").join("com.github.kenansalar.melodia.metainfo.xml");

    let desktop_body = render_desktop(DESKTOP_TEMPLATE, &exec_path);

    // Independent writes, so one failure doesn't suppress the rest. `None` is
    // an errored write, `Some(changed)` a successful one.
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

/// Substitute `@EXEC@` in the template with the binary's absolute path, quoted
/// for `Exec=` where [`quote_exec`] says it must be. `Path::display()` keeps
/// UTF-8 paths verbatim, which is the case on every distro targeted.
pub(crate) fn render_desktop(template: &str, exec: &Path) -> String {
    template.replace("@EXEC@", &quote_exec(&exec.display().to_string()))
}

/// Quote the command for an `Exec=` line, but only when it needs it.
///
/// The value is parsed with shell-like quoting, so a home directory with a space
/// in it splits into two arguments and the launcher runs neither. Only when
/// needed, an unquoted command being what the four packaged sources ship;
/// `scripts/install-linux.sh` makes the same call for the same reason.
fn quote_exec(command: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(',
        ')', '`',
    ];

    if !command.contains(RESERVED) {
        return command.to_owned();
    }

    let mut quoted = String::with_capacity(command.len() + 2);
    quoted.push('"');
    for ch in command.chars() {
        match ch {
            // Two layers unescape this value — the desktop-entry `string` type
            // (`\\` → `\`) runs before the shell-like quoting — so a literal
            // backslash owes an escape to each and lands as four in the file.
            // The spec spells that out; nothing else here needs the second one.
            '\\' => quoted.push_str(r"\\\\"),
            '"' | '$' | '`' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

/// Write `payload` to `path` only when the on-disk BLAKE3 differs, returning
/// whether it did. Creates parent dirs, and persists through a temp file so a
/// crash mid-write can't leave a half-written launcher behind.
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

/// Best-effort cache refresh so the new files appear without a logout. Both
/// binaries may be absent on minimal installs, and errors are silent: the writes
/// already succeeded and the session picks them up on next start anyway.
fn refresh_caches(data_home: &Path) {
    let apps = data_home.join("applications");
    let icons = data_home.join("icons").join("hicolor");
    let _ = std::process::Command::new("update-desktop-database").arg("-q").arg(&apps).output();
    let _ = std::process::Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg("-t")
        .arg(&icons)
        .output();
}

#[cfg(test)]
#[path = "tests/desktop_integration_tests.rs"]
mod tests;

/// The bundled payloads, reachable by tests without the runtime probe gating.
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
