//! A fetched station logo, composed into the square opaque tile a card draws.
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

use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};

use crate::media::artwork::STORE_MAX_DIM;
use crate::media::image_decode::{FilterType, fit_within, resize_rgb8};

/// Alpha at or below which a pixel counts as transparent rather than faint. Above zero because an
/// exporter's fully-clear pixels are not always exactly clear, and a mark's own soft edge is the
/// thing that must not read as ground.
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

/// `decoded` as a square, fully opaque tile, or `None` where it already is one.
///
/// `None` is the common answer and the reason this is cheap: most of what a directory serves is
/// already a square opaque icon, and leaving it alone is what keeps `artwork::store_image`'s
/// byte-identical path.
///
/// **Blocking** — same contract as the resize behind it.
pub fn compose(decoded: &DynamicImage) -> Option<RgbImage> {
    let (width, height) = (decoded.width(), decoded.height());
    let square = width == height;
    if square && !decoded.color().has_alpha() {
        return None;
    }

    let source = decoded.to_rgba8();
    // A channel is not transparency: over half of what carries alpha never uses it, and those
    // want the untouched-bytes path as much as a plain JPEG does.
    if square && !source.pixels().copied().any(is_transparent) {
        return None;
    }

    let ground = ground_colour(&source);
    // Flattened before the downscale rather than after: resampling straight alpha against
    // undefined colour is where the halo around a mark's edge comes from.
    let flattened = DynamicImage::ImageRgb8(flatten(&source, ground)?);

    // The store would cap anything larger on the way in, so composing above it only builds a
    // buffer to throw away.
    let side = width.max(height).min(STORE_MAX_DIM);
    let (mark_w, mark_h) = fit_within(width, height, side, side);
    let mark = resize_rgb8(&flattened, mark_w, mark_h, FilterType::Lanczos3)?;

    let mut tile = RgbImage::from_pixel(side, side, ground);
    image::imageops::replace(
        &mut tile,
        &mark,
        i64::from((side - mark_w) / 2),
        i64::from((side - mark_h) / 2),
    );
    Some(tile)
}

fn is_transparent(pixel: Rgba<u8>) -> bool {
    pixel.0[3] <= TRANSPARENT_ALPHA
}

/// The colour the tile is padded and flattened with.
///
/// A source that came with its own ground keeps it, which is what makes a branded rectangle read
/// as the same tile it was: the ring is that ground wherever the logo is a rectangle rather than a
/// floating mark. Only when the ring is mostly clear is a neutral chosen instead.
fn ground_colour(source: &RgbaImage) -> Rgb<u8> {
    modal_opaque_ring(source).unwrap_or_else(|| {
        if mark_luma(source) > WHITE_GROUND_MAX_LUMA {
            GROUND_DARK
        } else {
            GROUND_LIGHT
        }
    })
}

/// The most common opaque colour around the source's outermost pixels, or `None` when too few of
/// them are opaque for the ring to be a ground at all.
fn modal_opaque_ring(source: &RgbaImage) -> Option<Rgb<u8>> {
    let (width, height) = source.dimensions();
    let mut tally: std::collections::HashMap<[u8; 3], u32> = std::collections::HashMap::new();
    let mut ring = 0u32;
    let mut opaque = 0u32;

    for (x, y, pixel) in source.enumerate_pixels() {
        if x != 0 && y != 0 && x + 1 != width && y + 1 != height {
            continue;
        }
        ring += 1;
        if is_transparent(*pixel) {
            continue;
        }
        opaque += 1;
        *tally.entry([pixel.0[0], pixel.0[1], pixel.0[2]]).or_default() += 1;
    }

    if ring == 0 || f64::from(opaque) < f64::from(ring) * OPAQUE_RING_FRACTION {
        return None;
    }
    tally.into_iter().max_by_key(|(_, count)| *count).map(|(rgb, _)| Rgb(rgb))
}

/// Rec. 709 luma of the mark alone. The clear field carries no colour, and averaging it in as
/// black is how a light mark ends up asking for a light ground.
fn mark_luma(source: &RgbaImage) -> f64 {
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
    if counted == 0 {
        0.0
    } else {
        sum / f64::from(counted)
    }
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
