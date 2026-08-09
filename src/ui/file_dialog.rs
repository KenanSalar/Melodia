//! Opening a native file dialog against the main window.
//!
//! `rfd` will happily build a parentless dialog, and on Linux that is
//! indistinguishable from a correct one — the XDG portal parents OS-side
//! regardless of what we hand it. On Windows and macOS the same dialog opens
//! *behind* Melodia. So the parenting is invisible on the platform every one of
//! these is written and reviewed on, which is why it lives here rather than at
//! five call sites; [`tests`] pins that it stays that way.

use slint::ComponentHandle;

use crate::AppWindow;

/// A native dialog parented to the main window, titled `title`.
///
/// Chain the rest at the call site — `add_filter`, `set_file_name`, and the
/// `pick_*` / `save_file` terminator all take and return the builder.
///
/// **Call from the UI thread**, i.e. inside a `slint::spawn_local`, which is
/// where every caller already sits: `Weak::upgrade` is UI-thread-only, and on
/// Linux that is also the only thread the GTK/portal-backed dialog may be
/// invoked from.
///
/// A dropped window is not an error — the dialog is simply built unparented,
/// which is the same thing that happens off Windows and macOS anyway.
/// `set_parent` only stashes the raw window / display handles (rfd 0.17.2,
/// `file_dialog.rs::set_parent`), so the strong handle taken here may drop as
/// soon as this returns; the Slint window itself stays alive through the
/// `AppWindow` `main.rs` owns.
pub fn parented(weak: &slint::Weak<AppWindow>, title: &str) -> rfd::AsyncFileDialog {
    let dialog = rfd::AsyncFileDialog::new().set_title(title);
    match weak.upgrade() {
        Some(ui) => dialog.set_parent(&ui.window().window_handle()),
        None => dialog,
    }
}

#[cfg(test)]
#[path = "tests/file_dialog_tests.rs"]
mod tests;
