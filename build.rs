fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Inject compile-time secrets (the Last.fm API keys) from a local, gitignored
    // `.env` so dev builds work without exporting them each session. Guarded on
    // every side — see `load_dotenv`.
    load_dotenv();

    // Bundle gettext-style `.po` translations into the compiled UI so locale
    // switching at runtime (`slint::select_bundled_translation`) re-renders
    // every `@tr(...)` without a system gettext dependency. Layout:
    //
    //   translations/<lang>/LC_MESSAGES/Melodia.po
    //
    // `DefaultTranslationContext::None` keeps msgids context-free: identical
    // English strings across components share a single msgstr ("Cancel"
    // translates the same everywhere). Matches `slint-tr-extractor
    // --no-default-translation-context` for extraction.
    //
    // Slint's AST compiler walks the UI tree recursively and overflows
    // Windows' 1 MiB default main-thread stack with STATUS_STACK_OVERFLOW.
    // Linux ships an 8 MiB default. Spawn the compile on an explicitly-
    // sized thread so the same code path works on every host without
    // depending on linker flags or env vars.
    let join = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let cfg = slint_build::CompilerConfiguration::new()
                .with_bundled_translations("translations")
                .with_default_translation_context(slint_build::DefaultTranslationContext::None);
            slint_build::compile_with_config("ui/app-window.slint", cfg)?;
            Ok(())
        })?;
    match join.join() {
        Ok(inner) => inner?,
        Err(payload) => std::panic::resume_unwind(payload),
    }
    println!("cargo:rerun-if-changed=translations");

    // Windows: embed `assets/melodia.ico` as the EXE's primary `ICON`
    // resource. Windows' shell pulls this for the titlebar's top-left
    // glyph, the taskbar button, the Alt-Tab thumbnail badge, and the
    // Explorer file icon. Without an embedded resource the running
    // window falls back to a generic placeholder even when the
    // Start-Menu shortcut has its own icon (WiX `ProductICO`).
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=assets/melodia.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/melodia.ico");
        res.compile()?;
    }

    Ok(())
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
