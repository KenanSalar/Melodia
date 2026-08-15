//! The final install step: an atomic in-place binary swap (per-user
//! installs) or an elevated package-manager install (RPM / .deb).
//!
//! `std::fs::rename` returns `EXDEV` across filesystems, and tmpfs is
//! a different filesystem from the user's home or `/opt`; this is why
//! the writable-parent case stays as a sibling, not under `$TEMP`. The
//! `install_tests::same_dir_swap_succeeds` regression test pins that.

use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;

use crate::error::{AppError, AppResult};

#[cfg(target_os = "linux")]
use crate::services::updater::linux_pkg;
use crate::services::updater::linux_pkg::LinuxPackageFormat;

/// Runs `pkexec <dnf|dnf5|apt|apt-get> install -y <staged>` so the
/// resolved package manager validates the local file, installs it, and
/// updates its DB. KDE / GNOME's polkit agent prompts the user for the
/// admin password.
///
/// Staged file is **kept** on every non-success exit. Auth-failure
/// (126) and missing-pkexec / no-polkit-agent (127) are obviously
/// retriable; package-manager failures (other exit codes) cover a
/// long tail of transient causes — `dnf` / `apt` transaction lock,
/// repo metadata expiry, mid-dep-download network blip — that retry
/// without re-download fixes. Stale files (>7d) are pruned at the
/// start of the next install attempt by [`super::staging::prune_stale_staging`].
///
/// **Failure-detection latency differs from the atomic-swap path.** The
/// per-user atomic-swap branch ([`swap_in_place`]) catches a broken new
/// binary inside [`super::verify::verify_swapped_binary`]'s 5 s smoke
/// test and rolls back from `.old` before the caller returns. This path
/// skips the smoke test ([`super::download_and_install`] gates on
/// `pkg_format.is_none()`): the bytes are consumed by `dnf`/`apt` with
/// no `.old` snapshot to restore, and the install is journaled, so a
/// broken upgrade is recoverable out-of-band via `dnf history undo` /
/// `apt install <prev-version>` — but failure is only observed by the
/// user on the *next* launch, not by the updater inline.
#[cfg(target_os = "linux")]
pub(super) fn install_via_package_manager(
    format: LinuxPackageFormat,
    staged: &Path,
) -> AppResult<()> {
    // Prefer the branded polkit helper at `/usr/libexec/melodia-update-helper`
    // when it's installed — the polkit policy
    // `com.github.kenansalar.melodia.update` registers that path and
    // makes the KDE/GNOME auth dialog show "Install Melodia update"
    // instead of the raw `dnf install ...` command. Both the RPM spec
    // and the deb `[package.metadata.deb]` assets list ship the helper
    // and the .policy file. Falls back to the direct `pkexec dnf/apt`
    // form when the helper is absent (development builds, hand-installed
    // tarball that somehow got rpm/dpkg-owned, …).
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
        // No `--` end-of-options token: dnf5 (Fedora 41+) rejects a
        // bare `--` after the `install` subcommand. `staged` is an
        // absolute path under the update-staging dir, so it can't be
        // mistaken for a flag.
        cmd.arg(program).arg("install").arg("-y").arg(staged);
    }

    let output = cmd.output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Don't delete `staged`; a retry with polkit installed
            // reuses the verified bytes.
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
                // Package-manager failure (broken dep, bad signature in
                // their own repo metadata, conflicting package, dnf/apt
                // transaction lock, mid-dep-download network blip, ...).
                // Keep the staged file — the verified `.rpm` / `.deb`
                // bytes are by definition not the problem (sig already
                // passed), so a retry without re-download often clears
                // the failure. `prune_stale_staging` reaps files older
                // than 7d at the next install attempt.
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

    // Successful install rewrites `/usr/bin/melodia`; the running
    // process's open fd points at the unlinked inode and stays valid
    // until exec/respawn at the next event-loop quit.
    let _ = std::fs::remove_file(staged);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) fn install_via_package_manager(
    _format: LinuxPackageFormat,
    _staged: &Path,
) -> AppResult<()> {
    // `current_target_key()` never returns rpm/deb off Linux, so
    // `resolve_install_method()` never picks `LinuxPackage` and this
    // branch is dead. The stub exists to keep the call site
    // compile-clean on non-Linux.
    Err(AppError::Settings("package-manager install is only supported on Linux".into()))
}

/// Launches `msiexec /i <staged>.msi /qb!` to install the signed MSI.
/// Mirrors `install_via_package_manager` shape-for-shape but for
/// Windows: elevation comes from the per-machine MSI's UAC prompt
/// rather than polkit; the `MajorUpgrade` element +
/// `util:RestartResource` in `wix/main.wxs` replace the running
/// version cleanly via Windows Installer's Restart Manager.
///
/// **Non-blocking by design.** Spawned without `.output()` so the
/// running `Melodia.exe` can exit cleanly while msiexec works in the
/// background — Restart Manager would otherwise `WM_CLOSE` us mid-call
/// and dangling the parent process inside `spawn_blocking` would block
/// the runtime thread. The staged `.msi` survives this function
/// returning; msiexec reads it lazily and the 7d staging pruner reaps
/// it on the next install attempt.
///
/// **Same skip-smoke-test rationale as the Linux package branch.** No
/// in-process rollback exists once msiexec starts, no `.old` snapshot
/// is retained, and running `Melodia --version` after spawning msiexec
/// would race the live binary's replacement.
///
/// `/qb!` = basic UI (progress dialog, *no* cancel button). The user
/// already confirmed the install in-app; surfacing a second cancel
/// midway through is footgun-shaped (cancelling mid-write leaves the
/// install half-done, and Windows Installer's rollback isn't bullet-
/// proof when Restart Manager is in play). `/qb` (with cancel) is the
/// safer general default but a worse fit for the auto-update flow.
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
    // Keep the staged file on disk on failure — a retry without
    // re-download reuses the verified bytes. PATH issues are the
    // most likely failure cause (msiexec lives at
    // `%SystemRoot%\System32\msiexec.exe` and shouldn't be missing,
    // but stripped containers / sandboxed dev envs can drop the
    // System32 dir from PATH).
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
    // `current_target_key()` never returns windows-*-msi off Windows,
    // so `resolve_install_method()` never picks `WindowsMsi` and this
    // branch is dead. The stub exists to keep the call site
    // compile-clean on non-Windows.
    Err(AppError::Settings("msiexec install is only supported on Windows".into()))
}

/// Atomic in-place swap of the live binary at `target` with the
/// already-verified `staged` file. Branches on `cfg!(target_os)`:
///
/// * **Linux** — `fs::rename(staged, target)` directly over the
///   running file. The kernel unlinks the old inode but the running
///   process's open fd stays valid; new launches pick up the swapped
///   binary. If the rename fails with `PermissionDenied` (RPM/.deb
///   install at `/usr/bin/`), falls back to `pkexec mv` so KDE /
///   GNOME's polkit agent can prompt the user for elevation.
/// * **Windows** — Windows refuses to delete or rename a file that's
///   currently loaded as a running executable. The dance is: rename
///   the running `Melodia.exe` → `Melodia.exe.old`, rename
///   `Melodia.exe.new` → `Melodia.exe`, then schedule
///   `Melodia.exe.old` for delete on reboot via
///   `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)`. Most of the time
///   the early `remove_file` in `main()` will clear the stale `.old`
///   on the next launch (before it's loaded again); the reboot
///   fallback only kicks in if `.old` is still pinned (e.g. user
///   opened a second window before quitting).
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
    // Same-filesystem two-step rename so we keep a `.old` snapshot of
    // the previously-running binary for rollback if `verify_swapped_binary`
    // (back in the async caller) reports the new binary won't boot.
    // Mirrors `windows_swap`.
    //
    // Step 1: clear any stale `.old` from a previous swap (best-effort —
    // a stuck `.old` would only be reaped by `main()`'s startup remove,
    // which doesn't run if the current process is the one that left it).
    let old = old_path(target);
    let _ = std::fs::remove_file(&old);

    // Step 2: target → target.old (same-fs rename, atomic, preserves
    // the inode for rollback). If this fails with PermissionDenied
    // (root-owned dir like /opt/) or CrossesDevices, we can't keep a
    // `.old` and have to fall through to the pkexec path. That path
    // doesn't carry rollback safety — see the comment on
    // `elevate_swap_via_pkexec`.
    if let Err(e) = std::fs::rename(target, &old) {
        if matches!(
            e.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::CrossesDevices
        ) {
            return elevate_swap_via_pkexec(target, staged);
        }
        return Err(AppError::from(e));
    }

    // Step 3: staged → target. If this fails, rename `.old` back so
    // the live binary path isn't missing. Restore failure is logged
    // but doesn't override the original swap error.
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

    // `.old` deliberately stays on disk. Reaped by main()'s startup
    // remove on first successful boot of the new binary (which only
    // gets a chance to run after `verify_swapped_binary` accepts).
    Ok(())
}

/// `pkexec mv` is **not atomic** when the source and target live on
/// different filesystems — `mv` falls back to copy-then-unlink, so a
/// power loss mid-copy can leave a partial target. We accept that
/// trade-off here: the alternative is two pkexec invocations (one to
/// stage inside the target's filesystem, one to rename) which doubles
/// the polkit prompts. Production RPM/deb installs never reach this
/// path because `is_system_install()` gates the UI before the daily
/// task spawns; what's left is the rare "user dropped a tarball under
/// root-owned `/opt/`" case where retry-on-fail is acceptable.
#[cfg(target_os = "linux")]
fn elevate_swap_via_pkexec(target: &Path, staged: &Path) -> AppResult<()> {
    // KDE: polkit-kde-authentication-agent-1 catches the request and
    // shows a password dialog parented to the active session.
    // GNOME: polkit-gnome-authentication-agent-1.
    // Both ship by default on Fedora KDE / Workstation. On stripped
    // installs without polkit we surface a clear error and let the
    // user pick a per-user install instead.
    log::info!("updater: elevating swap via pkexec ({} → {})", staged.display(), target.display());
    let output =
        std::process::Command::new("pkexec").arg("mv").arg("--").arg(staged).arg(target).output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Clean up the staged file so we don't leak it; the
            // user's next action will be to re-install elsewhere.
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
        // pkexec exit codes per pkexec(1): 126 = user dismissed the
        // auth dialog; 127 = catch-all for "not authorized OR no
        // polkit agent OR missing policy OR D-Bus error OR auth
        // failed" — pkexec does not distinguish these, so we surface
        // stderr verbatim. Anything else is the spawned `mv` failing
        // (or a `mv`-exit code that happens to overlap pkexec's 127,
        // which is unavoidable). Don't delete `staged` here — keep it
        // so a retry can re-use the verified bytes (download was
        // successful + signature already checked).
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
    // Clear any stale `.old` from a previous incomplete swap. Best-effort
    // — on the rare case where it's still loaded the reboot-delete will
    // pick it up.
    let _ = std::fs::remove_file(&old);

    std::fs::rename(target, &old)?;
    if let Err(e) = std::fs::rename(staged, target) {
        // Try to undo the first rename so the user isn't left with a
        // missing executable. If that fails too the user has to recover
        // manually — but the more common case is a transient AV lock
        // that lets the rollback succeed.
        let _ = std::fs::rename(&old, target);
        return Err(AppError::from(e));
    }

    let wide: Vec<u16> = wide_with_nul(&old);
    // Best-effort: returns 0 on failure but the binary swap already
    // succeeded — the user keeps a stale `.old` until reboot or the
    // next launch's `remove_file` cleanup. Logged, not failed.
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

/// `<target>.old` — the rollback copy preserved by `linux_swap` and
/// `windows_swap` after a successful binary rename. Mirrors the layout
/// of [`super::staging::staged_path`] (which appends `.new`) so the swap
/// dance leaves `target` + `target.old` siblings on disk until first
/// successful boot reaps the `.old`.
///
/// `pub(crate)` rather than module-private so
/// `services::updater::install_target_old()` (the version `main.rs`
/// calls at startup) shares this exact path-derivation logic.
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
