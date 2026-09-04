//! The locale codes the app ships, as plain data.
//!
//! Here rather than beside the settings field that persists one, because three tiers read the
//! list and none of them owns it: `melodia-app` validates a persisted code against it,
//! `melodia-views` indexes its native-name labels by it, and the catalogue pin asks the
//! `translations/` tree for a `.po` per entry.

/// Locale codes the bundled `.po` files cover, in the Language dropdown's display order.
///
/// Index 0 is the default and ships no catalogue — English is the msgid baseline, living in the
/// `.slint` sources directly. A new locale is an entry here, a native name beside
/// `ui::settings::locale`'s 1:1 list, and a `.po` beside its siblings.
pub const SUPPORTED_LOCALES: &[&str] = &["en", "de", "fr", "es", "tr", "el", "it"];
