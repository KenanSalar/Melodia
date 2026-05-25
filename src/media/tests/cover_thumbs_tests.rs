use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Write a solid-colour square PNG to a fresh temp dir; returns the dir
/// (kept alive by the caller) and the file path.
fn write_test_png(side: u32) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("cover.png");
    image::RgbImage::from_pixel(side, side, image::Rgb([120, 60, 200])).save(&path)?;
    Ok((tmp, path))
}

#[test]
fn new_decodes_to_row_tier_size() -> TestResult {
    let thumbs = CoverThumbs::new();
    let (_tmp, path) = write_test_png(600)?;
    let buf = thumbs
        .get_or_load_rgb8(&path)
        .ok_or("row-tier cover failed to decode")?;
    assert_eq!(buf.width(), THUMB_SIZE);
    assert_eq!(buf.height(), THUMB_SIZE);
    Ok(())
}

#[test]
fn with_config_decodes_to_requested_size() -> TestResult {
    let cap = NonZeroUsize::new(8).ok_or("cap must be > 0")?;
    let thumbs = CoverThumbs::with_config(512, cap);
    let (_tmp, path) = write_test_png(1000)?;
    let buf = thumbs
        .get_or_load_rgb8(&path)
        .ok_or("album-tier cover failed to decode")?;
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
