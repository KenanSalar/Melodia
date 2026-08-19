fn main() {
    // Inject compile-time secrets (the Last.fm API keys) from a local, gitignored
    // `.env` so dev builds work without exporting them each session. Guarded on
    // every side — see `load_dotenv`.
    //
    // This stays put rather than following the Slint compilation into
    // `melodia-ui/build.rs`: `cargo:rustc-env` only reaches the crate whose build
    // script emitted it, so moving it would leave `option_env!` resolving to
    // `None` and every build silently shipping keyless.
    load_dotenv();

    #[cfg(target_os = "windows")]
    embed_windows_icon();
}

/// Embed `assets/melodia.ico` as the EXE's primary `ICON` resource. Windows'
/// shell pulls this for the titlebar's top-left glyph, the taskbar button, the
/// Alt-Tab thumbnail badge, and the Explorer file icon. Without an embedded
/// resource the running window falls back to a generic placeholder even when
/// the Start-Menu shortcut has its own icon (`WiX` `ProductICO`).
#[cfg(target_os = "windows")]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=assets/melodia.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/melodia.ico");
    if let Err(e) = res.compile() {
        // `cargo::error=` is the build script's own failure channel — a clearer
        // report than unwinding out of `main` with a Debug-formatted error.
        println!("cargo::error=failed to embed assets/melodia.ico: {e}");
        std::process::exit(1);
    }
}

/// Load compile-time secrets (the Last.fm API keys) from a local, gitignored
/// `.env`, injecting each `KEY=value` via `cargo:rustc-env` so `option_env!(KEY)`
/// picks it up at compile time. Purely a local-dev convenience — guarded so no
/// other build path can break:
///
/// - **No `.env`** — the case for contributors, forks, and release/CI builds — is
///   a silent no-op, so a build never fails on its absence.
/// - **The environment wins**: a key already set (a shell export, or CI's
///   GitHub-secret env var from `release.yml`) is left untouched, never
///   overwritten by `.env`.
///
/// A keyless build is fully supported: `lastfm::is_configured()` returns false
/// and the app ships ListenBrainz-only with an inert Last.fm Connect button.
fn load_dotenv() {
    // Re-run this script when `.env` appears / changes / vanishes. Harmless when
    // the file never exists.
    println!("cargo:rerun-if-changed=.env");
    let Ok(contents) = std::fs::read_to_string(".env") else {
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
