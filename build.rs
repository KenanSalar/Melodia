fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
