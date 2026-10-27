//! Injects the compile-time API keys the Last.fm and Discord modules read with `option_env!`.
//!
//! Here rather than in the binary's build script because `cargo:rustc-env` reaches only the crate
//! whose script emitted it. The `.env` stays at the repo root, where contributors and the
//! `gh secret` workflow already look for it, reached through `CARGO_MANIFEST_DIR` since cargo runs
//! a build script from its own package root.

use std::path::{Path, PathBuf};

fn main() {
    load_dotenv();
}

fn repo_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    Path::new(&manifest).join("..").join("..")
}

/// Hands each `KEY=value` in the gitignored `.env` to `option_env!`, a local-dev convenience only.
/// A missing file is a silent no-op, which is every fork's, CI's and release's case, and a key
/// already in the environment (a shell export, CI's secret) is never overwritten.
fn load_dotenv() {
    // Watched even while absent, so a `.env` created later is picked up.
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
        if !key.is_empty() && std::env::var_os(key).is_none() {
            println!("cargo:rustc-env={key}={value}");
        }
    }
}
