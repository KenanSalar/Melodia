//! Re-render what a language switch cannot reach.
//!
//! `slint::select_bundled_translation` dirties one property that every live `@tr` binding
//! reads, so the markup re-resolves itself on the next paint. What it cannot touch is a
//! string Rust rendered through one of the `pure callback` trampolines and stored in a model
//! or property: nothing re-reads those.
//!
//! Almost everything so stored is behind a section that hands its models back on the way to
//! the Settings page, and comes back through the enter's own fetch. **The notification stack
//! is the exception, and the reason this seam exists**: it is on screen during the switch,
//! its rows outlive every navigation, and one of them is raised by the Settings page itself.
//! It cannot be reached inline from [`crate::ui::settings::locale`] either — `install_locale`
//! runs well ahead of the `Rc<NotificationsUi>`, and `Settings.language-changed` has a single
//! handler slot a second registration would clobber.

use slint::Weak;

use crate::AppWindow;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Run `refresh` on the UI thread after every language switch.
///
/// The closure takes no `Send` bound on purpose: `spawn_local` runs it on the UI thread, so a
/// subscriber may capture the `Rc` handles the shell owns.
pub fn on_locale_changed<F>(state: &AppState, weak: Weak<AppWindow>, refresh: F) -> AppResult<()>
where
    F: Fn(&AppWindow) + 'static,
{
    let mut rx = state.locale_changed_tx.subscribe();
    slint::spawn_local(async_compat::Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let Some(ui) = weak.upgrade() else { break };
            refresh(&ui);
        }
    }))
    .map(|_| ())
    .map_err(|e| AppError::Window(format!("locale-changed subscriber: {e}")))
}
