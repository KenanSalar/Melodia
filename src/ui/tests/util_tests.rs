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

/// How blurred a backdrop *looks* is the sigma relative to the buffer it is applied to, so
/// retuning one of these without the other changes every backdrop in the app — invisibly in
/// review, and on both tiers at once.
#[test]
fn the_wash_stays_proportional_to_the_buffer_it_is_applied_to() {
    /// Sigma as a fraction of the buffer's side. Its value is taste; that the two constants
    /// agree on *a* value is the invariant.
    const WASH_FRACTION: f64 = 0.125;

    let ratio = f64::from(BLUR_SIGMA) / f64::from(BLUR_TARGET);
    assert!((ratio - WASH_FRACTION).abs() < 1e-6, "wash ratio drifted to {ratio}");
}

/// Two claims `detail_artwork::BLUR`'s own doc makes and nothing else checks. The band is a
/// landscape strip, and it runs lighter than Now Playing because a gradient floor and a solved
/// scrim sit on top of it — swap either way round and both still build, paint, and look wrong.
#[test]
fn the_hero_band_stays_landscape_and_lighter_than_now_playing() {
    let band = crate::ui::detail_artwork::BLUR;
    assert!(
        band.height < BLUR_TARGET,
        "band {} is not landscape at {BLUR_TARGET} wide",
        band.height
    );
    assert!(band.sigma < BLUR_SIGMA, "band wash {} is not lighter than {BLUR_SIGMA}", band.sigma);
}

#[test]
fn buffer_from_rgb_round_trips_dimensions_and_bytes() {
    use image::{ImageBuffer, Rgb};
    let mut img = ImageBuffer::from_pixel(3, 2, Rgb([1u8, 2, 3]));
    img.put_pixel(2, 1, Rgb([9, 8, 7]));
    let buf = buffer_from_rgb(&img);
    assert_eq!(buf.width(), 3);
    assert_eq!(buf.height(), 2);
    assert_eq!(buf.as_bytes(), img.as_raw().as_slice());
}
