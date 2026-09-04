//! The final install step: an atomic in-place binary swap for a per-user
//! install, or an elevated package-manager install for RPM / deb.
//!
//! `std::fs::rename` returns `EXDEV` across filesystems and tmpfs is a different
//! filesystem from the user's home or `/opt`, which is why the writable-parent
//! case stages as a *sibling* rather than under `$TEMP`. Pinned by
//! `install_tests::same_dir_swap_succeeds`.

use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;

use melodia_core::error::{AppError, AppResult};

#[cfg(target_os = "linux")]
use melodia_platform::services::platform::install_kind::linux_pkg;
use melodia_platform::services::platform::install_kind::linux_pkg::LinuxPackageFormat;

/// Hand the staged package to the resolved package manager under `pkexec`, so
/// it validates the file, installs it and updates its own DB.
///
/// The staged file is **kept** on every non-success exit: an auth failure is
/// obviously retriable, and the package-manager exit codes cover a long tail of
/// transient causes — transaction lock, expired repo metadata, a mid-download
/// blip — that a retry without re-download fixes.
/// [`super::staging::prune_stale_staging`] reaps what goes unused.
///
/// **Failure detection is later here than on the atomic-swap path.** That branch
/// catches a broken binary in [`super::verify::verify_swapped_binary`]'s smoke
/// test and rolls back from `.old` before returning; this one has no `.old` to
/// restore and skips the test, so a broken upgrade surfaces on the *next* launch
/// and is recovered out-of-band through the package manager's own journal.
#[cfg(target_os = "linux")]
pub(super) fn install_via_package_manager(
    format: LinuxPackageFormat,
    staged: &Path,
) -> AppResult<()> {
    // The polkit policy registers this path, so the auth dialog reads "Install
    // Melodia update" rather than a raw `dnf install …` command line. Both the
    // RPM spec and the deb assets list ship it; absent (a development build,
    // say) we fall back to invoking the package manager directly.
    const HELPER: &str = "/usr/libexec/melodia-update-helper";
    let use_helper = std::path::Path::new(HELPER).is_file();

    let program = linux_pkg::resolve_install_program(format).ok_or_else(|| {
        AppError::Settings(format!(
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
        // No `--` end-of-options token — dnf5 rejects a bare one after the
        // `install` subcommand, and `staged` is an absolute path under the
        // staging dir so it can't be mistaken for a flag.
        cmd.arg(program).arg("install").arg("-y").arg(staged);
    }

    let output = cmd.output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Keep `staged` — a retry with polkit installed reuses the verified
            // bytes.
            return Err(AppError::Settings(
                "polkit (pkexec) is not installed; install polkit or run \
                 `dnf update melodia` / `apt install ./melodia.deb` manually"
                    .into(),
            ));
        }
        Err(e) => return Err(AppError::Io(e)),
    };

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = match code {
            126 => "authentication cancelled".to_string(),
            127 => format!(
                "authorization unavailable (no polkit agent, missing policy, or auth failed): {}",
                stderr.trim()
            ),
            other => {
                // The staged bytes already passed signature verification, so
                // they are by definition not the problem and a retry without
                // re-download often clears this.
                format!("{program} install exited {other}: {}", stderr.trim())
            }
        };
        log::info!(
            "updater: keeping verified staged file at {} for retry-without-redownload \
             (auto-reaped after 7d)",
            staged.display()
        );
        return Err(AppError::Settings(format!(
            "update install requires admin privileges and {reason}"
        )));
    }

    // The install rewrote the live binary; this process's open fd points at the
    // unlinked inode and stays valid until the respawn.
    let _ = std::fs::remove_file(staged);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) fn install_via_package_manager(
    _format: LinuxPackageFormat,
    _staged: &Path,
) -> AppResult<()> {
    // Unreachable — `resolve_install_method` never picks `LinuxPackage` off
    // Linux. The stub keeps the call site compile-clean.
    Err(AppError::Settings("package-manager install is only supported on Linux".into()))
}

/// [`install_via_package_manager`]'s Windows twin: elevation comes from the
/// per-machine MSI's UAC prompt, and `wix/main.wxs`'s `MajorUpgrade` +
/// `util:RestartResource` replace the running version through Restart Manager.
///
/// **Non-blocking by design** — spawned without `.output()` so this process can
/// exit cleanly while msiexec works, Restart Manager otherwise `WM_CLOSE`ing us
/// mid-call and dangling the parent inside `spawn_blocking`. msiexec reads the
/// staged file lazily and the staging pruner reaps it.
///
/// Skips the smoke test for the same reason the Linux package branch does; there
/// is additionally nothing to run, `--version` after spawning msiexec racing the
/// live binary's replacement.
///
/// `/qb!` is basic UI with **no** cancel button. The user already confirmed the
/// install, and cancelling mid-write leaves it half-done — Windows Installer's
/// rollback is not bulletproof with Restart Manager in play.
#[cfg(target_os = "windows")]
pub(super) fn install_via_msiexec(staged: &Path) -> AppResult<()> {
    use std::process::Command;

    let path_str = staged.to_str().ok_or_else(|| {
        AppError::Settings(format!(
            "staged MSI path {} contains non-UTF-8 bytes — cannot pass to msiexec",
            staged.display()
        ))
    })?;
    log::info!("updater: launching msiexec /i {path_str} /qb!");
    // Keep the staged file on failure, so a retry reuses the verified bytes. A
    // PATH missing System32 is the likeliest cause — stripped containers and
    // sandboxed dev environments drop it.
    Command::new("msiexec").args(["/i", path_str, "/qb!"]).spawn().map_err(|e| {
        AppError::Settings(format!(
            "failed to spawn msiexec for {}: {e} — install requires \
                 Windows Installer (msiexec.exe). The staged file is \
                 retained for retry; the 7d staging pruner reaps it if \
                 unused.",
            staged.display()
        ))
    })?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(super) fn install_via_msiexec(_staged: &Path) -> AppResult<()> {
    // Unreachable — `resolve_install_method` never picks `WindowsMsi` off
    // Windows. The stub keeps the call site compile-clean.
    Err(AppError::Settings("msiexec install is only supported on Windows".into()))
}

/// Atomic in-place swap of the live binary at `target` with the
/// already-verified `staged` file.
///
/// **Linux** renames straight over the running file — the kernel unlinks the old
/// inode while this process's open fd stays valid — falling back to `pkexec mv`
/// on `PermissionDenied` at a root-owned target. **Windows** refuses to rename a
/// loaded executable at all, so it dances: running → `.old`, staged → running,
/// then `.old` scheduled for delete on reboot. That schedule is the cleanup
/// rather than a fallback: `main()`'s startup reaper is Linux-only, so the only
/// earlier clear is the best-effort `remove_file` at the top of the next swap.
pub fn swap_in_place(target: &Path, staged: &Path) -> AppResult<()> {
    #[cfg(target_os = "windows")]
    {
        windows_swap(target, staged)
    }
    #[cfg(target_os = "linux")]
    {
        linux_swap(target, staged)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        std::fs::rename(staged, target)?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn linux_swap(target: &Path, staged: &Path) -> AppResult<()> {
    // A same-filesystem two-step rename, mirroring `windows_swap`, so a `.old`
    // snapshot survives for rollback if `verify_swapped_binary` reports the new
    // binary won't boot. Clearing a stale one first is best-effort.
    let old = old_path(target);
    let _ = std::fs::remove_file(&old);

    // `PermissionDenied` (a root-owned dir) or `CrossesDevices` means no `.old`
    // is possible, so fall through to pkexec — which carries no rollback safety.
    if let Err(e) = std::fs::rename(target, &old) {
        if matches!(
            e.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::CrossesDevices
        ) {
            return elevate_swap_via_pkexec(target, staged);
        }
        return Err(AppError::from(e));
    }

    // On failure rename `.old` back so the live binary path isn't missing; a
    // failed restore is logged but must not override the original error.
    if let Err(e) = std::fs::rename(staged, target) {
        if let Err(restore_err) = std::fs::rename(&old, target) {
            log::warn!(
                "updater: linux swap rollback failed after staged→target rename failed: \
                 {restore_err} (original error: {e}); user may need to manually rename \
                 {} → {}",
                old.display(),
                target.display()
            );
        }
        return Err(AppError::from(e));
    }

    // `.old` deliberately stays, reaped by `main()`'s startup remove on the
    // first boot of the new binary — which only happens once
    // `verify_swapped_binary` has accepted it.
    Ok(())
}

/// `pkexec mv` is **not atomic** across filesystems — `mv` falls back to
/// copy-then-unlink, so a power loss mid-copy leaves a partial target. Accepted:
/// the alternative is two pkexec invocations and so two polkit prompts.
/// Production RPM/deb installs never reach here, `is_system_install()` gating
/// the UI first; what is left is a tarball dropped under a root-owned `/opt/`,
/// where retry-on-fail is fine.
#[cfg(target_os = "linux")]
fn elevate_swap_via_pkexec(target: &Path, staged: &Path) -> AppResult<()> {
    log::info!("updater: elevating swap via pkexec ({} → {})", staged.display(), target.display());
    let output =
        std::process::Command::new("pkexec").arg("mv").arg("--").arg(staged).arg(target).output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Drop the staged file rather than leak it — the next action here is
            // to re-install somewhere else.
            let _ = std::fs::remove_file(staged);
            return Err(AppError::Settings(
                "polkit (pkexec) is not installed; install it or move Melodia to \
                 a user-writable location like ~/.local/share/Melodia/"
                    .into(),
            ));
        }
        Err(e) => return Err(AppError::Io(e)),
    };

    if !output.status.success() {
        // Per pkexec(1), 127 is a catch-all — not authorized, no agent, missing
        // policy, D-Bus error, auth failed — which it doesn't distinguish, so
        // stderr goes through verbatim. Anything else is the spawned `mv`, or a
        // `mv` code overlapping 127, which is unavoidable. Keep `staged` so a
        // retry reuses the verified bytes.
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = match code {
            126 => "authentication cancelled".to_string(),
            127 => format!(
                "authorization unavailable (no polkit agent, missing policy, or auth failed): {}",
                stderr.trim()
            ),
            other => format!("pkexec mv exited {other}: {}", stderr.trim()),
        };
        return Err(AppError::Settings(format!(
            "update install requires admin privileges and {reason}"
        )));
    }

    Ok(())
}

#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "FFI call to MoveFileExW scheduling delete-on-reboot for the .old rollback copy; null lpNewFileName + MOVEFILE_DELAY_UNTIL_REBOOT is the documented contract"
)]
fn windows_swap(target: &Path, staged: &Path) -> AppResult<()> {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};

    let old = old_path(target);
    // Best-effort: if a stale `.old` is still loaded, the reboot-delete below
    // picks it up instead.
    let _ = std::fs::remove_file(&old);

    std::fs::rename(target, &old)?;
    if let Err(e) = std::fs::rename(staged, target) {
        // Undo the first rename so the user isn't left with no executable. The
        // common cause is a transient AV lock, which lets the rollback succeed.
        let _ = std::fs::rename(&old, target);
        return Err(AppError::from(e));
    }

    let wide: Vec<u16> = wide_with_nul(&old);
    // Best-effort — the swap already succeeded, so a failure only leaves a stale
    // `.old` until reboot or the next launch's cleanup.
    // SAFETY: `wide` is NUL-terminated by `wide_with_nul` and outlives the call.
    // The null `lpNewFileName` is not an omission — it is what
    // `MOVEFILE_DELAY_UNTIL_REBOOT` documents as delete-on-reboot.
    let ok = unsafe { MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
    if ok == 0 {
        log::warn!(
            "updater: failed to schedule {} for delete-on-reboot; cleanup will retry on next launch",
            old.display()
        );
    }
    Ok(())
}

/// `<target>.old` — the rollback copy both swaps keep after a successful
/// rename, laid out like [`super::staging::staged_path`]'s `.new` sibling.
///
/// `pub(crate)` rather than module-private so
/// `services::updater::install_target_old()`, which `main.rs` calls at startup,
/// shares this exact derivation.
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
