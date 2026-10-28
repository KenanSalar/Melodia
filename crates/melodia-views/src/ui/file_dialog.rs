//! Opening a native file dialog against the main window. A parentless `rfd` dialog is
//! indistinguishable from a correct one on Linux, the XDG portal parenting OS-side, and opens
//! *behind* Melodia on Windows and macOS, hence one helper. Its test pins this file to still
//! calling `set_parent`, a deletion here reading as a simplification.

use slint::ComponentHandle;

use melodia_ui::AppWindow;

/// A native dialog parented to the main window, titled `title`. Chain the rest at the call
/// site.
///
/// **Call from the UI thread**: `Weak::upgrade` is UI-thread-only, and on Linux so is the
/// portal-backed dialog. A dropped window builds the dialog unparented. `set_parent` only stashes
/// the raw handles, so the strong handle taken here may drop as soon as this returns.
pub fn parented(weak: &slint::Weak<AppWindow>, title: &str) -> rfd::AsyncFileDialog {
    let dialog = rfd::AsyncFileDialog::new().set_title(title);
    match weak.upgrade() {
        Some(ui) => dialog.set_parent(&ui.window().window_handle()),
        None => dialog,
    }
}
