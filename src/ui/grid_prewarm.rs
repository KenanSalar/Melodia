//! Small shared helpers for the entity grids' cover caches.
//!
//! Three things every grid needs and none of them should own. Every entity grid
//! (`albums`, `artists`, `playlists`, the three grid tabs) and the genre
//! detail view turns an iterator of optional artwork-path strings into a
//! deduplicated, display-ordered `Vec<PathBuf>` to hand to
//! `CoverThumbs::prewarm`; every one with a cover cache sizes that cache
//! against the display it will actually be drawn on; and every grid *tab*
//! resolves a card's cover against its page's `covers-generation`. The
//! per-entity `first_screenful_paths` wrappers still own the entity-specific
//! projection (which field, how many ahead); only these cores are shared.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use slint::{ComponentHandle, Image};

use crate::AppWindow;
use crate::media::cover_thumbs::CoverThumbs;

/// Deduplicated, non-empty artwork paths from an iterator of optional path
/// strings, preserving first-seen order and stopping at `cap` — fed to
/// `CoverThumbs::prewarm`.
///
/// `cap` is what keeps a detail view over a huge entity from allocating a
/// path per track only for `prewarm` to discard all but the LRU's worth:
/// pass the cache capacity for a full-list prewarm, or the screenful count
/// for a grid. Paths must arrive in **display order** so the prefix that
/// survives the cap is the one that paints first.
pub fn unique_artwork_paths<'a>(
    paths: impl Iterator<Item = Option<&'a str>>,
    cap: usize,
) -> Vec<PathBuf> {
    // Reserve against the input, not the cap — the full-list callers pass a
    // whole cache capacity, and a twelve-track album detail shouldn't lay out
    // hundreds of buckets to fill twelve. An upper hint bounds the items, so
    // it bounds the kept paths too; it's the exact count for the plain `map`
    // callers and merely an over-estimate through a `filter_map`. Either way
    // `.min(cap)` keeps it no worse than the cap it replaced, which is what
    // an iterator with no upper hint falls back to.
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
/// The footprint (~260 px wide incl. gap, ~320 px tall incl. text + gap) is a
/// deliberate over-estimate against a card that packs down toward
/// `GridGeometry`'s 180 px `min-card-w` on a wide panel — it under-counts
/// columns, where a tighter number would claim more cards are on screen than
/// are. `rows` adds one partial row as the only scroll-back headroom: no extra
/// multiplier, because even fullscreen at 1440p only ~50 cards are visible at
/// once, so a 1.5× cushion was just dead weight. Clamped to `[32, 96]` — a
/// ~6–18 MB band at [`GRID_COVER_SIZE`], ~18–55 MB at
/// [`GRID_COVER_SIZE_HIDPI`] — and every one of these caches is released
/// entirely when the user leaves its section anyway. The footprint constants
/// and clamps are the tunable knobs. Lands ≈ 1080p → 35, 1440p → 54, 4K → 96.
///
/// One function rather than one per grid: it was copied verbatim into
/// `albums`, `artists` and `playlists`, so the numbers above had three places
/// to drift and the Favorites tabs would have made a fourth. Every grid draws
/// the same card at the same size — there is nothing per-entity to tune.
pub fn cover_cap(logical_w: u32, logical_h: u32, fallback: NonZeroUsize) -> NonZeroUsize {
    const CARD_FOOTPRINT_W: u32 = 260;
    const ROW_FOOTPRINT_H: u32 = 320;
    const MIN_CAP: usize = 32;
    const MAX_CAP: usize = 96;

    let cols = (logical_w / CARD_FOOTPRINT_W).max(1);
    // `+ 1` for the partially-visible row — the only scroll headroom.
    let rows = logical_h.div_ceil(ROW_FOOTPRINT_H) + 1;
    let visible = usize::try_from(cols.saturating_mul(rows)).unwrap_or(MAX_CAP);
    let cap = visible.clamp(MIN_CAP, MAX_CAP);
    NonZeroUsize::new(cap).unwrap_or(fallback)
}

/// Square decode size (px) for every grid-card tile, at a 1× display.
///
/// `GridGeometry` packs cards at `min-card-w: 180px`, so a card lands at
/// ~190 px on a wide panel and grows past this only below three columns — a
/// panel showing one or two cards, where the tier holds almost nothing. Sized
/// to the wide case rather than that edge: `FemtoVG` minifies bilinear with no
/// mipmaps, so covering the edge costs every card in every grid to sharpen the
/// layout that needs it least.
pub const GRID_COVER_SIZE: u32 = 256;

/// The same tile on a `HiDPI` display, which draws it at twice the pixels.
pub const GRID_COVER_SIZE_HIDPI: u32 = 448;

/// Grid decode size for a display at `scale`.
///
/// One function rather than a constant per grid: `448` had been copied into
/// `albums`, `artists`, `playlists`, `browse` and both mosaic pages, each
/// justifying it in its own doc comment, and every grid draws the same card at
/// the same size. Threshold matches
/// [`crate::media::cover_thumbs::row_cover_size`] — the two tiers are asking
/// the same question about the same display.
pub fn cover_size(scale: f64) -> u32 {
    if scale > 1.25 {
        GRID_COVER_SIZE_HIDPI
    } else {
        GRID_COVER_SIZE
    }
}

/// Grid decode size for the display this window is on.
///
/// Unlike [`cover_cap_for_window`] this needs no winit round trip and has no
/// failure arm — the scale factor is Slint's own and always readable.
pub fn cover_size_for_window(app: &AppWindow) -> u32 {
    cover_size(f64::from(app.window().scale_factor()))
}

/// Convert a physical pixel extent + DPI scale into a logical extent.
/// Saturating boundary for the `f64 → u32` step — mirrors
/// `media::artwork::f64_to_pixel`; monitor extents stay far below
/// `u32::MAX` in practice.
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

/// Derive a grid-cover cap from the window's own logical size.
///
/// **Must run after `app.show()`** — see the deferred call in
/// `boot::ui_setup::install_views`. A zero extent means it ran early anyway, and
/// falls back rather than clamping to the floor.
///
/// Reads Slint's window rather than winit's monitor. The monitor was the wrong
/// question — it caps against a screen the window may occupy a corner of — and
/// asking it cost a `with_winit_window` round trip that returned `None` for the
/// entire window-less boot, so every tier silently kept its construction default
/// on every platform.
pub fn cover_cap_for_window(app: &AppWindow, fallback: NonZeroUsize) -> NonZeroUsize {
    let window = app.window();
    let physical = window.size();
    if physical.width == 0 || physical.height == 0 {
        return fallback;
    }
    let scale = f64::from(window.scale_factor());
    cover_cap(
        logical_dim(physical.width, scale),
        logical_dim(physical.height, scale),
        fallback,
    )
}

/// Resolve one grid card's cover, decoding only once the tier is known warm.
///
/// `generation` is the page's `covers-generation`: 0 means the tab was just
/// entered and its tier was cleared on the previous tab-leave, so answer from
/// the cache alone and let the card paint its placeholder. Decoding here instead
/// puts one grid-tier decode per visible card on the UI thread, in the frame that
/// mounts the grid — the off-thread prewarm bumps the counter when it lands,
/// which re-runs these bindings and lets rows scrolled to later load on demand.
/// Same contract as `Queue.request-cover`; see the "Covers" section of
/// `.claude/rules/ui-patterns.md`.
///
/// Shared by the three grid tabs rather than spelled out per page: the tier and
/// the counter differ, the rule doesn't, and a copy that grew a decoding `else`
/// arm would look right and quietly retire the whole mechanism.
pub fn grid_cover(thumbs: &CoverThumbs, artwork_path: &str, generation: i32) -> Image {
    let path = nonempty_artwork_path(artwork_path);
    if generation == 0 {
        thumbs.get_cached_opt(path)
    } else {
        thumbs.get_or_load_opt(path)
    }
}

/// The `""` → `None` normalization every cover lookup owes its tier.
///
/// Slint has no null string, so a row with no artwork carries an empty one, and
/// the `*_opt` lookups take an `Option` precisely so that case never reaches the
/// decoder. Named rather than inlined because the two mosaic lookups — which
/// don't go through [`grid_cover`], their tier being warmed by a fetch rather
/// than a tab — spelled the filter out themselves.
pub fn nonempty_artwork_path(artwork_path: &str) -> Option<&str> {
    Some(artwork_path).filter(|s| !s.is_empty())
}

#[cfg(test)]
#[path = "tests/grid_prewarm_tests.rs"]
mod tests;
