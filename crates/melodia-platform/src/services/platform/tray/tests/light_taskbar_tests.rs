//! The expected colours are mixes of `BRAND_MARK_ON_LIGHT`'s pair, Latte blue `#1e66f5` and teal
//! `#179299`, at the fraction of the diagonal each pixel centre sits on.

use super::{mix, paint};

/// White at four different alphas, one per pixel, so a repaint that touched alpha shows.
fn white_two_by_two() -> Vec<u8> {
    vec![255, 255, 255, 10, 255, 255, 255, 20, 255, 255, 255, 30, 255, 255, 255, 40]
}

#[test]
fn the_gradient_runs_from_the_blue_top_left_to_the_teal_bottom_right() {
    let mut rgba = white_two_by_two();

    paint(&mut rgba, 2, 2);

    assert_eq!(
        rgba,
        [28, 113, 222, 10, 26, 124, 199, 20, 26, 124, 199, 30, 24, 135, 176, 40],
        "a quarter, a half, a half and three quarters along, alphas untouched"
    );
}

/// The alpha is the icon's shape. Painting it over would turn the mark into a filled square.
#[test]
fn a_transparent_pixel_stays_transparent() {
    let mut rgba = vec![0, 0, 0, 0];

    paint(&mut rgba, 1, 1);

    assert_eq!(rgba, [26, 124, 199, 0]);
}

#[test]
fn the_mix_lands_on_each_stop_at_the_ends_of_its_span() {
    assert_eq!(mix(0x001e_66f5, 0x0017_9299, 16, 0, 16), 0x1e);
    assert_eq!(mix(0x001e_66f5, 0x0017_9299, 16, 16, 16), 0x17);
}

/// A zero width would make the row chunks zero long, which `chunks_exact_mut` panics on.
#[test]
fn an_icon_with_no_pixels_paints_nothing() {
    let mut rgba: Vec<u8> = Vec::new();

    paint(&mut rgba, 0, 0);

    assert!(rgba.is_empty());
}
