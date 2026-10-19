//! What a list or a card grid holding only ids reads: its ordered ids, the rows behind a page of
//! them, and what its hero states about the whole set.

use crate::library::smart_playlists;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::artist::{ArtistStats, FavoriteArtist};
use melodia_core::entities::genre::GenreStats;
use melodia_core::entities::grid_order::{AlbumSort, ArtistSort, FavoriteArtistSort, GenreSort};
use melodia_core::entities::playlist::PlaylistStats;
use melodia_core::entities::track::{MostPlayedFavorite, TrackListRow};
use melodia_core::entities::track_list::{
    MostPlayedTotals, ScopeFacts, SortDir, TrackListScope, TrackOrder,
};
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;
use melodia_store::database::queries::entity_keyset;
use melodia_store::database::{DbPool, queries};

/// Every track in `scope` that `needle` matches, as ids in `order`.
pub async fn track_ids(
    db: &DbPool,
    scope: TrackListScope,
    order: TrackOrder,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    queries::track::track_ids(db, scope, order, needle).await
}

/// The rows behind `ids`, in the order asked. An id whose row has gone since the ids were read is
/// dropped, so a short page is a library that changed under the list, not an error.
pub async fn rows_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<TrackListRow>, AppError> {
    queries::track::get_track_list_rows_by_ids(db, ids).await
}

/// The ids in `ids` that still have a row, in the order asked and each once, which is what a list
/// handed to the queue becomes: [`rows_by_ids`]'s answer without fetching a row.
pub async fn existing_in_order(db: &DbPool, ids: &[i64]) -> Result<Vec<i64>, AppError> {
    let mut existing = queries::track::existing_ids(db, ids).await?;
    Ok(ids.iter().copied().filter(|id| existing.remove(id)).collect())
}

/// How many tracks `scope` holds, how long they run and how many artists and albums they span.
pub async fn facts(db: &DbPool, scope: TrackListScope) -> Result<ScopeFacts, AppError> {
    queries::track::scope_facts(db, scope).await
}

/// Every genre line `scope`'s tracks carry, with how many carry it, the commonest first.
pub async fn genre_lines(
    db: &DbPool,
    scope: TrackListScope,
) -> Result<Vec<(String, i64)>, AppError> {
    queries::track::genre_lines(db, scope).await
}

/// The library-wide Most Played ranking that `needle` matches.
pub async fn most_played_ids(db: &DbPool, needle: &Needle) -> Result<Vec<i64>, AppError> {
    queries::track::most_played_ids(db, needle).await
}

/// [`most_played_ids`] over the favorites alone.
pub async fn most_played_favorite_ids(db: &DbPool, needle: &Needle) -> Result<Vec<i64>, AppError> {
    queries::track::most_played_favorite_ids(db, needle).await
}

/// What the library-wide Most Played ranking adds up to, unfiltered.
pub async fn most_played_totals(db: &DbPool) -> Result<MostPlayedTotals, AppError> {
    queries::track::most_played_totals(db).await
}

/// [`most_played_totals`] over the favorites alone.
pub async fn most_played_favorite_totals(db: &DbPool) -> Result<MostPlayedTotals, AppError> {
    queries::track::most_played_favorite_totals(db).await
}

/// The Most Played cards behind `ids`, in the order asked, dropping any whose row has gone.
pub async fn most_played_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<MostPlayedFavorite>, AppError> {
    queries::track::get_most_played_by_ids(db, ids).await
}

/// The Albums grid's ids in `sort`, narrowed by `needle`.
pub async fn album_ids(
    db: &DbPool,
    sort: AlbumSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    entity_keyset::album_ids(db, sort, dir, needle).await
}

/// The Artists grid's ids in `sort`, narrowed by `needle`.
pub async fn artist_ids(
    db: &DbPool,
    sort: ArtistSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    entity_keyset::artist_ids(db, sort, dir, needle).await
}

/// The Genres grid's ids in `sort`, narrowed by `needle`.
pub async fn genre_ids(
    db: &DbPool,
    sort: GenreSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    entity_keyset::genre_ids(db, sort, dir, needle).await
}

/// The Playlists grid's ids by when each last changed, narrowed by `needle`.
pub async fn playlist_ids(
    db: &DbPool,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    entity_keyset::playlist_ids(db, dir, needle).await
}

/// Favorites' Favorite Artists ids in `sort`, narrowed by `needle`.
pub async fn favorite_artist_ids(
    db: &DbPool,
    sort: FavoriteArtistSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    entity_keyset::favorite_artist_ids(db, sort, dir, needle).await
}

/// How many albums the Albums grid lists unfiltered.
pub async fn album_count(db: &DbPool) -> Result<i64, AppError> {
    entity_keyset::album_count(db).await
}

/// How many artists the Artists grid lists unfiltered.
pub async fn artist_count(db: &DbPool) -> Result<i64, AppError> {
    entity_keyset::artist_count(db).await
}

/// How many genres the Genres grid lists unfiltered.
pub async fn genre_count(db: &DbPool) -> Result<i64, AppError> {
    entity_keyset::genre_count(db).await
}

/// How many playlists there are, smart ones included.
pub async fn playlist_count(db: &DbPool) -> Result<i64, AppError> {
    entity_keyset::playlist_count(db).await
}

/// How many artists are credited on a favourite.
pub async fn favorite_artist_count(db: &DbPool) -> Result<i64, AppError> {
    entity_keyset::favorite_artist_count(db).await
}

/// The album cards behind `ids`, in the order asked, dropping any whose row has gone.
pub async fn albums_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<AlbumStats>, AppError> {
    queries::album::get_albums_by_ids(db, ids).await
}

/// The artist cards behind `ids`, in the order asked, dropping any whose row has gone.
pub async fn artists_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<ArtistStats>, AppError> {
    queries::artist::get_artists_by_ids(db, ids).await
}

/// The genre cards behind `ids`, in the order asked, dropping any whose row has gone.
pub async fn genres_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<GenreStats>, AppError> {
    queries::genre::get_genres_by_ids(db, ids).await
}

/// The playlist cards behind `ids`, in the order asked, each smart one carrying its live count.
pub async fn playlists_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<PlaylistStats>, AppError> {
    let mut playlists = queries::playlist::get_playlists_by_ids(db, ids).await?;
    let smart: Vec<usize> =
        playlists.iter().enumerate().filter(|(_, p)| p.is_smart).map(|(i, _)| i).collect();
    smart_playlists::recount(db, &mut playlists, &smart).await;
    Ok(playlists)
}

/// The Favorite Artists cards behind `ids`, in the order asked, dropping any with no favourite
/// left.
pub async fn favorite_artists_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<FavoriteArtist>, AppError> {
    queries::artist::get_favorite_artists_by_ids(db, ids).await
}
