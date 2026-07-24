//! Album-cover resolution for the presence card's `large_image`.
//!
//! Discord can't read local files, so the cover has to be an external `https://`
//! URL its CDN fetches server-side. We resolve one via a Deezer album search
//! (`media::deezer::search_album_cover`) and cache the result — bounded LRU,
//! keyed case-insensitively on `(artist, album)` — so a repeat play of the same
//! album never re-queries. The lookup is driven from the detector task only on a
//! track change; pause/resume/seek reuse the task's last resolved URL and never
//! reach here.

use std::num::NonZeroUsize;
use std::time::Duration;

use lru::LruCache;
use parking_lot::Mutex;

/// Bounded well inside the memory rules — 64 recently-played albums' URLs.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(64) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// Per-lookup budget. The shared client's own `read_timeout` is a minute (fine
/// for the updater's large downloads, far too slow for a presence update), so
/// this caps the presence path on top of it.
const ARTWORK_TIMEOUT: Duration = Duration::from_secs(2);

/// Lowercased `(artist, album)` — a case-insensitive cache key.
type ArtKey = (String, String);

/// The service's cover-URL cache. `None` = a definitive miss (Deezer answered,
/// matched nothing) so a repeat play doesn't re-query.
pub(super) type ArtworkCache = Mutex<LruCache<ArtKey, Option<String>>>;

pub(super) fn new_cache() -> ArtworkCache {
    Mutex::new(LruCache::new(ARTWORK_CACHE_CAP))
}

/// Resolve an album cover URL for `large_image`, cache-first.
///
/// On a miss, queries Deezer under a 2 s budget. Only a **definitive** result
/// (`Some` URL or a matched-nothing `None`) is cached — a timeout or transport
/// error caches nothing, so a momentary hiccup doesn't blank that album's cover
/// for the rest of the session.
pub(super) async fn resolve_album_cover(
    client: &reqwest::Client,
    cache: &ArtworkCache,
    artist: &str,
    album: &str,
) -> Option<String> {
    let key = (artist.to_lowercase(), album.to_lowercase());
    if let Some(hit) = cache.lock().get(&key) {
        return hit.clone();
    }

    let lookup = crate::media::deezer::search_album_cover(client, artist, album);
    match tokio::time::timeout(ARTWORK_TIMEOUT, lookup).await {
        Ok(Ok(url)) => {
            cache.lock().put(key, url.clone());
            url
        }
        Ok(Err(e)) => {
            log::debug!("discord: album cover lookup failed: {e}");
            None
        }
        Err(_) => {
            log::debug!("discord: album cover lookup timed out after {ARTWORK_TIMEOUT:?}");
            None
        }
    }
}
