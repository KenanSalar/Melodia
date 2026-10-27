//! The final install step: an atomic in-place swap for a per-user install, or an elevated
//! package-manager or MSI install.
//!
//! A rename across filesystems fails with `EXDEV`, so a writable parent stages beside the target
//! rather than under `$TEMP` (`swap_tests::same_dir_swap_succeeds`).

use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;

use melodia_core::error::{AppError, AppResult};

#[cfg(target_os = "linux")]
use melodia_platform::services::platform::install_kind::linux_pkg;
use melodia_platform::services::platform::install_kind::linux_pkg::LinuxPackageFormat;

use super::InstallOutcome;

/// Hands the staged package to dnf or apt under `pkexec`. Every failure keeps the staged file for
/// a retry without re-download, the verified bytes not being the problem;
/// [`super::staging::prune_stale_staging`] reaps it. No `.old` and no smoke test here, so a broken
/// upgrade surfaces at the next launch and recovers through the package manager's journal.
#[cfg(target_os = "linux")]
pub(super) fn install_via_package_manager(
    format: LinuxPackageFormat,
    staged: &Path,
) -> AppResult<InstallOutcome> {
    // The polkit policy registers this path, so the prompt reads "Install Melodia update" rather
    // than a command line. A development build has none and runs the package manager directly.
    const HELPER: &str = "/usr/libexec/melodia-update-helper";
    let use_helper = std::path::Path::new(HELPER).is_file();

    let program = linux_pkg::resolve_install_program(format).ok_or_else(|| {
        AppError::updater_msg(format!(
            "no supported package manager found for {format:?} install \
             (looked for dnf5/dnf for RPM, apt/apt-get for DEB on $PATH)"
        ))
    })?;

    let mut cmd = std::process::Command::new("pkexec");
    if use_helper {
        let format_arg = match format {
            LinuxPackageFormat::Rpm => "rpm",
            LinuxPackageFormat::Deb => "deb",
        };
        log::info!(
            "updater: elevating package install via pkexec {HELPER} {format_arg} {}",
            staged.display()
        );
        cmd.arg(HELPER).arg(format_arg).arg(staged);
    } else {
        log::info!(
            "updater: elevating package install via pkexec ({program} install -y {}); \
             {HELPER} not installed — falling back to direct invocation",
            staged.display()
        );
        // No `--`: dnf5 rejects one after `install`, and an absolute `staged` can't read as a flag.
        cmd.arg(program).arg("install").arg("-y").arg(staged);
    }

    let output = cmd.output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::updater_msg(
                "polkit (pkexec) is not installed; install polkit or run \
                 `dnf update melodia` / `apt install ./melodia.deb` manually",
            ));
        }
        Err(e) => return Err(AppError::updater("failed to spawn pkexec", e)),
    };

    if !output.status.success() {
        let outcome = pkexec_failure(
            &format!("{program} install"),
            output.status.code().unwrap_or(-1),
            &output.stderr,
        );
        log::info!(
            "updater: keeping verified staged file at {} for retry-without-redownload \
             (auto-reaped after 7d)",
            staged.display()
        );
        return outcome;
    }

    let _ = std::fs::remove_file(staged);
    Ok(InstallOutcome::Installed)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn install_via_package_manager(
    _format: LinuxPackageFormat,
    _staged: &Path,
) -> AppResult<InstallOutcome> {
    // Unreachable: `resolve_install_method` picks `LinuxPackage` only on Linux.
    Err(AppError::updater_msg("package-manager install is only supported on Linux"))
}

/// The public property `wix/main.wxs` gates its post-install relaunch on, Restart Manager being
/// unable to reopen the app it closed.
#[cfg(target_os = "windows")]
const RELAUNCH_PROPERTY: &str = "MELODIA_RELAUNCH=1";

/// [`install_via_package_manager`]'s Windows twin, elevated by the per-machine MSI's UAC prompt.
///
/// **Normally this never returns**: Restart Manager closes Melodia partway through and the MSI
/// reopens the new build, so the child is deliberately not `kill_on_drop`.
///
/// `/qb!` has no cancel button, since cancelling mid-write with Restart Manager in play can leave
/// the install half-done. Every exit keeps the staged file and its verbose log for a retry; the
/// staging pruner reaps both.
#[cfg(target_os = "windows")]
pub(super) async fn install_via_msiexec(staged: &Path) -> AppResult<InstallOutcome> {
    let mut log_name = staged.as_os_str().to_owned();
    log_name.push(".log");
    let log = PathBuf::from(log_name);

    log::info!("updater: launching msiexec /i {} /qb! {RELAUNCH_PROPERTY}", staged.display());
    let mut child = tokio::process::Command::new("msiexec")
        .arg("/i")
        .arg(staged)
        .args(["/qb!", RELAUNCH_PROPERTY, "/l*v"])
        .arg(&log)
        .spawn()
        .map_err(|e| AppError::updater("failed to spawn msiexec (Windows Installer)", e))?;

    let _running = super::InstallerRunning::raise();
    let status = child.wait().await.map_err(|e| AppError::updater("waiting on msiexec", e))?;
    let outcome = status.code().and_then(|code| u32::try_from(code).ok()).and_then(msiexec_outcome);
    let Some(outcome) = outcome else {
        log::warn!("updater: msiexec exited with {status}; its log is {}", log.display());
        return Err(AppError::updater_msg(format!("Windows Installer exited with {status}")));
    };
    if outcome == InstallOutcome::Cancelled {
        log::info!("updater: msiexec cancelled; keeping {} for a retry", staged.display());
    } else if !status.success() {
        log::info!(
            "updater: msiexec exited with {status}, a success pending a Windows restart; its log \
             is {}",
            log.display()
        );
    }
    Ok(outcome)
}

/// What msiexec's exit code came to, `None` being a failure. Both reboot codes are success: a file
/// in use is moved aside and the new one put in its place, so restarting Melodia runs the new
/// build. Only a file locked against that move waits for the Windows restart.
#[cfg(target_os = "windows")]
fn msiexec_outcome(code: u32) -> Option<InstallOutcome> {
    use windows_sys::Win32::Foundation::{
        ERROR_INSTALL_USEREXIT, ERROR_SUCCESS, ERROR_SUCCESS_REBOOT_INITIATED,
        ERROR_SUCCESS_REBOOT_REQUIRED,
    };

    match code {
        ERROR_SUCCESS | ERROR_SUCCESS_REBOOT_REQUIRED | ERROR_SUCCESS_REBOOT_INITIATED => {
            Some(InstallOutcome::Installed)
        }
        ERROR_INSTALL_USEREXIT => Some(InstallOutcome::Cancelled),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn install_via_msiexec(_staged: &Path) -> std::future::Ready<AppResult<InstallOutcome>> {
    // Unreachable: `resolve_install_method` picks `WindowsMsi` only on Windows.
    std::future::ready(Err(AppError::updater_msg("msiexec install is only supported on Windows")))
}

/// Swaps the verified `staged` file in over the live binary at `target`.
///
/// Linux renames over the running file, whose open fd stays valid, falling back to `pkexec mv` at
/// a root-owned target. Windows can't rename over a loaded executable, so the running one moves to
/// `.old` first and is scheduled for delete on reboot, `main()`'s startup reaper being Linux-only.
pub fn swap_in_place(target: &Path, staged: &Path) -> AppResult<InstallOutcome> {
    #[cfg(target_os = "windows")]
    {
        windows_swap(target, staged)?;
        Ok(InstallOutcome::Installed)
    }
    #[cfg(target_os = "linux")]
    {
        linux_swap(target, staged)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        std::fs::rename(staged, target)?;
        Ok(InstallOutcome::Installed)
    }
}

#[cfg(target_os = "linux")]
fn linux_swap(target: &Path, staged: &Path) -> AppResult<InstallOutcome> {
    // Two renames, so a `.old` survives for `verify_swapped_binary`'s rollback.
    let old = old_path(target);
    let _ = std::fs::remove_file(&old);

    // A root-owned dir or another filesystem leaves no room for a `.old`; pkexec has no rollback.
    if let Err(e) = std::fs::rename(target, &old) {
        if matches!(
            e.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::CrossesDevices
        ) {
            return elevate_swap_via_pkexec(target, staged);
        }
        return Err(AppError::from(e));
    }

    // Put `.old` back so the path isn't left empty, reporting the original error either way.
    if let Err(e) = std::fs::rename(staged, target) {
        if let Err(restore_err) = std::fs::rename(&old, target) {
            log::warn!(
                "updater: linux swap rollback failed after staged→target rename failed: \
                 {} (original error: {}); user may need to manually rename \
                 {} → {}",
                melodia_core::error::describe(&restore_err),
                melodia_core::error::describe(&e),
                old.display(),
                target.display()
            );
        }
        return Err(AppError::from(e));
    }

    // `.old` stays for the smoke test's rollback; the new binary's first boot reaps it.
    Ok(InstallOutcome::Installed)
}

/// Not atomic across filesystems, `mv` copying then unlinking, which is accepted over a second
/// polkit prompt. Only a tarball under a root-owned `/opt/` reaches here, `is_system_install()`
/// keeping RPM and deb installs off this path.
#[cfg(target_os = "linux")]
fn elevate_swap_via_pkexec(target: &Path, staged: &Path) -> AppResult<InstallOutcome> {
    log::info!("updater: elevating swap via pkexec ({} → {})", staged.display(), target.display());
    let output =
        std::process::Command::new("pkexec").arg("mv").arg("--").arg(staged).arg(target).output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No retry here: the way forward is a writable location.
            let _ = std::fs::remove_file(staged);
            return Err(AppError::updater_msg(
                "polkit (pkexec) is not installed; install it or move Melodia to \
                 a user-writable location like ~/.local/share/Melodia/",
            ));
        }
        Err(e) => return Err(AppError::updater("failed to spawn pkexec", e)),
    };

    if !output.status.success() {
        // Keep `staged` so a retry reuses the verified bytes.
        return pkexec_failure("pkexec mv", output.status.code().unwrap_or(-1), &output.stderr);
    }

    Ok(InstallOutcome::Installed)
}

/// pkexec's exit when the user dismissed the authentication dialog, per pkexec(1). Only an agent
/// that reports the dismissal produces it: GNOME Shell's does, while Plasma's completes the request
/// unauthenticated, which pkexec reports as [`PKEXEC_REFUSED`].
#[cfg(target_os = "linux")]
const PKEXEC_DISMISSED: i32 = 126;

/// pkexec's catch-all for every other refusal: not authorized, a failed or cancelled prompt, no
/// agent, a D-Bus error. It doesn't say which, so stderr goes through verbatim.
#[cfg(target_os = "linux")]
const PKEXEC_REFUSED: i32 = 127;

/// What a non-zero exit from `pkexec <command>` came to. Any code but pkexec's own two is
/// `command`'s, and one of `command`'s landing on those two can't be told apart from them.
#[cfg(target_os = "linux")]
fn pkexec_failure(command: &str, code: i32, stderr: &[u8]) -> AppResult<InstallOutcome> {
    if code == PKEXEC_DISMISSED {
        log::info!("updater: {command} cancelled at the polkit prompt");
        return Ok(InstallOutcome::Cancelled);
    }
    let stderr = String::from_utf8_lossy(stderr);
    let reason = match code {
        PKEXEC_REFUSED => format!(
            "authorization was refused (the prompt was cancelled or failed, or no polkit agent \
             is running): {}",
            stderr.trim()
        ),
        other => format!("{command} exited {other}: {}", stderr.trim()),
    };
    Err(AppError::updater_msg(format!("update install requires admin privileges and {reason}")))
}

#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "FFI call to MoveFileExW scheduling delete-on-reboot for the .old rollback copy; null lpNewFileName + MOVEFILE_DELAY_UNTIL_REBOOT is the documented contract"
)]
fn windows_swap(target: &Path, staged: &Path) -> AppResult<()> {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};

    let old = old_path(target);
    // Best-effort: a stale `.old` still loaded goes at the reboot-delete scheduled below.
    let _ = std::fs::remove_file(&old);

    std::fs::rename(target, &old)?;
    if let Err(e) = std::fs::rename(staged, target) {
        // The usual cause, a transient AV lock, lets this undo succeed.
        let _ = std::fs::rename(&old, target);
        return Err(AppError::from(e));
    }

    let wide: Vec<u16> = wide_with_nul(&old);
    // SAFETY: `wide` is NUL-terminated by `wide_with_nul` and outlives the call. A null
    // `lpNewFileName` is what `MOVEFILE_DELAY_UNTIL_REBOOT` documents as delete-on-reboot.
    let ok = unsafe { MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
    if ok == 0 {
        log::warn!(
            "updater: failed to schedule {} for delete-on-reboot; cleanup will retry on next launch",
            old.display()
        );
    }
    Ok(())
}

/// `<target>.old`, the rollback copy both swaps keep. `pub(crate)` so the startup reaper
/// (`updater::install_target_old`) derives the same name.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) fn old_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".old");
    target.with_file_name(name)
}

#[cfg(target_os = "windows")]
fn wide_with_nul(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
#[path = "../tests/swap_tests.rs"]
mod tests;
