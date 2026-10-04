//! The one Rust caller of the clipboard.
//!
//! The clipboard itself is `AppWindow.copy-to-clipboard`, and its doc says why it is a hidden field
//! rather than a crate. Funnelled through here so the confirmation is stated once and so the rule
//! below has one place to hold: **the text is never logged**. A stream URL can carry a session
//! token, and the rolling log ships inside bug reports.

use melodia_core::utils::toast::{self, ToastKind};
use melodia_ui::AppWindow;

/// Puts `text` on the clipboard and says so, or says there was nothing to copy when it is empty.
pub fn write(ui: &AppWindow, text: &str) {
    if text.is_empty() {
        toast::notify(ToastKind::NothingToCopy, "");
        return;
    }
    ui.invoke_copy_to_clipboard(text.into());
    toast::notify(ToastKind::Copied, "");
}
