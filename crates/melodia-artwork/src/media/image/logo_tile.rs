//! A fetched station logo, composed into the square opaque tile a card draws and filed in the
//! radio-logo store.
//!
//! **A directory's logo field is a favicon field**, so what arrives is whatever a browser tab
//! wanted, and only one of the three shapes it comes in survives the card unchanged. A wide
//! wordmark is stretched: `cover_thumbs` resizes into a square buffer aspect-blind, and the
//! `image-fit: cover` over it has nothing left to correct. A transparent mark is worse than
//! stretched — every tier below is RGB8, so the channel is dropped rather than composited and
//! the mark lands on whatever bytes sat under the transparency, black about as often as white.
//!
//! Both are answered **once, at fetch**, so the store holds a square the rest of the app can
//! treat as ordinary artwork and no tier needs a mode. That is also what the directories whose
//! logos look deliberate do: they serve a pre-composed square, not the station's own icon.

use std::path::Path;

use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
use melodia_core::entities::radio::StoredLogo;

use crate::media::image::artwork::{self, STORE_MAX_DIM};
use crate::media::image::image_decode::{self, FilterType, fit_within, resize_rgb8};

/// Shortest edge a source may have and still be worth storing.
///
/// Set at favicon size rather than at what the surfaces would like. A grid card and a hero tile
/// are both hundreds of logical pixels wide and `FemtoVG` magnifies bilinear with no mipmaps, so a
/// source down here is visibly soft. But a station's logo field *is* a favicon field, and holding
/// out for something drawable mostly bought the glyph instead. What it still keeps out is the 1×1
/// pixel that turns up where the field was filled in by a tracker. One gate at the writer rather
/// than one per surface, so nothing this small enters the store for a later tier to reject.
const MIN_LOGO_DIM: u32 = 32;

/// Alpha at or below which a pixel counts as transparent rather than faint. Above zero because an
/// exporter's fully-clear pixels are not always exactly clear, and a mark's own soft edge is the
/// thing that must not read as ground.
///
/// Answers where the *ground* comes from, never whether to composite at all — [`is_opaque`] owns
/// that, and it has to be exact.
const TRANSPARENT_ALPHA: u8 = 8;

/// How much of the border ring must be opaque before the ring is taken to name the source's own
/// ground. Short of 1.0 so a rounded-corner icon still answers with its fill rather than falling
/// through to a neutral.
const OPAQUE_RING_FRACTION: f64 = 0.75;

/// Mark luma past which white stops working and the dark ground is taken instead: the grey of
/// this byte is where WCAG's 3:1 non-text ratio against white runs out. **White is the default and
/// the threshold is deliberately high** — a mark drawn for transparency was drawn for a web page,
/// so it is a light mark that needs rescuing, not a mid-tone one. A midpoint would send half of
/// them to a ground they were never designed for.
///
/// Rec. 709 luma stands in for that grey's relative luminance rather than deriving it: this
/// chooses between two constants, where `ui::backdrop` solves a scrim against a target and does
/// the transfer function properly.
const WHITE_GROUND_MAX_LUMA: f64 = 149.0;

/// The two grounds a floating mark is laid on. Neutral rather than drawn from the mark: a ground
/// carrying its own hue argues with every colour in a multi-colour logo, and the one directory
/// worth copying pads white far more often than it pads with a brand colour.
const GROUND_LIGHT: Rgb<u8> = Rgb([0xff, 0xff, 0xff]);
const GROUND_DARK: Rgb<u8> = Rgb([0x1a, 0x1a, 0x1a]);

/// `bytes` filed in the store under `dir` as `compose` leaves them, or `None` where nothing
/// drawable arrived.
///
/// The floor is asked first and off the header alone, so a source too small to draw costs no
/// decode at all. `extension` names the source's own bytes; a composed tile is always PNG.
///
/// **Blocking**, like `compose`.
pub fn store(bytes: &[u8], extension: &str, dir: &Path) -> Option<StoredLogo> {
    let (width, height) = image_decode::memory_dimensions(bytes, image_decode::MAX_SOURCE_DIM)?;
    if width < MIN_LOGO_DIM || height < MIN_LOGO_DIM {
        return None;
    }
    // Held to the end rather than around the decode: `compose` flattens at full source resolution
    // before it downscales, and the store decodes again to bound an oversized source. The
    // download's byte cap is no substitute: it bounds the *compressed* size, which a flat-colour
    // icon stays under at any dimension, and the fetch window runs several at once.
    let _oversized = image_decode::large_decode_guard(u64::from(width) * u64::from(height));
    // `None` is "store the source's own bytes": it is already a square opaque icon, or the tile
    // it wanted would not encode and the source is the better of the two.
    let tile = match tile_for(bytes) {
        Tile::Composed(tile) => encoded_png(tile),
        Tile::SourceIsFine => None,
        // Refused rather than stored untreated. The caller reads `None` as "nothing usable came
        // back" and the card falls back to its monogram, where the source paints an empty square.
        Tile::Undrawable => return None,
    };

    let path = match tile.as_deref() {
        Some(tile) => artwork::store_image(tile, "png", dir),
        None => artwork::store_image(bytes, extension, dir),
    }?;
    // A file the store just wrote or already had; a stat that fails says nothing about whether it
    // is drawable, so the answer stands and the size falls back to what arrived.
    let stored = std::fs::metadata(&path).map_or(bytes.len() as u64, |meta| meta.len());
    Some(StoredLogo { path, bytes: stored })
}

/// What [`compose`] makes of the source, past the decode it needs.
///
/// A source the decoder refuses is [`Tile::SourceIsFine`] rather than [`Tile::Undrawable`]: the
/// header read in [`store`] already says the store can hold it, and with no decode there is no
/// evidence it wanted a tile at all.
fn tile_for(bytes: &[u8]) -> Tile {
    match image_decode::decode_memory_capped(bytes, image_decode::MAX_SOURCE_DIM) {
        Some(decoded) => compose(decoded),
        None => Tile::SourceIsFine,
    }
}

/// A composed tile as PNG rather than the store's JPEG.
///
/// A tile is flat ground behind a hard-edged mark, which is what JPEG rings around and what PNG
/// holds in a few kilobytes; `store_image` re-encodes anyway for the rare composite that lands
/// over its byte bound.
fn encoded_png(tile: RgbImage) -> Option<Vec<u8>> {
    let mut encoded = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut encoded);
    DynamicImage::ImageRgb8(tile).write_with_encoder(encoder).ok()?;
    Some(encoded)
}

/// What [`compose`] decided about a source.
///
/// **Three answers, not two.** "Already fine" and "nothing worth storing" both come back from
/// [`compose`] with no tile in hand and mean opposite things to the caller: one keeps the source's
/// own bytes, the other has to refuse the logo entirely so the card draws its monogram. Folded into
/// one `None` they were indistinguishable, and a fully transparent source went into the store to be
/// painted as an empty tile.
enum Tile {
    /// Composed into a square opaque tile.
    Composed(RgbImage),
    /// Already a square opaque source. Keep its bytes, which is what preserves
    /// `artwork::store_image`'s byte-identical path.
    SourceIsFine,
    /// The source needs a tile and none could be built from it — no opaque pixel to take a ground
    /// from, so it would be flat ground and nothing else. Refuse it: storing the untreated source
    /// paints exactly the empty square this module exists to prevent.
    Undrawable,
}

/// `decoded` as a square, fully opaque tile, or the reason there isn't one.
///
/// [`Tile::SourceIsFine`] is the common answer and the reason this is cheap: most of what a
/// directory serves is already a square opaque icon.
///
/// **Blocking** — same contract as the resize behind it.
///
/// Takes the decode by value: it is the caller's last use of it, and `into_rgba8` then costs
/// nothing on a source that already is one, where `to_rgba8` copies the whole buffer.
fn compose(decoded: DynamicImage) -> Tile {
    let (width, height) = (decoded.width(), decoded.height());
    let square = width == height;
    if square && !decoded.color().has_alpha() {
        return Tile::SourceIsFine;
    }

    let source = decoded.into_rgba8();
    // A channel is not transparency: over half of what carries alpha never uses it, and those want
    // the untouched-bytes path as much as a plain JPEG does. Exact rather than
    // `TRANSPARENT_ALPHA`'s fudge — a tier below drops the channel instead of compositing it, so
    // anything short of fully opaque still has to be flattened here.
    if square && source.pixels().copied().all(is_opaque) {
        return Tile::SourceIsFine;
    }

    let Some(ground) = ground_colour(&source) else {
        return Tile::Undrawable;
    };
    // Flattened before the downscale rather than after: resampling straight alpha against
    // undefined colour is where the halo around a mark's edge comes from.
    let Some(flattened) = flatten(&source, ground).map(DynamicImage::ImageRgb8) else {
        return Tile::Undrawable;
    };

    // The store would cap anything larger on the way in, so composing above it only builds a
    // buffer to throw away.
    let side = width.max(height).min(STORE_MAX_DIM);
    let (mark_w, mark_h) = fit_within(width, height, side, side);
    let Some(mark) = resize_rgb8(&flattened, mark_w, mark_h, FilterType::Lanczos3) else {
        return Tile::Undrawable;
    };

    let mut tile = RgbImage::from_pixel(side, side, ground);
    image::imageops::replace(
        &mut tile,
        &mark,
        i64::from((side - mark_w) / 2),
        i64::from((side - mark_h) / 2),
    );
    Tile::Composed(tile)
}

fn is_transparent(pixel: Rgba<u8>) -> bool {
    pixel.0[3] <= TRANSPARENT_ALPHA
}

fn is_opaque(pixel: Rgba<u8>) -> bool {
    pixel.0[3] == u8::MAX
}

/// The colour the tile is padded and flattened with, or `None` where the source has no opaque
/// pixel to take one from.
///
/// A source that came with its own ground keeps it, which is what makes a branded rectangle read
/// as the same tile it was: the ring is that ground wherever the logo is a rectangle rather than a
/// floating mark. Only when the ring is mostly clear is a neutral chosen instead.
fn ground_colour(source: &RgbaImage) -> Option<Rgb<u8>> {
    if let Some(ring) = modal_opaque_ring(source) {
        return Some(ring);
    }
    let luma = mark_luma(source)?;
    Some(if luma > WHITE_GROUND_MAX_LUMA { GROUND_DARK } else { GROUND_LIGHT })
}

/// The most common opaque colour around the source's outermost pixels, or `None` when too few of
/// them are opaque for the ring to be a ground at all.
///
/// Ties break on the colour itself. `HashMap` iteration order varies per process, so a ring of all
/// distinct colours — a photo behind the byte cap — would otherwise pad differently on every fetch,
/// and the store being content-addressed on the composed bytes means that lands as a new file each
/// time rather than deduplicating.
fn modal_opaque_ring(source: &RgbaImage) -> Option<Rgb<u8>> {
    let (width, height) = source.dimensions();
    let mut tally: std::collections::HashMap<[u8; 3], u32> = std::collections::HashMap::new();
    let mut ring = 0u32;
    let mut opaque = 0u32;

    let mut tally_pixel = |pixel: &Rgba<u8>| {
        ring += 1;
        if is_transparent(*pixel) {
            return;
        }
        opaque += 1;
        *tally.entry([pixel.0[0], pixel.0[1], pixel.0[2]]).or_default() += 1;
    };

    // The border alone, rather than every pixel with the interior skipped: the ring is O(w + h) of
    // an O(w · h) buffer, and this runs on the decode pool once per composed logo.
    for x in 0..width {
        tally_pixel(source.get_pixel(x, 0));
        if height > 1 {
            tally_pixel(source.get_pixel(x, height - 1));
        }
    }
    for y in 1..height.saturating_sub(1) {
        tally_pixel(source.get_pixel(0, y));
        if width > 1 {
            tally_pixel(source.get_pixel(width - 1, y));
        }
    }

    if ring == 0 || f64::from(opaque) < f64::from(ring) * OPAQUE_RING_FRACTION {
        return None;
    }
    tally.into_iter().max_by_key(|(rgb, count)| (*count, *rgb)).map(|(rgb, _)| Rgb(rgb))
}

/// Rec. 709 luma of the mark alone, or `None` where there is no mark. The clear field carries no
/// colour, and averaging it in as black is how a light mark ends up asking for a light ground.
fn mark_luma(source: &RgbaImage) -> Option<f64> {
    let mut sum = 0f64;
    let mut counted = 0u32;
    for pixel in source.pixels() {
        if is_transparent(*pixel) {
            continue;
        }
        let [r, g, b, _] = pixel.0;
        sum +=
            0.0722f64.mul_add(f64::from(b), 0.2126f64.mul_add(f64::from(r), 0.7152 * f64::from(g)));
        counted += 1;
    }
    (counted > 0).then(|| sum / f64::from(counted))
}

/// `source` composited over a flat `ground`, in gamma space as the renderer would have done it.
fn flatten(source: &RgbaImage, ground: Rgb<u8>) -> Option<RgbImage> {
    let mut out = Vec::with_capacity(source.len() / 4 * 3);
    for pixel in source.chunks_exact(4) {
        for (channel, under) in ground.0.into_iter().enumerate() {
            out.push(over(pixel[channel], pixel[3], under));
        }
    }
    RgbImage::from_raw(source.width(), source.height(), out)
}

/// One channel of `top` at `alpha` over `under`. Rounded half-up rather than truncated, so a fully
/// opaque pixel comes back as itself.
fn over(top: u8, alpha: u8, under: u8) -> u8 {
    let alpha = u32::from(alpha);
    let blended = u32::from(top) * alpha + u32::from(under) * (255 - alpha);
    u8::try_from((blended + 127) / 255).unwrap_or(u8::MAX)
}

#[cfg(test)]
#[path = "tests/logo_tile_tests.rs"]
mod tests;
