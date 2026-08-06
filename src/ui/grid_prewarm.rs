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
/// The flex-filled grid cards are *large* (the user runs them well past
/// 200 px), so this uses a generous footprint (~260 px wide incl. gap,
/// ~320 px tall incl. text + gap) — a smaller footprint over-counts what's
/// really on screen. `rows` adds one partial row as the only scroll-back
/// headroom: no extra multiplier, because even fullscreen at 1440p only
/// ~50 cards are visible at once, so a 1.5× cushion was just dead weight.
/// Clamped to `[32, 96]` — at 448 px / ~600 KB per entry that's a ~19–58 MB
/// band, and every one of these caches is released entirely when the user
/// leaves its section anyway. The footprint constants and clamps are the
/// tunable knobs. Lands ≈ 1080p → 35, 1440p → 54, 4K → 96.
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

/// Query the window's current monitor and derive a grid-cover cap from its
/// logical resolution. Falls back to `fallback` when the monitor can't be read
/// (e.g. some Wayland setups report `None`).
///
/// Call once at startup, after the winit window is live — each cache is
/// constructed with its own default and resized from here.
pub fn cover_cap_for_window(app: &AppWindow, fallback: NonZeroUsize) -> NonZeroUsize {
    use slint::winit_030::WinitWindowAccessor;

    app.window()
        .with_winit_window(|w| {
            let monitor = w.current_monitor()?;
            let physical = monitor.size();
            let scale = w.scale_factor();
            Some(cover_cap(
                logical_dim(physical.width, scale),
                logical_dim(physical.height, scale),
                fallback,
            ))
        })
        .flatten()
        .unwrap_or(fallback)
}

/// Resolve one grid card's cover, decoding only once the tier is known warm.
///
/// `generation` is the page's `covers-generation`: 0 means the tab was just
/// entered and its tier was cleared on the previous tab-leave, so answer from
/// the cache alone and let the card paint its placeholder. Decoding here instead
/// puts one 448 px decode per visible card on the UI thread, in the frame that
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
