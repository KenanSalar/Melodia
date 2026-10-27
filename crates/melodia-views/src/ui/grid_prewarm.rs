//! The one cover tier every entity grid draws from, and the helpers that shape it.
//!
//! One tier rather than one per grid, every grid drawing the same card at the same size; a leave
//! hands back the bytes and keeps the picture ([`hand_back_covers`]). Search's strips decode at
//! sizes of their own, the queue panel's tier lives only while the panel shows, and Radio's logo
//! card reads the decoded extent (`ui::radio::covers`), so those three stay private.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use slint::{ComponentHandle, Image};

use melodia_artwork::media::image::artwork::STORE_MAX_DIM;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_ui::{AppWindow, WindowChrome};

/// LRU capacity a tier carries until [`tune_for_display`] measures the window.
pub const GRID_COVER_CAP_FALLBACK: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("GRID_COVER_CAP_FALLBACK > 0"),
};

/// Covers a grid warms before it paints, about a screenful. Under [`cover_cap`]'s floor, so a
/// prewarm can't evict its own work.
pub const GRID_PREWARM_AHEAD: usize = 24;

/// Side a leaving section shrinks its covers to, in px: how soft a tile may read for one
/// re-entry's prewarm, set by eye. The bytes it saves are a rounding error at any value.
pub const PROXY_COVER_DIM: u32 = 64;

/// A proxy reads as stale only while under every size the tier can be asked for: `holds` compares
/// the recorded size, so an equal one would skip the re-entry's prewarm and leave the page soft.
/// `grid_prewarm_tests` sweeps the derived half.
const _: () = assert!(
    PROXY_COVER_DIM < MIN_CARD_W && PROXY_COVER_DIM < GRID_COVER_FALLBACK,
    "a proxy must read as stale"
);

/// The cover tier every card grid draws from. A singleton, as `ui::nav_history::nav()` is: there
/// is one, and no view owns it.
pub fn tier() -> &'static Arc<CoverThumbs> {
    static TIER: LazyLock<Arc<CoverThumbs>> = LazyLock::new(|| {
        Arc::new(CoverThumbs::with_config(GRID_COVER_FALLBACK, GRID_COVER_CAP_FALLBACK))
    });
    &TIER
}

/// Hands a grid's decoded pixels back as it goes off screen, keeping a proxy of each so the next
/// mount paints the cover rather than the glyph. Blocking.
///
/// A `release_section_state` frees more after this and trims once at its end, so it takes
/// [`shrink_covers`] instead.
pub fn hand_back_covers() {
    shrink_covers();
    melodia_platform::services::platform::allocator::trim();
}

/// [`hand_back_covers`] without the trim, for a teardown that trims once when it is done.
pub fn shrink_covers() {
    tier().shrink_to_proxy(PROXY_COVER_DIM);
}

/// Retunes the tier to the display, after `app.show()` and on every resize. The cap and the size
/// are two halves of one budget.
pub fn tune_for_display(app: &AppWindow) {
    let size = cover_size_for_content(app);
    let cap = cover_cap_for_window(app, size);
    let grid_tier = tier();
    // A sidebar or queue panel ease calls this every frame, nearly always to the same answer.
    if grid_tier.capacity() == cap.get() && grid_tier.thumb_size() == size {
        return;
    }
    grid_tier.resize(cap);
    grid_tier.set_thumb_size(size);
    log::debug!("ui::grid_prewarm cover tier tuned to cap {cap}, {size} px");
}

/// The prewarm every grid runs before it publishes rows. Blocking; pass paths in display order.
pub fn prewarm(paths: &[PathBuf]) {
    tier().prewarm(paths);
}

/// Runs `warm` on the blocking pool and waits, so the hop that follows paints cache hits.
pub async fn prewarm_off_thread<G>(grid: &Arc<G>, warm: impl FnOnce(&G) + Send + 'static)
where
    G: Send + Sync + 'static,
{
    let grid = Arc::clone(grid);
    let _ = tokio::task::spawn_blocking(move || warm(&grid)).await;
}

/// Deduplicated, non-empty artwork paths in first-seen order, stopping at `cap` so a detail over a
/// huge entity doesn't allocate a path per track for `prewarm` to discard. Pass them in display
/// order, so the prefix that survives the cap is what paints first.
pub fn unique_artwork_paths<'a>(
    paths: impl Iterator<Item = Option<&'a str>>,
    cap: usize,
) -> Vec<PathBuf> {
    // Against the input too: a twelve-track album shouldn't lay out a cache's worth of buckets.
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
/// The count has to reach the cards actually mounted, or the overflow sits on placeholders;
/// measuring the window rather than the grid is the margin. The ceiling is on bytes rather than
/// entries, a buffer costing the square of `thumb_size`.
pub fn cover_cap(
    logical_w: u32,
    logical_h: u32,
    thumb_size: u32,
    fallback: NonZeroUsize,
) -> NonZeroUsize {
    /// The row pitch beside [`CARD_PITCH_W`]'s column one, the tile being square above its text.
    const ROW_PITCH_H: u32 = MIN_CARD_W + CARD_TEXT_H + GAP;
    const MIN_CAP: usize = 32;
    /// RGB8 bytes the tier may hold, at the largest tier's worst case. A leave shrinks to
    /// [`PROXY_COVER_DIM`], so only the on-screen case nears it.
    const MAX_TIER_BYTES: usize = 56 * 1024 * 1024;

    // Ceiling where [`widest_card`] floors: the cap has to cover what is mounted, so an
    // over-count is the safe direction here and the wrong one there.
    let cols = logical_w.div_ceil(CARD_PITCH_W).max(1);
    // `+ 1` for the partly visible row, the only scroll headroom.
    let rows = logical_h.div_ceil(ROW_PITCH_H) + 1;

    let side = usize::try_from(thumb_size.max(1)).unwrap_or(usize::MAX);
    let bytes_each = side.saturating_mul(side).saturating_mul(3).max(1);
    let affordable = (MAX_TIER_BYTES / bytes_each).max(MIN_CAP);

    let visible = usize::try_from(cols.saturating_mul(rows)).unwrap_or(affordable);
    let cap = visible.clamp(MIN_CAP, affordable);
    NonZeroUsize::new(cap).unwrap_or(fallback)
}

/// `GridGeometry`'s length defaults, which all five mounts take. Radio alone raises
/// `card-text-h`, which only makes rows taller than [`cover_cap`] counts: the safe direction.
const MIN_CARD_W: u32 = 180;
const GAP: u32 = 20;
const CARD_TEXT_H: u32 = 46;
/// The ceiling the component stops a card growing at.
const MAX_CARD_W: u32 = 224;

const CARD_PITCH_W: u32 = MIN_CARD_W + GAP;

/// Browse's `page-inset` at both edges, the most any grid page insets. Over-subtracting for the
/// other grids costs a step of tier size; under, their cards would upscale.
const PAGE_INSET_W: u32 = 2 * 16;

/// The size a tier is built at, before any geometry. Sized for a wide panel's card, for the run
/// where the deferred retune never schedules and this stays the live size.
pub const GRID_COVER_FALLBACK: u32 = 256;

/// Steps the derived size falls on. A size that moves re-decodes every drawn cover and is
/// re-derived on every resize, so it has to be flat across a drag: [`widest_card`] makes it flat,
/// and the step collapses the column counts a desktop passes through into a few tiers.
const SIZE_STEP: u32 = 32;

/// The widest card `GridGeometry` will draw into a content panel this wide.
///
/// The bound rather than the card, so the tier holds flat across a column band, and never under
/// the draw: `FemtoVG` minifies bilinear with no mipmaps. Capped at [`MAX_CARD_W`], which is the
/// one place it can land under the draw, an under-filled row holding the ceiling where the packing
/// stepped below it. Flooring instead would cost entries on the displays that mount the most cards.
fn widest_card(panel_w: u32) -> u32 {
    let body = panel_w.saturating_sub(PAGE_INSET_W).max(CARD_PITCH_W);
    let cols = (body.saturating_sub(GAP) / CARD_PITCH_W).max(1);
    (MIN_CARD_W + CARD_PITCH_W / cols).min(MAX_CARD_W)
}

/// Square decode size (px) for a card tile in a content panel `panel_w` logical px wide: the
/// physical size the card occupies, `widest_card × scale`. Clamped to [`STORE_MAX_DIM`], past which
/// a tier would upscale from a source the store already discarded.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped into `[MIN_CARD_W, STORE_MAX_DIM]` ahead of the cast"
)]
pub fn cover_size(panel_w: u32, scale: f64) -> u32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let physical = (f64::from(widest_card(panel_w)) * scale)
        .clamp(f64::from(MIN_CARD_W), f64::from(STORE_MAX_DIM)) as u32;
    physical.div_ceil(SIZE_STEP).saturating_mul(SIZE_STEP).min(STORE_MAX_DIM)
}

/// Grid decode size at the window's scale, off the panel's last reported width. Before the first
/// report the tier keeps the size it was built at.
pub fn cover_size_for_content(app: &AppWindow) -> u32 {
    let panel_w = f64::from(app.global::<WindowChrome>().get_content_panel_width());
    if panel_w.is_nan() || panel_w < 1.0 {
        return GRID_COVER_FALLBACK;
    }
    cover_size(whole_px(panel_w), f64::from(app.window().scale_factor()))
}

/// A laid-out logical width in whole pixels, saturating at `u32::MAX`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded and clamped into `u32`'s range ahead of the cast"
)]
fn whole_px(logical: f64) -> u32 {
    logical.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// A physical pixel extent and DPI scale as a logical extent, saturating like
/// `media::image::artwork::f64_to_pixel`.
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

/// A grid-cover cap off the window's logical size, each entry costed at `thumb_size`. After
/// `app.show()`: a zero extent means it ran early, and falls back. The window rather than the
/// monitor or the content panel, its surplus over the grid being the headroom.
pub fn cover_cap_for_window(app: &AppWindow, thumb_size: u32) -> NonZeroUsize {
    let window = app.window();
    let physical = window.size();
    if physical.width == 0 || physical.height == 0 {
        return GRID_COVER_CAP_FALLBACK;
    }
    let scale = f64::from(window.scale_factor());
    cover_cap(
        logical_dim(physical.width, scale),
        logical_dim(physical.height, scale),
        thumb_size,
        GRID_COVER_CAP_FALLBACK,
    )
}

/// One card's cover, never decoded on the calling thread: a miss is scheduled, and the page's
/// `covers-generation` re-runs the binding when it lands. Shared so no copy grows a decoding arm.
pub fn grid_cover(artwork_path: &str) -> Image {
    tier().get_or_schedule_opt(nonempty_artwork_path(artwork_path))
}

/// [`grid_cover`] for a surface with no generation to re-run its binding, so it decodes inline.
/// Only for bounded, prewarmed ones: Artist Detail's Albums strip and the Edit Artwork dialog.
pub fn grid_cover_blocking(artwork_path: &str) -> Image {
    tier().get_or_load_opt(nonempty_artwork_path(artwork_path))
}

/// Slint has no null string, so a row with no artwork carries `""`; the `*_opt` lookups take
/// `None` instead, so that case never reaches the decoder.
pub fn nonempty_artwork_path(artwork_path: &str) -> Option<&str> {
    Some(artwork_path).filter(|s| !s.is_empty())
}

#[cfg(test)]
#[path = "tests/grid_prewarm_tests.rs"]
mod tests;
