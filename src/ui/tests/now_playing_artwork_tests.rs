use super::*;

#[test]
fn missing_file_returns_none_and_is_cached() {
    let artwork = NowPlayingArtwork::new();
    let missing = Path::new("/nonexistent/melodia/cover-does-not-exist.jpg");

    // A path that can't be opened decodes to `None`...
    assert!(artwork.get_or_decode(missing).is_none());

    // ...and the failure is remembered, so the entry is now present in
    // the cache (a second call still returns `None` without re-opening).
    assert!(artwork.cache.lock().contains(missing));
    assert!(artwork.get_or_decode(missing).is_none());
}

#[test]
fn lru_evicts_beyond_capacity() {
    let artwork = NowPlayingArtwork::new();
    // Insert one more failure entry than the cap; the oldest must be gone.
    for i in 0..=ARTWORK_CACHE_CAP.get() {
        let p = std::path::PathBuf::from(format!("/nonexistent/melodia/{i}.jpg"));
        let _ = artwork.get_or_decode(&p);
    }
    let cache = artwork.cache.lock();
    assert_eq!(cache.len(), ARTWORK_CACHE_CAP.get());
    assert!(!cache.contains(Path::new("/nonexistent/melodia/0.jpg")));
}

#[test]
fn clear_empties_the_cache() {
    let artwork = NowPlayingArtwork::new();
    let p = Path::new("/nonexistent/melodia/np-clear.jpg");
    // Populate with a (failure) entry, then clear it back out.
    let _ = artwork.get_or_decode(p);
    assert!(artwork.cache.lock().contains(p));
    artwork.clear();
    assert_eq!(artwork.cache.lock().len(), 0);
}
