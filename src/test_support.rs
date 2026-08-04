//! Test-only helpers shared across the whole crate's unit tests.
//!
//! Gated on `cfg(test)` at the `mod` declaration in `lib.rs` — never compiled
//! into production binaries.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::config::Paths;

/// A solid-colour `side` × `side` PNG in a fresh temp dir. The dir is returned
/// alongside the path so the caller can keep it alive — dropping it deletes the
/// file, which is the failure mode to watch for when adopting this.
///
/// Shared because every cover-cache test needs a real decodable image and the
/// two that did each wrote their own; the tier tests want a large source to
/// downscale from and the lookup tests only want *an* image, so the size is the
/// one thing worth parameterising.
pub(crate) fn write_test_png(
    side: u32,
) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("cover.png");
    image::RgbImage::from_pixel(side, side, image::Rgb([120, 60, 200])).save(&path)?;
    Ok((tmp, path))
}

/// A [`Paths`] rooted in a throwaway directory, with the same subdirectories
/// [`Paths::resolve`] creates already in place — so a test that writes into one
/// doesn't have to know which of the fields is a directory.
///
/// Shared rather than hand-rolled per test file because the struct literal names
/// every field, so each copy is one more site to fix when `Paths` grows.
/// Directory creation is best-effort: the caller passed a `TempDir` it just
/// created, and a failure here surfaces as a plain missing-file error in the
/// test body.
pub(crate) fn paths_in(dir: &Path) -> Paths {
    let artwork_dir = dir.join("artwork");
    let artists_dir = dir.join("artists");
    let backups_dir = dir.join("backups");
    for sub in [&artwork_dir, &artists_dir, &backups_dir] {
        let _ = std::fs::create_dir_all(sub);
    }

    Paths {
        data_dir: dir.to_path_buf(),
        db_path: dir.join("melodia.db"),
        settings_path: dir.join("settings.json"),
        view_state_path: dir.join("views.json"),
        queue_path: dir.join("queue.json"),
        search_history_path: dir.join("search_history.json"),
        scrobble_credentials_path: dir.join("scrobble_credentials.json"),
        scrobble_queue_path: dir.join("scrobble_queue.json"),
        scrobble_mbid_state_path: dir.join("scrobble_mbid_attempted.json"),
        artwork_dir,
        artists_dir,
        backups_dir,
    }
}

/// Serialises every test in this binary that mutates the process environment,
/// and every test that opts into reading it through [`reading_env`].
///
/// One lock for the whole binary, not one per file or per variable: `cargo
/// test` runs `#[test]` bodies in parallel threads of a single process, the
/// environment is process-global, and glibc's `setenv` may reallocate `environ`
/// out from under another thread's `getenv`. Two tests holding *different*
/// locks are therefore still racing, however careful each is on its own — which
/// is exactly the race Rust 2024 made `set_var`/`remove_var` unsafe for. It
/// lives at the crate root rather than beside any one caller because the set of
/// callers spans unrelated modules and the variables they touch overlap through
/// readers neither of them owns: `SettingsData::default()` reaches
/// `XDG_CURRENT_DESKTOP` via `is_kde_desktop()` *and* all four locale variables
/// via `default_locale()`, and `install_target()` reaches `$APPIMAGE` through
/// `target::current_target_key()`.
///
/// **The read side is opt-in, and that is the limit of what this enforces.**
/// `set_var`'s contract is symmetric — std spells it "no other threads
/// concurrently writing or *reading*(!) the environment" — so a test that only
/// reads is as much a party to the race as a second writer. Nothing makes such a
/// test take a lock; [`reading_env`] is how one opts in, and a reader that
/// hasn't is still racing. Wrap one as soon as you find it rather than assuming
/// this lock already covers it.
///
/// **Private on purpose** — the helpers below are the only way to take it. A
/// `pub(crate)` lock invites a caller to hand-roll the snapshot/restore around
/// it, and three of those had already diverged before they were consolidated
/// here; the restore is the half that goes missing.
static ENV_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// Set while this thread holds [`ENV_LOCK`]. The mutex is not reentrant, so
    /// a nested call would hang the binary with no message and no failing
    /// assertion — the worst failure mode to leave undetected. This turns it
    /// into a named panic instead.
    static ENV_LOCK_HELD: Cell<bool> = const { Cell::new(false) };
}

/// [`ENV_LOCK`] and the reentrancy flag held together, so both are released on
/// the way out of a panicking body as well as a returning one.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// # Panics
    ///
    /// If this thread already holds the lock. Panicking is the point: the
    /// alternative is a silent deadlock.
    fn acquire() -> Self {
        assert!(
            !ENV_LOCK_HELD.get(),
            "the env helpers are not reentrant: this thread already holds the \
             environment lock, so taking it again would deadlock with no message \
             and no failing assertion. A per-variable wrapper must *delegate* to \
             `with_env_set` rather than lock and then call it."
        );
        // A poisoned guard is accepted: the previous holder restored the
        // environment before it resumed unwinding, so the state is consistent.
        let lock = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        ENV_LOCK_HELD.set(true);
        Self { _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        ENV_LOCK_HELD.set(false);
    }
}

/// Runs `body` with `clear` removed from the environment and `set` applied on
/// top, then restores the originals — including when `body` panics, so a failing
/// assertion can't leak a variable into the rest of the process.
///
/// The whole shape in one place: **lock → snapshot → clear → set →
/// `catch_unwind` → restore → `resume_unwind`.** Every variable in `set` must
/// also appear in `clear`, or there is nothing snapshotted to put it back from.
///
/// Safe to call, and that is encapsulation rather than a gap: this is the only
/// place in the binary that mutates the environment, so "every mutation happens
/// under `ENV_LOCK`" is a property of this module instead of something each
/// caller re-argues.
///
/// # Panics
///
/// Re-raises whatever `body` panicked with, after the environment is restored.
/// Panics up front on a nested call — see [`ENV_LOCK_HELD`].
#[allow(
    unsafe_code,
    reason = "env::set_var/remove_var are unsafe in Rust 2024; every mutation in the test binary happens in this function, under ENV_LOCK, restored under catch_unwind."
)]
pub(crate) fn with_env_set<F: FnOnce() -> R, R>(
    clear: &[&str],
    set: &[(&str, &str)],
    body: F,
) -> R {
    debug_assert!(
        set.iter().all(|(var, _)| clear.contains(var)),
        "a variable in `set` that isn't in `clear` is never restored",
    );

    let _guard = EnvGuard::acquire();
    let saved: Vec<(&str, Option<String>)> =
        clear.iter().map(|&v| (v, std::env::var(v).ok())).collect();

    // SAFETY: `ENV_LOCK` is held across every mutation below *and* across
    // `body`, and the restore runs whether `body` returns or unwinds — so with
    // every other mutation in the binary coming through here too, no writer can
    // overlap another. That discharges the writer half of `set_var`'s contract;
    // the reader half is `reading_env`'s job and is opt-in, which the `ENV_LOCK`
    // doc says plainly rather than letting this comment imply otherwise.
    unsafe {
        for &v in clear {
            std::env::remove_var(v);
        }
        for (var, value) in set {
            std::env::set_var(var, value);
        }
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));

    // SAFETY: as above — still the same guard, still the same lock.
    unsafe {
        for (var, value) in saved {
            match value {
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

/// Runs `body` with `var` set to `value`, or merely cleared when it is `None`.
pub(crate) fn with_env_var<F: FnOnce() -> R, R>(var: &str, value: Option<&str>, body: F) -> R {
    match value {
        Some(v) => with_env_set(&[var], &[(var, v)], body),
        None => with_env_set(&[var], &[], body),
    }
}

/// Runs `body` with `$APPIMAGE` set to `value`, or cleared when it is `None`.
///
/// Named rather than spelled out at each call site because three unrelated test
/// modules override it — `updater::{target,linux_pkg,system_install}::tests` —
/// and it reaches production code none of them owns (`install_target()` →
/// `target::current_target_key()`).
pub(crate) fn with_appimage_env<F: FnOnce() -> R, R>(value: Option<&str>, body: F) -> R {
    with_env_var("APPIMAGE", value, body)
}

/// Runs `body` under the same lock the mutating helpers take, without touching a
/// variable.
///
/// For a test that only *reads* the environment, directly or through production
/// code that does. `set_var`'s contract is symmetric, so such a test races a
/// sibling's mutation exactly as a second mutator would; this is how it opts out
/// of that race. `SettingsData::default()` is the reader in this tree — it
/// reaches `XDG_CURRENT_DESKTOP` and all four locale variables through its serde
/// defaults, and the tests that build one sit in the same file as the tests that
/// mutate both.
pub(crate) fn reading_env<F: FnOnce() -> R, R>(body: F) -> R {
    let _guard = EnvGuard::acquire();
    body()
}

#[cfg(test)]
#[path = "tests/test_support_tests.rs"]
mod tests;
