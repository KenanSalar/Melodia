//! Test-only helpers shared across the whole crate's unit tests.
//!
//! Gated on `cfg(test)` at the `mod` declaration in `lib.rs` — never compiled
//! into production binaries.

#![allow(
    unsafe_code,
    reason = "env::set_var/remove_var are unsafe in Rust 2024; every mutation here happens under ENV_LOCK and is restored under catch_unwind."
)]

use std::sync::Mutex;

/// Serialises **every** test in this binary that mutates or reads the process
/// environment.
///
/// One lock for the whole binary, not one per file or per variable: `cargo
/// test` runs `#[test]` bodies in parallel threads of a single process, the
/// environment is process-global, and glibc's `setenv` may reallocate `environ`
/// out from under another thread's `getenv`. Two tests holding *different*
/// locks are therefore still racing, however careful each is on its own — which
/// is exactly the race Rust 2024 made `set_var`/`remove_var` unsafe for. It
/// lives at the crate root rather than beside any one caller because the set of
/// callers spans unrelated modules and the variables they touch overlap through
/// readers neither of them owns: `SettingsData::default()` reads
/// `XDG_CURRENT_DESKTOP` via `is_kde_desktop()`, and `install_target()` reaches
/// `$APPIMAGE` through `target::current_target_key()`.
///
/// **Private on purpose** — [`with_env_vars`] is the only way to take it. A
/// `pub(crate)` lock invites a caller to hand-roll the snapshot/restore around
/// it, and three of those had already diverged before they were consolidated
/// here; the restore is the half that goes missing.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquires [`ENV_LOCK`], saves `vars`, clears them, runs `body`, then restores
/// the originals — including when `body` panics, so a failing assertion can't
/// leak its variable into the rest of the process.
///
/// The whole shape in one place: **lock → snapshot → clear → `catch_unwind` →
/// restore → `resume_unwind`.** `body` may set any of `vars` itself; it runs
/// with the lock held, so its own `set_var` is sound for the same reason this
/// function's is.
///
/// # Safety
///
/// `std::env::set_var` / `remove_var` mutate shared process state. Holding
/// [`ENV_LOCK`] across the entire sequence is what makes that sound, and it only
/// works while *every* env-mutating test in the binary goes through here — so
/// don't reach past this function for a second lock.
///
/// # Panics
///
/// Re-raises whatever `body` panicked with, after the environment is restored.
pub(crate) unsafe fn with_env_vars<F: FnOnce() -> R, R>(vars: &[&str], body: F) -> R {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let saved: Vec<(&str, Option<String>)> =
        vars.iter().map(|&v| (v, std::env::var(v).ok())).collect();
    for &v in vars {
        unsafe { std::env::remove_var(v) };
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    for (var, val) in saved {
        unsafe {
            match val {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }
    }
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Runs `body` with `$APPIMAGE` set to `value`, or cleared when it is `None`.
///
/// Lives here rather than beside either caller because two unrelated test
/// modules override it — `updater::target::tests` and
/// `updater::linux_pkg::tests` — and it reaches production code neither of them
/// owns (`install_target()` → `target::current_target_key()`).
pub(crate) fn with_appimage_env<F: FnOnce()>(value: Option<&str>, body: F) {
    // SAFETY: `with_env_vars` holds `ENV_LOCK` across the whole body and puts
    // `$APPIMAGE` back afterwards, panicking assertion or not. It has already
    // cleared the variable by the time `body` runs, so `None` needs no write.
    unsafe {
        with_env_vars(&["APPIMAGE"], || {
            if let Some(path) = value {
                std::env::set_var("APPIMAGE", path);
            }
            body();
        });
    }
}
