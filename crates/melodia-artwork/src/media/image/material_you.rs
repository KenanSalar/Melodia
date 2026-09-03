//! Material You — Material 3 dynamic colour generation from album artwork,
//! over the [`material-colors`](https://docs.rs/material-colors) crate.
//!
//! Decode and downscale to a fixed quantizer input, dropping the full-resolution
//! buffer before quantization; take the seed off the clusters
//! ([`seed_from_pixels`] argues the fallback); build a `DynamicScheme` of the
//! requested style and map the M3 roles into a [`crate::themes::Palette`].
//!
//! **[`population_seeds`] answers to none of that**, and is here because this is
//! where a cover becomes colours: the backdrop asks what the artwork mostly *is*
//! where everything else asks what makes the best UI seed, hence a second
//! quantizer.
//!
//! All public functions are sync and may block — call from `spawn_blocking`.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use color_thief::ColorFormat;
use image::imageops::FilterType;
use lru::LruCache;
use material_colors::color::Argb;
use material_colors::hct::Hct;
use material_colors::quantize::{Quantizer, QuantizerCelebi};
use material_colors::scheme::variant::{
    SchemeContent, SchemeExpressive, SchemeFidelity, SchemeMonochrome, SchemeNeutral,
    SchemeTonalSpot, SchemeVibrant,
};
use material_colors::score::Score;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::media::image::image_decode::decode_capped;
use crate::themes::{Palette, material3};

/// The seven Material 3 scheme variants, plus a `None` that disables Material
/// You and falls back to the static M3 palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemeStyle {
    None,
    TonalSpot,
    Content,
    Vibrant,
    Expressive,
    Fidelity,
    Neutral,
    Monochrome,
}

impl SchemeStyle {
    /// The id persisted in `settings.json`.
    pub fn as_id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::TonalSpot => "tonal_spot",
            Self::Content => "content",
            Self::Vibrant => "vibrant",
            Self::Expressive => "expressive",
            Self::Fidelity => "fidelity",
            Self::Neutral => "neutral",
            Self::Monochrome => "monochrome",
        }
    }

    /// Round-trips with [`Self::as_id`]; unknown ids resolve to `None` so
    /// stale settings can't break startup.
    pub fn from_id(s: &str) -> Self {
        match s {
            "tonal_spot" => Self::TonalSpot,
            "content" => Self::Content,
            "vibrant" => Self::Vibrant,
            "expressive" => Self::Expressive,
            "fidelity" => Self::Fidelity,
            "neutral" => Self::Neutral,
            "monochrome" => Self::Monochrome,
            _ => Self::None,
        }
    }

    /// Display order for the Settings chip group.
    pub fn all() -> &'static [Self] {
        &[
            Self::None,
            Self::TonalSpot,
            Self::Content,
            Self::Vibrant,
            Self::Expressive,
            Self::Fidelity,
            Self::Neutral,
            Self::Monochrome,
        ]
    }
}

/// The square the source is down-sampled to before quantization — enough for a
/// seed off album art, and cheap.
const QUANTIZE_DIM: u32 = 64;

/// Cluster centres handed to `QuantizerCelebi`. Lower muddies the scoring pass,
/// higher wastes CPU.
const QUANTIZE_MAX_COLOURS: usize = 128;

/// Defensive source cap for the path-based [`extract_source_argb`]. Real album
/// art is well under it; the limit stops an oversized embedded picture spiking a
/// transient decode buffer the glibc heap then keeps reserved. The current-track
/// path goes through `CoverThumbs` instead, so this only fires when a caller
/// exercises the path-based variant directly.
const MATERIAL_YOU_MAX_SOURCE_DIM: u32 = 2048;

/// Decode `artwork_path`, downscale, and take the seed off it via
/// [`seed_from_pixels`]. `None` when the decode failed or left nothing opaque.
/// **Blocking** — call from `spawn_blocking` only.
pub fn extract_source_argb(artwork_path: &Path) -> Option<u32> {
    let decoded = match decode_capped(artwork_path, MATERIAL_YOU_MAX_SOURCE_DIM) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("material_you: decode {}: {e}", artwork_path.display());
            return None;
        }
    };

    // Resize before converting, so the small buffer is the only pixel data kept
    // past this point. `Triangle` is the sweet spot — good enough for colour
    // quantization, far cheaper than Lanczos3, and much better than Nearest.
    let small = decoded.resize_exact(QUANTIZE_DIM, QUANTIZE_DIM, FilterType::Triangle).into_rgba8();
    drop(decoded);

    let mut pixels: Vec<Argb> = Vec::with_capacity((QUANTIZE_DIM * QUANTIZE_DIM) as usize);
    for chunk in small.chunks_exact(4) {
        let r = chunk[0];
        let g = chunk[1];
        let b = chunk[2];
        let a = chunk[3];
        // Skip near-transparent pixels, so alpha can't bias the seed toward
        // whatever bleeds through the edges.
        if a < 128 {
            continue;
        }
        pixels.push(Argb::new(0xff, r, g, b));
    }

    seed_from_pixels(&pixels)
}

/// Quantize `pixels` and score the best UI seed out of the resulting clusters.
///
/// `Score` discards every cluster under its own chroma cutoff and answers Google Blue when nothing
/// survives — a brand colour, not a fact about this artwork, so a greyscale sleeve gets vivid blue
/// chrome. The *dominant* cluster is such a fact: what the image mostly is, whatever its chroma.
/// Handed over as the fallback it leaves a colourless cover seeding its own grey, and being the
/// only fallback named here it keeps the crate's out of reach.
///
/// **It reaches the app-wide palette too, not just the backdrop**, so which
/// Color Style is picked decides how visible that is: the four variants that
/// force their own primary chroma still land blue-ish, a neutral sRGB grey
/// having the white point's CAM16 hue, while `Content` and `Fidelity` take the
/// source's chroma and theme the app near-neutral. That is the faithful answer,
/// and the one Android gives a greyscale wallpaper — but unlike
/// `ui::backdrop`'s tiers there is no tone band downstream to floor it.
///
/// `None` means the quantizer had nothing to say, which both consumers already
/// spell as the theme accent. So there is no last-resort colour to pick here,
/// and no layer-crossing theme dependency to acquire in order to pick one.
fn seed_from_pixels(pixels: &[Argb]) -> Option<u32> {
    if pixels.is_empty() {
        return None;
    }
    let counts = QuantizerCelebi::quantize(pixels, QUANTIZE_MAX_COLOURS).color_to_count;
    let dominant = counts.iter().max_by_key(|(_, count)| **count).map(|(argb, _)| *argb)?;
    Score::score(&counts, Some(1), Some(dominant), None).into_iter().next().map(argb_to_u32)
}

/// [`extract_source_argb`]'s pipeline starting from the RGB8 thumbnail
/// [`crate::media::image::cover_thumbs::CoverThumbs`] already keeps for row rendering.
/// Small enough that quantizing it directly is qualitatively the same as
/// quantizing a downsample of the source, and it skips the transient decode peak
/// the path-based variant produces on large embedded artwork.
///
/// **Blocking** (CPU-bound quantize) — call from `spawn_blocking`.
pub fn extract_source_argb_from_rgb8(buf: &SharedPixelBuffer<Rgb8Pixel>) -> Option<u32> {
    let bytes = buf.as_bytes();
    // No alpha here, so the path-based sibling's alpha-skip collapses to a plain push.
    let mut pixels: Vec<Argb> = Vec::with_capacity(bytes.len() / 3);
    for chunk in bytes.chunks_exact(3) {
        pixels.push(Argb::new(0xff, chunk[0], chunk[1], chunk[2]));
    }
    seed_from_pixels(&pixels)
}

/// Up to `desired` colours the cover is mostly *made of*, most prominent first.
///
/// [`seed_from_pixels`]'s opposite number, and the one the backdrop washes across its surface.
/// `Score` ranks by how *usable* a colour is: on a photographic sleeve that picks the reddest thing
/// in the frame over what the frame mostly is, and on a greyscale one its chroma cutoff answers
/// **once**, leaving a backdrop to invent three of its four washes. Median cut has no cutoff and
/// asks the other question.
///
/// **A short list is rare but real, so the caller still owes a filling rule**: a near-white sleeve
/// drops to nothing — the crate discards every pixel over 250 on all three channels — and comes
/// back as a single white. Anything else fills the list, median cut splitting past the point where
/// a box holds one colour.
///
/// Reads tightly-packed RGB8 in place, with no intermediate pixel vector, at quality 1 — the finest
/// stride on offer and **not** every pixel, the crate advancing by `bytes-per-pixel × quality`.
///
/// **Blocking** — cheap, but it belongs on the worker that decoded the cover either way.
pub fn population_seeds(rgb: &[u8], desired: usize) -> Vec<u32> {
    // Guarded rather than left to the crate: on an empty slice it builds an inverted box and
    // answers `Ok([white])`, where every caller here spells "no artwork" as an empty list.
    if rgb.is_empty() {
        return Vec::new();
    }

    // The crate asserts below two, and a backdrop asking for one wash is not a caller's error to
    // report from here.
    let max_colors = u8::try_from(desired).unwrap_or(u8::MAX).max(2);
    match color_thief::get_palette(rgb, ColorFormat::Rgb, 1, max_colors) {
        Ok(palette) => palette
            .into_iter()
            .map(|c| (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b))
            .collect(),
        // A box it could neither cut nor average. Nothing to say about this cover, which is the
        // same answer as no cover — and not worth a log line on a path that runs per track.
        Err(_) => Vec::new(),
    }
}

/// Edit `argb` in HCT and pack the result back. **The round trip is gamut-mapped**, so a saturated
/// seed can come back a little less saturated — stated here rather than at each helper below.
/// [`clamp_to_tone_band`] deliberately doesn't use it: returning an in-band colour *verbatim* is
/// what keeps its own chroma, and a round trip would map it.
fn with_hct(argb: u32, edit: impl FnOnce(&mut Hct)) -> u32 {
    let mut hct = Hct::new(Argb::from_u32(argb));
    edit(&mut hct);
    argb_to_u32(Argb::from(hct))
}

/// Move `argb` into the `min_tone..=max_tone` HCT lightness band, leaving hue
/// and chroma alone; returns it unchanged when it is already inside.
///
/// The M3 way to make an extracted colour legible on a known surface: tone *is*
/// the contrast axis, so flooring it lifts a near-black accent into view while
/// keeping the colour recognisably the album's. A multiplicative brighten
/// (Slint's `.brighter()`) can't — it scales HSV value, so near-black stays
/// near-black.
///
/// **The two bounds are not symmetric in what they buy.** The floor buys
/// contrast; the ceiling stops a near-white seed out-shining everything solved
/// beside it, a pure-white sleeve quantizing above every text band there is.
/// Leaving the seed untouched *inside* the band is what keeps a bright accent's
/// own chroma rather than dragging it to a bound it didn't need.
///
/// The round-trip is gamut-mapped, so a saturated seed comes back a little less
/// saturated at high tones — the correct trade, legibility being the point.
///
/// An out-of-range bound doesn't panic: `set_tone` forwards to the solver, which
/// answers with a greyscale `Argb::from_lstar`. An *inverted* band does —
/// `f64::clamp` panics on `min > max` before the solver sees the tone — hence
/// the debug assertion.
pub fn clamp_to_tone_band(argb: u32, min_tone: f64, max_tone: f64) -> u32 {
    debug_assert!(min_tone <= max_tone, "inverted tone band {min_tone}..={max_tone}");
    let mut hct = Hct::new(Argb::from_u32(argb));
    let tone = hct.get_tone();
    if tone >= min_tone && tone <= max_tone {
        return argb;
    }
    hct.set_tone(tone.clamp(min_tone, max_tone));
    argb_to_u32(Argb::from(hct))
}

/// Drive `argb` to exactly `tone`, capping chroma at `max_chroma` and keeping
/// the hue.
///
/// [`clamp_to_tone_band`]'s sibling, for surfaces wanting the source colour's
/// *identity* but not its saturation — Now-Playing body text, which should read
/// near-white with a whisper of the album's warmth rather than as coloured type.
/// Tone is set rather than floored: the caller already solved it against a
/// contrast target, so landing above would be as wrong as landing below.
///
/// Chroma is capped *before* the tone is set, the solver gamut-mapping against
/// whatever chroma it is given.
pub fn to_tone_capped_chroma(argb: u32, tone: f64, max_chroma: f64) -> u32 {
    with_hct(argb, |hct| {
        if hct.get_chroma() > max_chroma {
            hct.set_chroma(max_chroma);
        }
        hct.set_tone(tone);
    })
}

/// Scale `argb`'s HCT lightness by `factor`, keeping hue and chroma — a darker *sibling* of a
/// colour rather than a legible version of it, `ui::aurora` pairing a wash with a deeper
/// one so an art-less surface has the value structure a cover gave it. Multiplicative so the
/// answer scales with whatever the theme's accent is; a fixed tone flattens every palette onto one
/// pair.
pub fn scale_tone(argb: u32, factor: f64) -> u32 {
    with_hct(argb, |hct| hct.set_tone((hct.get_tone() * factor).clamp(0.0, 100.0)))
}

/// Turn `argb` `degrees` around the HCT hue wheel, keeping tone and chroma, and wrapping so any
/// `degrees` is in range. For making a *second* colour out of a first rather than making one
/// legible: `ui::aurora` fills a short seed list this way. HCT rather than HSL keeps the
/// sibling at the same apparent lightness, which is why the tint band states one tone ceiling.
pub fn rotate_hue(argb: u32, degrees: f64) -> u32 {
    with_hct(argb, |hct| hct.set_hue((hct.get_hue() + degrees).rem_euclid(360.0)))
}

/// Build a `DynamicScheme` of `style` × `is_dark` from the seed and map the M3
/// roles onto a [`crate::themes::Palette`] plus accent. Pure CPU and
/// sub-millisecond, so a tokio worker can call it without `spawn_blocking`.
pub fn generate_palette(source_argb: u32, is_dark: bool, style: SchemeStyle) -> (Palette, u32) {
    let hct = Hct::new(Argb::from_u32(source_argb));
    // Read the resolved-role accessors off the inner `DynamicScheme` directly —
    // they compute on demand and each role is read once, so a
    // `Scheme::from(DynamicScheme)` would only be an intermediate clone.
    let dyn_scheme = match style {
        SchemeStyle::Content => SchemeContent::new(hct, is_dark, None).scheme,
        SchemeStyle::Vibrant => SchemeVibrant::new(hct, is_dark, None).scheme,
        SchemeStyle::Expressive => SchemeExpressive::new(hct, is_dark, None).scheme,
        SchemeStyle::Fidelity => SchemeFidelity::new(hct, is_dark, None).scheme,
        SchemeStyle::Neutral => SchemeNeutral::new(hct, is_dark, None).scheme,
        SchemeStyle::Monochrome => SchemeMonochrome::new(hct, is_dark, None).scheme,
        // The coordinator filters `None` out earlier, so reaching this arm
        // through it is an upstream bug — but TonalSpot is the safe default.
        SchemeStyle::TonalSpot | SchemeStyle::None => {
            SchemeTonalSpot::new(hct, is_dark, None).scheme
        }
    };

    let surface = argb_to_u32(dyn_scheme.surface());
    let surface_container_low = argb_to_u32(dyn_scheme.surface_container_low());
    let surface_dim = argb_to_u32(dyn_scheme.surface_dim());
    let surface_container_high = argb_to_u32(dyn_scheme.surface_container_high());
    let surface_container_highest = argb_to_u32(dyn_scheme.surface_container_highest());
    let outline = argb_to_u32(dyn_scheme.outline());
    let outline_variant = argb_to_u32(dyn_scheme.outline_variant());
    let on_surface = argb_to_u32(dyn_scheme.on_surface());
    let on_surface_variant = argb_to_u32(dyn_scheme.on_surface_variant());
    let primary = argb_to_u32(dyn_scheme.primary());
    let error = argb_to_u32(dyn_scheme.error());

    // Interpolated middle tones, so the dynamic palette keeps the static one's
    // depth range.
    let surface2 = mix_rgb_u32(surface_container_highest, outline);
    let subtext0 = mix_rgb_u32(on_surface_variant, on_surface);

    // From the static M3 theme, not the scheme: a dynamic scheme has no green or
    // yellow role, and the three surfaces reading them — traffic light,
    // success/warning toasts, star rating — are semantic signals that must stay
    // recognisable, so they get the pair Color Style = None paints.
    let (green, yellow) = if is_dark {
        (material3::DARK_GREEN, material3::DARK_YELLOW)
    } else {
        (material3::LIGHT_GREEN, material3::LIGHT_YELLOW)
    };

    let palette = Palette {
        base: surface,
        mantle: surface_container_low,
        crust: surface_dim,
        surface0: surface_container_high,
        surface1: surface_container_highest,
        surface2,
        overlay0: outline_variant,
        overlay1: outline,
        overlay2: on_surface_variant,
        text: on_surface,
        subtext0,
        subtext1: outline,
        // Match the static M3 palette: `border == surface_container_highest`.
        border: surface_container_highest,
        red: error,
        green,
        yellow,
    };

    (palette, primary)
}

/// Covers the immediate skip-around window. An entry is a path and a `u32`,
/// so the whole cache is smaller than one decoded cover — and caching the
/// palette instead buys nothing, generation from a known seed being sub-ms.
const SEED_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(32) {
    Some(n) => n,
    None => panic!("SEED_CACHE_CAP > 0"),
};

/// Bounded LRU cache for source ARGB seeds keyed on the artwork path.
pub struct SeedCache {
    inner: LruCache<PathBuf, u32>,
}

impl SeedCache {
    pub fn new() -> Self {
        Self {
            inner: LruCache::new(SEED_CACHE_CAP),
        }
    }

    /// Look up `path`'s cached seed, invoking `f` on a miss and storing only a
    /// `Some`. `f` may block, so callers should already be on a blocking worker.
    pub fn get_or_insert_with<F>(&mut self, path: &Path, f: F) -> Option<u32>
    where
        F: FnOnce() -> Option<u32>,
    {
        if let Some(&seed) = self.inner.get(path) {
            return Some(seed);
        }
        let seed = f()?;
        self.inner.put(path.to_path_buf(), seed);
        Some(seed)
    }
}

impl Default for SeedCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Pack an `Argb` into the `0x00RRGGBB` the palette tables use, dropping alpha.
fn argb_to_u32(a: Argb) -> u32 {
    (u32::from(a.red) << 16) | (u32::from(a.green) << 8) | u32::from(a.blue)
}

/// Channel-wise 50/50 mix of two `0x00RRGGBB` colours.
fn mix_rgb_u32(a: u32, b: u32) -> u32 {
    let ar = (a >> 16) & 0xff;
    let ag = (a >> 8) & 0xff;
    let ab = a & 0xff;
    let br = (b >> 16) & 0xff;
    let bg = (b >> 8) & 0xff;
    let bb = b & 0xff;
    let r = (ar + br) >> 1;
    let g = (ag + bg) >> 1;
    let bl = (ab + bb) >> 1;
    (r << 16) | (g << 8) | bl
}

#[cfg(test)]
#[path = "tests/material_you_tests.rs"]
mod tests;
