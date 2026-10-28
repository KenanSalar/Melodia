//! The process environment, changed and read under one lock per test binary.

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Serialises every test in this binary that mutates the process environment, and every test
/// opting into reading it through [`reading_env`]. One lock for the whole *binary*, since two
/// tests holding different locks still race; the read side opt-in, so a reader that hasn't
/// wrapped itself still races; and private, an exported lock inviting the hand-rolled
/// snapshot/restore whose restore goes missing.
static ENV_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// Set while this thread holds [`ENV_LOCK`]. The mutex is not reentrant, so a nested
    /// call would hang the binary with no message; this turns that into a named panic.
    static ENV_LOCK_HELD: Cell<bool> = const { Cell::new(false) };
}

/// [`ENV_LOCK`] and the reentrancy flag held together, so both release on the way out of a
/// panicking body as well as a returning one.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// # Panics
    ///
    /// If this thread already holds the lock — the alternative is a silent deadlock.
    fn acquire() -> Self {
        assert!(
            !ENV_LOCK_HELD.get(),
            "the env helpers are not reentrant: this thread already holds the \
             environment lock, so taking it again would deadlock with no message \
             and no failing assertion. A per-variable wrapper must *delegate* to \
             `with_env_set` rather than lock and then call it."
        );
        // Poison is accepted — the previous holder restored the environment before unwinding.
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

/// Runs `body` with `clear` removed from the environment and `set` applied on top, then
/// restores the originals — including when `body` panics, so a failing assertion can't leak a
/// variable into the rest of the process.
///
/// Every variable in `set` must also appear in `clear`, or there is nothing snapshotted to
/// put it back from. Safe to call, which is encapsulation rather than a gap: this being the
/// binary's only mutation site, "every mutation happens under `ENV_LOCK`" is a property of
/// the module rather than something each caller re-argues.
///
/// # Panics
///
/// Re-raises whatever `body` panicked with, after the environment is restored. Panics up
/// front on a nested call — see [`ENV_LOCK_HELD`].
#[allow(
    unsafe_code,
    reason = "env::set_var/remove_var are unsafe in Rust 2024; every mutation in the test binary happens in this function, under ENV_LOCK, restored under catch_unwind."
)]
pub fn with_env_set<F: FnOnce() -> R, R>(clear: &[&str], set: &[(&str, &str)], body: F) -> R {
    debug_assert!(
        set.iter().all(|(var, _)| clear.contains(var)),
        "a variable in `set` that isn't in `clear` is never restored",
    );

    let _guard = EnvGuard::acquire();
    let saved: Vec<(&str, Option<String>)> =
        clear.iter().map(|&v| (v, std::env::var(v).ok())).collect();

    // SAFETY: `ENV_LOCK` is held across every mutation below *and* across `body`, and the
    // restore runs whether `body` returns or unwinds — so with every other mutation in the
    // binary coming through here too, no writer can overlap another. That discharges the
    // writer half of `set_var`'s contract; the reader half is `reading_env`'s, and opt-in.
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
pub fn with_env_var<F: FnOnce() -> R, R>(var: &str, value: Option<&str>, body: F) -> R {
    match value {
        Some(v) => with_env_set(&[var], &[(var, v)], body),
        None => with_env_set(&[var], &[], body),
    }
}

/// Runs `body` with `$APPIMAGE` set to `value`, or cleared when it is `None`.
///
/// Named rather than spelled at each call site because three unrelated test modules override
/// it and it reaches production code none of them owns (`install_target()`).
pub fn with_appimage_env<F: FnOnce() -> R, R>(value: Option<&str>, body: F) -> R {
    with_env_var("APPIMAGE", value, body)
}

/// Runs `body` under the same lock the mutating helpers take, without touching a
/// variable.
///
/// For a test that only *reads* the environment, directly or through production code that
/// does: a read through `std::env` is safe beside a sibling's mutation, std locking the two
/// against each other, but it can see the value that sibling set, which a test asserting on
/// what it read can't afford. `SettingsData::default()` is the reader in this tree, reaching
/// `XDG_CURRENT_DESKTOP` and all four locale variables through its serde defaults.
pub fn reading_env<F: FnOnce() -> R, R>(body: F) -> R {
    let _guard = EnvGuard::acquire();
    body()
}

#[cfg(test)]
#[path = "tests/env_tests.rs"]
mod tests;
