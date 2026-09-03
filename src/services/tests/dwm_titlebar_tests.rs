//! The third copy of the luminance split, checked against the one that owns it.
//!
//! `is_dark_from_rgb` restates `themes::on_accent_hex`'s weights and threshold rather than
//! calling it, because `apply` is what calls *into* this module. Nothing else links the two, and
//! `test-windows` is the only place this runs.

use super::is_dark_from_rgb;
use crate::themes::on_accent_hex;

/// Asks the twin in its own terms. `on_accent_hex` answers with an ink rather than a bool, so the
/// ink a known-dark surface gets is what "dark" means here — no fourth copy of the weights.
fn twin_reads_dark(rgb: u32) -> bool {
    on_accent_hex(rgb) == on_accent_hex(0x0000_0000)
}

/// Every colour whose luminance lands exactly on the threshold, from a walk of the full cube.
/// This is the only input where `<` and `<=` differ, so it is the whole of what regressed — and
/// these are reachable: a Material You mantle is derived from the playing track's album art.
const ON_THE_THRESHOLD: [u32; 9] = [
    0x002f_8fd3,
    0x003d_9c29,
    0x005f_888b,
    0x0070_7ebc,
    0x0081_74ed,
    0x00a0_7774,
    0x00b1_6da5,
    0x00c2_63d6,
    0x00f2_5c8e,
];

#[test]
fn a_colour_on_the_threshold_reads_dark_in_both_copies() {
    for rgb in ON_THE_THRESHOLD {
        assert!(
            twin_reads_dark(rgb),
            "test fixture is stale: #{rgb:06x} no longer sits on `on_accent_hex`'s dark side"
        );
        assert!(
            is_dark_from_rgb(rgb),
            "#{rgb:06x} paints a light caption under chrome the palette treats as dark"
        );
    }
}

#[test]
fn the_caption_flag_is_the_exact_complement_of_the_ink_pick() {
    // Strided rather than exhaustive: 2^24 colours would dominate the binary's runtime, and the
    // one arm a sweep can miss is pinned by name above.
    for r in (0u32..=255).step_by(5) {
        for g in (0u32..=255).step_by(5) {
            for b in (0u32..=255).step_by(5) {
                let rgb = (r << 16) | (g << 8) | b;
                assert_eq!(
                    is_dark_from_rgb(rgb),
                    twin_reads_dark(rgb),
                    "#{rgb:06x} splits light from dark differently than `on_accent_hex` does"
                );
            }
        }
    }
}

/// The sweep above only proves the two copies agree, which they would still do if both flipped.
/// This is the half that says which way round they point.
#[test]
fn a_dark_mantle_reads_dark_and_a_light_one_light() {
    assert!(is_dark_from_rgb(0x0000_0000), "black is a dark caption");
    assert!(!is_dark_from_rgb(0x00ff_ffff), "white is a light caption");
    assert!(is_dark_from_rgb(0x0018_1825), "Catppuccin Mocha's mantle is dark");
    assert!(!is_dark_from_rgb(0x00e6_e9ef), "Catppuccin Latte's mantle is light");
}
