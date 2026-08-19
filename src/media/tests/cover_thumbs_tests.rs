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

// ── the scheduling lookup ──

/// How long a test waits on the decode pool. Generous rather than tight — the pool is two to four
/// threads shared with every other test in the binary, and the failure this guards is "never",
/// not "slowly".
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The whole point of the lookup, and the thing its callers cannot check for themselves: a miss
/// answers immediately, because for every caller the calling thread is the event loop.
#[test]
fn a_scheduled_miss_answers_with_the_placeholder() -> TestResult {
    let thumbs = Arc::new(CoverThumbs::new());
    let (_tmp, path) = write_test_png(600)?;
    let path = path.to_str().ok_or("temp path is not UTF-8")?;

    assert_eq!(
        thumbs.get_or_schedule_opt(Some(path)).size().width,
        0,
        "a miss must hand back the placeholder rather than decode on the calling thread"
    );
    Ok(())
}

/// The other half of the contract. A placeholder is only temporary because the batch lands *and*
/// the notifier fires — without the second the card never comes back, which is exactly what a
/// caller with no generation to bump would produce.
#[test]
fn a_scheduled_miss_lands_and_notifies() -> TestResult {
    let thumbs = Arc::new(CoverThumbs::new());
    let (_tmp, path) = write_test_png(600)?;
    let path = path.to_str().ok_or("temp path is not UTF-8")?;

    let (tx, rx) = std::sync::mpsc::channel();
    thumbs.set_decoded_notifier(move || {
        let _ = tx.send(());
    });

    assert_eq!(thumbs.get_or_schedule_opt(Some(path)).size().width, 0);
    rx.recv_timeout(DRAIN_TIMEOUT).map_err(|_| "the scheduled decode never announced itself")?;

    assert_eq!(
        thumbs.get_or_schedule_opt(Some(path)).size().width,
        ROW_THUMB_SIZE,
        "the lookup after the announcement must answer with what the batch decoded"
    );
    Ok(())
}

/// A remembered failure is a cache *hit*, so it re-queues nothing. Were it a miss, every bump
/// would re-queue it and its own drain would bump again — a broken cover would spin the decode
/// pool for the life of the session.
#[test]
fn a_remembered_failure_never_re_queues() -> TestResult {
    let thumbs = Arc::new(CoverThumbs::new());
    let missing = "/nonexistent/melodia/cover-missing.png";

    let (tx, rx) = std::sync::mpsc::channel();
    thumbs.set_decoded_notifier(move || {
        let _ = tx.send(());
    });

    let _ = thumbs.get_or_schedule_opt(Some(missing));
    rx.recv_timeout(DRAIN_TIMEOUT).map_err(|_| "the failed decode never announced itself")?;
    assert!(thumbs.cache.lock().contains(Path::new(missing)), "a failure must be remembered");

    let _ = thumbs.get_or_schedule_opt(Some(missing));
    assert!(
        thumbs.pending.lock().queued.is_empty(),
        "a remembered failure must not re-enter the queue it just came out of"
    );
    Ok(())
}

/// An empty path is the "this row has no artwork" case and reaches neither the cache nor the
/// pool — every model row carries one, so a queued entry per artless track is the cost.
#[test]
fn an_empty_path_schedules_nothing() {
    let thumbs = Arc::new(CoverThumbs::new());
    for path in [None, Some("")] {
        assert_eq!(thumbs.get_or_schedule_opt(path).size().width, 0);
    }
    assert!(thumbs.pending.lock().queued.is_empty());
    assert_eq!(thumbs.cache.lock().len(), 0);
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
