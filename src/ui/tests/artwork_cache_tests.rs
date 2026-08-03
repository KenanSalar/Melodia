use super::*;

/// A tier at the Now Playing shape — the specifics don't matter to any test
/// here, which are all about the LRU and the remembered-failure rule.
fn test_cache(capacity: usize) -> ArtworkCache {
    let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
    ArtworkCache::new(
        cap,
        BlurSpec {
            height: BLUR_TARGET,
            sigma: 24.0,
        },
    )
}

#[test]
fn missing_file_returns_none_and_is_cached() {
    let artwork = test_cache(8);
    let missing = Path::new("/nonexistent/melodia/cover-does-not-exist.jpg");

    // A path that can't be opened decodes to `None`...
    assert!(artwork.get_or_decode(missing).is_none());

    // ...and the failure is remembered, so the entry is now present in
    // the cache (a second call still returns `None` without re-opening).
    assert!(artwork.contains(missing));
    assert!(artwork.get_or_decode(missing).is_none());
}

#[test]
fn lru_evicts_beyond_capacity() {
    const CAP: usize = 8;
    let artwork = test_cache(CAP);
    // Insert one more failure entry than the cap; the oldest must be gone.
    for i in 0..=CAP {
        let p = std::path::PathBuf::from(format!("/nonexistent/melodia/{i}.jpg"));
        let _ = artwork.get_or_decode(&p);
    }
    assert_eq!(artwork.len(), CAP);
    assert!(!artwork.contains(Path::new("/nonexistent/melodia/0.jpg")));
}

#[test]
fn clear_empties_the_cache() {
    let artwork = test_cache(8);
    let p = Path::new("/nonexistent/melodia/np-clear.jpg");
    // Populate with a (failure) entry, then clear it back out.
    let _ = artwork.get_or_decode(p);
    assert!(artwork.contains(p));
    artwork.clear();
    assert_eq!(artwork.len(), 0);
}

/// Both tiers are newtypes whose whole content is a capacity and a
/// [`BlurSpec`], so nothing about them fails to compile — this walks each one's
/// two forwards so a tier wired to nothing is a failing test rather than a
/// silently coverless one.
#[test]
fn both_tiers_forward_to_the_cache_they_wrap() {
    let np = crate::ui::now_playing_artwork::NowPlayingArtwork::new();
    let detail = crate::ui::detail_artwork::DetailArtwork::new();
    let missing = Path::new("/nonexistent/melodia/tier-forward.jpg");

    assert!(np.get_or_decode(missing).is_none());
    assert!(detail.get_or_decode(missing).is_none());
    // The remembered failure is the observable half of the forward: a second
    // lookup answers from the cache rather than re-opening the file.
    assert!(np.get_or_decode(missing).is_none());
    assert!(detail.get_or_decode(missing).is_none());

    np.clear();
    detail.clear();
}
