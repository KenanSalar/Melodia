use super::*;
use crate::test_support::write_test_png;

const GRID_GEOMETRY: &str =
    include_str!("../../../../melodia-ui/ui/components/grid-geometry.slint");

/// **Both answers in this module are built out of `GridGeometry`'s defaults**, and Slint declares
/// them where Rust can only restate them. Nothing else holds the two trees together: a `min-card-w`
/// nudged in the component silently moves every tier size and every cap, in the direction that
/// leaves the grid's overflow on placeholders.
#[test]
fn the_card_constants_are_the_ones_the_component_declares() {
    for (name, value) in [
        ("min-card-w", MIN_CARD_W),
        ("gap", GAP),
        ("card-text-h", CARD_TEXT_H),
    ] {
        let declared = format!("in property <length> {name}: {value}px;");
        assert!(
            GRID_GEOMETRY.contains(&declared),
            "`grid-geometry.slint` no longer declares `{declared}` — Rust sizes every grid tier \
             and cap off that number"
        );
    }
}

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
    // Paired with the size that display would really be tuned to, so the two move together here
    // as they do at the one call site that reads them.
    let cap = |w, h| super::cover_cap(w, h, super::cover_size(w, 1.0), fallback).get();

    // A tiny display can't fill many cards — clamps to the floor (32).
    assert_eq!(cap(640, 480), 32);
    // A mid-range display lands off the floor...
    let mid = cap(1920, 1080);
    assert!(mid > 32, "1080p cap {mid} should sit off the floor");
    // ...and the cap is monotonic in display area.
    assert!(cap(1280, 720) <= mid);
    assert!(mid <= cap(2560, 1440));
}

/// **The ceiling is bytes, so the same grid affords more entries at the smaller tier.** An entry
/// count is wrong by the square of the tier size, and it was wrong the expensive way round: the
/// big logical desktops that mount the most cards are the ones packing the smallest cards, where
/// the buffers are a fraction the size. A 4K panel at 1× used to be clamped to the same 96 as a
/// narrow one holding a handful of huge tiles, and paid for it in placeholders.
///
/// Sizes are spelled rather than derived: this is the budget's own arithmetic, and it has to hold
/// for whatever [`super::cover_size`] hands it.
#[test]
fn the_ceiling_is_bytes_rather_than_entries() {
    const BUDGET: usize = 56 * 1024 * 1024;
    let fallback = NonZeroUsize::new(48).unwrap_or(NonZeroUsize::MIN);
    let cap = |tier| super::cover_cap(3840, 2160, tier, fallback).get();

    let (small, large) = (cap(192), cap(512));
    assert!(
        small > large,
        "the same grid affords {small} entries at 192px and {large} at 512px — a ceiling that \
         doesn't move with the buffer size is one of the two answers being wrong"
    );

    for (entries, side) in [(small, 192usize), (large, 512usize)] {
        let bytes = entries.saturating_mul(side).saturating_mul(side).saturating_mul(3);
        assert!(
            bytes <= BUDGET,
            "{entries} entries at {side}px is {bytes} bytes, past the {BUDGET}-byte tier budget"
        );
    }
}

/// `GridGeometry`'s own `card-w`, which production no longer computes — it sizes to the widest
/// card a column count can pack, and this is what that bound has to clear.
fn drawn_card_width(body_w: u32) -> u32 {
    let cols = (body_w.saturating_sub(GAP) / (MIN_CARD_W + GAP)).max(1);
    body_w.saturating_sub((cols + 1) * GAP) / cols
}

/// `GridGeometry`'s own arithmetic, so the pin below measures the cap against the number of
/// cards the grid really mounts rather than against a restated guess.
fn mounted_cards(logical_w: u32, logical_h: u32) -> u32 {
    let cols = (logical_w.saturating_sub(GAP) / (MIN_CARD_W + GAP)).max(1);
    let row_h = drawn_card_width(logical_w) + CARD_TEXT_H + GAP;
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
/// Each size is paired with the tier [`super::cover_size`] really derives for it, the two being
/// read together at the one call site: the cap is what the grid can hold, and the size is what
/// each entry costs, so checking either against a spelled-out constant would check the wrong pair.
#[test]
fn the_cap_covers_the_cards_the_grid_mounts() {
    let fallback = NonZeroUsize::new(48).unwrap_or(NonZeroUsize::MIN);

    for (w, h, scale) in [
        (1280, 720, 1.0),
        (1366, 768, 1.0),
        (1600, 900, 1.0),
        (1920, 1080, 1.0),
        (2560, 1440, 1.0),
        // A 4K panel at 1×, the case an entry-count ceiling could not cover.
        (3840, 2160, 1.0),
        // The same panel at 200%, and a 5K at 150%.
        (1920, 1080, 2.0),
        (2560, 1440, 1.5),
    ] {
        let tier = super::cover_size(w, scale);
        let cap = super::cover_cap(w, h, tier, fallback).get();
        let mounted = usize::try_from(mounted_cards(w, h)).unwrap_or(usize::MAX);
        assert!(
            cap >= mounted,
            "{w}x{h} at {scale}× derives a {tier}px tier and mounts {mounted} cards against a \
             cap of {cap} — the overflow can only paint placeholders"
        );
    }
}

/// **The tier is the card's own physical size**, where it used to be a step off the scale factor
/// alone. That got the trade backwards: `GridGeometry` packs toward `min-card-w`, so a card is
/// *smallest* on the panels mounting the most of them, and a fixed pair of sizes made those
/// displays hold every buffer at roughly twice the pixels it drew.
#[test]
fn the_tier_follows_the_card_it_draws() {
    // A wider panel packs smaller cards, so at a fixed scale the tier never grows with the
    // display. That is the case the old constants over-paid for, and the whole 1× range sits
    // under the 256 they spent on it.
    let wide = super::cover_size(1920, 1.0);
    assert!(super::cover_size(2560, 1.0) <= wide);
    assert!(super::cover_size(3840, 1.0) <= super::cover_size(2560, 1.0));
    assert!(wide >= MIN_CARD_W, "a {wide}px tier can't cover a {MIN_CARD_W}px card");

    // Scale multiplies what the card occupies, so the tier follows it up.
    assert!(super::cover_size(1920, 1.5) > wide);
    assert!(super::cover_size(1920, 2.0) > super::cover_size(1920, 1.5));
    assert!(super::cover_size(1920, 2.0) >= MIN_CARD_W * 2);

    // A narrow panel packs one much larger card, and is sized for that rather than for the wide
    // case — the handful of tiles it mounts is what makes that affordable.
    assert!(super::cover_size(500, 1.0) > wide);

    // Nothing may exceed what the store keeps, however extreme the pairing.
    assert_eq!(super::cover_size(400, 4.0), melodia_artwork::media::image::artwork::STORE_MAX_DIM);
}

/// **The tier has to hold still through a resize drag.** `WindowChrome.display-changed` re-derives
/// it on every winit `Resized` and a genuine `set_thumb_size` clears the whole tier, so a size
/// that flips between two steps as the window moves drops every decoded cover and repaints the
/// grid as placeholders, over and over, under the drag.
///
/// Sizing to the widest card a column count can pack is what holds it flat. The card itself
/// sweeps `min-card-w` up to that bound inside *every* column band, so a tier tracking it crosses
/// a step boundary twice a band — which is a wipe per crossing, not a rounding difference.
#[test]
fn the_tier_holds_still_through_a_resize_drag() {
    const MAX_RETUNES: usize = 8;

    for scale in [1.0, 1.25, 1.5, 2.0] {
        let mut retunes = 0;
        let mut previous = None;
        for logical_w in 800..=3840 {
            let size = super::cover_size(logical_w, scale);
            if previous.is_some_and(|prev| prev != size) {
                retunes += 1;
            }
            previous = Some(size);
        }
        assert!(
            retunes <= MAX_RETUNES,
            "a drag from 800 to 3840 logical px at {scale}× retunes the tier {retunes} times, \
             past the {MAX_RETUNES} it can absorb — each one clears every grid's covers"
        );
    }
}

/// **The tier may not land under the card the grid draws.** Rust measures the *window* while the
/// grid gets the body, and the sidebar between them is the user's to drag across a range no
/// window measurement sees. `BODY_CHROME_W` assumes the widest of them so the estimate can only
/// run wide; the other way round every card is upscaled from a tier too small for it, and
/// `FemtoVG` minifies bilinear with no mipmaps.
#[test]
fn the_tier_covers_the_card_at_every_sidebar_width() {
    // `Theme.sidebar-collapsed-w` through `sidebar-max-w`, plus the page's `pad-lg` at both edges.
    const SIDEBAR_WIDTHS: [u32; 5] = [46, 105, 180, 240, 400];
    const PAGE_PAD: u32 = 32;

    for sidebar in SIDEBAR_WIDTHS {
        for logical_w in (800_u32..=3840).step_by(7) {
            let Some(body) = logical_w.checked_sub(sidebar + PAGE_PAD) else {
                continue;
            };
            for scale in [1.0, 1.5, 2.0] {
                let drawn = f64::from(drawn_card_width(body)) * scale;
                let tier = super::cover_size(logical_w, scale);
                // The store's own cap is the one honest exception — past it there is no sharper
                // source left to decode.
                if tier == melodia_artwork::media::image::artwork::STORE_MAX_DIM {
                    continue;
                }
                assert!(
                    f64::from(tier) >= drawn,
                    "a {logical_w}px window beside a {sidebar}px sidebar draws {drawn} px of card \
                     at {scale}× against a {tier}px tier — the covers upscale"
                );
            }
        }
    }
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
