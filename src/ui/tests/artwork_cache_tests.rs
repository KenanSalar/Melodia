use super::*;
use crate::ui::util::BLUR_SIGMA;

/// A tier at the Now Playing shape — the specifics don't matter to any test
/// here, which are all about the LRU and the remembered-failure rule, but a
/// literal drifts off the tier it claims to be the moment either is retuned.
fn test_cache(capacity: usize) -> ArtworkCache {
    let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
    ArtworkCache::new(
        cap,
        Some(BlurSpec {
            height: BLUR_TARGET,
            sigma: BLUR_SIGMA,
        }),
    )
}

#[test]
fn missing_file_returns_none_and_is_cached() {
    let artwork = test_cache(8);
    let missing = Path::new("/nonexistent/melodia/cover-does-not-exist.jpg");

    // A path that can't be opened decodes to `None`...
    assert!(artwork.get_or_decode(missing).is_none());

    // ...and the failure is remembered, so the entry is now present in
    // the cache (a second call still returns `None` without re-opening).
    assert!(artwork.contains(missing));
    assert!(artwork.get_or_decode(missing).is_none());
}

#[test]
fn lru_evicts_beyond_capacity() {
    const CAP: usize = 8;
    let artwork = test_cache(CAP);
    // Insert one more failure entry than the cap; the oldest must be gone.
    for i in 0..=CAP {
        let p = std::path::PathBuf::from(format!("/nonexistent/melodia/{i}.jpg"));
        let _ = artwork.get_or_decode(&p);
    }
    assert_eq!(artwork.len(), CAP);
    assert!(!artwork.contains(Path::new("/nonexistent/melodia/0.jpg")));
}

#[test]
fn clear_empties_the_cache() {
    let artwork = test_cache(8);
    let p = Path::new("/nonexistent/melodia/np-clear.jpg");
    // Populate with a (failure) entry, then clear it back out.
    let _ = artwork.get_or_decode(p);
    assert!(artwork.contains(p));
    artwork.clear();
    assert_eq!(artwork.len(), 0);
}

/// A tier with no spec is the aurora setting, where nothing paints a blur. The seeds have to
/// survive that, the aurora being exactly what wants them — and the brightness has to not, no
/// scrim being solved on that arm and the percentile being the dearer half of the two.
#[test]
fn a_specless_pair_keeps_the_seeds_and_skips_the_blur_and_the_brightness() {
    let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(64, 64, |x, _| {
        image::Rgb(if x < 32 { [200, 30, 40] } else { [30, 50, 200] })
    }));

    let pair = pair_from_image(&source, None);

    assert!(pair.blur.is_none());
    assert!(pair.sample.luma.is_none());
    assert!(pair.sample.accent_argb.is_some());
    assert!(pair.sample.seeds.iter().any(Option::is_some));
}

/// The two halves of a measurement read two different buffers, and only one of them is drawn.
///
/// `scrim_alpha` solves the *composite* onto `TARGET_BACKDROP_TONE`, so the percentile has to come
/// off the blurred buffer the scrim is painted over. A mostly-black sleeve carries its wordmark in
/// too few pixels to reach the 90th percentile sharp, and the blur is what smears it into the
/// mid-bright region the title then sits on; off the sharp downscale — where the *seeds* belong —
/// that region is stepped over and the scrim comes back at its floor.
#[test]
fn the_brightness_comes_off_the_blur_and_the_seeds_off_the_sharp_downscale() {
    // A wordmark's worth of white on black, deliberately *under* `PERCENTILE_TAIL` so the sharp
    // percentile steps over it — the whole case this split exists for.
    let source = DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(192, 192, |x, y| {
        image::Rgb(if x < 56 && y < 56 {
            [255, 255, 255]
        } else {
            [0, 0, 0]
        })
    }));

    let pair = pair_from_image(
        &source,
        Some(BlurSpec {
            height: BLUR_TARGET,
            sigma: 24.0,
        }),
    );

    assert!(pair.blur.is_some(), "a tier holding a spec must build a blur");
    assert_eq!(
        pair.sample.luma,
        pair.blur.as_ref().and_then(|blur| BackdropSample::measure(
            blur.as_bytes(),
            blur.as_bytes()
        )
        .luma),
        "the percentile must read the blurred buffer the scrim is composited over"
    );

    // An impossible `None` becomes `NaN` and fails the comparison rather than slipping through
    // it — `unwrap` is denied crate-wide, tests included.
    let painted = pair.sample.luma.unwrap_or(f64::NAN);
    let sharp = BackdropSample::measure(pair.cover.as_bytes(), pair.cover.as_bytes())
        .luma
        .unwrap_or(f64::NAN);
    assert!(
        painted > sharp + 10.0,
        "the blur has to surface the bright region the sharp percentile steps over: \
         painted L*{painted} against sharp L*{sharp}"
    );
}

/// Both tiers are newtypes whose whole content is a capacity and a
/// [`BlurSpec`], so nothing about them fails to compile — this walks each one's
/// two forwards so a tier wired to nothing is a failing test rather than a
/// silently coverless one.
#[test]
fn both_tiers_forward_to_the_cache_they_wrap() {
    let np = crate::ui::now_playing_artwork::NowPlayingArtwork::new(None);
    let detail = crate::ui::detail_artwork::DetailArtwork::new(None);
    let missing = Path::new("/nonexistent/melodia/tier-forward.jpg");

    assert!(np.get_or_decode(missing).is_none());
    assert!(detail.get_or_decode(missing).is_none());
    // The remembered failure is the observable half of the forward: a second
    // lookup answers from the cache rather than re-opening the file.
    assert!(np.get_or_decode(missing).is_none());
    assert!(detail.get_or_decode(missing).is_none());

    np.clear();
    detail.clear();
}
