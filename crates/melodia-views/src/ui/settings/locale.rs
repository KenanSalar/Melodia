//! The language picker. [`slint::select_bundled_translation`] is the source of truth for the
//! active locale, one switch re-rendering every `@tr` on the next frame, so `Settings` carries
//! only a `language-idx`. Boot makes the same call before `app.show()`, so the first frame paints
//! in the persisted language. Each name is in its own script and never translated.

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::locale::{DEFAULT_LOCALE, SUPPORTED_LOCALES};
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Settings};

/// Native-name labels for [`SUPPORTED_LOCALES`], index for index, in alphabetical order so a
/// reader finds their own language by its name. A new locale goes in at that place in both.
const LOCALE_NATIVE_NAMES: &[&str] = &[
    "Bahasa Indonesia",
    "Deutsch",
    "English",
    "Español",
    "Français",
    "Italiano",
    "Magyar",
    "Nederlands",
    "Polski",
    "Português (Brasil)",
    "Português (Portugal)",
    "Tiếng Việt",
    "Türkçe",
    "Ελληνικά",
    "Русский",
    "Українська",
];

// A short list hides the last locale from the picker, and nothing at runtime says so.
const _: () = assert!(
    LOCALE_NATIVE_NAMES.len() == SUPPORTED_LOCALES.len(),
    "LOCALE_NATIVE_NAMES must stay 1:1 with SUPPORTED_LOCALES"
);

/// The synchronous shadow of `settings.locale` the root `CLAUDE.md`'s sibling-callback rule asks
/// for, [`crate::ui::appearance::PersistedAccent`]'s shape. No sibling reads it yet; it stops the
/// next one racing a read against an in-flight write.
type PersistedLocale = Arc<parking_lot::Mutex<String>>;

/// Hydrate the language lists, seed `language-idx` from the persisted locale, and wire
/// the change callback. **After** `AppWindow::new()`, so the global is mounted, and
/// **before** `app.show()`.
pub fn install_locale(ui: &AppWindow, state: &AppState) {
    let persisted = library::settings::get_settings(&state.paths).map_or_else(
        |e| {
            log::warn!("locale: read settings failed: {}", describe(&e));
            DEFAULT_LOCALE.to_owned()
        },
        |s| s.locale,
    );

    let names: Vec<SharedString> =
        LOCALE_NATIVE_NAMES.iter().map(|n| SharedString::from(*n)).collect();
    let codes: Vec<SharedString> =
        SUPPORTED_LOCALES.iter().map(|c| SharedString::from(*c)).collect();

    // A code this build doesn't ship (hand-edited, or left by a newer build) renders in the
    // default language, so the picker says that.
    let index_of = |code: &str| SUPPORTED_LOCALES.iter().position(|c| *c == code);
    let idx = index_of(&persisted).or_else(|| index_of(DEFAULT_LOCALE)).unwrap_or(0);

    {
        let g = ui.global::<Settings>();
        g.set_language_names(ModelRc::from(Rc::new(VecModel::from(names))));
        g.set_language_codes(ModelRc::from(Rc::new(VecModel::from(codes))));
        g.set_language_search_terms(LOCALE_NATIVE_NAMES.join(" ").into());
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

        let Some(&code) = usize::try_from(idx_i32).ok().and_then(|idx| SUPPORTED_LOCALES.get(idx))
        else {
            log::warn!("language-changed: out-of-range idx {idx_i32}");
            return;
        };

        // Idempotent and cheap, so an unchanged pick isn't short-circuited.
        if let Err(e) = slint::select_bundled_translation(code) {
            log::warn!("select_bundled_translation({code}): {}", describe(&e));
        }

        // The switch reaches every live `@tr` binding and nothing Rust rendered and stored. The
        // band's chips can outlive their page, `clear_if_stale` keeping them under Now Playing
        // and Settings opened from there leaving no gate edge, so they go; every enter
        // republishes.
        crate::ui::hero_chips::clear(&ui);
        s.locale_changed.bump();

        // The picker's binding writes it too; this covers a call that didn't come from it.
        ui.global::<Settings>().set_language_idx(idx_i32);

        code.clone_into(&mut *shadow.lock());

        let code_owned = code.to_owned();
        s.persist_blocking("persist locale", move |paths| {
            library::settings::set_locale(paths, code_owned)
        });
    });
}

#[cfg(test)]
#[path = "tests/locale_tests.rs"]
mod tests;
