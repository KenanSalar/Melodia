fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Bundle gettext-style `.po` translations into the compiled UI so locale
    // switching at runtime (`slint::select_bundled_translation`) re-renders
    // every `@tr(...)` without a system gettext dependency. Layout:
    //
    //   translations/<lang>/LC_MESSAGES/melodia-ui.po
    //
    // The basename is not a free choice: slint-build derives the gettext domain
    // from `CARGO_PKG_NAME` and exposes no override, so the catalogs have to be
    // named after *this* package. Renaming the crate means renaming all six
    // `.po` files with it, or the build fails with "No translations found".
    //
    // `DefaultTranslationContext::None` keeps msgids context-free: identical
    // English strings across components share a single msgstr ("Cancel"
    // translates the same everywhere). Matches `slint-tr-extractor
    // --no-default-translation-context` for extraction.
    //
    // Both paths are relative to this crate's manifest dir, which is where
    // slint-build resolves them from — not the working directory. The UI tree
    // and the catalogs live inside the crate, so neither needs to reach out.
    //
    // Slint's AST compiler walks the UI tree recursively and overflows
    // Windows' 1 MiB default main-thread stack with STATUS_STACK_OVERFLOW.
    // Linux ships an 8 MiB default. Spawn the compile on an explicitly-
    // sized thread so the same code path works on every host without
    // depending on linker flags or env vars.
    let join = std::thread::Builder::new().stack_size(16 * 1024 * 1024).spawn(
        || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let cfg = slint_build::CompilerConfiguration::new()
                .with_bundled_translations("translations")
                .with_default_translation_context(slint_build::DefaultTranslationContext::None);
            slint_build::compile_with_config("ui/app-window.slint", cfg)?;
            Ok(())
        },
    )?;
    match join.join() {
        Ok(inner) => inner?,
        Err(payload) => std::panic::resume_unwind(payload),
    }
    println!("cargo:rerun-if-changed=translations");

    Ok(())
}
