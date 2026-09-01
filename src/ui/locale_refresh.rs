//! Re-render what a language switch cannot reach.
//!
//! `slint::select_bundled_translation` dirties one property that every live `@tr` binding
//! reads, so the markup re-resolves itself on the next paint. What it cannot touch is a
//! string Rust rendered through one of the `pure callback` trampolines and stored in a model
//! or property: nothing re-reads those, so they keep the language they were built in until
//! some unrelated fetch happens to replace them.
//!
//! Two shapes fix that. A surface reachable from the window alone re-renders inline in
//! `ui::settings::locale` — the hero band, which keeps its own facts. Everything else needs
//! its view handle, and `install_locale` runs *ahead* of `install_views` so the first frame
//! resolves in the persisted language; those subscribe here instead, from inside their own
//! slice's `install` where the handle is in hand.

use slint::Weak;

use crate::AppWindow;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Run `republish` on the UI thread after every language switch.
///
/// The caller owes its own section gate: rebuilding a hidden view's rows would write into
/// models its section leave cleared, so the shape to copy is the library-changed refresher's
/// — re-publish when mounted, mark dirty when not and let the re-enter's fetch render in the
/// new language.
pub fn on_locale_changed<F>(state: &AppState, weak: Weak<AppWindow>, republish: F) -> AppResult<()>
where
    F: Fn(&AppWindow) + 'static,
{
    let mut rx = state.locale_changed_tx.subscribe();
    slint::spawn_local(async_compat::Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let _ = rx.borrow_and_update();
            let Some(ui) = weak.upgrade() else { break };
            republish(&ui);
        }
    }))
    .map(|_| ())
    .map_err(|e| AppError::Window(format!("locale-changed subscriber: {e}")))
}
