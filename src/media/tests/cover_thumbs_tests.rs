use super::*;
use crate::test_support::write_test_png;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn new_decodes_to_row_tier_size() -> TestResult {
    let thumbs = CoverThumbs::new();
    let (_tmp, path) = write_test_png(600)?;
    let buf = thumbs.get_or_load_rgb8(&path).ok_or("row-tier cover failed to decode")?;
    assert_eq!(buf.width(), ROW_THUMB_SIZE);
    assert_eq!(buf.height(), ROW_THUMB_SIZE);
    Ok(())
}

/// A cover smaller than the tier is drawn at the tier's size either way — `image-fit: cover` on a
/// GPU texture magnifies it at draw time. Padding the buffer out to the tile only spends memory
/// on pixels carrying no information, and the box-filtered upscale it used to bake in looks
/// slightly worse than the bilinear one the GPU does anyway.
#[test]
fn a_source_smaller_than_the_tier_keeps_its_own_size() -> TestResult {
    let cap = NonZeroUsize::new(8).ok_or("cap must be > 0")?;
    let thumbs = CoverThumbs::with_config(448, cap);
    let (_tmp, path) = write_test_png(128)?;

    let buf = thumbs.get_or_load_rgb8(&path).ok_or("small cover failed to decode")?;

    assert_eq!((buf.width(), buf.height()), (128, 128), "the tier must not enlarge a source");
    Ok(())
}

#[test]
fn with_config_decodes_to_requested_size() -> TestResult {
    let cap = NonZeroUsize::new(8).ok_or("cap must be > 0")?;
    let thumbs = CoverThumbs::with_config(512, cap);
    let (_tmp, path) = write_test_png(1000)?;
    let buf = thumbs.get_or_load_rgb8(&path).ok_or("album-tier cover failed to decode")?;
    assert_eq!(buf.width(), 512);
    assert_eq!(buf.height(), 512);
    Ok(())
}

#[test]
fn missing_file_returns_none_and_caches_failure() {
    let thumbs = CoverThumbs::new();
    let missing = Path::new("/nonexistent/melodia/cover-missing.png");

    // Undecodable path yields `None`...
    assert!(thumbs.get_or_load_rgb8(missing).is_none());
    // ...and the failure is remembered so a refilter doesn't re-open it.
    assert!(thumbs.cache.lock().contains(missing));
    assert!(thumbs.get_or_load_rgb8(missing).is_none());
}

#[test]
fn with_config_honours_lru_capacity() -> TestResult {
    let cap = NonZeroUsize::new(2).ok_or("cap must be > 0")?;
    let thumbs = CoverThumbs::with_config(128, cap);
    // Three distinct (failing) paths into a cap-2 cache — oldest evicted.
    for i in 0..3 {
        let p = PathBuf::from(format!("/nonexistent/melodia/{i}.png"));
        let _ = thumbs.get_or_load_rgb8(&p);
    }
    let cache = thumbs.cache.lock();
    assert_eq!(cache.len(), 2);
    assert!(!cache.contains(Path::new("/nonexistent/melodia/0.png")));
    Ok(())
}

#[test]
fn clear_empties_the_cache() -> TestResult {
    let thumbs = CoverThumbs::new();
    let (_tmp, path) = write_test_png(120)?;
    let _ = thumbs.get_or_load_rgb8(&path);
    assert!(thumbs.cache.lock().contains(&path));
    thumbs.clear();
    assert_eq!(thumbs.cache.lock().len(), 0);
    Ok(())
}

/// The oversized-decode gate is driven entirely by this probe, so a probe that
/// answered `None` for real files would leave it permanently disengaged while
/// everything still built and every cover still decoded.
#[test]
fn source_pixels_reads_dimensions_from_the_header() -> TestResult {
    let (_tmp, path) = write_test_png(120)?;
    assert_eq!(source_pixels(&path), Some(120 * 120));
    assert_eq!(source_pixels(Path::new("/nonexistent/melodia/none.png")), None);
    Ok(())
}

/// The row tier feeds a 36 px track-row tile and a now-playing bar tile that
/// clamps at 46 px, so the 1× size only has to beat the larger of those. The
/// `HiDPI` size is 2× the *row* tile and deliberately short of 2× the bar tile
/// — that ratio predates the split and is left where it was, this change being
/// about what a 1× display pays.
#[test]
fn row_cover_size_steps_up_for_hidpi_and_covers_the_bar_tile() {
    const ROW_TILE: u32 = 36;
    const BAR_TILE_MAX: u32 = 46;

    assert_eq!(row_cover_size(1.0), ROW_THUMB_SIZE);
    assert_eq!(row_cover_size(2.0), ROW_THUMB_SIZE_HIDPI);
    // A fractional scale rounds up — softness is the worse failure.
    assert_eq!(row_cover_size(1.5), ROW_THUMB_SIZE_HIDPI);

    assert!(row_cover_size(1.0) > BAR_TILE_MAX);
    assert!(row_cover_size(2.0) >= ROW_TILE * 2);
    assert!(row_cover_size(2.0) > row_cover_size(1.0));
}

/// Every cached buffer was decoded at the old size, so a genuine change has to
/// drop them — a retune that kept them would serve the wrong resolution for the
/// rest of the session. A no-op change must keep them, the one call site
/// running on every boot.
#[test]
fn set_thumb_size_drops_stale_buffers_only_when_the_size_moves() -> TestResult {
    let thumbs = CoverThumbs::new();
    let (_tmp, path) = write_test_png(120)?;
    let _ = thumbs.get_or_load_rgb8(&path);
    assert_eq!(thumbs.cache.lock().len(), 1);

    thumbs.set_thumb_size(ROW_THUMB_SIZE);
    assert_eq!(thumbs.cache.lock().len(), 1, "a no-op retune kept the tier");

    thumbs.set_thumb_size(ROW_THUMB_SIZE_HIDPI);
    assert_eq!(thumbs.cache.lock().len(), 0);

    let buf = thumbs.get_or_load_rgb8(&path).ok_or("cover failed to decode at the retuned size")?;
    assert_eq!(buf.width(), ROW_THUMB_SIZE_HIDPI);
    Ok(())
}

#[test]
fn resize_shrinks_cap_and_evicts() -> TestResult {
    let cap = NonZeroUsize::new(4).ok_or("cap must be > 0")?;
    let thumbs = CoverThumbs::with_config(96, cap);
    for i in 0..4 {
        let p = PathBuf::from(format!("/nonexistent/melodia/{i}.png"));
        let _ = thumbs.get_or_load_rgb8(&p);
    }
    assert_eq!(thumbs.cache.lock().len(), 4);

    let smaller = NonZeroUsize::new(2).ok_or("cap must be > 0")?;
    thumbs.resize(smaller);
    let cache = thumbs.cache.lock();
    assert_eq!(cache.cap(), smaller);
    assert_eq!(cache.len(), 2);
    Ok(())
}
