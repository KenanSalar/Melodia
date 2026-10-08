use super::{DEFAULT_LOCALE, SUPPORTED_LOCALES};

/// The picker falls back to the default's row for a code it doesn't know, and with the list sorted
/// by name nothing places that row anywhere in particular.
#[test]
fn the_default_locale_is_one_the_picker_offers() {
    assert!(
        SUPPORTED_LOCALES.contains(&DEFAULT_LOCALE),
        "{DEFAULT_LOCALE} is missing from {SUPPORTED_LOCALES:?}"
    );
}
