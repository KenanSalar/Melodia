//! Small shared helpers for the entity grids' cover caches.
//!
//! Three things every grid needs and none should own: turning an iterator of optional
//! artwork paths into a deduplicated, display-ordered `Vec<PathBuf>` for
//! `CoverThumbs::prewarm`, sizing that cache against the display it will be drawn on, and
//! resolving a card's cover against its page's `covers-generation`. The per-entity
//! `first_screenful_paths` wrappers still own the projection.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Image};

use crate::AppWindow;
use crate::media::artwork::STORE_MAX_DIM;
use crate::media::cover_thumbs::CoverThumbs;

/// Deduplicated, non-empty artwork paths from an iterator of optional path strings,
/// first-seen order preserved, stopping at `cap`.
///
/// `cap` keeps a detail over a huge entity from allocating a path per track only for
/// `prewarm` to discard all but the LRU's worth: the cache capacity for a full-list
/// prewarm, the screenful count for a grid. Paths must arrive in **display order** so the
/// prefix that survives the cap is the one that paints first.
pub fn unique_artwork_paths<'a>(
    paths: impl Iterator<Item = Option<&'a str>>,
    cap: usize,
) -> Vec<PathBuf> {
    // Against the input, not the cap: the full-list callers pass a whole cache capacity,
    // and a twelve-track album detail shouldn't lay out hundreds of buckets to fill
    // twelve. An upper hint bounds the items and so the kept paths.
    let prealloc = paths.size_hint().1.unwrap_or(cap).min(cap);
    let mut seen: HashSet<&str> = HashSet::with_capacity(prealloc);
    let mut out: Vec<PathBuf> = Vec::with_capacity(prealloc);
    for p in paths.flatten() {
        if out.len() >= cap {
            break;
        }
        if !p.is_empty() && seen.insert(p) {
            out.push(PathBuf::from(p));
        }
    }
    out
}

/// How many grid covers to keep resident on a display of this logical size.
///
/// **The pitches are `GridGeometry`'s own, and the estimate has to come out at or above the
/// number of cards actually mounted.** A tier smaller than the grid draws leaves the overflow
/// on placeholders until a scroll, because the lookup behind them schedules against a cache
/// that cannot hold them all. The margin comes from measuring the *window* rather than the
/// grid's own box: the sidebar and the bands above and below are headroom this never subtracts.
/// `rows` adds one more for the partially-visible row.
///
/// **The ceiling is on bytes, not on entries**, `thumb_size` being what a decoded buffer costs:
/// a fixed entry count is wrong by the square of the tier size, and it was wrong in the
/// direction that hurts — the big logical desktops that mount the most cards are exactly the
/// ones running the *small* 1× tier, where the same bytes buy several times the entries.
///
/// One function rather than one per grid: every grid draws the same card at the same size,
/// so the pitches and the budget are the only knobs.
pub fn cover_cap(
    logical_w: u32,
    logical_h: u32,
    thumb_size: u32,
    fallback: NonZeroUsize,
) -> NonZeroUsize {
    /// The row pitch beside [`CARD_PITCH_W`]'s column one, the tile being square above its text.
    const ROW_PITCH_H: u32 = MIN_CARD_W + CARD_TEXT_H + GAP;
    const MIN_CAP: usize = 32;
    /// Ceiling on what one grid tier may hold, in bytes of RGB8. Set at what the old
    /// entry-count ceiling cost at the largest tier, so the worst case is where it always was
    /// and only the small-tier cases gain. Every one of these caches is released entirely on
    /// section leave.
    const MAX_TIER_BYTES: usize = 56 * 1024 * 1024;

    // Ceiling where [`widest_card`] floors: the cap has to cover what is mounted, so an
    // over-count is the safe direction here and the wrong one there.
    let cols = logical_w.div_ceil(CARD_PITCH_W).max(1);
    // `+ 1` for the partially-visible row — the only scroll headroom.
    let rows = logical_h.div_ceil(ROW_PITCH_H) + 1;

    let side = usize::try_from(thumb_size.max(1)).unwrap_or(usize::MAX);
    let bytes_each = side.saturating_mul(side).saturating_mul(3).max(1);
    let affordable = (MAX_TIER_BYTES / bytes_each).max(MIN_CAP);

    let visible = usize::try_from(cols.saturating_mul(rows)).unwrap_or(affordable);
    let cap = visible.clamp(MIN_CAP, affordable);
    NonZeroUsize::new(cap).unwrap_or(fallback)
}

/// `GridGeometry`'s three defaults. Every one of its five mounts binds `avail-width: body.width`
/// and the shared gap and takes the width pitch as declared, so there is one answer to what a card
/// measures and this is what derives it. Radio is the one mount that moves `card-text-h`, its
/// station card carrying a third line, and that can only make its rows taller than [`cover_cap`]'s
/// `ROW_PITCH_H` assumes: fewer rows on screen than counted, which is the safe direction for a cap.
const MIN_CARD_W: u32 = 180;
const GAP: u32 = 20;
const CARD_TEXT_H: u32 = 46;

/// The pitch `GridGeometry` packs columns at.
const CARD_PITCH_W: u32 = MIN_CARD_W + GAP;

/// What the window keeps before the grid's body starts: the widest the sidebar can be dragged
/// (`Theme.sidebar-max-w`) plus the page's `pad-lg` at both edges.
///
/// Subtracted rather than ignored, the sidebar being the user's to drag while only the window is
/// measurable from here. Assuming the *widest* one is the safe direction: over-subtracting costs
/// a tier one step too large, under-subtracting puts the cards on an upscale.
const BODY_CHROME_W: u32 = 400 + 2 * 16;

/// The size a tier is built at, before any geometry is known.
///
/// A **fallback, not a tier size**: everything decoded at it is discarded the moment
/// [`cover_size_for_window`] retunes. Sized for the run where that never happens — the deferred
/// retune in `boot::ui_setup` can fail to schedule — so it covers a wide panel's card rather than
/// taking the cheap end.
pub const GRID_COVER_FALLBACK: u32 = 256;

/// Steps the derived size falls on. Quantized because [`crate::media::cover_thumbs::CoverThumbs`]
/// **clears the whole tier** when the size genuinely moves, and `WindowChrome.display-changed`
/// re-derives it on every winit `Resized`: the answer has to be flat across a resize drag, not
/// merely close. [`widest_card`] is what makes it flat; the step is what collapses the column
/// counts a desktop drags through into a handful of tiers.
const SIZE_STEP: u32 = 32;

/// The widest card `GridGeometry` can pack into the body of a window this wide.
///
/// The bound rather than the card itself: `card-w` climbs from `min-card-w` to this as a column
/// band fills, so sizing to the bound holds the tier flat across the band and steps only where
/// the column count really moves. It also can't land under what the grid draws, which is the
/// failure that shows — `FemtoVG` minifies bilinear with no mipmaps, so a soft tile is the worse
/// of the two.
fn widest_card(logical_w: u32) -> u32 {
    let body = logical_w.saturating_sub(BODY_CHROME_W).max(CARD_PITCH_W);
    let cols = (body.saturating_sub(GAP) / CARD_PITCH_W).max(1);
    MIN_CARD_W + CARD_PITCH_W / cols
}

/// Square decode size (px) for a grid-card tile on this window.
///
/// **Derived from the card the grid draws**, rather than stepped off the scale factor alone. The
/// two constants this replaced were a tier size for the wide case and twice it for `HiDPI`, which
/// got the trade backwards: a card is *smallest* on the panels that mount the most of them, so
/// the displays paying for the most buffers were the ones holding each one at roughly twice the
/// pixels it drew. The physical size the card occupies is the whole question, and it is
/// `widest_card × scale`.
///
/// Clamped to [`STORE_MAX_DIM`], past which every tier would upscale from a source the store
/// already discarded — which is also the ceiling on how sharp a single-column panel can get.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped into `[MIN_CARD_W, STORE_MAX_DIM]` ahead of the cast"
)]
pub fn cover_size(logical_w: u32, scale: f64) -> u32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let physical = (f64::from(widest_card(logical_w)) * scale)
        .clamp(f64::from(MIN_CARD_W), f64::from(STORE_MAX_DIM)) as u32;
    physical.div_ceil(SIZE_STEP).saturating_mul(SIZE_STEP).min(STORE_MAX_DIM)
}

/// Grid decode size for the display this window is on. Unlike [`cover_cap_for_window`]
/// this needs no winit round trip, the scale factor being Slint's own; it shares that
/// function's zero-extent bail, a window with no size having no card to measure.
pub fn cover_size_for_window(app: &AppWindow) -> u32 {
    let window = app.window();
    let scale = f64::from(window.scale_factor());
    let physical_w = window.size().width;
    if physical_w == 0 {
        return GRID_COVER_FALLBACK;
    }
    cover_size(logical_dim(physical_w, scale), scale)
}

/// A physical pixel extent and DPI scale as a logical extent. The saturating boundary
/// for the `f64 → u32` step, mirroring `media::artwork::f64_to_pixel`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "logical screen extent stays well below u32::MAX; this is the saturating boundary"
)]
fn logical_dim(physical: u32, scale: f64) -> u32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let v = (f64::from(physical) / scale).round();
    if v.is_nan() || v <= 0.0 {
        physical
    } else if v >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        v as u32
    }
}

/// Derive a grid-cover cap from the window's own logical size. **Must run after
/// `app.show()`** — see the deferred call in `boot::ui_setup::install_views`; a zero
/// extent means it ran early anyway and falls back rather than clamping to the floor.
///
/// Slint's window rather than winit's monitor: the monitor caps against a screen the
/// window may occupy a corner of, and asking it cost a `with_winit_window` round trip that
/// answered `None` for the whole window-less boot.
pub fn cover_cap_for_window(app: &AppWindow, fallback: NonZeroUsize) -> NonZeroUsize {
    let window = app.window();
    let physical = window.size();
    if physical.width == 0 || physical.height == 0 {
        return fallback;
    }
    let scale = f64::from(window.scale_factor());
    let logical_w = logical_dim(physical.width, scale);
    cover_cap(
        logical_w,
        logical_dim(physical.height, scale),
        cover_size(logical_w, scale),
        fallback,
    )
}

/// Resolve one grid card's cover without ever decoding on the calling thread.
///
/// `generation` is the page's `covers-generation`: 0 means the tab was just entered and
/// its tier cleared on the previous leave, so answer from the cache alone and let the card
/// paint its placeholder — the off-thread prewarm bumps the counter when it lands, re-running
/// these bindings. Past 0 a miss is scheduled rather than served, which covers the case the
/// prewarm can't: a library with more unique covers than the tier holds, where scrolling past
/// the warmed prefix used to decode under the event loop one card at a time. Full contract in
/// the "Covers" section of `.claude/rules/ui-patterns.md`.
///
/// Shared by the three grid tabs rather than spelled per page: the tier and the counter
/// differ, the rule doesn't, and a copy that grew a decoding `else` arm would look right
/// and quietly retire the mechanism.
pub fn grid_cover(thumbs: &Arc<CoverThumbs>, artwork_path: &str, generation: i32) -> Image {
    let path = nonempty_artwork_path(artwork_path);
    if generation == 0 {
        thumbs.get_cached_opt(path)
    } else {
        thumbs.get_or_schedule_opt(path)
    }
}

/// [`grid_cover`] for a surface with no generation behind it, which has to decode inline: nothing
/// would re-run the binding, so a scheduled cover would never arrive.
///
/// Only for bounded, prewarmed surfaces — Artist Detail's Albums strip, whose callback carries no
/// counter, and the Edit Artwork dialog's cover slot, a one-shot property write. Shared for the
/// same reason [`grid_cover`] is: the choice between the two is the whole decision, and a per-view
/// copy is where it stops being visible.
pub fn grid_cover_blocking(thumbs: &CoverThumbs, artwork_path: &str) -> Image {
    thumbs.get_or_load_opt(nonempty_artwork_path(artwork_path))
}

/// The `""` → `None` normalization every cover lookup owes its tier: Slint has no null
/// string, so a row with no artwork carries an empty one and the `*_opt` lookups take an
/// `Option` precisely so that case never reaches the decoder. Named rather than inlined
/// because the two mosaic lookups don't go through [`grid_cover`], their tier being warmed
/// by a fetch rather than a tab.
pub fn nonempty_artwork_path(artwork_path: &str) -> Option<&str> {
    Some(artwork_path).filter(|s| !s.is_empty())
}

#[cfg(test)]
#[path = "tests/grid_prewarm_tests.rs"]
mod tests;
