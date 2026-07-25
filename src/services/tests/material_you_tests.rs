use image::{ImageBuffer, Rgb};
use slint::{Rgb8Pixel, SharedPixelBuffer};
use tempfile::NamedTempFile;

use crate::services::material_you::{
    SchemeStyle, extract_source_argb, extract_source_argb_from_rgb8, generate_palette,
    lift_to_min_tone,
};

/// sRGB relative luminance of a `0x00RRGGBB` value, 0..1. Independent of the
/// HCT machinery under test, so the assertions below can't pass by agreeing
/// with a bug in it.
fn relative_luminance(rgb: u32) -> f64 {
    let r = f64::from((rgb >> 16) & 0xFF) / 255.0;
    let g = f64::from((rgb >> 8) & 0xFF) / 255.0;
    let b = f64::from(rgb & 0xFF) / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[test]
fn scheme_style_id_round_trips() {
    for &s in SchemeStyle::all() {
        assert_eq!(SchemeStyle::from_id(s.as_id()), s);
    }
}

#[test]
fn scheme_style_unknown_id_falls_back_to_none() {
    assert_eq!(SchemeStyle::from_id(""), SchemeStyle::None);
    assert_eq!(SchemeStyle::from_id("not_a_scheme"), SchemeStyle::None);
}

#[test]
fn scheme_style_all_starts_with_none() {
    // Settings UI relies on index 0 == "None" for the
    // "is dynamic colour active?" check (`color-style-idx != 0`).
    assert_eq!(SchemeStyle::all().first().copied(), Some(SchemeStyle::None));
}

#[test]
fn generate_palette_dark_produces_distinct_text_and_base() {
    // Google Blue seed (matches Tauri's fallback) — every variant should
    // produce a usable dark palette where text is meaningfully separable
    // from the base surface, regardless of which dynamic style is picked.
    let seed = 0x0042_85F4_u32;
    for &style in &[
        SchemeStyle::TonalSpot,
        SchemeStyle::Content,
        SchemeStyle::Vibrant,
        SchemeStyle::Expressive,
        SchemeStyle::Fidelity,
        SchemeStyle::Neutral,
        SchemeStyle::Monochrome,
    ] {
        let (palette, accent) = generate_palette(seed, true, style);
        assert_ne!(palette.text, palette.base, "style={style:?}");
        assert!(accent <= 0x00FF_FFFF, "style={style:?}");
    }
}

#[test]
fn generate_palette_light_and_dark_differ() {
    let (dark, _) = generate_palette(0x0042_85F4, true, SchemeStyle::TonalSpot);
    let (light, _) = generate_palette(0x0042_85F4, false, SchemeStyle::TonalSpot);
    assert_ne!(dark.base, light.base);
    assert_ne!(dark.text, light.text);
}

#[allow(
    clippy::expect_used,
    reason = "test setup failures should abort the test; not production code"
)]
#[test]
fn extract_source_argb_rejects_oversized_source() {
    // `extract_source_argb`'s defensive `image::Limits` caps source
    // dimensions at `MATERIAL_YOU_MAX_SOURCE_DIM` (2048) — emit a 2200×2200
    // uniform-colour PNG to a temp file and assert the decoder bails before
    // the multi-MB pixel buffer is allocated. Uniform colour keeps the PNG
    // small on disk; the test verifies the limit fires, not encode speed.
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(2200, 2200, Rgb([230, 120, 30]));
    let tmp = NamedTempFile::with_suffix(".png").expect("tempfile");
    img.save(tmp.path()).expect("encode oversized PNG");

    let seed = extract_source_argb(tmp.path());
    assert!(
        seed.is_none(),
        "expected Limits to reject 2200x2200, got seed 0x{:08X}",
        seed.unwrap_or(0)
    );
}

#[allow(
    clippy::expect_used,
    reason = "test setup failures should abort the test; not production code"
)]
#[test]
fn extract_source_argb_decodes_in_bounds_source() {
    // Sanity check the path-based fallback still works for in-bounds
    // dimensions — a 256×256 vibrant uniform colour passes Limits and
    // produces a usable seed that the palette generator can consume.
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(256, 256, Rgb([230, 120, 30]));
    let tmp = NamedTempFile::with_suffix(".png").expect("tempfile");
    img.save(tmp.path()).expect("encode in-bounds PNG");

    let seed = extract_source_argb(tmp.path()).expect("in-bounds source should produce a seed");
    let r = (seed >> 16) & 0xFF;
    let b = seed & 0xFF;
    assert!(
        r > b,
        "expected red-dominant seed for orange input, got 0x{seed:06X}"
    );
}

#[test]
fn extract_source_argb_from_rgb8_rejects_empty_buffer() {
    let buf = SharedPixelBuffer::<Rgb8Pixel>::new(0, 0);
    assert_eq!(extract_source_argb_from_rgb8(&buf), None);
}

#[allow(
    clippy::expect_used,
    reason = "test setup failures should abort the test; not production code"
)]
#[test]
fn extract_source_argb_from_rgb8_produces_seed_for_uniform_colour() {
    // 72×72 mirrors `cover_thumbs::THUMB_SIZE` — the actual buffer the
    // production path consumes. Filling with a vibrant orange exercises
    // the full quantize+score pipeline and confirms the seed is in the
    // expected hue family (R-dominant for orange).
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(72, 72);
    let bytes = buf.make_mut_bytes();
    for px in bytes.chunks_exact_mut(3) {
        px[0] = 230;
        px[1] = 120;
        px[2] = 30;
    }
    let seed = extract_source_argb_from_rgb8(&buf)
        .expect("uniform vibrant buffer should produce a seed");
    let r = (seed >> 16) & 0xFF;
    let b = seed & 0xFF;
    assert!(
        r > b,
        "expected red-dominant seed for orange input, got 0x{seed:06X}"
    );
}

// --- lift_to_min_tone --------------------------------------------------------
//
// The visualizer bars paint an artwork accent opaquely over the Now-Playing
// backdrop, so a dark album's near-black accent has to be lifted or it's
// invisible. These pin the guarantee that lift provides.

#[test]
fn lift_to_min_tone_leaves_an_already_light_colour_alone() {
    // Near-white sits far above any floor we'd ask for, so it must be returned
    // byte-identical — no gratuitous gamut round-trip.
    let light = 0x00F0_F0F0;
    assert_eq!(lift_to_min_tone(light, 70.0), light);
}

#[test]
fn lift_to_min_tone_brightens_pure_black() {
    // The case a multiplicative brighten cannot fix: scaling HSV value leaves
    // black black forever, which is exactly how a dark cover used to sink the
    // bars into the backdrop.
    let lifted = lift_to_min_tone(0x0000_0000, 70.0);
    assert!(
        relative_luminance(lifted) > 0.3,
        "black should lift well clear of the backdrop, got 0x{lifted:06X}"
    );
}

#[test]
fn lift_to_min_tone_brightens_a_dark_chromatic_accent() {
    // A deep navy — the realistic dark-album case, not the degenerate one.
    let lifted = lift_to_min_tone(0x0010_1A3A, 70.0);
    assert!(
        relative_luminance(lifted) > relative_luminance(0x0010_1A3A),
        "expected a lift, got 0x{lifted:06X}"
    );
    assert!(
        relative_luminance(lifted) > 0.3,
        "lifted navy should clear the backdrop, got 0x{lifted:06X}"
    );
}

#[test]
fn lift_to_min_tone_keeps_the_dominant_hue() {
    // Tone is the only axis we move: a dark red must come back a light red,
    // not a neutral. That's what keeps the bars recognisably the album's colour.
    let lifted = lift_to_min_tone(0x0033_0000, 70.0);
    let r = (lifted >> 16) & 0xFF;
    let g = (lifted >> 8) & 0xFF;
    let b = lifted & 0xFF;
    assert!(
        r > g && r > b,
        "expected a red-dominant lift, got 0x{lifted:06X}"
    );
}

#[test]
fn lift_to_min_tone_is_idempotent() {
    // A second pass must be a no-op, or repeated track changes would creep the
    // colour lighter each time.
    let once = lift_to_min_tone(0x0010_1A3A, 70.0);
    assert_eq!(lift_to_min_tone(once, 70.0), once);
}
