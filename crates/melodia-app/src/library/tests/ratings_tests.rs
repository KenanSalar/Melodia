use super::clamp_rating;

#[test]
fn clamp_rating_bounds_to_zero_through_five() {
    assert_eq!(clamp_rating(0), 0);
    assert_eq!(clamp_rating(3), 3);
    assert_eq!(clamp_rating(5), 5);
    // Out-of-range values saturate to the nearest bound.
    assert_eq!(clamp_rating(7), 5);
    assert_eq!(clamp_rating(-2), 0);
    assert_eq!(clamp_rating(i32::MAX), 5);
    assert_eq!(clamp_rating(i32::MIN), 0);
}
