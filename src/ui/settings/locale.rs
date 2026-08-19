//! Language picker wiring for the Settings page.
//!
//! [`slint::select_bundled_translation`] is the source of truth for the active locale —
//! one thread-local switch flipping every `@tr(...)` in the compiled tree on the next
//! frame — so the `Settings` global carries only a `language-idx`.
//!
//! `install_locale` populates the name and code lists from
//! [`services::settings::SUPPORTED_LOCALES`] and seeds the index from the persisted
//! code; `wire_language_changed` resolves a click back to a code, calls
//! `select_bundled_translation` synchronously, updates the [`PersistedLocale`] shadow
//! and spawns the disk write. `main.rs` makes the same call once before `app.run()`, so
//! the first frame already paints in the persisted language.
//!
//! Native-name labels are always rendered in their own script, never translated — the
//! universal convention for language pickers.

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::library;
use crate::services;
use crate::state::AppState;
use crate::{AppWindow, Settings};

/// Native-name labels for [`services::settings::SUPPORTED_LOCALES`]. Indices
/// match 1:1 — adding a locale means appending to both arrays.
const LOCALE_NATIVE_NAMES: &[&str] = &[
    "English",
    "Deutsch",
    "Français",
    "Español",
    "Türkçe",
    "Ελληνικά",
    "Italiano",
];

/// Synchronous in-memory shadow of `settings.locale`, updated by the language-changed
/// callback before it spawns the disk write —
/// [`crate::ui::appearance::PersistedAccent`]'s shape, and the root `CLAUDE.md`'s
/// sibling-callback rule. No sibling reads it yet; it is what stops the next one racing
/// `read_settings` against an in-flight `mutate_settings`.
type PersistedLocale = Arc<parking_lot::Mutex<String>>;

/// Hydrate the language lists, seed `language-idx` from the persisted locale, and wire
/// the change callback. **After** `AppWindow::new()`, so the global is mounted, and
/// **before** `app.run()`.
pub fn install_locale(ui: &AppWindow, state: &AppState) {
    debug_assert_eq!(
        services::settings::SUPPORTED_LOCALES.len(),
        LOCALE_NATIVE_NAMES.len(),
        "LOCALE_NATIVE_NAMES must stay 1:1 with SUPPORTED_LOCALES"
    );

    let persisted = library::settings::get_settings(state).map_or_else(
        |e| {
            log::warn!("locale: read settings failed: {e}");
            "en".to_owned()
        },
        |s| s.locale,
    );

    let names: Vec<SharedString> =
        LOCALE_NATIVE_NAMES.iter().map(|n| SharedString::from(*n)).collect();
    let codes: Vec<SharedString> =
        services::settings::SUPPORTED_LOCALES.iter().map(|c| SharedString::from(*c)).collect();

    let idx =
        services::settings::SUPPORTED_LOCALES.iter().position(|c| *c == persisted).unwrap_or(0);

    {
        let g = ui.global::<Settings>();
        g.set_language_names(ModelRc::from(Rc::new(VecModel::from(names))));
        g.set_language_codes(ModelRc::from(Rc::new(VecModel::from(codes))));
        g.set_language_idx(i32::try_from(idx).unwrap_or(0));
    }

    let shadow: PersistedLocale = Arc::new(parking_lot::Mutex::new(persisted));
    wire_language_changed(ui, state, shadow);
}

fn wire_language_changed(ui: &AppWindow, state: &AppState, shadow: PersistedLocale) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_language_changed(move |idx_i32| {
        let Some(ui) = weak.upgrade() else { return };

        let idx = usize::try_from(idx_i32).unwrap_or(0);
        let Some(&code) = services::settings::SUPPORTED_LOCALES.get(idx) else {
            log::warn!("language-changed: out-of-range idx {idx_i32}");
            return;
        };

        // Synchronous switch — re-renders every `@tr(...)` on the next
        // paint. Idempotent and cheap; no need to short-circuit on equal
        // current value.
        if let Err(e) = slint::select_bundled_translation(code) {
            log::warn!("select_bundled_translation({code}): {e:?}");
        }

        // Keep `language-idx` in sync defensively — the dropdown's two-way
        // bind already writes it, but a future code path could call the
        // callback programmatically without touching the dropdown.
        ui.global::<Settings>().set_language_idx(i32::try_from(idx).unwrap_or(0));

        // Synchronous shadow update before the async write so any sibling
        // reader sees the new code before the disk catches up.
        code.clone_into(&mut *shadow.lock());

        let code_owned = code.to_owned();
        s.persist_blocking("persist locale", move |s| library::settings::set_locale(s, code_owned));
    });
}

#[cfg(test)]
#[path = "tests/locale_tests.rs"]
mod tests;
