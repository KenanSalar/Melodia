//! Album-cover resolution for the presence card's `large_image`.
//!
//! Discord can't read local files, so the cover has to be an external `https://`
//! URL its CDN fetches server-side. We resolve one via a Deezer album search
//! (`media::deezer::search_album_cover`), falling back to the iTunes Search API
//! (`media::itunes::search_album_cover`) when Deezer has no match — their misses
//! differ, so the second provider fills gaps the first leaves. The result is cached
//! (bounded LRU, keyed case-insensitively on `(artist, album)`) so a repeat play of
//! the same album never re-queries. The lookup is driven from the detector task only
//! on a track change; pause/resume/seek reuse the task's last resolved URL and never
//! reach here.

use std::future::Future;
use std::num::NonZeroUsize;
use std::time::Duration;

use lru::LruCache;
use parking_lot::Mutex;

use crate::error::AppError;

/// Bounded well inside the memory rules — 64 recently-played albums' URLs.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(64) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// Per-provider budget. The shared client's own `read_timeout` is a minute (fine
/// for the updater's large downloads, far too slow for a presence update), so this
/// caps each hop on top of it. Two providers means a worst case of twice this — but
/// only on the Deezer-miss path; a Deezer hit returns without touching iTunes.
const PROVIDER_TIMEOUT: Duration = Duration::from_millis(1500);

/// Lowercased `(artist, album)` — a case-insensitive cache key.
type ArtKey = (String, String);

/// The service's cover-URL cache. `None` = a definitive miss (both providers
/// answered and neither had it) so a repeat play doesn't re-query.
pub(super) type ArtworkCache = Mutex<LruCache<ArtKey, Option<String>>>;

pub(super) fn new_cache() -> ArtworkCache {
    Mutex::new(LruCache::new(ARTWORK_CACHE_CAP))
}

/// The outcome of a single provider lookup, classified for the cache rule below.
/// `Miss` is a definitive answer (the provider replied, nothing matched); everything
/// non-definitive — a timeout or transport error — is `Unavailable`.
enum Lookup {
    Found(String),
    Miss,
    Unavailable,
}

/// Run one provider lookup under the per-provider budget and classify the result.
async fn run_lookup(
    lookup: impl Future<Output = Result<Option<String>, AppError>>,
    provider: &str,
) -> Lookup {
    match tokio::time::timeout(PROVIDER_TIMEOUT, lookup).await {
        Ok(Ok(Some(url))) => Lookup::Found(url),
        Ok(Ok(None)) => Lookup::Miss,
        Ok(Err(e)) => {
            log::debug!("discord: {provider} cover lookup failed: {e}");
            Lookup::Unavailable
        }
        Err(_) => {
            log::debug!("discord: {provider} cover lookup timed out after {PROVIDER_TIMEOUT:?}");
            Lookup::Unavailable
        }
    }
}

/// Resolve an album cover URL for `large_image`, cache-first.
///
/// On a miss, queries Deezer, then iTunes if Deezer had no match. Only a
/// **definitive** result is cached: a `Some` URL from either provider, or a `None`
/// when **both** answered and neither had it. A timeout or transport error on either
/// hop caches nothing, so a momentary hiccup doesn't blank that album's cover for the
/// rest of the session.
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

    let deezer = run_lookup(
        crate::media::deezer::search_album_cover(client, artist, album),
        "deezer",
    )
    .await;
    let deezer_definite_miss = matches!(deezer, Lookup::Miss);
    if let Lookup::Found(url) = deezer {
        cache.lock().put(key, Some(url.clone()));
        return Some(url);
    }

    let itunes = run_lookup(
        crate::media::itunes::search_album_cover(client, artist, album),
        "itunes",
    )
    .await;
    match itunes {
        Lookup::Found(url) => {
            cache.lock().put(key, Some(url.clone()));
            Some(url)
        }
        // Cache a definitive miss only when both providers answered empty.
        Lookup::Miss if deezer_definite_miss => {
            cache.lock().put(key, None);
            None
        }
        _ => None,
    }
}
