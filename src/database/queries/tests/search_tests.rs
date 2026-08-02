use super::*;
use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::{insert_test_track, setup_seeded_db};
use crate::error::AppError;

#[test]
fn build_fts_query_single_word() {
    assert_eq!(build_fts_query("hello"), r#""hello"*"#);
}

#[test]
fn build_fts_query_multiple_words() {
    assert_eq!(build_fts_query("hello world"), r#""hello"* "world"*"#);
}

#[test]
fn build_fts_query_with_quotes() {
    assert_eq!(build_fts_query(r#"word"s"#), r#""word""s"*"#);
}

#[test]
fn build_fts_query_empty_input() {
    assert_eq!(build_fts_query(""), "");
    assert_eq!(build_fts_query("   "), "");
}

// === Async DB tests for search_all ===

#[tokio::test]
async fn search_all_empty_query_returns_empty() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    let results = queries::search::search_all(&db, "   ").await?;
    assert!(results.tracks.is_empty());
    assert!(results.albums.is_empty());
    assert!(results.artists.is_empty());
    assert!(results.genres.is_empty());
    Ok(())
}

#[tokio::test]
async fn search_all_finds_track_by_title() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Alpha").await?;
    assert!(!results.tracks.is_empty());
    assert!(results.tracks.iter().any(|t| t.title == "Alpha Song"));
    Ok(())
}

#[tokio::test]
async fn search_all_finds_albums_and_artists() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Album One").await?;
    assert!(!results.albums.is_empty());
    assert!(results.albums.iter().any(|a| a.name == "Album One"));
    Ok(())
}

#[tokio::test]
async fn search_all_no_results() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "zzz_nonexistent_xyz").await?;
    assert!(results.tracks.is_empty());
    assert!(results.albums.is_empty());
    assert!(results.artists.is_empty());
    assert!(results.genres.is_empty());
    Ok(())
}

/// Genre is a track-list column, and the FTS index didn't carry it — so
/// this query matched nothing at all. The seeded fixture puts "Rock" on
/// two of its three tracks and on no title, artist or album, so a hit
/// here can only have come through the genre column.
#[tokio::test]
async fn search_all_finds_tracks_by_genre() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Rock").await?;

    let titles: Vec<&str> = results.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles.len(), 2, "both Rock tracks, got {titles:?}");
    assert!(titles.contains(&"Alpha Song"), "got {titles:?}");
    assert!(titles.contains(&"Gamma Song"), "got {titles:?}");
    Ok(())
}

/// The other half of the same gap. `year` is an INTEGER column; fts5
/// indexes its text form, so the app's prefix query reaches it.
#[tokio::test]
async fn search_all_finds_tracks_by_year() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "2024").await?;
    assert_eq!(results.tracks.len(), 3, "every seeded track is from 2024");
    Ok(())
}

/// Feeds the Top Result card. `Pop` must stay out — this is a filtered
/// result set, not the genre list.
#[tokio::test]
async fn search_all_returns_matching_genres() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Rock").await?;

    let names: Vec<&str> = results.genres.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Rock"], "only the matching genre");
    Ok(())
}

/// A genre query used to leave both strips empty beside a full Songs
/// list, because they only ever matched their own names. Nothing in the
/// fixture is *named* "Rock", so every row here arrived through a track —
/// and "Album Two" / "Artist B" are Pop-only, which is what keeps this
/// from passing on a query that simply returns everything.
#[tokio::test]
async fn a_genre_query_surfaces_the_albums_and_artists_holding_it() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Rock").await?;

    let albums: Vec<&str> = results.albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(albums, ["Album One"], "the Rock album only");

    let artists: Vec<&str> = results.artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(artists, ["Artist A"], "the Rock artist only");
    Ok(())
}

/// Searching a song title returned the track and *nothing else*, so the
/// Top Result ranks over two empty lists and the page rendered a lone
/// Songs list with no card. Nothing here is named "Gamma"; the album and
/// the artist are reachable only through the track that is.
#[tokio::test]
async fn a_song_title_surfaces_its_album_and_artist() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Gamma").await?;

    assert_eq!(results.tracks.len(), 1, "the one matching song");
    let albums: Vec<&str> = results.albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(albums, ["Album One"], "the album that song is on");
    let artists: Vec<&str> = results.artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(artists, ["Artist A"], "the artist that song is by");
    Ok(())
}

/// The track-derived arm is an `OR`, so it must not cost the name matches
/// their own results — "Album Two" is reachable by name regardless.
#[tokio::test]
async fn matching_by_name_still_works_alongside_the_track_arm() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Album Two").await?;

    let albums: Vec<&str> = results.albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(albums, ["Album Two"]);
    Ok(())
}

/// Name matches and track-derived ones share 20 slots, so a name match is
/// sorted first or a broad query can push an album out of its own search.
/// "Alpha Records" sorts first alphabetically and matches only through a
/// track title, so plain `ORDER BY name` would put it on top.
#[tokio::test]
async fn a_name_match_outranks_one_found_through_its_tracks() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/a.mp3", "Zebra Dance", "Kite", "Alpha Records", "Rock")
        .await?;
    insert_test_track(&db, "/music/b.mp3", "Quiet Hours", "Wren", "Zebra Sessions", "Rock")
        .await?;

    let results = queries::search::search_all(&db, "Zebra").await?;

    let albums: Vec<&str> = results.albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        albums,
        ["Zebra Sessions", "Alpha Records"],
        "the album actually named for the query leads"
    );
    Ok(())
}

/// The FTS update trigger only fires for the columns named in its
/// `AFTER UPDATE OF` list, so an edit touching one of the newly-indexed
/// columns and nothing else has to reindex on its own. Five assertions,
/// five separate ways for the rebuilt table or trigger to be wrong.
#[tokio::test]
async fn a_narrow_retag_reindexes_the_new_fts_columns() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    sqlx::query(
        "UPDATE tracks SET genre = ?, year = ?, album_artist = ?, composer = ?
         WHERE file_path = ?",
    )
    .bind("Ambient")
    .bind(1977)
    .bind("Various Artists")
    .bind("Gyorgy Ligeti")
    .bind("/music/track1.mp3")
    .execute(db.write())
    .await?;

    let hit = |r: &queries::SearchResults| r.tracks.iter().any(|t| t.title == "Alpha Song");

    assert!(
        !hit(&queries::search::search_all(&db, "Rock").await?),
        "the replaced genre must stop matching"
    );
    assert!(hit(&queries::search::search_all(&db, "Ambient").await?), "new genre");
    assert!(hit(&queries::search::search_all(&db, "1977").await?), "new year");
    assert!(hit(&queries::search::search_all(&db, "Various").await?), "album_artist");
    assert!(hit(&queries::search::search_all(&db, "Ligeti").await?), "composer");
    Ok(())
}
