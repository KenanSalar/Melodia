//! Language picker wiring for the Settings page.
//!
//! Slint's bundled-translation runtime ([`slint::select_bundled_translation`])
//! is the source of truth for the active locale: a single thread-local switch
//! that flips every `@tr(...)` expression in the compiled UI tree on the next
//! frame. We never expose the active locale through the `Settings` global
//! beyond a `language-idx` index — the runtime owns it.
//!
//! Flow:
//! 1. `install_locale` reads the persisted `locale` from `settings.json`,
//!    populates `Settings.language-{names,codes}` with the canonical lists
//!    from [`services::settings::SUPPORTED_LOCALES`], and sets
//!    `language-idx` to match the persisted code (falling back to en).
//! 2. `wire_language_changed` listens on `Settings.language-changed(int)`.
//!    On click the callback resolves idx → code, calls
//!    `select_bundled_translation` synchronously on the UI thread (the
//!    next paint re-renders every `@tr`), updates the in-memory
//!    [`PersistedLocale`] shadow, and spawns the disk write through
//!    [`library::settings::set_locale`].
//!
//! `main.rs` separately calls `select_bundled_translation(&locale)` once
//! before `app.run()` so the very first frame paints in the persisted
//! language.
//!
//! Native-name labels (`English`, `Deutsch`, …) are always rendered in their
//! own script, never translated — that's the universal convention for
//! language pickers.

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::library;
use crate::services;
use crate::state::AppState;
use crate::{AppWindow, Settings};

/// Native-name labels for [`services::settings::SUPPORTED_LOCALES`]. Indices
/// match 1:1 — adding a locale means appending to both arrays.
const LOCALE_NATIVE_NAMES: &[&str] =
    &["English", "Deutsch", "Français", "Español", "Türkçe", "Ελληνικά", "Italiano"];

/// Synchronous in-memory shadow of `settings.locale`. Updated by the
/// language-changed callback before it spawns the async disk write.
/// Mirrors [`crate::ui::appearance::PersistedAccent`] (see CLAUDE.md
/// "Sibling-callback writes need a synchronous shadow"). Not consumed by
/// any current sibling, but cheap insurance against future code adding a
/// reader that would race `read_settings` against the in-flight
/// `mutate_settings`.
type PersistedLocale = Arc<parking_lot::Mutex<String>>;

/// Hydrate the `Settings` global's language lists from
/// [`services::settings::SUPPORTED_LOCALES`], seed `language-idx` from the
/// persisted locale, and wire the change callback.
///
/// Call **after** `AppWindow::new()` (so the global is mounted) and **before**
/// `app.run()` so the first frame's `@tr(...)` resolutions land in the
/// persisted language. The companion `slint::select_bundled_translation`
/// call lives in `main.rs` so the very first frame paints correctly even
/// before any callback fires.
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

    let names: Vec<SharedString> = LOCALE_NATIVE_NAMES
        .iter()
        .map(|n| SharedString::from(*n))
        .collect();
    let codes: Vec<SharedString> = services::settings::SUPPORTED_LOCALES
        .iter()
        .map(|c| SharedString::from(*c))
        .collect();

    let idx = services::settings::SUPPORTED_LOCALES
        .iter()
        .position(|c| *c == persisted)
        .unwrap_or(0);

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
        ui.global::<Settings>()
            .set_language_idx(i32::try_from(idx).unwrap_or(0));

        // Synchronous shadow update before the async write so any sibling
        // reader sees the new code before the disk catches up.
        code.clone_into(&mut *shadow.lock());

        let code_owned = code.to_owned();
        s.persist_blocking("persist locale", move |s| {
            library::settings::set_locale(s, code_owned)
        });
    });
}

#[cfg(test)]
#[path = "../tests/locale_tests.rs"]
mod tests;
