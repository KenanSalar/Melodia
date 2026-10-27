//! Material 3 dynamic colour from album artwork, over the `material-colors` crate.
//!
//! [`population_seeds`] asks what the artwork mostly *is*, for the backdrop, where everything else
//! asks what makes the best UI seed; hence a second quantizer.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use color_thief::ColorFormat;
use image::imageops::FilterType;
use lru::LruCache;
use material_colors::color::Rgb;
use material_colors::dynamic_color::{DynamicScheme, Platform, Role, SpecVersion, Variant};
use material_colors::hct::Hct;
use material_colors::quantize::{Quantizer, QuantizerCelebi};
use material_colors::score::Score;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::media::image::image_decode::decode_capped;
use melodia_core::error::describe;
use melodia_core::themes::{
    CARD_CONTROL_HOVER_PERCENT, CARD_CONTROL_PERCENT, Palette, material3, toward,
};

/// The seven Material 3 scheme variants, plus `None` for the static M3 palette.
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

    /// Unknown ids resolve to `None`, so stale settings can't break startup.
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

/// The quantizer's input square: enough for a seed off album art, and cheap.
const QUANTIZE_DIM: u32 = 64;

/// Cluster centres for `QuantizerCelebi`. Lower muddies the scoring, higher wastes CPU.
const QUANTIZE_MAX_COLOURS: usize = 128;

/// Tighter than `MAX_SOURCE_DIM` because this only feeds the quantizer, and a large transient
/// decode stays reserved in the glibc heap. The live path seeds off `CoverThumbs` instead.
const MATERIAL_YOU_MAX_SOURCE_DIM: u32 = 2048;

/// The seed for the cover at `artwork_path`, `None` when the decode failed or left nothing
/// opaque. **Blocking.**
pub fn extract_source_rgb(artwork_path: &Path) -> Option<u32> {
    let decoded = match decode_capped(artwork_path, MATERIAL_YOU_MAX_SOURCE_DIM) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("material_you: decode {}: {}", artwork_path.display(), describe(&e));
            return None;
        }
    };

    // Resized before converting, so only the small buffer survives; Triangle is plenty to quantize.
    let small = decoded.resize_exact(QUANTIZE_DIM, QUANTIZE_DIM, FilterType::Triangle).into_rgba8();
    drop(decoded);

    let mut pixels: Vec<Rgb> = Vec::with_capacity((QUANTIZE_DIM * QUANTIZE_DIM) as usize);
    for chunk in small.chunks_exact(4) {
        let r = chunk[0];
        let g = chunk[1];
        let b = chunk[2];
        let a = chunk[3];
        // Near-transparent pixels would bias the seed toward whatever bleeds through the edges.
        if a < 128 {
            continue;
        }
        pixels.push(Rgb::new(r, g, b));
    }

    seed_from_pixels(&pixels)
}

/// Quantizes `pixels` and scores the best UI seed out of the clusters. The dominant cluster is
/// `Score`'s fallback in place of the Google Blue it answers when nothing clears its chroma cutoff,
/// so a greyscale sleeve seeds its own grey: `Content` and `Fidelity` theme the app near-neutral,
/// and the variants forcing chroma land blue-ish, a neutral grey having the white point's CAM16
/// hue. `None` reads as the theme accent downstream.
fn seed_from_pixels(pixels: &[Rgb]) -> Option<u32> {
    if pixels.is_empty() {
        return None;
    }
    let counts = QuantizerCelebi::quantize(pixels, QUANTIZE_MAX_COLOURS).color_to_count;
    let dominant = counts.iter().max_by_key(|(_, count)| **count).map(|(rgb, _)| *rgb)?;
    Score::score(&counts, Some(1), Some(dominant), None).first().map(Rgb::as_u32)
}

/// [`extract_source_rgb`] over the RGB8 thumbnail `CoverThumbs` already holds, skipping the
/// decode peak a large embedded cover costs. Small enough that quantizing it directly is as good
/// as quantizing a downsample of the source. **Blocking.**
pub fn extract_source_rgb_from_rgb8(buf: &SharedPixelBuffer<Rgb8Pixel>) -> Option<u32> {
    let bytes = buf.as_bytes();
    let mut pixels: Vec<Rgb> = Vec::with_capacity(bytes.len() / 3);
    for chunk in bytes.chunks_exact(3) {
        pixels.push(Rgb::new(chunk[0], chunk[1], chunk[2]));
    }
    seed_from_pixels(&pixels)
}

/// Up to `desired` colours the cover is mostly *made of*, most prominent first, for the backdrop.
///
/// Median cut rather than `Score`, which ranks by usability, picking a photo's reddest detail over
/// what the photo mostly is, and leaves a greyscale sleeve one colour. The list can still come
/// back short, so the caller owes a filling rule: the crate drops pixels over 250 on all three
/// channels, leaving a near-white sleeve a single white. Quality 1 is the crate's finest stride,
/// not every pixel. **Blocking.**
pub fn population_seeds(rgb: &[u8], desired: usize) -> Vec<u32> {
    // On an empty slice the crate answers `Ok([white])`.
    if rgb.is_empty() {
        return Vec::new();
    }

    // The crate asserts on fewer than two.
    let max_colors = u8::try_from(desired).unwrap_or(u8::MAX).max(2);
    match color_thief::get_palette(rgb, ColorFormat::Rgb, 1, max_colors) {
        Ok(palette) => palette
            .into_iter()
            .map(|c| (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b))
            .collect(),
        // The same answer as no cover, and not worth a log line on a path that runs per track.
        Err(_) => Vec::new(),
    }
}

/// Edits `rgb` in HCT. The round trip is gamut-mapped, so a saturated colour can come back a
/// little less so; [`clamp_to_tone_band`] avoids it to return in-band colours verbatim.
fn with_hct(rgb: u32, edit: impl FnOnce(&mut Hct)) -> u32 {
    let mut hct = Hct::new(Rgb::from_u32(rgb));
    edit(&mut hct);
    Rgb::from(hct).as_u32()
}

/// Moves `rgb` into the `min_tone..=max_tone` HCT band, keeping hue and chroma, and returns it
/// unchanged when already inside.
///
/// Tone is the M3 contrast axis, so the floor lifts a near-black accent into view while keeping it
/// the album's colour, which a multiplicative brighten can't (it scales HSV value). The ceiling
/// stops a near-white seed out-shining everything solved beside it. An inverted band would panic
/// in `f64::clamp`, so the debug assertion names it first.
pub fn clamp_to_tone_band(rgb: u32, min_tone: f64, max_tone: f64) -> u32 {
    debug_assert!(min_tone <= max_tone, "inverted tone band {min_tone}..={max_tone}");
    let mut hct = Hct::new(Rgb::from_u32(rgb));
    let tone = hct.get_tone();
    if tone >= min_tone && tone <= max_tone {
        return rgb;
    }
    hct.set_tone(tone.clamp(min_tone, max_tone));
    Rgb::from(hct).as_u32()
}

/// Drives `rgb` to exactly `tone` with chroma capped at `max_chroma`, keeping the hue: a colour's
/// identity without its saturation, as Now Playing's body text wants. Set rather than floored, the
/// caller having solved `tone` against a contrast target. Chroma goes first, the solver
/// gamut-mapping against whatever chroma it is given.
pub fn to_tone_capped_chroma(rgb: u32, tone: f64, max_chroma: f64) -> u32 {
    with_hct(rgb, |hct| {
        if hct.get_chroma() > max_chroma {
            hct.set_chroma(max_chroma);
        }
        hct.set_tone(tone);
    })
}

/// Scales `rgb`'s HCT tone by `factor`, keeping hue and chroma: a darker sibling rather than a
/// legible version, as `ui::aurora` pairs its washes. Multiplicative so it follows the theme's
/// accent, where a fixed tone would flatten every palette onto one pair.
pub fn scale_tone(rgb: u32, factor: f64) -> u32 {
    with_hct(rgb, |hct| hct.set_tone((hct.get_tone() * factor).clamp(0.0, 100.0)))
}

/// Turns `rgb` `degrees` around the HCT hue wheel, keeping tone and chroma, to make a second
/// colour out of a first. HCT rather than HSL keeps the sibling at the same apparent lightness.
pub fn rotate_hue(rgb: u32, degrees: f64) -> u32 {
    with_hct(rgb, |hct| hct.set_hue((hct.get_hue() + degrees).rem_euclid(360.0)))
}

/// The spec every scheme is built against, named because the crate still defaults to 2021. Not
/// 2026, which changes nothing for the styles offered, so naming it would only let a crate bump
/// re-tone every palette unannounced. Content, Fidelity and Monochrome have no 2025 definition and
/// resolve to 2021 regardless.
const SPEC: SpecVersion = SpecVersion::Spec2025;

/// Builds the `style` × `is_dark` scheme from the seed and maps its M3 roles onto a [`Palette`]
/// plus accent. Pure CPU and cheap, so a tokio worker may call it directly.
///
/// `themes::material3`'s two variants are this function over the M3 baseline seed, so a change
/// to the mapping owes them a regenerate.
pub fn generate_palette(source_rgb: u32, is_dark: bool, style: SchemeStyle) -> (Palette, u32) {
    let hct = Hct::new(Rgb::from_u32(source_rgb));
    let variant = match style {
        SchemeStyle::Content => Variant::Content,
        SchemeStyle::Vibrant => Variant::Vibrant,
        SchemeStyle::Expressive => Variant::Expressive,
        SchemeStyle::Fidelity => Variant::Fidelity,
        SchemeStyle::Neutral => Variant::Neutral,
        SchemeStyle::Monochrome => Variant::Monochrome,
        // The coordinator filters `None` out first; TonalSpot is the safe answer if it slips by.
        SchemeStyle::TonalSpot | SchemeStyle::None => Variant::TonalSpot,
    };
    // `Phone` is the spec's only platform besides a watch.
    let scheme = DynamicScheme::from_spec(hct, variant, is_dark, None, Platform::Phone, SPEC);
    // One resolver, so the surface tones most roles solve against are derived once.
    let roles = scheme.resolver();
    let read = |role: Role| roles.rgb(role).as_u32();

    let surface = read(Role::Surface);
    let surface_container_low = read(Role::SurfaceContainerLow);
    let surface_dim = read(Role::SurfaceDim);
    let surface_container_high = read(Role::SurfaceContainerHigh);
    let surface_container_highest = read(Role::SurfaceContainerHighest);
    let outline = read(Role::Outline);
    let outline_variant = read(Role::OutlineVariant);
    let on_surface = read(Role::OnSurface);
    let on_surface_variant = read(Role::OnSurfaceVariant);
    let primary = read(Role::Primary);
    let error = read(Role::Error);

    // M3 has no role between these pairs, where `Palette` has a slot each.
    let surface2 = mix_rgb_u32(surface_container_highest, outline);
    let subtext0 = mix_rgb_u32(on_surface_variant, on_surface);

    // From the static M3 theme: no dynamic role is green or yellow, and what reads them are
    // semantic signals that must stay recognisable.
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
        // One surface up, where the other theme families put it.
        border: surface_container_highest,
        // M3 sets controls off a container by outline rather than fill, so these take the shared
        // rule instead of a role.
        card_control: toward(surface_container_high, on_surface, CARD_CONTROL_PERCENT),
        card_control_hover: toward(surface_container_high, on_surface, CARD_CONTROL_HOVER_PERCENT),
        red: error,
        green,
        yellow,
    };

    (palette, primary)
}

/// The immediate skip-around window. Seeds rather than palettes, a palette from a known seed
/// being cheap to generate.
const SEED_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(32) {
    Some(n) => n,
    None => panic!("SEED_CACHE_CAP > 0"),
};

pub struct SeedCache {
    inner: LruCache<PathBuf, u32>,
}

impl SeedCache {
    pub fn new() -> Self {
        Self { inner: LruCache::new(SEED_CACHE_CAP) }
    }

    /// Caches only a `Some`. `f` may block, so call from a blocking worker.
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
