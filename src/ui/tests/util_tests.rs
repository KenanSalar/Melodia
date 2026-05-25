use super::*;

#[test]
fn clamp_i64_to_i32_passes_through_in_range_and_saturates_outside() {
    assert_eq!(clamp_i64_to_i32(0), 0);
    assert_eq!(clamp_i64_to_i32(42), 42);
    assert_eq!(clamp_i64_to_i32(-7), -7);
    assert_eq!(clamp_i64_to_i32(i64::from(i32::MAX)), i32::MAX);
    assert_eq!(clamp_i64_to_i32(i64::from(i32::MIN)), i32::MIN);
    assert_eq!(clamp_i64_to_i32(i64::MAX), i32::MAX);
    assert_eq!(clamp_i64_to_i32(i64::MIN), i32::MIN);
}

#[test]
fn opt_lc_lowercases_and_treats_none_as_empty() {
    assert_eq!(opt_lc(Some("AbC")), "abc");
    assert_eq!(opt_lc(Some("")), "");
    assert_eq!(opt_lc(None), "");
}
