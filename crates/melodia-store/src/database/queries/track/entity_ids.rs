//! The track ids behind a set of entities, for the card grids' right-click actions.
//!
//! Id-only and batched, where the `for_list` siblings are per-entity and return whole rows: queueing
//! five albums is one statement rather than five that fetch a projection the caller throws away.
//!
//! **Each returns `(entity_id, track_id)` pairs, per-entity order preserved, grouping left to the
//! caller.** `chunked_in_query` splits on the *entity* ids, so every one of an entity's rows lands
//! in a single chunk and its `ORDER BY` survives; the order the entities themselves come back in
//! does not, and is the caller's to restore from the ids it asked with.

use crate::database::{DbPool, chunked_in_query};
use melodia_core::error::AppError;

/// Album tracks, disc/track-number ordered, matching what the album detail shows.
pub async fn track_ids_by_albums(
    db: &DbPool,
    album_ids: &[i64],
) -> Result<Vec<(i64, i64)>, AppError> {
    chunked_in_query(db.read(), album_ids, |placeholders| {
        format!(
            "SELECT album_id, id FROM tracks \
             WHERE album_id IN ({placeholders}) \
             ORDER BY disc_number ASC, track_number ASC"
        )
    })
    .await
}

/// Artist tracks in natural order.
///
/// **Joins where [`super::get_tracks_by_artist_for_list`] deliberately doesn't**, the artist id
/// having to be in the projection for the caller to group on. So a row repeats both across
/// selected artists and inside one that credits the same artist twice, and the caller's dedupe is
/// what covers the second case here.
pub async fn track_ids_by_artists(
    db: &DbPool,
    artist_ids: &[i64],
) -> Result<Vec<(i64, i64)>, AppError> {
    chunked_in_query(db.read(), artist_ids, |placeholders| {
        format!(
            "SELECT ta.artist_id, t.id FROM tracks t \
             JOIN track_artists ta ON ta.track_id = t.id \
             WHERE ta.artist_id IN ({placeholders}) \
             ORDER BY t.sort_key COLLATE NOCASE ASC"
        )
    })
    .await
}

/// Genre tracks in natural order, joined and deduped for the reason the artist arm above gives.
pub async fn track_ids_by_genres(
    db: &DbPool,
    genre_ids: &[i64],
) -> Result<Vec<(i64, i64)>, AppError> {
    chunked_in_query(db.read(), genre_ids, |placeholders| {
        format!(
            "SELECT tg.genre_id, t.id FROM tracks t \
             JOIN track_genres tg ON tg.track_id = t.id \
             WHERE tg.genre_id IN ({placeholders}) \
             ORDER BY t.sort_key COLLATE NOCASE ASC"
        )
    })
    .await
}

/// Manual-playlist membership in playlist order. **Smart playlists have no `playlist_items` rows**
/// and come back empty here; resolving them means their stored criteria, which is why the caller
/// splits the two before asking.
pub async fn track_ids_by_playlists(
    db: &DbPool,
    playlist_ids: &[i64],
) -> Result<Vec<(i64, i64)>, AppError> {
    chunked_in_query(db.read(), playlist_ids, |placeholders| {
        format!(
            "SELECT playlist_id, track_id FROM playlist_items \
             WHERE playlist_id IN ({placeholders}) \
             ORDER BY position ASC"
        )
    })
    .await
}

/// Every track under `dir_path`, subdirectories included, in path order.
///
/// Recursive where [`super::get_tracks_in_directory`] deliberately isn't: that one answers what
/// Browse *lists* for a directory it has navigated into, this one answers what "play this folder"
/// means for a card, and an artist folder holding only album subfolders would otherwise play
/// nothing at all.
///
/// Shares that function's platform invariant through `list::directory_like_prefix`: `file_path`
/// is stored with the platform's native separator, so the pattern carries it too.
pub async fn track_ids_under_directory(db: &DbPool, dir_path: &str) -> Result<Vec<i64>, AppError> {
    let pattern = format!("{}%", super::list::directory_like_prefix(dir_path));
    let ids = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM tracks WHERE file_path LIKE ? ESCAPE '\\' ORDER BY file_path ASC",
    )
    .bind(&pattern)
    .fetch_all(db.read())
    .await?;
    Ok(ids)
}
