//! Opening a native file dialog against the main window.
//!
//! `rfd` will happily build a parentless dialog, and on Linux that is indistinguishable
//! from a correct one — the XDG portal parents OS-side regardless — where on Windows and
//! macOS it opens *behind* Melodia. The parenting is invisible on the platform every one
//! of these is written and reviewed on, hence one helper rather than five call sites. Its
//! test walks the tree instead of naming them, and pins this file to still calling
//! `set_parent` — deleting it here unparents all five and reads as a simplification.

use slint::ComponentHandle;

use crate::AppWindow;

/// A native dialog parented to the main window, titled `title`. Chain the rest at the call
/// site.
///
/// **Call from the UI thread**, where every caller already sits: `Weak::upgrade` is
/// UI-thread-only, and on Linux so is the GTK/portal-backed dialog. A dropped window is
/// not an error — the dialog is built unparented, which is what happens off Windows and
/// macOS anyway. `set_parent` only stashes the raw window / display handles (rfd 0.17.2),
/// so the strong handle taken here may drop as soon as this returns.
pub fn parented(weak: &slint::Weak<AppWindow>, title: &str) -> rfd::AsyncFileDialog {
    let dialog = rfd::AsyncFileDialog::new().set_title(title);
    match weak.upgrade() {
        Some(ui) => dialog.set_parent(&ui.window().window_handle()),
        None => dialog,
    }
}
