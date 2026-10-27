fn main() {
    #[cfg(target_os = "windows")]
    embed_windows_resource();
}

/// The repo-root icon, the file `wix/main.wxs` hands the MSI as `ProductICO`, so the two can't
/// drift. Relative to the package root, a build script's working directory.
#[cfg(target_os = "windows")]
const ICON: &str = "../../assets/melodia.ico";

/// What Task Manager and the volume mixer call the running app. winresource would take the
/// lowercase package name.
#[cfg(target_os = "windows")]
const APP_NAME: &str = "Melodia";

/// Embeds [`ICON`] and [`APP_NAME`] in the EXE. Without the resource the running window shows a
/// generic icon even where the Start-menu shortcut has its own.
#[cfg(target_os = "windows")]
fn embed_windows_resource() {
    println!("cargo:rerun-if-changed={ICON}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ICON).set("ProductName", APP_NAME).set("FileDescription", APP_NAME);
    if let Err(e) = res.compile() {
        // Cargo's own failure channel, clearer than a panic's Debug-formatted error.
        println!("cargo::error=failed to embed the Windows resource: {e}");
        std::process::exit(1);
    }
}
