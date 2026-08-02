use image::{ImageBuffer, Rgb, RgbImage};

use super::{blit, compose_mosaic_blur};
use crate::error::AppError;

fn solid(w: u32, h: u32, rgb: [u8; 3]) -> RgbImage {
    ImageBuffer::from_pixel(w, h, Rgb(rgb))
}

#[test]
fn blit_fills_exact_destination_rect() {
    let mut dst = solid(4, 4, [0, 0, 0]);
    let src = solid(2, 2, [255, 0, 0]);
    blit(&mut dst, &src, 2, 2, 2, 2);
    // Bottom-right quadrant painted, everything else untouched.
    for y in 0..4 {
        for x in 0..4 {
            let expected = if x >= 2 && y >= 2 { [255, 0, 0] } else { [0, 0, 0] };
            assert_eq!(dst.get_pixel(x, y).0, expected, "pixel ({x},{y})");
        }
    }
}

#[test]
fn blit_stretches_source_over_larger_rect() {
    let mut dst = solid(4, 4, [0, 0, 0]);
    // 2×1 source: left pixel red, right pixel green.
    let mut src = solid(2, 1, [255, 0, 0]);
    src.put_pixel(1, 0, Rgb([0, 255, 0]));
    blit(&mut dst, &src, 0, 0, 4, 4);
    // Nearest-neighbour: left half red, right half green, full height.
    assert_eq!(dst.get_pixel(0, 0).0, [255, 0, 0]);
    assert_eq!(dst.get_pixel(1, 3).0, [255, 0, 0]);
    assert_eq!(dst.get_pixel(2, 0).0, [0, 255, 0]);
    assert_eq!(dst.get_pixel(3, 3).0, [0, 255, 0]);
}

#[test]
fn blit_with_empty_source_is_a_no_op() {
    let mut dst = solid(2, 2, [7, 7, 7]);
    let src: RgbImage = ImageBuffer::new(0, 0);
    blit(&mut dst, &src, 0, 0, 2, 2);
    assert_eq!(dst.get_pixel(1, 1).0, [7, 7, 7]);
}

#[test]
fn compose_returns_none_for_empty_path_list() {
    assert!(compose_mosaic_blur(&[]).is_none());
}

#[test]
fn compose_returns_none_when_every_decode_fails() {
    let paths = vec![
        "/nonexistent/one.png".to_owned(),
        "/nonexistent/two.jpg".to_owned(),
    ];
    assert!(compose_mosaic_blur(&paths).is_none());
}

#[test]
fn compose_produces_target_sized_buffer_from_a_real_image() -> Result<(), AppError> {
    // Write a tiny real PNG so the decode path runs end-to-end.
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tile.png");
    solid(8, 8, [10, 200, 30])
        .save(&path)
        .map_err(|e| AppError::Validation(format!("write png: {e}")))?;

    let composed = compose_mosaic_blur(&[path.to_string_lossy().into_owned()])
        .ok_or_else(|| AppError::Validation("one good tile must compose".into()))?;
    assert_eq!(composed.blur.width(), 192);
    assert_eq!(composed.blur.height(), 192);
    // A composed mosaic must hand the hero a hue to seed from, or the banner
    // silently falls back to `Theme.accent` and tracks the theme again.
    assert!(composed.sample.accent_argb.is_some());
    assert!(composed.sample.luma.is_some());
    Ok(())
}
