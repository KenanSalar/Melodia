//! Packed `0x00RRGGBB` values to and from Slint's `Color` and `Brush`, the form every palette table
//! and every solved tier is held in on the Rust side.

use slint::{Brush, Color};

/// A `0x00RRGGBB` value as an opaque solid `Brush`.
pub fn brush(rgb: u32) -> Brush {
    Brush::SolidColor(color(rgb))
}

/// The same as a bare `Color`, which the Now Playing gradient floor needs —
/// Slint's `.mix()` and gradient stops take `color`, not `brush`.
pub fn color(rgb: u32) -> Color {
    let r = ((rgb >> 16) & 0xff) as u8;
    let g = ((rgb >> 8) & 0xff) as u8;
    let b = (rgb & 0xff) as u8;
    Color::from_rgb_u8(r, g, b)
}

/// The same plus an alpha, for the two solved scrims. Their opacity is solved
/// per artwork, so baking it in keeps the Slint side one `background:` binding
/// rather than a colour plus a float the view has to recombine.
pub fn brush_with_alpha(rgb: u32, alpha: u8) -> Brush {
    Brush::SolidColor(color_with_alpha(rgb, f32::from(alpha) / 255.0))
}

/// [`color`] carrying a weight in its alpha, for a gradient stop that has to stay a `color`. The
/// aurora's tints arrive this way: `transparentize` on the Slint side multiplies rather than sets,
/// so the falloff shape and the per-artwork weight compose without either restating the other.
pub fn color_with_alpha(rgb: u32, alpha: f32) -> Color {
    color(rgb).with_alpha(alpha)
}

/// A solid `Brush` back to `0x00RRGGBB`, dropping alpha — how a solved surface
/// reads the theme accent's *hue* out of the global as an artwork-less fallback.
/// A gradient answers with its first stop, the right approximation here.
pub fn brush_to_rgb(brush: &Brush) -> u32 {
    color_to_rgb(brush.color())
}

/// Inverse of [`color`]. The Genre hero reads the hash-derived gradient stops
/// off `GenreRow` — they arrive as `Color`, never as a `Brush` — and has to
/// measure their lightness before it can solve a scrim against them.
pub fn color_to_rgb(c: Color) -> u32 {
    (u32::from(c.red()) << 16) | (u32::from(c.green()) << 8) | u32::from(c.blue())
}

#[cfg(test)]
#[path = "tests/rgb_tests.rs"]
mod tests;
