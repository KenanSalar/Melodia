use super::theme_for;

#[test]
fn an_app_mode_of_zero_is_dark() {
    assert_eq!(theme_for(Some(0)), "dark");
}

#[test]
fn an_app_mode_of_one_is_light() {
    assert_eq!(theme_for(Some(1)), "light");
}

/// The module fails light: a value that won't read is the mode Windows draws apps in by default.
#[test]
fn a_missing_app_mode_is_light() {
    assert_eq!(theme_for(None), "light");
}

/// Only an outright zero is dark, so no other value can turn a System variant dark.
#[test]
fn an_app_mode_other_than_zero_or_one_is_light() {
    assert_eq!(theme_for(Some(2)), "light");
}
