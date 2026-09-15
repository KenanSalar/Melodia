//! The tray icon for a light taskbar. The raster carries the mark's dark-surface pastels, which all
//! but vanish on a light taskbar, so there it takes [`BRAND_MARK_ON_LIGHT`] through the same alpha,
//! the way the titlebar colorizes its mark.

use melodia_core::themes::BRAND_MARK_ON_LIGHT;

/// Repaints straight-alpha `rgba`, `width` pixels to a row, in the light gradient, keeping every
/// pixel's alpha. The gradient runs from the top-left corner to the bottom-right, as the titlebar's
/// `135deg` brush does.
pub(super) fn paint(rgba: &mut [u8], width: u32, height: u32) {
    let Ok(row_len) = usize::try_from(width).map(|pixels| pixels * 4) else { return };
    if row_len == 0 || height == 0 {
        return;
    }
    let [from, to] = BRAND_MARK_ON_LIGHT;
    let (width, height) = (u64::from(width), u64::from(height));

    // How far along the diagonal a pixel centre sits, `(x + ½) / width` and `(y + ½) / height`
    // averaged, kept as `offset / span` so the arithmetic stays in integers.
    let span = 4 * width * height;
    for (y, row) in (0u64..).zip(rgba.chunks_exact_mut(row_len)) {
        for (x, pixel) in (0u64..).zip(row.chunks_exact_mut(4)) {
            let offset = (2 * x + 1) * height + (2 * y + 1) * width;
            if let [red, green, blue, _alpha] = pixel {
                *red = mix(from, to, 16, offset, span);
                *green = mix(from, to, 8, offset, span);
                *blue = mix(from, to, 0, offset, span);
            }
        }
    }
}

/// The channel at `shift` of `from`, moved `offset / span` of the way to `to`'s.
fn mix(from: u32, to: u32, shift: u32, offset: u64, span: u64) -> u8 {
    let start = u64::from((from >> shift) & 0xff);
    let end = u64::from((to >> shift) & 0xff);
    u8::try_from((start * (span - offset) + end * offset) / span).unwrap_or(u8::MAX)
}

#[cfg(test)]
#[path = "tests/light_taskbar_tests.rs"]
mod tests;
