//! Deterministic per-genre tint colours.
//!
//! Genres have no intrinsic artwork, so each card / detail header gets a
//! hash-derived gradient. [`hash_string_utf16`] is value-identical to the
//! Tauri build's hash, so a genre keeps its colour across the port.
//!
//! The *gradient shape* deliberately diverges from that original, which used
//! one hue plus a fixed +30° offset at constant saturation and value — under
//! that scheme hue 120° and hue 140° both just read as "green-ish". Three
//! independent name-derived axes separate them instead:
//!
//!   1. **Hue 1** — the primary, 0..360°.
//!   2. **Hue offset** — 60..120°, so a wide-offset card carries two clearly
//!      different colours while a narrow-offset one stays analogous.
//!   3. **Saturation + value jitter** — one-sided positive, so the base
//!      values act as a floor and jitter only lifts a few cards toward vivid
//!      without breaking the family look.
//!
//! Each axis reads a differently-rotated view of the same `u32` hash, so they
//! stay decorrelated even for short names where raw hash entropy is low.
//! Colours resolve to `slint::Color`s on `GenreRow`; the Slint side just
//! plugs them into `@linear-gradient(135deg, …)` and holds no HSV math.

use slint::Color;

/// JavaScript `((h << 5) - h + ch) | 0` reproduced over the string's
/// UTF-16 code units. `wrapping_*` mimics the JS `| 0` 32-bit
/// truncation; `.unsigned_abs()` mirrors `Math.abs(i32)` (handles the
/// `-2^31` corner where regular `.abs()` would overflow). UTF-16
/// iteration matches JS `charCodeAt(i)` exactly, so non-ASCII genre
/// names hash to the same value Tauri produced.
fn hash_string_utf16(s: &str) -> u32 {
    let mut hash: i32 = 0;
    for unit in s.encode_utf16() {
        hash = (hash << 5).wrapping_sub(hash).wrapping_add(i32::from(unit));
    }
    hash.unsigned_abs()
}

/// The four gradient stops a genre carries — two for the small tile
/// (grid card + detail header artwork tile, both saturated) and two
/// for the full-bleed detail hero floor (lower saturation / dimmer so
/// scrim + foreground text stay legible).
// `slint::Color` doesn't implement `Eq` (it stores ARGB as `u8`s but the
// upstream type only derives `PartialEq` + `Hash`), so we can't derive
// `Eq` on the wrapper either.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GenreAccent {
    pub tile_color_1: Color,
    pub tile_color_2: Color,
    pub hero_color_1: Color,
    pub hero_color_2: Color,
}

/// Hash the name into a `GenreAccent`. Deterministic — same name in,
/// same colours out, across runs and across the Tauri build (hash) /
/// this build (composition).
pub fn genre_accent(name: &str) -> GenreAccent {
    let hash = hash_string_utf16(name);
    // Use rotated views of the hash for the offset / jitter axes so
    // short names (where the raw hash is small) still spread across
    // the parameter space. `rotate_right(N)` is a free bit shuffle.
    let hue1 = hue_from_hash(hash);
    let offset_deg = 60 + u16::try_from(hash.rotate_right(11) % 61).unwrap_or(0);
    let hue2 = (hue1 + offset_deg) % 360;
    // Jitters: 0..15, scaled to 0..0.15. Small enough to preserve the
    // family look across the grid, large enough to lift a few cards
    // out of the monotone middle.
    let sat_jitter = jitter_0_to_15(hash.rotate_right(19));
    let val_jitter = jitter_0_to_15(hash.rotate_right(23));

    let hue1_f = hue_to_f32(hue1);
    let hue2_f = hue_to_f32(hue2);

    GenreAccent {
        // Saturated stops for the small tile.
        tile_color_1: hsv_to_color(hue1_f, 0.72 + sat_jitter, 0.68 + val_jitter),
        tile_color_2: hsv_to_color(hue2_f, 0.65 + sat_jitter, 0.48 + val_jitter),
        // Dimmer / less saturated stops for the wide hero floor, so
        // the scrim + foreground text on the detail header stays
        // readable.
        hero_color_1: hsv_to_color(hue1_f, 0.52 + sat_jitter, 0.52 + val_jitter),
        hero_color_2: hsv_to_color(hue2_f, 0.44 + sat_jitter, 0.22 + val_jitter),
    }
}

fn hue_from_hash(hash: u32) -> u16 {
    u16::try_from(hash % 360).unwrap_or(0)
}

fn hue_to_f32(hue: u16) -> f32 {
    // `u16 → f32` is exact: u16's max value (65 535) fits in f32's
    // 24-bit mantissa, and we're capped at 359 anyway.
    f32::from(hue)
}

fn jitter_0_to_15(rotated_hash: u32) -> f32 {
    // Mask to a `u8` first so the cast to f32 is exact (8 bits ≪ 24-bit
    // mantissa). Output is 0..0.15.
    let raw = u8::try_from(rotated_hash % 16).unwrap_or(0);
    f32::from(raw) * 0.01
}

/// HSV → `slint::Color`. `hue` in degrees, `sat` / `val` in 0..1.
/// Thin wrapper around `Color::from_hsva` that pre-normalises the
/// inputs (wraps hue into 0..360, clamps S/V) so callers don't have
/// to. Alpha is fixed at 1.0 — the gradient stops are always opaque
/// and the scrim is applied separately in Slint.
fn hsv_to_color(hue: f32, sat: f32, val: f32) -> Color {
    Color::from_hsva(
        hue.rem_euclid(360.0),
        sat.clamp(0.0, 1.0),
        val.clamp(0.0, 1.0),
        1.0,
    )
}

#[cfg(test)]
#[path = "tests/color_tests.rs"]
mod tests;
