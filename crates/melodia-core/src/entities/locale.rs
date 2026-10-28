//! The locale codes the app ships, here because three tiers read the list and none owns it.

/// The locale the msgids are written in, so it ships no catalogue, and the one a missing or
/// unknown code falls back to.
pub const DEFAULT_LOCALE: &str = "en";

/// In the Language list's order, by each language's own name, 1:1 with `ui::settings::locale`'s
/// labels. A new locale owes a row here and in `LOCALE_NATIVE_NAMES`, a catalog started from the
/// template with `msginit`, and a `LANGUAGE_LETTERS` row in `scripts/patch_vazirmatn.py` wherever
/// the bundled faces lack its alphabet.
pub const SUPPORTED_LOCALES: &[&str] = &[
    "id", "de", "en", "es", "fr", "it", "hu", "nl", "pl", "pt_BR", "pt", "vi", "tr", "el", "ru",
    "uk",
];

#[cfg(test)]
#[path = "tests/locale_tests.rs"]
mod tests;
