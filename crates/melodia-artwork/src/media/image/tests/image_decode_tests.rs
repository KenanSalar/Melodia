//! The shared resize primitive. That it stays the *only* one is a question about every
//! crate, so `melodia-tidy` holds it.

use super::*;
use crate::test_support::{write_test_jpeg, write_test_jpeg_sized, write_test_png};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// ── decode_capped_to ──

/// The contract every tier leans on, and the one a scale factor could quietly break: whatever
/// the decoder picks, the result still covers the target on its long edge, so the resize behind
/// it is unchanged and still never upscales.
#[test]
fn a_scaled_decode_still_covers_its_target() -> TestResult {
    const SOURCE: u32 = 512;
    let (_tmp, path) = write_test_jpeg(SOURCE)?;

    // Every live tier size, the two row tiers through the grid and detail ones.
    for target in [48, 72, 180, 200, 256, 384, 448] {
        let decoded = decode_capped_to(&path, MAX_SOURCE_DIM, target)?;
        let long_edge = decoded.width().max(decoded.height());
        assert!(long_edge >= target, "target {target} came back at {long_edge}px");
        assert!(long_edge <= SOURCE, "a decode may never enlarge: {target} gave {long_edge}px");
    }
    Ok(())
}

/// The same contract on a source that isn't square, which is the half a square fixture cannot
/// fail. `jpeg-decoder` takes the first scale where *either* axis clears the request, always the
/// long one — so a request of `target` alone comes back under it on the short edge, and
/// [`crate::media::image::cover_thumbs`]'s square resize enlarges what it was handed.
#[test]
fn a_scaled_decode_covers_its_target_on_the_short_edge_too() -> TestResult {
    // 16:9 and a banner, either side of the ratio where a long-edge bound starts falling short.
    for (width, height) in [(1920, 1080), (2000, 400), (600, 1500)] {
        let (_tmp, path) = write_test_jpeg_sized(width, height)?;
        let source_short = width.min(height);

        for target in [48, 72, 180, 256, 384, 448] {
            let decoded = decode_capped_to(&path, MAX_SOURCE_DIM, target)?;
            let short = decoded.width().min(decoded.height());
            // A source already under the target has nothing to downscale, so it is its own floor.
            let owed = target.min(source_short);
            assert!(
                short >= owed,
                "{width}x{height} at target {target} came back {short}px on the short edge, \
                 under the {owed}px the tier resizes from"
            );
        }
    }

    // Meeting the short edge must not have cost the fast path: a row tile off a 16:9 source is
    // still a fraction of it, and the assertions above would pass a decode that gave up entirely.
    let (_tmp, path) = write_test_jpeg_sized(1920, 1080)?;
    let decoded = decode_capped_to(&path, MAX_SOURCE_DIM, 48)?;
    assert!(
        decoded.width() < 1920,
        "a 48px tile decoded a 1920x1080 source whole; scale-on-decode is not being reached"
    );
    Ok(())
}

/// The point of the call. A row tile asks for a fraction of the source, and getting the whole
/// thing back means the fast path silently stopped applying — which costs only time, so nothing
/// else in the tree would notice.
#[test]
fn a_small_target_decodes_below_the_source() -> TestResult {
    let (_tmp, path) = write_test_jpeg(512)?;
    let decoded = decode_capped_to(&path, MAX_SOURCE_DIM, 48)?;
    assert!(
        decoded.width() < 512,
        "a 48px tile decoded the full 512px source; scale-on-decode is not being reached"
    );
    Ok(())
}

/// Picking the arm off the name is what keeps a non-JPEG cover to one `open` instead of two, and
/// the cost of guessing from a name is that it can be wrong. It may only ever cost the fast path:
/// the fallback guesses the format from the header, so a JPEG under any other name still decodes,
/// at its own size.
#[test]
fn a_mislabelled_jpeg_still_decodes_through_the_fallback() -> TestResult {
    let (tmp, jpeg) = write_test_jpeg(200)?;
    let misnamed = tmp.path().join("cover.png");
    std::fs::rename(&jpeg, &misnamed)?;

    let decoded = decode_capped_to(&misnamed, MAX_SOURCE_DIM, 48)?;
    assert_eq!(
        (decoded.width(), decoded.height()),
        (200, 200),
        "the name sends this down the fallback, which sniffs the header and must still decode it"
    );
    Ok(())
}

/// The dimension bound stays [`capped_limits`]' alone: the fast path *declines* an oversized
/// source rather than reporting it, and the fallback runs the same file through the guard. What
/// must not happen is the fast path answering where the fallback would refuse.
#[test]
fn a_source_over_the_cap_is_refused() -> TestResult {
    let (_tmp, path) = write_test_jpeg(64)?;
    assert!(
        decode_capped_to(&path, 32, 16).is_err(),
        "a source past `max_dim` must be refused however small the target"
    );
    Ok(())
}

/// A container with no scale-on-decode still decodes, at its own size — the fallback is most of
/// what makes the call safe to use everywhere.
#[test]
fn a_container_without_scale_on_decode_falls_back() -> TestResult {
    let (_tmp, path) = write_test_png(200)?;
    let decoded = decode_capped_to(&path, MAX_SOURCE_DIM, 48)?;
    assert_eq!((decoded.width(), decoded.height()), (200, 200));
    Ok(())
}

// ── fit_within ──

/// The rule every tier depends on: a cover smaller than the tile is drawn at the tile's size
/// either way, so padding the buffer out to it buys nothing but memory.
#[test]
fn a_rectangle_already_inside_the_bound_is_returned_untouched() {
    assert_eq!(fit_within(128, 128, 448, 448), (128, 128));
    assert_eq!(fit_within(1, 1, 512, 512), (1, 1));
    assert_eq!(fit_within(448, 448, 448, 448), (448, 448));
}

/// One ratio for both axes, so the rectangle's own shape survives. The backdrop band relies on
/// this: clamping the axes independently would make how much it squashes depend on the cover.
#[test]
fn an_oversized_rectangle_shrinks_by_a_single_ratio() {
    assert_eq!(fit_within(1024, 1024, 512, 512), (512, 512));
    assert_eq!(fit_within(1000, 500, 500, 500), (500, 250));
    assert_eq!(fit_within(500, 1000, 500, 500), (250, 500));

    // A landscape target into a square source — the backdrop band's own case, where the *target*
    // is what shrinks.
    let (width, height) = fit_within(128, 85, 120, 120);
    assert!(width <= 120 && height <= 120);
    let target_ratio = f64::from(128) / f64::from(85);
    let fitted_ratio = f64::from(width) / f64::from(height);
    assert!(
        (target_ratio - fitted_ratio).abs() < 0.05,
        "the band's aspect must survive the fit: {target_ratio} vs {fitted_ratio}"
    );
}

/// Integer division must not round an axis away entirely — a zero-width target is a resize the
/// resampler refuses, which would read as an undecodable cover.
#[test]
fn an_extreme_ratio_never_rounds_an_axis_to_zero() {
    let (width, height) = fit_within(4000, 3, 512, 512);
    assert!(width > 0 && height > 0, "got {width}x{height}");
    assert!(width <= 512 && height <= 512);
}
