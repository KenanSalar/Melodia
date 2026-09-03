fn main() {
    #[cfg(target_os = "windows")]
    embed_windows_icon();
}

/// The repo-root icon, reached from this package rather than copied into it, so the MSI's
/// `ProductICO` and the embedded resource cannot drift apart. `wix/main.wxs` spells the same
/// file through its own `$(var.RepoRoot)`.
///
/// Relative to the package root, which is where cargo puts a build script's working directory.
#[cfg(target_os = "windows")]
const ICON: &str = "../../assets/melodia.ico";

/// Embed [`ICON`] as the EXE's primary `ICON` resource. Windows' shell pulls this for the
/// titlebar's top-left glyph, the taskbar button, the Alt-Tab thumbnail badge, and the
/// Explorer file icon. Without an embedded resource the running window falls back to a
/// generic placeholder even when the Start-Menu shortcut has its own icon (`WiX`
/// `ProductICO`).
#[cfg(target_os = "windows")]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed={ICON}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ICON);
    if let Err(e) = res.compile() {
        // `cargo::error=` is the build script's own failure channel — a clearer
        // report than unwinding out of `main` with a Debug-formatted error.
        println!("cargo::error=failed to embed {ICON}: {e}");
        std::process::exit(1);
    }
}
