//! Material You — Material 3 dynamic colour generation from album artwork.
//!
//! Mirrors the Tauri version's `dynamicColor.ts` behaviour but in pure Rust
//! using the [`material-colors`](https://docs.rs/material-colors) crate.
//! The pipeline is:
//!
//! 1. Decode the artwork file at native resolution and resize to 64×64 RGBA8
//!    via [`image::imageops::FilterType::Triangle`] — the full-resolution
//!    decode buffer is dropped before quantization, so we only ever hold
//!    `64 × 64 × 4 = 16 KiB` of pixels.
//! 2. Run `QuantizerCelebi::quantize` with up to 128 cluster centres, then
//!    `Score::score(desired = 1)` to pick the best UI-suitable seed colour —
//!    falling back to the *dominant* cluster rather than to the crate's own
//!    Google Blue when the artwork has no colour above the scorer's chroma
//!    bar. See [`seed_from_pixels`].
//! 3. Build a `DynamicScheme` of the requested style (`SchemeTonalSpot`,
//!    `SchemeVibrant`, …) at contrast 0.0 / `is_dark` matching the user's
//!    variant, then map the M3 roles directly into a [`crate::themes::Palette`].
//!
//! All public functions are sync and may block — call from
//! `tokio::task::spawn_blocking` only.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

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

use crate::media::image_decode::decode_capped;
use crate::themes::{Palette, material3};

/// One of the seven Material 3 dynamic-colour scheme variants exposed by
/// the [`material-colors`](https://docs.rs/material-colors) crate, plus a
/// `None` value that disables Material You and falls back to the static
/// M3 palette. Order matches Tauri's `AppearanceSection.tsx`.
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
    /// Persisted id used in `settings.json` (matches Tauri).
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

/// Decoded source image down-sampled to a fixed 64×64 input for the
/// quantizer. Tauri uses the same size; balances quantization quality and
/// CPU/memory cost for an album-art-sized image.
const QUANTIZE_DIM: u32 = 64;

/// Upper bound on cluster centres handed to `QuantizerCelebi`. Tauri uses
/// 128; lower values muddy the scoring pass, higher values waste CPU.
const QUANTIZE_MAX_COLOURS: usize = 128;

/// Defensive cap on source dimensions for the path-based
/// [`extract_source_argb`] fallback. Real album art is well under this; the
/// limit just prevents a 3000×3000+ embedded picture (common on Bandcamp /
/// Apple Music releases) from spiking a multi-MB transient decode buffer
/// the glibc heap then keeps reserved. The current-track path goes through
/// `CoverThumbs::get_or_load_rgb8` (cap 8192 via `MAX_SOURCE_DIM`), so this
/// only fires when a caller exercises the path-based variant directly —
/// tests, the future palette debug tool, etc.
const MATERIAL_YOU_MAX_SOURCE_DIM: u32 = 2048;

/// Decode `artwork_path`, downscale to 64×64 RGBA8, then take the seed off it
/// via [`seed_from_pixels`]. Returns the seed colour as a 32-bit `0xAARRGGBB`
/// ARGB integer, or `None` when the decode failed or left nothing opaque.
/// **Blocking** — call from `tokio::task::spawn_blocking` only.
pub fn extract_source_argb(artwork_path: &Path) -> Option<u32> {
    let decoded = match decode_capped(artwork_path, MATERIAL_YOU_MAX_SOURCE_DIM) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("material_you: decode {}: {e}", artwork_path.display());
            return None;
        }
    };

    // Resize before converting to RGBA8 — the resized buffer is the only
    // pixel data we keep beyond this point. `Triangle` is the sweet spot:
    // good enough for colour quantization, much faster than Lanczos3, and
    // markedly better than Nearest for averaged seed quality.
    let small = decoded.resize_exact(QUANTIZE_DIM, QUANTIZE_DIM, FilterType::Triangle).into_rgba8();
    drop(decoded);

    let mut pixels: Vec<Argb> = Vec::with_capacity((QUANTIZE_DIM * QUANTIZE_DIM) as usize);
    for chunk in small.chunks_exact(4) {
        // chunks_exact(4) → [R, G, B, A]
        let r = chunk[0];
        let g = chunk[1];
        let b = chunk[2];
        let a = chunk[3];
        // Skip nearly-transparent pixels so the alpha channel doesn't
        // bias the seed toward whatever colour bleeds through edges.
        if a < 128 {
            continue;
        }
        pixels.push(Argb::new(0xff, r, g, b));
    }

    seed_from_pixels(&pixels)
}

/// Quantize `pixels` and score the best UI seed out of the resulting clusters.
///
/// `Score` discards every cluster under its own chroma cutoff and answers
/// Google Blue when nothing survives — a brand colour, not a fact about this
/// artwork, which is how a greyscale sleeve came to paint vivid blue chrome
/// over a grey banner. The *dominant* cluster is such a fact: what the image
/// mostly is, whatever its chroma. Handed over as the fallback it leaves a
/// colourless cover seeding its own grey — which every consumer then owns the
/// lightness of — and, being the only fallback named here, keeps the crate's
/// out of reach rather than merely unlikely.
///
/// **The fallback reaches the app-wide palette too, not just the backdrop**, so
/// which Color Style is picked decides how visible it is. [`SchemeStyle`]'s
/// `TonalSpot`, `Vibrant`, `Expressive` and `Neutral` force their own primary
/// chroma, and a neutral sRGB grey still has a CAM16 hue (≈203, the white
/// point's own), so those land blue-ish much as the scorer's default did.
/// `Content` and `Fidelity` take the *source's* chroma, which for a grey is
/// nearly none — so a monochrome sleeve now themes the app near-neutral where
/// it used to theme it blue. That is the faithful answer and the same one
/// Android gives a greyscale wallpaper, but unlike [`crate::ui::backdrop`]'s
/// tiers there is no tone band downstream to put a floor under it.
///
/// `None` means the quantizer had nothing to say, and both consumers already
/// spell that as the theme accent ([`crate::ui::backdrop`]'s `unwrap_or`, and
/// the static palette in `tasks::material_you`). So there is no last-resort
/// colour to pick in here, and no layer-crossing theme dependency to acquire
/// in order to pick one.
fn seed_from_pixels(pixels: &[Argb]) -> Option<u32> {
    if pixels.is_empty() {
        return None;
    }
    let counts = QuantizerCelebi::quantize(pixels, QUANTIZE_MAX_COLOURS).color_to_count;
    let dominant = *counts.iter().max_by_key(|(_, count)| **count)?.0;
    let seed = *Score::score(&counts, Some(1), Some(dominant), None).first()?;
    Some(argb_to_u32(seed))
}

/// Same quantization pipeline as [`extract_source_argb`] but starting from
/// an already-decoded RGB8 buffer (the 72×72 thumbnail
/// [`crate::media::cover_thumbs::CoverThumbs`] already keeps in memory for
/// row rendering). The buffer is small enough that quantizing it directly
/// is qualitatively equivalent to quantizing a 64×64 downsample of the
/// source — and skips the multi-MB transient decode peak that the
/// path-based variant produces on large embedded artwork.
///
/// Returns `None` if the buffer is empty (zero-width or zero-height).
/// **Blocking** (CPU-bound quantize) — call from `spawn_blocking`.
pub fn extract_source_argb_from_rgb8(buf: &SharedPixelBuffer<Rgb8Pixel>) -> Option<u32> {
    let bytes = buf.as_bytes();
    // RGB8 has no alpha → every pixel is opaque, so the alpha-skip branch
    // from `extract_source_argb` collapses to an unconditional push.
    let mut pixels: Vec<Argb> = Vec::with_capacity(bytes.len() / 3);
    for chunk in bytes.chunks_exact(3) {
        pixels.push(Argb::new(0xff, chunk[0], chunk[1], chunk[2]));
    }
    seed_from_pixels(&pixels)
}

/// Move `argb` into the `min_tone..=max_tone` HCT lightness band, leaving hue
/// and chroma alone; returns it unchanged when it is already inside.
///
/// This is the M3 way to make an arbitrary extracted colour legible on a known
/// surface: tone *is* the contrast axis, so flooring it lifts a near-black
/// artwork accent into view while keeping the colour recognisably the album's.
/// A naive multiplicative brighten (Slint's `.brighter()`) can't do this — it
/// scales HSV value, so anything near black stays near black.
///
/// **The ceiling is not symmetric with the floor in what it's for.** The floor
/// buys contrast; the ceiling stops a seed that is already near-white from
/// out-shining everything solved beside it — a pure-white sleeve quantizes to
/// tone 100, which is above every text band there is. Leaving the seed
/// untouched *inside* the band is what keeps a bright accent's own chroma
/// rather than dragging it to a bound it didn't need.
///
/// Note the round-trip is gamut-mapped: at high tones sRGB can't hold the
/// original chroma, so a saturated seed comes back a little less saturated.
/// That's the correct trade — legibility is the point.
///
/// A bound outside the valid 0..=100 HCT range doesn't panic: `set_tone`
/// forwards to the solver, which answers an out-of-range lightness with a plain
/// greyscale `Argb::from_lstar`. An *inverted* band is the one shape that does
/// — `f64::clamp` panics on `min > max`, and it runs before the solver ever
/// sees the tone — so the ordering is asserted in debug.
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
/// The sibling of [`clamp_to_tone_band`] for surfaces that want the source
/// colour's *identity* but not its saturation — Now-Playing body text, which
/// should read as near-white carrying a whisper of the album's warmth rather
/// than as coloured type. Tone is set unconditionally (not floored): the
/// caller has already solved it for a contrast target, so landing above it
/// would be as wrong as landing below.
///
/// Order matters — chroma is capped *before* the tone is set, because the
/// solver gamut-maps against the chroma it is given and a saturated seed
/// pushed to a high tone comes back desaturated anyway.
pub fn to_tone_capped_chroma(argb: u32, tone: f64, max_chroma: f64) -> u32 {
    let mut hct = Hct::new(Argb::from_u32(argb));
    if hct.get_chroma() > max_chroma {
        hct.set_chroma(max_chroma);
    }
    hct.set_tone(tone);
    argb_to_u32(Argb::from(hct))
}

/// Build a `DynamicScheme` of `style` × `is_dark` (contrast = default)
/// from the seed and map the M3 roles to a [`crate::themes::Palette`] +
/// accent hex. Matches the M3 → palette role mapping from Tauri's
/// `dynamicColor.ts::generateDynamicColors`. Pure CPU, sub-millisecond —
/// safe to call from a tokio worker without `spawn_blocking` if you've
/// already got the seed.
pub fn generate_palette(source_argb: u32, is_dark: bool, style: SchemeStyle) -> (Palette, u32) {
    let hct = Hct::new(Argb::from_u32(source_argb));
    // Each variant wraps a `DynamicScheme`; we pull it out and read the
    // resolved-role accessors directly (no `Scheme::from(DynamicScheme)`
    // intermediate clone — the accessors compute on demand and we only
    // read each role once).
    let dyn_scheme = match style {
        SchemeStyle::Content => SchemeContent::new(hct, is_dark, None).scheme,
        SchemeStyle::Vibrant => SchemeVibrant::new(hct, is_dark, None).scheme,
        SchemeStyle::Expressive => SchemeExpressive::new(hct, is_dark, None).scheme,
        SchemeStyle::Fidelity => SchemeFidelity::new(hct, is_dark, None).scheme,
        SchemeStyle::Neutral => SchemeNeutral::new(hct, is_dark, None).scheme,
        SchemeStyle::Monochrome => SchemeMonochrome::new(hct, is_dark, None).scheme,
        // None falls through to TonalSpot — the `None` variant is filtered
        // earlier in the coordinator; reaching this branch is a logic bug
        // upstream, but TonalSpot is the safe canonical default.
        SchemeStyle::TonalSpot | SchemeStyle::None => {
            SchemeTonalSpot::new(hct, is_dark, None).scheme
        }
    };

    // M3 → palette role mapping. Comment column shows the M3 source.
    let surface = argb_to_u32(dyn_scheme.surface()); // surface
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

    // `surface2` and `subtext0` need an interpolated middle tone to keep
    // the existing palette's depth range. Both midpoints ported from
    // Tauri's `interpolateNeutral()`.
    let surface2 = mix_rgb_u32(surface_container_highest, outline);
    let subtext0 = mix_rgb_u32(on_surface_variant, on_surface);

    // Green and yellow come from the static M3 theme rather than the scheme.
    // A dynamic scheme has no green or yellow role to map, and the three
    // surfaces reading them — the maximize traffic light, the success/warning
    // toasts, the star rating — are semantic signals that have to stay
    // recognisable, so they get the same pair Color Style = None paints.
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

    /// Look up `path`'s cached seed; on miss invoke `f`, store the result
    /// (only on `Some`), and return whichever value won. `f` may block
    /// (file decode + quantize); callers should already be on a
    /// `spawn_blocking` worker.
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

/// Pack an `Argb { alpha, red, green, blue }` into the project's
/// `0x00RRGGBB` u32 format (alpha discarded, top byte zero — matches the
/// rest of the palette tables).
fn argb_to_u32(a: Argb) -> u32 {
    (u32::from(a.red) << 16) | (u32::from(a.green) << 8) | u32::from(a.blue)
}

/// 50/50 mix of two `0x00RRGGBB` colours, channel-wise. Used for
/// `surface2` and `subtext0` to keep the dynamic palette's depth range
/// matching the static M3 palette.
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
