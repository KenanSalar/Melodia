//! The ordered ids behind every card grid, filtered and sorted in SQL.
//!
//! A grid keeps only these and fetches the cards near its viewport by id, as the track lists do
//! (`track::track_ids`, whose packed read-back these share). Each arm reproduces the in-memory
//! sort it replaced: a lowercased name compares through `UNICODE_LOWER`, a missing album year
//! reads as `0`, and every arm ends in the id, so the order is total and descending reverses every
//! key. Cards that sort tied on every key come back in id order rather than fetch order.

use sqlx::{AssertSqlSafe, QueryBuilder, Sqlite};

use super::track::{push_folded_match, unpack_ids};
use crate::database::DbPool;
use crate::database::collation::UNICODE_LOWER;
use melodia_core::entities::grid_order::{AlbumSort, ArtistSort, FavoriteArtistSort, GenreSort};
use melodia_core::entities::track_list::SortDir;
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;

/// The playlist orders SQL can answer on its own. By track count is not one: a smart playlist
/// keeps no count, so the library layer sorts that arm over the counts it takes live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistKeyOrder {
    Updated,
    Name,
}

/// One grid's rows: where they come from, which of them it lists, and what a needle matches.
struct Grid {
    id: &'static str,
    from: &'static str,
    member: Option<&'static str>,
    search: &'static str,
    year: Option<&'static str>,
    group_by: Option<&'static str>,
}

const ALBUMS: Grid = Grid {
    id: "al.id",
    from: "albums al JOIN artists ar ON ar.id = al.artist_id",
    member: Some("al.track_count > 0"),
    search: "al.search_name",
    year: Some("al.year"),
    group_by: None,
};

const ARTISTS: Grid = Grid {
    id: "ar.id",
    from: "artists ar",
    member: Some("ar.track_count > 0"),
    search: "ar.search_name",
    year: None,
    group_by: None,
};

const GENRES: Grid = Grid {
    id: "g.id",
    from: "genres g",
    member: Some("g.track_count > 0"),
    search: "g.search_name",
    year: None,
    group_by: None,
};

const PLAYLISTS: Grid = Grid {
    id: "p.id",
    from: "playlists p",
    member: None,
    search: "p.search_name",
    year: None,
    group_by: None,
};

/// Every artist credited on a favourite, as many times as it has favourites until grouped.
const FAVORITE_ARTISTS: Grid = Grid {
    id: "ar.id",
    from: "artists ar \
           JOIN track_artists ta ON ta.artist_id = ar.id \
           JOIN tracks t ON t.id = ta.track_id AND t.is_favorite = TRUE",
    member: None,
    search: "ar.search_name",
    year: None,
    group_by: Some("ar.id"),
};

/// The Albums grid's ids in `sort`, narrowed by `needle` to the albums whose name or artist line
/// carries it, or whose year does.
pub async fn album_ids(
    db: &DbPool,
    sort: AlbumSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    let keys = match sort {
        AlbumSort::Name => vec![lowered("al.name")],
        AlbumSort::Year => vec!["COALESCE(al.year, 0)".to_owned(), lowered("al.name")],
        AlbumSort::Artist => {
            vec![lowered("COALESCE(al.artist_credit, ar.name)"), lowered("al.name")]
        }
    };
    ids(db, &ALBUMS, needle, &keys, dir).await
}

/// The Artists grid's ids in `sort`, narrowed by `needle` to the artists whose name or sort name
/// carries it.
pub async fn artist_ids(
    db: &DbPool,
    sort: ArtistSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    let keys = match sort {
        ArtistSort::Name => vec![lowered("ar.name")],
        ArtistSort::TrackCount => vec!["ar.track_count".to_owned(), lowered("ar.name")],
        ArtistSort::AlbumCount => vec!["ar.album_count".to_owned(), lowered("ar.name")],
    };
    ids(db, &ARTISTS, needle, &keys, dir).await
}

/// The Genres grid's ids in `sort`, narrowed by `needle`.
pub async fn genre_ids(
    db: &DbPool,
    sort: GenreSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    let keys = match sort {
        GenreSort::Name => vec![lowered("g.name")],
        GenreSort::TrackCount => vec!["g.track_count".to_owned(), lowered("g.name")],
        GenreSort::Duration => vec!["g.total_duration_ms".to_owned(), lowered("g.name")],
    };
    ids(db, &GENRES, needle, &keys, dir).await
}

/// The Playlists grid's ids in `order`, narrowed by `needle`. `updated_at` is RFC 3339, so its
/// text order is its time order.
pub async fn playlist_ids(
    db: &DbPool,
    order: PlaylistKeyOrder,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    let keys = match order {
        PlaylistKeyOrder::Updated => vec!["p.updated_at".to_owned()],
        PlaylistKeyOrder::Name => vec![lowered("p.name")],
    };
    ids(db, &PLAYLISTS, needle, &keys, dir).await
}

/// Favorites' Favorite Artists ids in `sort`, narrowed by `needle` as the Artists grid is.
pub async fn favorite_artist_ids(
    db: &DbPool,
    sort: FavoriteArtistSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    let keys = match sort {
        FavoriteArtistSort::FavoriteCount => {
            vec!["COUNT(DISTINCT t.id)".to_owned(), lowered("ar.name")]
        }
        FavoriteArtistSort::Name => vec![lowered("ar.name")],
    };
    ids(db, &FAVORITE_ARTISTS, needle, &keys, dir).await
}

/// How many albums the Albums grid lists unfiltered.
pub async fn album_count(db: &DbPool) -> Result<i64, AppError> {
    count(db, "SELECT COUNT(*) FROM albums WHERE track_count > 0").await
}

/// How many artists the Artists grid lists unfiltered.
pub async fn artist_count(db: &DbPool) -> Result<i64, AppError> {
    count(db, "SELECT COUNT(*) FROM artists WHERE track_count > 0").await
}

/// How many genres the Genres grid lists unfiltered.
pub async fn genre_count(db: &DbPool) -> Result<i64, AppError> {
    count(db, "SELECT COUNT(*) FROM genres WHERE track_count > 0").await
}

/// How many playlists there are, smart ones included.
pub async fn playlist_count(db: &DbPool) -> Result<i64, AppError> {
    count(db, "SELECT COUNT(*) FROM playlists").await
}

/// How many artists are credited on a favourite.
pub async fn favorite_artist_count(db: &DbPool) -> Result<i64, AppError> {
    count(
        db,
        "SELECT COUNT(DISTINCT ta.artist_id) FROM track_artists ta \
         JOIN tracks t ON t.id = ta.track_id AND t.is_favorite = TRUE",
    )
    .await
}

async fn count(db: &DbPool, sql: &'static str) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar(AssertSqlSafe(sql)).fetch_one(db.read()).await?)
}

/// `grid`'s ids that `needle` matches, ordered by `keys` then the id, every one in `dir`, packed
/// as `track::track_ids` argues.
async fn ids(
    db: &DbPool,
    grid: &Grid,
    needle: &Needle,
    keys: &[String],
    dir: SortDir,
) -> Result<Vec<i64>, AppError> {
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT group_concat(id, ',') FROM (SELECT ");
    qb.push(grid.id).push(" AS id FROM ").push(grid.from);
    if let Some(member) = grid.member {
        qb.push(" WHERE ").push(member);
    }
    if !needle.is_empty() {
        qb.push(if grid.member.is_some() { " AND " } else { " WHERE " });
        push_folded_match(&mut qb, needle, grid.search, grid.year);
    }
    if let Some(group_by) = grid.group_by {
        qb.push(" GROUP BY ").push(group_by);
    }
    qb.push(" ORDER BY ").push(order_by(keys, grid.id, dir)).push(")");
    let packed: Option<String> =
        qb.build_query_scalar().persistent(false).fetch_one(db.read()).await?;
    packed.map_or_else(|| Ok(Vec::new()), |packed| unpack_ids(&packed))
}

fn order_by(keys: &[String], id: &str, dir: SortDir) -> String {
    let dir = match dir {
        SortDir::Asc => "ASC",
        SortDir::Desc => "DESC",
    };
    keys.iter()
        .map(String::as_str)
        .chain([id])
        .map(|key| format!("{key} {dir}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A name compared the way `to_lowercase()` compares it.
fn lowered(column: &str) -> String {
    format!("{column} COLLATE {UNICODE_LOWER}")
}
