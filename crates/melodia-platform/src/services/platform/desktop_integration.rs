//! Self-deploys the `.desktop` entry, icon and `AppStream` metainfo for a tarball install.
//!
//! RPM/DEB redeploy theirs on every upgrade and an `AppImage` carries its own, but the in-app
//! updater's binary swap never refreshes what `install-linux.sh` deployed. So the payloads are
//! compiled in and every boot BLAKE3s the on-disk copies against them, rewriting on mismatch.
//!
//! Skipped on RPM/DEB, where user-scoped copies diverging from the package's would leave which one
//! wins to `XDG_DATA_DIRS` order, on an `AppImage`, and on a development build, whose `Exec=` would
//! hijack the installed entry. `@EXEC@` takes the binary's absolute path, a tarball install not
//! being on `$PATH`.

use std::path::Path;

use melodia_core::error::{AppError, AppResult, describe};

use super::install_kind::{install_target, linux_pkg};

const DESKTOP_TEMPLATE: &str = include_str!("../../../../../assets/desktop/Melodia.desktop.tmpl");

/// The with-background variant, the disc belonging in launcher contexts, matching what
/// `install-linux.sh` and `build-rpm.sh` ship.
const ICON_SVG: &[u8] = include_bytes!("../../../../../assets/icons/logo-with-background.svg");

/// So a tarball install shows in software centres with the right name, developer and licence; the
/// file RPM/DEB ship system-wide, verbatim.
const METAINFO_XML: &str =
    include_str!("../../../../../packaging/com.github.kenansalar.melodia.metainfo.xml");

/// Idempotently deploys the three files to `XDG_DATA_HOME`. A failed write is logged and the next
/// still attempted, an entry without its icon beating neither. The one `Err` is an unresolvable
/// `$XDG_DATA_HOME`.
pub fn refresh_user_install() -> AppResult<()> {
    // The free checks first, ahead of the `rpm -qf` probe.
    if std::env::var("APPIMAGE").is_ok_and(|p| !p.is_empty()) {
        log::info!("desktop_integration: skipping — running from AppImage (bundled .desktop/icon)");
        return Ok(());
    }

    if melodia_core::utils::exe::is_dev_build() {
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
        AppError::settings_msg("could not resolve $XDG_DATA_HOME for .desktop / icon self-install")
    })?;

    let exec_path = install_target()?;
    let apps_dir = data_home.join("applications");
    // Reverse-DNS, so software centres merge the entry with the `AppStream` component of that id.
    let desktop_path = apps_dir.join("com.github.kenansalar.melodia.desktop");
    // Earlier releases' names, which show as a duplicate tile beside a system entry. Best-effort.
    for legacy in ["Melodia.desktop", "melodia.desktop"] {
        let legacy_path = apps_dir.join(legacy);
        if let Err(e) = std::fs::remove_file(&legacy_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::debug!(
                "desktop_integration: legacy {} remove failed: {}",
                legacy_path.display(),
                describe(&e)
            );
        }
    }
    let icon_path =
        data_home.join("icons").join("hicolor").join("scalable").join("apps").join("melodia.svg");
    let metainfo_path =
        data_home.join("metainfo").join("com.github.kenansalar.melodia.metainfo.xml");

    let desktop_body = render_desktop(DESKTOP_TEMPLATE, &exec_path);

    // `None` is a failed write, `Some(changed)` a successful one.
    let write = |path: &Path, payload: &[u8]| -> Option<bool> {
        match write_if_changed(path, payload) {
            Ok(changed) => Some(changed),
            Err(e) => {
                log::warn!(
                    "desktop_integration: write {} failed: {}",
                    path.display(),
                    describe(&e)
                );
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

pub(crate) fn render_desktop(template: &str, exec: &Path) -> String {
    template.replace("@EXEC@", &quote_exec(&exec.display().to_string()))
}

/// `Exec=` is parsed with shell-like quoting, so a home directory with a space would split in two.
/// Quoted only when needed, an unquoted command being what the packaged sources ship;
/// `install-linux.sh` makes the same call.
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
            // Two layers unescape this, the desktop-entry string type before the shell-like
            // quoting, so a literal backslash owes an escape to each and lands as four.
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

/// Writes only when the on-disk BLAKE3 differs, returning whether it did, and through a temp file
/// so a crash can't leave a half-written launcher.
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

/// So the files appear without a logout. Silent on failure: either binary may be missing, the
/// writes already landed, and the next session picks them up.
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

#[cfg(test)]
pub(crate) fn test_write_if_changed(path: &Path, payload: &[u8]) -> AppResult<bool> {
    write_if_changed(path, payload)
}
