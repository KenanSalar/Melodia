use super::*;
use crate::test_support::write_test_png;

fn paths(of: &[Option<&str>], cap: usize) -> Vec<String> {
    unique_artwork_paths(of.iter().copied(), cap)
        .iter()
        .filter_map(|p| p.to_str().map(str::to_owned))
        .collect()
}

#[test]
fn duplicates_collapse_and_first_seen_order_survives() {
    let out = paths(&[Some("b.jpg"), Some("a.jpg"), Some("b.jpg"), Some("c.jpg")], 16);
    assert_eq!(out, vec!["b.jpg", "a.jpg", "c.jpg"]);
}

#[test]
fn missing_and_empty_paths_are_skipped() {
    let out = paths(&[None, Some(""), Some("a.jpg"), None, Some("")], 16);
    assert_eq!(out, vec!["a.jpg"]);
}

#[test]
fn the_cap_counts_kept_paths_not_input_items() {
    // Five inputs, three of which are duplicates of the first: a cap of 2
    // has to yield two *distinct* covers, not stop after the second input.
    let out = paths(
        &[
            Some("a.jpg"),
            Some("a.jpg"),
            Some("a.jpg"),
            Some("b.jpg"),
            Some("c.jpg"),
        ],
        2,
    );
    assert_eq!(out, vec!["a.jpg", "b.jpg"]);
}

#[test]
fn a_zero_cap_yields_nothing() {
    assert!(paths(&[Some("a.jpg")], 0).is_empty());
}

/// The cap has to grow with the display and stop at both ends: too small and the grid's overflow
/// paints placeholders, too large and the tier alone is tens of megabytes of resident buffers on
/// a laptop that can't show them. It lived in three byte-identical copies under `albums` /
/// `artists` / `playlists`, each with its own copy of this test, so the band had three places to
/// drift.
#[test]
fn cover_cap_clamps_and_scales_with_resolution() {
    let fallback = NonZeroUsize::new(48).unwrap_or(NonZeroUsize::MIN);
    let cap = |w, h, tier| super::cover_cap(w, h, tier, fallback).get();

    // A tiny display can't fill many cards — clamps to the floor (32).
    assert_eq!(cap(640, 480, super::GRID_COVER_SIZE), 32);
    // A mid-range display lands off both clamps...
    let mid = cap(1920, 1080, super::GRID_COVER_SIZE);
    assert!(mid > 32, "1080p cap {mid} should sit off the floor");
    // ...and the cap is monotonic in display area at a fixed tier.
    assert!(cap(1280, 720, super::GRID_COVER_SIZE) <= mid);
    assert!(mid <= cap(2560, 1440, super::GRID_COVER_SIZE));
}

/// **The ceiling is bytes, so the same grid affords more entries at the smaller tier.** An entry
/// count is wrong by the square of the tier size, and it was wrong the expensive way round: the
/// big logical desktops that mount the most cards are the ones running the 1× tier, where the
/// buffers are a fifth the size. A 4K panel at 1× used to be clamped to the same 96 as a 5K one
/// at 150%, and paid for it in placeholders.
#[test]
fn the_ceiling_is_bytes_rather_than_entries() {
    let fallback = NonZeroUsize::new(48).unwrap_or(NonZeroUsize::MIN);
    let cap = |w, h, tier| super::cover_cap(w, h, tier, fallback).get();

    let small_tier = cap(3840, 2160, super::GRID_COVER_SIZE);
    let large_tier = cap(3840, 2160, super::GRID_COVER_SIZE_HIDPI);
    assert!(
        small_tier > large_tier,
        "the same grid affords {small_tier} entries at {}px and {large_tier} at {}px — a ceiling \
         that doesn't move with the buffer size is one of the two answers being wrong",
        super::GRID_COVER_SIZE,
        super::GRID_COVER_SIZE_HIDPI
    );

    // Neither tier may spend more than the budget the ceiling exists to hold.
    for (entries, side) in [
        (small_tier, super::GRID_COVER_SIZE),
        (large_tier, super::GRID_COVER_SIZE_HIDPI),
    ] {
        let side = usize::try_from(side).unwrap_or(usize::MAX);
        let bytes = entries.saturating_mul(side).saturating_mul(side).saturating_mul(3);
        assert!(
            bytes <= 56 * 1024 * 1024,
            "{entries} entries at {side}px is {bytes} bytes, past the tier budget"
        );
    }
}

/// `GridGeometry`'s own arithmetic, so the pin below measures the cap against the number of
/// cards the grid really mounts rather than against a restated guess.
fn mounted_cards(logical_w: u32, logical_h: u32) -> u32 {
    const MIN_CARD_W: u32 = 180;
    const GAP: u32 = 20;
    const CARD_TEXT_H: u32 = 46;

    let cols = ((logical_w.saturating_sub(GAP)) / (MIN_CARD_W + GAP)).max(1);
    let card_w = logical_w.saturating_sub((cols + 1) * GAP) / cols;
    let row_h = card_w + CARD_TEXT_H + GAP;
    // `+ 1` for the partially-visible row, matching `cover_cap`.
    cols * (logical_h.div_ceil(row_h) + 1)
}

/// **The cap has to cover what the grid draws.** The lookup behind a card schedules against this
/// tier, so a cap under the mounted count leaves the overflow on placeholders until a scroll —
/// and it is the pitches, not the clamp, that decide: a cap derived from a card footprint the
/// grid doesn't use is wrong by a ratio at every size.
///
/// The margin is that `cover_cap` measures the *window* while the grid gets what's left of it
/// after the sidebar and the bands. Above the byte ceiling the guarantee stops, deliberately.
///
/// Sizes are paired with the tier they can actually occur at: `cover_size` steps up past a 1.25
/// scale factor, so a large *logical* desktop is by construction running the 1× tier, and the
/// `HiDPI` one only ever sees the smaller logical sizes a scaled display leaves.
#[test]
fn the_cap_covers_the_cards_the_grid_mounts() {
    let fallback = NonZeroUsize::new(48).unwrap_or(NonZeroUsize::MIN);

    for (w, h, tier) in [
        (1280, 720, super::GRID_COVER_SIZE),
        (1366, 768, super::GRID_COVER_SIZE),
        (1600, 900, super::GRID_COVER_SIZE),
        (1920, 1080, super::GRID_COVER_SIZE),
        (2560, 1440, super::GRID_COVER_SIZE),
        // A 4K panel at 1×, the case an entry-count ceiling could not cover.
        (3840, 2160, super::GRID_COVER_SIZE),
        // The same panel at 200%, and a 5K at 150% — the largest logical sizes the HiDPI tier
        // is reachable at.
        (1920, 1080, super::GRID_COVER_SIZE_HIDPI),
        (2560, 1440, super::GRID_COVER_SIZE_HIDPI),
    ] {
        let cap = super::cover_cap(w, h, tier, fallback).get();
        let mounted = usize::try_from(mounted_cards(w, h)).unwrap_or(usize::MAX);
        assert!(
            cap >= mounted,
            "{w}x{h} at the {tier}px tier mounts {mounted} cards against a cap of {cap} — the \
             overflow can only paint placeholders"
        );
    }
}

/// A grid card is drawn at roughly `GridGeometry`'s 180 px `min-card-w` on a
/// wide panel, so the 1× tier only has to beat that; the `HiDPI` tier has to
/// beat twice it. The threshold sits below 1.5 so a fractional-scale desktop
/// rounds up — a soft tile is the worse of the two failures.
#[test]
fn cover_size_steps_up_for_hidpi_and_never_below_a_card() {
    const MIN_CARD_W: u32 = 180;

    assert_eq!(super::cover_size(1.0), super::GRID_COVER_SIZE);
    assert_eq!(super::cover_size(2.0), super::GRID_COVER_SIZE_HIDPI);
    // A fractional scale rounds up rather than down.
    assert_eq!(super::cover_size(1.5), super::GRID_COVER_SIZE_HIDPI);

    assert!(super::cover_size(1.0) > MIN_CARD_W);
    assert!(super::cover_size(2.0) > MIN_CARD_W * 2);
}

/// **Neither generation may decode on the calling thread.** 0 means the tier was cleared when
/// its tab was left and the lookup answers from the cache alone; past 0 a miss is handed to the
/// decode pool and still answers with the placeholder, the card coming back on the bump that
/// follows. Neither is "return nothing" — an entry already in the tier resolves at any
/// generation, which is what makes a re-entered warm tab paint instantly.
#[test]
fn no_generation_decodes_on_the_calling_thread() -> Result<(), Box<dyn std::error::Error>> {
    let cap = NonZeroUsize::new(4).ok_or("cap must be > 0")?;
    let thumbs = Arc::new(CoverThumbs::with_config(64, cap));
    let (_tmp, path) = write_test_png(512)?;
    let path = path.to_str().ok_or("temp path is not UTF-8")?;

    assert_eq!(
        super::grid_cover(&thumbs, path, 0).size().width,
        0,
        "a cold tier must hand back a placeholder rather than decode on the UI thread"
    );
    assert_eq!(
        super::grid_cover(&thumbs, path, 1).size().width,
        0,
        "past 0 a miss schedules and still answers with the placeholder — decoding here is the \
         regression, one grid-tier decode per visible card in the frame that mounts the grid"
    );

    // What the scheduled decode will have done, without racing the pool for it.
    thumbs.prewarm(&[PathBuf::from(path)]);
    for generation in [0, 1] {
        assert_eq!(
            super::grid_cover(&thumbs, path, generation).size().width,
            64,
            "a cover already in the tier resolves at every generation"
        );
    }
    Ok(())
}
