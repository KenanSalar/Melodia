//! Deterministic colours for an entity with no artwork, from its name alone. The hash and its
//! rotated views are shared; each caller composes its own stops
//! ([`crate::ui::genres::genre_accent`], [`crate::ui::radio::station_tile`]).
//!
//! [`hash_name`] may not change: it keeps each genre the colour its users already know.

use slint::Color;

/// JavaScript `((h << 5) - h + ch) | 0` over the string's UTF-16 units, which a non-ASCII name's
/// colour depends on. `unsigned_abs` is `Math.abs` without `abs`'s overflow at `-2^31`.
pub fn hash_name(name: &str) -> u32 {
    let mut hash: i32 = 0;
    for unit in name.encode_utf16() {
        hash = (hash << 5).wrapping_sub(hash).wrapping_add(i32::from(unit));
    }
    hash.unsigned_abs()
}

/// A hue in 0..360. Callers pass a `rotate_right(N)` view for a second axis, which keeps the axes
/// decorrelated for short names.
pub fn hue_from_hash(hash: u32) -> u16 {
    u16::try_from(hash % 360).unwrap_or(0)
}

/// `u16 → f32` is exact: u16's max fits in f32's 24-bit mantissa, and a hue is capped at 359.
pub fn hue_to_f32(hue: u16) -> f32 {
    f32::from(hue)
}

/// A 0..0.15 jitter, one-sided so a caller's base saturation and value act as a floor.
pub fn jitter_0_to_15(rotated_hash: u32) -> f32 {
    let raw = u8::try_from(rotated_hash % 16).unwrap_or(0);
    f32::from(raw) * 0.01
}

/// HSV → `slint::Color`, `hue` in degrees and `sat` / `val` in 0..1, normalised here and opaque.
pub fn hsv_color(hue: f32, sat: f32, val: f32) -> Color {
    Color::from_hsva(hue.rem_euclid(360.0), sat.clamp(0.0, 1.0), val.clamp(0.0, 1.0), 1.0)
}

#[cfg(test)]
#[path = "tests/name_palette_tests.rs"]
mod tests;
