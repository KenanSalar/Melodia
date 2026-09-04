fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // The `translations/<lang>/LC_MESSAGES/melodia-ui.po` catalogs bundle into the
    // compiled UI, so `slint::select_bundled_translation` re-renders every `@tr(...)`
    // with no system gettext dependency. That basename is not a free choice:
    // slint-build derives the gettext domain from `CARGO_PKG_NAME` and exposes no
    // override, so renaming the crate means renaming all six or the build fails with
    // "No translations found". Both paths resolve against this crate's manifest dir
    // rather than the working directory.
    //
    // `DefaultTranslationContext::None` keeps msgids context-free, so identical
    // English across components shares one msgstr — matching extraction under
    // `slint-tr-extractor --no-default-translation-context`.
    //
    // The compile goes on an explicitly-sized thread because Slint's recursive AST
    // walk overflows Windows' 1 MiB default main-thread stack; Linux's 8 MiB hides it.
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
