use super::theme_for;

#[test]
fn a_mode_value_of_zero_is_dark() {
    assert_eq!(theme_for(Some(0)), "dark");
}

#[test]
fn a_mode_value_of_one_is_light() {
    assert_eq!(theme_for(Some(1)), "light");
}

/// The module fails light: a value that won't read is the mode Windows draws in by default.
#[test]
fn a_missing_mode_value_is_light() {
    assert_eq!(theme_for(None), "light");
}

/// Only an outright zero is dark, so a value Windows never writes can't turn anything dark.
#[test]
fn a_mode_value_other_than_zero_or_one_is_light() {
    assert_eq!(theme_for(Some(2)), "light");
}
