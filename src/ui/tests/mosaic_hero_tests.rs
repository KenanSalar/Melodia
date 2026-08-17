use image::{ImageBuffer, Rgb, RgbImage};

use super::*;
use crate::error::AppError;
use crate::ui::util::{BLUR_TARGET, COVER_SIZE};

fn solid(width: u32, height: u32, rgb: [u8; 3]) -> RgbImage {
    ImageBuffer::from_pixel(width, height, Rgb(rgb))
}

#[test]
fn a_composed_collage_carries_both_halves_and_a_hue_to_seed_from() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tile.png");
    solid(8, 8, [10, 200, 30])
        .save(&path)
        .map_err(|e| AppError::Validation(format!("write png: {e}")))?;

    let pair = compose_hero_pair(&[path.to_string_lossy().into_owned()]);

    let cover = pair.cover.ok_or_else(|| AppError::Validation("no cover half".into()))?;
    let blur = pair.blur.ok_or_else(|| AppError::Validation("no blur half".into()))?;
    assert_eq!(cover.width(), COVER_SIZE);
    assert_eq!(blur.width(), BLUR_TARGET);
    assert_eq!(blur.height(), BLUR.height);
    // Without a seed the banner falls back to `Theme.accent` and silently tracks the theme
    // again instead of the record.
    assert!(pair.sample.accent_argb.is_some());
    assert!(pair.sample.luma.is_some());
    Ok(())
}

/// An empty pair is how "no artwork" reaches the publisher, which clears the slots and
/// re-solves the floor — so neither an empty list nor a dead path may look like a failure
/// the caller has to branch on.
#[test]
fn nothing_to_compose_gives_an_empty_pair() {
    for paths in [vec![], vec!["/nonexistent/one.png".to_owned()]] {
        let pair = compose_hero_pair(&paths);
        assert!(pair.cover.is_none());
        assert!(pair.blur.is_none());
        assert!(pair.sample.accent_argb.is_none());
    }
}

#[test]
fn the_guard_admits_one_publish_per_set_and_forgets_on_leave() {
    let guard = MosaicGuard::default();
    let top_four = vec!["a.jpg".to_owned(), "b.jpg".to_owned()];

    assert!(guard.is_stale(&top_four));
    assert!(guard.claim(top_four.clone()), "the first publish of a set paints");
    assert!(!guard.is_stale(&top_four));
    assert!(!guard.claim(top_four.clone()), "a second compose of the same set must not");

    guard.forget();
    assert!(guard.is_stale(&top_four), "a section leave owes the next enter a recompose");
}
