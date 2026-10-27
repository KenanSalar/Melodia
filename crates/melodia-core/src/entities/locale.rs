//! The locale codes the app ships, here because three tiers read the list and none owns it.

/// The locale the msgids are written in, so it ships no catalogue, and the one a missing or
/// unknown code falls back to.
pub const DEFAULT_LOCALE: &str = "en";

/// In the Language list's order, by each language's own name, 1:1 with `ui::settings::locale`'s
/// labels. The root `CLAUDE.md` has what a new locale owes.
pub const SUPPORTED_LOCALES: &[&str] = &[
    "id", "de", "en", "es", "fr", "it", "hu", "nl", "pl", "pt_BR", "pt", "vi", "tr", "el", "ru",
    "uk",
];

#[cfg(test)]
#[path = "tests/locale_tests.rs"]
mod tests;
