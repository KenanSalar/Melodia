//! Hero stats + live cover mosaic refresh.
//!
//! On every `library_changed_tx` tick the Favorites view is visible
//! for, this re-fetches `library::favorites::get_favorite_stats` and
//! produces a fresh hero blur from the top-4 most-played covers. The
//! blur is composed by tiling the source covers into a 2×2 atlas,
//! downscaling to the now-playing-tier `BLUR_DOWNSCALE` (192 px), and
//! running `image::imageops::fast_blur` (parity with
//! `now_playing_artwork::decode_artwork`). Write goes through
//! `write_crossfade_slot` so switching mosaics fades the previous blur
//! to the new one — outgoing slot stays painted for the full fade so
//! the hero never flashes empty.

use std::path::Path;
use std::sync::Arc;

use image::imageops::fast_blur;
use slint::{ComponentHandle, Image, Model, Rgb8Pixel, SharedPixelBuffer, SharedString, VecModel,
    Weak};

use super::FavoritesUi;
use crate::entities::track::FavoriteStats;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::now_playing::write_crossfade_slot;
use crate::{AppWindow, Favorites};

/// Atlas + blur target size. Matches `now_playing_artwork::BLUR_DOWNSCALE`
/// so the GPU pipeline / cache pressure stays consistent across blur
/// surfaces. Bigger than this gains nothing because the surface is
/// `image-fit: cover`-stretched.
const BLUR_TARGET: u32 = 192;

/// `fast_blur` sigma. Mirrors `now_playing_artwork::BLUR_SIGMA` so the
/// Favorites hero blur reads the same as the Now Playing backdrop.
const BLUR_SIGMA: f32 = 24.0;

/// Per-tile source decode cap before atlasing. Each tile is a quarter
/// of the atlas, so decoding above this is wasted work. Stays well
/// below the artwork hard cap so a forged dimension header in a tag
/// can't trigger an absurd allocation. Same hard cap as
/// `now_playing_artwork::MAX_SOURCE_DIM` / `cover_thumbs::MAX_DIM`.
const MAX_SOURCE_DIM: u32 = 8192;
const PER_TILE: u32 = BLUR_TARGET / 2;

/// Fetch fresh stats, push the count + duration text + mosaic paths
/// into `Favorites`, then kick a blocking composition+blur task whose
/// result lands on the UI thread via `upgrade_in_event_loop`. The
/// blur step is gated by `animate` — `true` for live refreshes (the
/// user is looking at the page), `false` for the seed-on-section-enter
/// case where we want the new state already in place when the view
/// becomes visible.
pub async fn refresh_hero(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    animate: bool,
) -> AppResult<()> {
    let stats = library::favorites::get_favorite_stats(state).await?;
    *fav_ui.state().stats.lock() = stats.clone();
    push_stats_to_slint(&stats, weak);

    let paths = stats.artwork_paths.clone();
    if paths.is_empty() {
        clear_hero_blur(weak);
        return Ok(());
    }

    // Composition + blur is CPU-bound — runs on the blocking pool to
    // keep the tokio worker free. Returns the raw `SharedPixelBuffer`
    // so it can cross the `upgrade_in_event_loop` boundary (`slint::
    // Image` is `!Send` so the wrap happens on the UI thread).
    let blur_buf = state
        .runtime
        .spawn_blocking(move || compose_mosaic_blur(&paths))
        .await
        .ok()
        .flatten();

    apply_hero_blur(weak, blur_buf, animate);
    Ok(())
}

fn push_stats_to_slint(stats: &FavoriteStats, weak: &Weak<AppWindow>) {
    let count = i32::try_from(stats.count).unwrap_or(i32::MAX);
    let duration = crate::ui::tracks::format_duration_ms(stats.total_duration_ms);
    let paths = stats.artwork_paths.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        g.set_track_count(count);
        g.set_duration_text(SharedString::from(duration));
        let model = g.get_mosaic_paths();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<SharedString>>() else {
            log::warn!("Favorites.mosaic-paths: VecModel<SharedString> downcast failed");
            return;
        };
        let rendered: Vec<SharedString> =
            paths.iter().map(|p| SharedString::from(p.as_str())).collect();
        vec.set_vec(rendered);
    });
}

fn clear_hero_blur(weak: &Weak<AppWindow>) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        // `None` through write_crossfade_slot clears `has-blur` without
        // wiping the previous slot, so any in-flight fade-out completes
        // naturally before the gradient floor takes over.
        write_crossfade_slot(
            None,
            true,
            g.get_blur_use_a(),
            |img| g.set_blur_img_a(img),
            |img| g.set_blur_img_b(img),
            |v| g.set_blur_use_a(v),
            |v| g.set_has_blur(v),
        );
    });
}

fn apply_hero_blur(
    weak: &Weak<AppWindow>,
    buf: Option<SharedPixelBuffer<Rgb8Pixel>>,
    animate: bool,
) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        let img = buf.map(Image::from_rgb8);
        write_crossfade_slot(
            img,
            animate,
            g.get_blur_use_a(),
            |i| g.set_blur_img_a(i),
            |i| g.set_blur_img_b(i),
            |v| g.set_blur_use_a(v),
            |v| g.set_has_blur(v),
        );
    });
}

/// Compose up to 4 source images into a `BLUR_TARGET × BLUR_TARGET`
/// 2×2 atlas, then blur. Mirrors the picker's mosaic layout for the
/// 4-tile case; the 1 / 2 / 3 / 0 cases fall back to "fill the whole
/// atlas with the available tiles" so a partially-populated mosaic
/// still produces a usable hero backdrop. Runs on the blocking pool.
///
/// Returns `None` when every source decode failed — the caller clears
/// `has-blur` so the accent gradient floor shows through.
fn compose_mosaic_blur(paths: &[String]) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
    use image::{ImageBuffer, RgbImage};

    if paths.is_empty() {
        return None;
    }

    let mut atlas: RgbImage = ImageBuffer::new(BLUR_TARGET, BLUR_TARGET);

    // Decode each source (capped at 4) into a per-tile thumbnail.
    let mut tiles: Vec<RgbImage> = Vec::with_capacity(4);
    for p in paths.iter().take(4) {
        if let Some(tile) = decode_tile(Path::new(p)) {
            tiles.push(tile);
        }
    }
    if tiles.is_empty() {
        return None;
    }

    // Lay tiles into the atlas. Layouts mirror the CoverMosaic
    // component for visual parity with the foreground mosaic:
    //   1 tile  → fill whole atlas
    //   2 tiles → left / right halves
    //   3 tiles → left full-height + right column split top/bottom
    //   4 tiles → 2×2 grid
    // The atlas is then heavily blurred so the exact layout is
    // imperceptible — but the tile-count branching keeps colour
    // distribution consistent with the visible mosaic above.
    match tiles.len() {
        1 => {
            blit(&mut atlas, &tiles[0], 0, 0, BLUR_TARGET, BLUR_TARGET);
        }
        2 => {
            blit(&mut atlas, &tiles[0], 0, 0, PER_TILE, BLUR_TARGET);
            blit(&mut atlas, &tiles[1], PER_TILE, 0, PER_TILE, BLUR_TARGET);
        }
        3 => {
            blit(&mut atlas, &tiles[0], 0, 0, PER_TILE, BLUR_TARGET);
            blit(&mut atlas, &tiles[1], PER_TILE, 0, PER_TILE, PER_TILE);
            blit(&mut atlas, &tiles[2], PER_TILE, PER_TILE, PER_TILE, PER_TILE);
        }
        _ => {
            blit(&mut atlas, &tiles[0], 0, 0, PER_TILE, PER_TILE);
            blit(&mut atlas, &tiles[1], PER_TILE, 0, PER_TILE, PER_TILE);
            blit(&mut atlas, &tiles[2], 0, PER_TILE, PER_TILE, PER_TILE);
            blit(&mut atlas, &tiles[3], PER_TILE, PER_TILE, PER_TILE, PER_TILE);
        }
    }

    let blurred = fast_blur(&atlas, BLUR_SIGMA);
    Some(buffer_from_rgb(&blurred))
}

/// Decode one cover at its tile size. Bounded so a forged header can't
/// allocate gigabytes before the downscale kicks in.
fn decode_tile(path: &Path) -> Option<image::RgbImage> {
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    Some(decoded.thumbnail_exact(BLUR_TARGET, BLUR_TARGET).to_rgb8())
}

/// Stretch-copy `src` into `dst` at the given destination rectangle.
/// `src` is already a square `BLUR_TARGET × BLUR_TARGET` thumbnail
/// (`decode_tile`'s output), so the per-tile blit is a sub-block of
/// the larger atlas; sampling is nearest-neighbour because the blur
/// pass that immediately follows obliterates any aliasing.
fn blit(
    dst: &mut image::RgbImage,
    src: &image::RgbImage,
    dx: u32,
    dy: u32,
    dw: u32,
    dh: u32,
) {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 {
        return;
    }
    for y in 0..dh {
        for x in 0..dw {
            let sx = x * sw / dw;
            let sy = y * sh / dh;
            let px = *src.get_pixel(sx, sy);
            dst.put_pixel(dx + x, dy + y, px);
        }
    }
}

fn buffer_from_rgb(img: &image::RgbImage) -> SharedPixelBuffer<Rgb8Pixel> {
    let (w, h) = img.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(img.as_raw());
    buf
}
