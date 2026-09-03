//! Deterministic colours for an entity that has no artwork, from its name alone.
//!
//! Two views need this and want different gradients out of it: a genre card is a colour tile by
//! design and goes vivid, a station tile has a monogram to keep legible and goes deep. So what is
//! shared is the *derivation* — one hash, rotated views of it per axis, HSV out — and each caller
//! composes its own stops on top. [`crate::ui::genres::genre_accent`] and
//! [`crate::ui::radio::station_tile`] are those two.
//!
//! [`hash_name`] is value-identical to the Tauri build's hash and may not change: a genre's colour
//! is the same colour across the port only because of that.

use slint::Color;

/// JavaScript `((h << 5) - h + ch) | 0` reproduced over the string's UTF-16 code units.
///
/// `wrapping_*` mimics the JS `| 0` 32-bit truncation; `.unsigned_abs()` mirrors `Math.abs(i32)`
/// (handling the `-2^31` corner where regular `.abs()` would overflow). UTF-16 iteration matches
/// JS `charCodeAt(i)` exactly, so a non-ASCII name hashes to the value Tauri produced.
pub fn hash_name(name: &str) -> u32 {
    let mut hash: i32 = 0;
    for unit in name.encode_utf16() {
        hash = (hash << 5).wrapping_sub(hash).wrapping_add(i32::from(unit));
    }
    hash.unsigned_abs()
}

/// A hue in 0..360 from `hash`. Callers pass a rotated view for a second axis — `rotate_right(N)`
/// is a free bit shuffle, and it is what keeps the axes decorrelated for short names where raw
/// hash entropy is low.
pub fn hue_from_hash(hash: u32) -> u16 {
    u16::try_from(hash % 360).unwrap_or(0)
}

/// `u16 → f32` is exact: u16's max fits in f32's 24-bit mantissa, and a hue is capped at 359.
pub fn hue_to_f32(hue: u16) -> f32 {
    f32::from(hue)
}

/// A 0..0.15 jitter off `rotated_hash`, in hundredths.
///
/// One-sided so a caller's base saturation and value act as a floor and the jitter only lifts a
/// few entries toward vivid, rather than scattering the grid either side of a midpoint.
pub fn jitter_0_to_15(rotated_hash: u32) -> f32 {
    // Masked to a `u8` first so the cast to f32 is exact — 8 bits is well inside the mantissa.
    let raw = u8::try_from(rotated_hash % 16).unwrap_or(0);
    f32::from(raw) * 0.01
}

/// HSV → `slint::Color`, with `hue` in degrees and `sat` / `val` in 0..1.
///
/// Pre-normalises the inputs so callers don't have to. Alpha is fixed at 1.0 — every gradient stop
/// built here is opaque, and what goes over it is applied on the Slint side.
pub fn hsv_color(hue: f32, sat: f32, val: f32) -> Color {
    Color::from_hsva(hue.rem_euclid(360.0), sat.clamp(0.0, 1.0), val.clamp(0.0, 1.0), 1.0)
}

#[cfg(test)]
#[path = "tests/name_palette_tests.rs"]
mod tests;
