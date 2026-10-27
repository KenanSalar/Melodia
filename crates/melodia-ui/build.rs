fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // The catalogs' basename is the gettext domain, which slint-build takes from `CARGO_PKG_NAME`
    // with no override: renaming the crate renames every catalog, or the build fails with
    // "No translations found".
    //
    // Context-free msgids let identical English across components share one msgstr, matching
    // `slint-tr-extractor --no-default-translation-context`.
    //
    // Slint's recursive AST walk overflows Windows' 1 MiB main-thread stack, which Linux's 8 MiB
    // hides.
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

    Ok(())
}
