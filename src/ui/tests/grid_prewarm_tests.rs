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

/// The cap has to grow with the display and stop at both ends: too small and
/// a 4K grid re-decodes every scroll, too large and the tier alone is tens of
/// megabytes of resident buffers on a laptop that can't show them. It lived in
/// three byte-identical copies under `albums` / `artists` / `playlists`, each
/// with its own copy of this test, so the band had three places to drift.
#[test]
fn cover_cap_clamps_and_scales_with_resolution() {
    let fallback = NonZeroUsize::new(48).unwrap_or(NonZeroUsize::MIN);
    let cap = |w, h| super::cover_cap(w, h, fallback).get();

    // A tiny display can't fill many cards — clamps to the floor (32).
    assert_eq!(cap(640, 480), 32);
    // A 4K panel shows far more than the ceiling — clamps to the cap (96).
    assert_eq!(cap(3840, 2160), 96);
    // A mid-range display lands strictly between the clamps...
    let mid = cap(1920, 1080);
    assert!(mid > 32 && mid < 96, "1080p cap {mid} should sit between the clamps");
    // ...and the cap is monotonic in display area.
    assert!(cap(1280, 720) <= mid && mid <= cap(2560, 1440));
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
/// after the sidebar and the bands. Above the ceiling the guarantee stops, deliberately — a
/// panel that wide is bounded on bytes instead.
#[test]
fn the_cap_covers_the_cards_the_grid_mounts() {
    let fallback = NonZeroUsize::new(48).unwrap_or(NonZeroUsize::MIN);

    for (w, h) in [
        (1280, 720),
        (1366, 768),
        (1600, 900),
        (1920, 1080),
        (2560, 1440),
    ] {
        let cap = super::cover_cap(w, h, fallback).get();
        let mounted = usize::try_from(mounted_cards(w, h)).unwrap_or(usize::MAX);
        assert!(
            cap >= mounted,
            "{w}x{h} mounts {mounted} cards against a tier of {cap} — the overflow can only \
             paint placeholders"
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
