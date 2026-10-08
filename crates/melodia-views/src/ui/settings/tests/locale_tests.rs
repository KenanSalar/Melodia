use melodia_core::utils::fold::fold;

use super::LOCALE_NATIVE_NAMES;

/// A locale appended at the end is the one nobody would look for there. Compared folded rather
/// than raw, so a name opening on an accented letter sorts with its base letter instead of after Z.
#[test]
fn the_picker_lists_each_language_alphabetically_by_its_own_name() {
    let mut sorted = LOCALE_NATIVE_NAMES.to_vec();
    sorted.sort_by_cached_key(|name| fold(name));

    assert_eq!(
        LOCALE_NATIVE_NAMES,
        sorted.as_slice(),
        "insert a new locale at its name's place, in this list and in SUPPORTED_LOCALES alike"
    );
}
