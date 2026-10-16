fn main() {
    #[cfg(target_os = "windows")]
    embed_windows_resource();
}

/// The repo-root icon, reached from this package rather than copied into it, so the MSI's
/// `ProductICO` and the embedded resource cannot drift apart. `wix/main.wxs` spells the same
/// file through its own `$(var.RepoRoot)`.
///
/// Relative to the package root, which is where cargo puts a build script's working directory.
#[cfg(target_os = "windows")]
const ICON: &str = "../../assets/melodia.ico";

/// What Windows calls the running app, in Task Manager and the volume mixer, which both read
/// `FileDescription`. winresource fills it and `ProductName` from the package name, which is
/// the lowercase `melodia`.
#[cfg(target_os = "windows")]
const APP_NAME: &str = "Melodia";

/// Embed [`ICON`] as the EXE's primary `ICON` resource, and [`APP_NAME`] in its version info.
/// Windows' shell pulls the icon for the titlebar's top-left glyph, the taskbar button, the
/// Alt-Tab thumbnail badge, and the Explorer file icon. Without an embedded resource the
/// running window falls back to a generic placeholder even when the Start-Menu shortcut has
/// its own icon (`WiX` `ProductICO`).
#[cfg(target_os = "windows")]
fn embed_windows_resource() {
    println!("cargo:rerun-if-changed={ICON}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ICON).set("ProductName", APP_NAME).set("FileDescription", APP_NAME);
    if let Err(e) = res.compile() {
        // `cargo::error=` is the build script's own failure channel — a clearer
        // report than unwinding out of `main` with a Debug-formatted error.
        println!("cargo::error=failed to embed the Windows resource: {e}");
        std::process::exit(1);
    }
}
