fn main() {
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
