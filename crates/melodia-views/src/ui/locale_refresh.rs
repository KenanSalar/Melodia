//! Re-render what a language switch cannot reach: a live `@tr` binding re-resolves on the next
//! paint, but a string Rust rendered through a `pure callback` and stored in a model is read by
//! nothing again.
//!
//! Almost everything so stored sits behind a section whose leave hands it back. **The
//! notification stack is the exception this seam exists for**: on screen during the switch, its
//! rows outliving every navigation. [`crate::ui::settings::locale`] can't reach it inline,
//! `install_locale` running ahead of the `Rc<NotificationsUi>` and `Settings.language-changed`
//! having a single handler slot.

use slint::Weak;

use crate::ui::signal::on_signal;
use melodia_app::state::AppState;
use melodia_core::error::AppResult;
use melodia_ui::AppWindow;

/// Run `refresh` on the UI thread after every language switch.
pub fn on_locale_changed<F>(state: &AppState, weak: Weak<AppWindow>, refresh: F) -> AppResult<()>
where
    F: Fn(&AppWindow) + 'static,
{
    on_signal(&state.locale_changed, weak, "locale-changed", refresh)
}
