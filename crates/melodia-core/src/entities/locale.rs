//! The locale codes the app ships, as plain data.
//!
//! Here rather than beside the settings field that persists one, because three tiers read the
//! list and none of them owns it: `melodia-app` validates a persisted code against it,
//! `melodia-views` indexes its native-name labels by it, and the catalogue pin asks the
//! `translations/` tree for a `.po` per entry.

/// The locale the msgids are written in, so it ships no catalogue, and the one a missing or
/// unknown code falls back to.
pub const DEFAULT_LOCALE: &str = "en";

/// Locale codes the bundled `.po` files cover, in the Language dropdown's display order: by each
/// language's own name, which `ui::settings::locale` holds 1:1 beside this.
///
/// A new locale goes in at its name's place, here and in that list, plus a `.po` beside its
/// siblings.
pub const SUPPORTED_LOCALES: &[&str] = &[
    "id", "de", "en", "es", "fr", "it", "hu", "nl", "pl", "pt_BR", "pt", "vi", "tr", "el", "ru",
    "uk",
];

#[cfg(test)]
#[path = "tests/locale_tests.rs"]
mod tests;
