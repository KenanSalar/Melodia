use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;
use crate::error::AppResult;
use crate::media::deezer;

/// Session-scoped negative memo: artist ids whose Deezer search returned a
/// definitive "no match" this session. `spawn_fetch` runs after every scan
/// completion, so without this every image-less artist is re-queried over
/// the network per scan — and the answer won't change until tags change
/// (which usually creates new artist rows anyway). Transient failures
/// (transport/HTTP errors, failed downloads) are deliberately NOT memoized
/// so a later scan retries them. Cleared only by process restart; bounded
/// by the artist-table size (bare `i64`s).
fn attempted_no_match() -> &'static Mutex<HashSet<i64>> {
    static ATTEMPTED: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    ATTEMPTED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Per-artist result of one Deezer round-trip.
enum FetchOutcome {
    /// Image found, downloaded, and cached at this path.
    Cached(String),
    /// Deezer answered definitively with no usable result — memoized.
    NoMatch,
    /// Transport/HTTP/download failure — retry on a later scan.
    Transient,
}

/// Fetch artist images from Deezer for artists that don't have one yet.
/// Processes in batches of 5 concurrent requests with rate limiting. Uses the
/// shared `reqwest::Client` from `AppState` so the connection pool is reused
/// across every HTTP-using service.
pub async fn fetch_artist_images(
    paths: &Paths,
    db: &DbPool,
    client: &reqwest::Client,
) -> AppResult<u32> {
    let mut artists = queries::artist::get_artists_without_images(db).await?;
    {
        let attempted = attempted_no_match().lock();
        artists.retain(|a| !attempted.contains(&a.id));
    }
    if artists.is_empty() {
        return Ok(0);
    }

    let artists_dir = paths.artists_dir.clone();
    let mut fetched_count: u32 = 0;

    for (batch_idx, batch) in artists.chunks(5).enumerate() {
        if batch_idx > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let mut set = tokio::task::JoinSet::new();

        for artist in batch {
            let client = client.clone();
            let name = artist.name.clone();
            let id = artist.id;
            let dir = artists_dir.clone();

            set.spawn(async move {
                let url = match deezer::search_artist_image_url(&client, &name).await {
                    Ok(Some(url)) => url,
                    Ok(None) => return (id, name, FetchOutcome::NoMatch),
                    Err(e) => {
                        log::warn!("Deezer search failed for '{name}': {e}");
                        return (id, name, FetchOutcome::Transient);
                    }
                };

                match deezer::download_and_cache_artist_image(&client, &url, &dir).await {
                    Ok(Some(path)) => (id, name, FetchOutcome::Cached(path)),
                    // A found URL that yielded no cacheable image could be a
                    // CDN hiccup — treat as transient so a later scan retries.
                    Ok(None) => (id, name, FetchOutcome::Transient),
                    Err(e) => {
                        log::warn!("Failed to download image for '{name}': {e}");
                        (id, name, FetchOutcome::Transient)
                    }
                }
            });
        }

        while let Some(result) = set.join_next().await {
            match result {
                Ok((id, name, FetchOutcome::Cached(path))) => {
                    if let Err(e) = queries::artist::update_artist_image_path(db, id, &path).await {
                        log::warn!("Failed to update artist image path for '{name}': {e}");
                    } else {
                        fetched_count += 1;
                    }
                }
                Ok((id, _name, FetchOutcome::NoMatch)) => {
                    attempted_no_match().lock().insert(id);
                }
                Ok((_id, _name, FetchOutcome::Transient)) => {}
                Err(e) => log::error!("Artist image fetch task failed: {e}"),
            }
        }
    }

    // Fetched images land in the DB but nothing signals the UI, so a grid
    // that's already painted won't pick them up until its next refresh.
    Ok(fetched_count)
}

/// Spawn a background task to fetch artist images. Fire-and-forget.
/// `client` is the shared client from `AppState::http_client()`.
pub fn spawn_fetch(paths: Arc<Paths>, db: DbPool, client: reqwest::Client) {
    tokio::spawn(async move {
        if let Err(e) = fetch_artist_images(&paths, &db, &client).await {
            log::warn!("Background artist image fetch failed: {e}");
        }
    });
}
