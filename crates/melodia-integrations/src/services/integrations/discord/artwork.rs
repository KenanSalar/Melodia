//! The card's cover, which has to be an `https://` URL since Discord's CDN fetches it
//! server-side. Deezer first and iTunes on a miss, their misses differing, behind a bounded LRU
//! so a repeat play of an album never asks again.

use std::future::Future;
use std::num::NonZeroUsize;
use std::time::Duration;

use lru::LruCache;
use parking_lot::Mutex;

use melodia_core::error::{AppError, describe};

/// Albums remembered, each holding a URL rather than an image.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(64) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// Per provider, under the shared client's minute-long `read_timeout`, which suits the updater's
/// downloads and not a presence update. Only a Deezer miss can spend it twice.
const PROVIDER_TIMEOUT: Duration = Duration::from_millis(1500);

/// Lowercased `(artist, album)` — a case-insensitive cache key.
type ArtKey = (String, String);

/// `None` is a miss both providers agreed on.
pub(super) type ArtworkCache = Mutex<LruCache<ArtKey, Option<String>>>;

pub(super) fn new_cache() -> ArtworkCache {
    Mutex::new(LruCache::new(ARTWORK_CACHE_CAP))
}

/// `Miss` is a definitive answer; a timeout or transport error is `Unavailable`.
enum Lookup {
    Found(String),
    Miss,
    Unavailable,
}

async fn run_lookup(
    lookup: impl Future<Output = Result<Option<String>, AppError>>,
    provider: &str,
) -> Lookup {
    match tokio::time::timeout(PROVIDER_TIMEOUT, lookup).await {
        Ok(Ok(Some(url))) => Lookup::Found(url),
        Ok(Ok(None)) => Lookup::Miss,
        Ok(Err(e)) => {
            log::debug!("discord: {provider} cover lookup failed: {}", describe(&e));
            Lookup::Unavailable
        }
        Err(_) => {
            log::debug!("discord: {provider} cover lookup timed out after {PROVIDER_TIMEOUT:?}");
            Lookup::Unavailable
        }
    }
}

/// Caches only a definitive answer, so a momentary hiccup doesn't blank an album's cover for the
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
        melodia_net::media::fetch::deezer::search_album_cover(client, artist, album),
        "deezer",
    )
    .await;
    let deezer_definite_miss = matches!(deezer, Lookup::Miss);
    if let Lookup::Found(url) = deezer {
        cache.lock().put(key, Some(url.clone()));
        return Some(url);
    }

    let itunes = run_lookup(
        melodia_net::media::fetch::itunes::search_album_cover(client, artist, album),
        "itunes",
    )
    .await;
    match itunes {
        Lookup::Found(url) => {
            cache.lock().put(key, Some(url.clone()));
            Some(url)
        }
        Lookup::Miss if deezer_definite_miss => {
            cache.lock().put(key, None);
            None
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests/artwork_tests.rs"]
mod tests;
