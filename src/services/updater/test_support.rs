//! Shared test-only helpers for the updater module's sibling test
//! files.
//!
//! Cargo runs `#[test]` bodies in parallel across threads within a
//! single test binary. Several updater tests mutate or read
//! `$APPIMAGE` (via `install_target()` → `target::current_target_key()`
//! → `system_install::is_system_install()`), and a per-file `static
//! Mutex` would let them race against each other across module
//! boundaries. Hoisting the lock here gives every caller a single
//! cross-module serialisation point.
//!
//! Gated on `cfg(test)` at the `mod` declaration in `mod.rs` —
//! never compiled into production binaries.

use std::sync::Mutex;

/// Serialises every test that mutates or reads `$APPIMAGE`. Acquired
/// for the entire `set → body → restore` sequence so no sibling test
/// can interleave a `set_var`/`remove_var` mid-flight. Poisoned
/// guards are accepted (the previous holder always restored the env
/// before `resume_unwind`, so the env state stays consistent).
///
/// Currently used by:
/// - `services::updater::target::tests` (5 tests)
/// - `services::updater::system_install::tests` (1 test)
pub(crate) static APPIMAGE_ENV_LOCK: Mutex<()> = Mutex::new(());
