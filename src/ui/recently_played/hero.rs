//! Hero stats + live cover mosaic/blur for the Recently-Played view.
//!
//! Unlike the Favorites hero (which re-queries `get_favorite_stats`), the data
//! here is derived from the already-fetched recency rows: the mosaic is the
//! up-to-4 most-recently-played *distinct* covers, and the count/duration are
//! summed off the same rows. The blur backdrop reuses the shared dual-slot
//! cross-fade (`write_crossfade_slot`) and the same atlas+blur recipe as
//! `favorites::hero` so both surfaces read identically.

use std::collections::HashSet;
use std::path::Path;

use image::imageops::fast_blur;
use slint::{
    ComponentHandle, Image, Model, Rgb8Pixel, SharedPixelBuffer, SharedString, VecModel, Weak,
};

use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::state::AppState;
use crate::ui::now_playing::write_crossfade_slot;
use crate::ui::tracks::format_duration_ms;
use crate::{AppWindow, RecentlyPlayed};

/// Atlas + blur target size. Matches `favorites::hero::BLUR_TARGET` so the GPU
/// pipeline / cache pressure stays consistent across blur surfaces.
const BLUR_TARGET: u32 = 192;
/// `fast_blur` sigma — same as the Favorites hero / Now Playing backdrop.
const BLUR_SIGMA: f32 = 24.0;
/// Per-tile source decode cap before atlasing.
const MAX_SOURCE_DIM: u32 = 8192;
const PER_TILE: u32 = BLUR_TARGET / 2;

/// The up-to-`n` most-recently-played *distinct* cover paths, in recency
/// order — the hero mosaic tiles. Skips empty/absent artwork.
pub fn mosaic_paths_from(rows: &[RsTrackListRow], n: usize) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(n);
    for r in rows {
        let Some(p) = r.artwork_path.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        if seen.insert(p) {
            out.push(p.to_owned());
            if out.len() == n {
                break;
            }
        }
    }
    out
}

/// Push the hero count + total-duration text + mosaic-path list into the Slint
/// global. Immediate (the blur composition is kicked separately).
pub fn push_hero_stats(count: i32, total_ms: i64, mosaic_paths: &[String], weak: &Weak<AppWindow>) {
    let duration = format_duration_ms(total_ms);
    let paths = mosaic_paths.to_vec();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<RecentlyPlayed>();
        g.set_track_count(count);
        g.set_duration_text(SharedString::from(duration.as_str()));
        let model = g.get_mosaic_paths();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<SharedString>>() else {
            log::warn!("RecentlyPlayed.mosaic-paths: VecModel<SharedString> downcast failed");
            return;
        };
        let rendered: Vec<SharedString> =
            paths.iter().map(|p| SharedString::from(p.as_str())).collect();
        vec.set_vec(rendered);
    });
}

/// Compose + apply the hero blur from `mosaic_paths` (or clear it when empty).
/// CPU-bound composition runs on the blocking pool; the result lands on the UI
/// thread. `animate` fades the cross-fade (true for live refreshes).
pub async fn refresh_blur(
    state: &AppState,
    mosaic_paths: Vec<String>,
    weak: &Weak<AppWindow>,
    animate: bool,
) {
    if mosaic_paths.is_empty() {
        clear_hero_blur(weak);
        return;
    }
    let blur_buf = state
        .runtime
        .spawn_blocking(move || compose_mosaic_blur(&mosaic_paths))
        .await
        .ok()
        .flatten();
    apply_hero_blur(weak, blur_buf, animate);
}

/// Clear the hero blur (e.g. no covers) without wiping the previous slot, so an
/// in-flight fade completes before the gradient floor takes over.
pub fn clear_hero_blur(weak: &Weak<AppWindow>) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<RecentlyPlayed>();
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

fn apply_hero_blur(weak: &Weak<AppWindow>, buf: Option<SharedPixelBuffer<Rgb8Pixel>>, animate: bool) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<RecentlyPlayed>();
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

/// Compose up to 4 covers into a `BLUR_TARGET × BLUR_TARGET` 2×2 atlas, then
/// blur. Mirrors `favorites::hero::compose_mosaic_blur` (same 1/2/3/4 layouts).
/// Runs on the blocking pool. `None` when every decode failed.
fn compose_mosaic_blur(paths: &[String]) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
    use image::{ImageBuffer, RgbImage};

    if paths.is_empty() {
        return None;
    }

    let mut atlas: RgbImage = ImageBuffer::new(BLUR_TARGET, BLUR_TARGET);

    let mut tiles: Vec<RgbImage> = Vec::with_capacity(4);
    for p in paths.iter().take(4) {
        if let Some(tile) = decode_tile(Path::new(p)) {
            tiles.push(tile);
        }
    }
    if tiles.is_empty() {
        return None;
    }

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

fn decode_tile(path: &Path) -> Option<image::RgbImage> {
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    Some(decoded.thumbnail_exact(BLUR_TARGET, BLUR_TARGET).to_rgb8())
}

fn blit(dst: &mut image::RgbImage, src: &image::RgbImage, dx: u32, dy: u32, dw: u32, dh: u32) {
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
