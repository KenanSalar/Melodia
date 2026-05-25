use std::sync::Arc;

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;
use crate::error::AppResult;
use crate::media::deezer;

/// Fetch artist images from Deezer for artists that don't have one yet.
/// Processes in batches of 5 concurrent requests with rate limiting. Uses the
/// shared `reqwest::Client` from `AppState` so the connection pool is reused
/// across every HTTP-using service.
pub async fn fetch_artist_images(
    paths: &Paths,
    db: &DbPool,
    client: &reqwest::Client,
) -> AppResult<u32> {
    let artists = queries::artist::get_artists_without_images(db).await?;
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
                    Ok(None) => return (id, name, None),
                    Err(e) => {
                        log::warn!("Deezer search failed for '{name}': {e}");
                        return (id, name, None);
                    }
                };

                let path = match deezer::download_and_cache_artist_image(&client, &url, &dir).await
                {
                    Ok(Some(path)) => Some(path),
                    Ok(None) => None,
                    Err(e) => {
                        log::warn!("Failed to download image for '{name}': {e}");
                        None
                    }
                };

                (id, name, path)
            });
        }

        while let Some(result) = set.join_next().await {
            match result {
                Ok((id, name, Some(path))) => {
                    if let Err(e) = queries::artist::update_artist_image_path(db, id, &path).await {
                        log::warn!("Failed to update artist image path for '{name}': {e}");
                    } else {
                        fetched_count += 1;
                    }
                }
                Ok((_id, _name, None)) => {}
                Err(e) => log::error!("Artist image fetch task failed: {e}"),
            }
        }
    }

    // TODO(phase 2): emit "artist-images-fetched" to UI when wired.
    Ok(fetched_count)
}

/// Spawn a background task to fetch artist images. Fire-and-forget.
/// `client` is the shared `AppState::http_client`.
pub fn spawn_fetch(paths: Arc<Paths>, db: DbPool, client: reqwest::Client) {
    tokio::spawn(async move {
        if let Err(e) = fetch_artist_images(&paths, &db, &client).await {
            log::warn!("Background artist image fetch failed: {e}");
        }
    });
}
