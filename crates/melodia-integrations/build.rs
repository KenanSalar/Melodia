//! Injects the compile-time API keys the Last.fm and Discord modules read with `option_env!`.
//!
//! Here rather than in the binary's build script because `cargo:rustc-env` reaches only the crate
//! whose script emitted it. The `.env` it reads stays at the repo root, where contributors and the
//! `gh secret` workflow already look for it, reached through `CARGO_MANIFEST_DIR` since cargo sets
//! a build script's working directory to its own package root.

use std::path::{Path, PathBuf};

fn main() {
    load_dotenv();
}

/// The repo root, two directories above this crate.
fn repo_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    Path::new(&manifest).join("..").join("..")
}

/// Load compile-time secrets (the Last.fm API keys) from a local, gitignored
/// `.env`, injecting each `KEY=value` via `cargo:rustc-env` so `option_env!(KEY)`
/// picks it up at compile time. Purely a local-dev convenience — guarded so no
/// other build path can break:
///
/// - **No `.env`** — the case for contributors, forks, and release/CI builds — is
///   a silent no-op, so a build never fails on its absence.
/// - **The environment wins**: a key already set (a shell export, or CI's
///   GitHub-secret env var from `release-build.yml`) is left untouched, never
///   overwritten by `.env`.
///
/// A keyless build is fully supported: `lastfm::is_configured()` returns false
/// and the app ships ListenBrainz-only with an inert Last.fm Connect button.
fn load_dotenv() {
    // Re-run this script when `.env` appears / changes / vanishes. Harmless when
    // the file never exists.
    let dotenv = repo_root().join(".env");
    println!("cargo:rerun-if-changed={}", dotenv.display());
    let Ok(contents) = std::fs::read_to_string(&dotenv) else {
        return;
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches(['"', '\'']);
        // Only fall back to `.env` when the variable isn't already in the build
        // environment, so a shell export or CI secret is never clobbered.
        if !key.is_empty() && std::env::var_os(key).is_none() {
            println!("cargo:rustc-env={key}={value}");
        }
    }
}
