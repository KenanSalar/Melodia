//! Whole-row `SELECT *` fetches, kept for unit-test fixtures only: assertions need a full `Track`,
//! where production reads the narrower projections beside this file.

use std::collections::HashMap;
use std::sync::LazyLock;

use super::list::TRACK_LIST_ORDER;
use crate::database::{DbPool, chunked_in_query, sql};
use melodia_core::entities::track;
use melodia_core::error::AppError;

static ALL_TRACKS: LazyLock<String> =
    LazyLock::new(|| format!("SELECT * FROM tracks ORDER BY {TRACK_LIST_ORDER}"));

pub async fn get_all_tracks(db: &DbPool) -> Result<Vec<track::Track>, AppError> {
    db.read(|conn| sql::fetch_all(conn, ALL_TRACKS.as_str(), ())).await
}

pub async fn get_tracks_by_album(
    db: &DbPool,
    album_id: i64,
) -> Result<Vec<track::Track>, AppError> {
    db.read(move |conn| {
        sql::fetch_all(
            conn,
            "SELECT * FROM tracks WHERE album_id = ? ORDER BY disc_number ASC, track_number ASC",
            (album_id,),
        )
    })
    .await
}

static TRACKS_BY_ARTIST: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT * FROM tracks \
         WHERE id IN (SELECT track_id FROM track_artists WHERE artist_id = ?) \
         ORDER BY {TRACK_LIST_ORDER}"
    )
});

pub async fn get_tracks_by_artist(
    db: &DbPool,
    artist_id: i64,
) -> Result<Vec<track::Track>, AppError> {
    db.read(move |conn| sql::fetch_all(conn, TRACKS_BY_ARTIST.as_str(), (artist_id,))).await
}

static TRACKS_BY_GENRE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT * FROM tracks \
         WHERE id IN (SELECT track_id FROM track_genres WHERE genre_id = ?) \
         ORDER BY {TRACK_LIST_ORDER}"
    )
});

pub async fn get_tracks_by_genre(
    db: &DbPool,
    genre_id: i64,
) -> Result<Vec<track::Track>, AppError> {
    db.read(move |conn| sql::fetch_all(conn, TRACKS_BY_GENRE.as_str(), (genre_id,))).await
}

pub async fn get_track_by_id(db: &DbPool, id: i64) -> Result<track::Track, AppError> {
    db.read(move |conn| sql::fetch_optional(conn, "SELECT * FROM tracks WHERE id = ?", (id,)))
        .await?
        .ok_or_else(|| AppError::not_found("Track", id))
}

/// Fetch tracks by IDs, preserving the input order.
pub async fn get_tracks_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<track::Track>, AppError> {
    let wanted = ids.to_vec();
    let tracks: Vec<track::Track> = db
        .read(move |conn| {
            chunked_in_query(conn, &wanted, |placeholders| {
                format!("SELECT * FROM tracks WHERE id IN ({placeholders})")
            })
        })
        .await?;

    let mut track_map: HashMap<i64, track::Track> = HashMap::with_capacity(tracks.len());
    track_map.extend(tracks.into_iter().map(|t| (t.id, t)));

    Ok(ids.iter().filter_map(|id| track_map.remove(id)).collect())
}
