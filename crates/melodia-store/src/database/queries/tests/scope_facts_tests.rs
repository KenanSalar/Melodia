//! What a hero states about a scope it holds only the ids of, counted over the membership its
//! keyset lists, the genre lines an album's chip is tallied from, and the covers the artwork
//! picker offers.
//!
//! Every fixture but the seeded cover ones holds tracks outside the scope asked about, so a
//! membership that lost its narrowing counts them and fails.

use std::collections::HashSet;
use std::path::Path;

use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::fixtures::{
    insert_tagged_track, make_test_metadata, set_test_artwork, setup_seeded_db,
};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::smart_criteria::Draw;
use melodia_core::entities::track_list::{
    ScopeFacts, SortDir, TrackListScope, TrackOrder, TrackSortField,
};
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;

const LIBRARY_ROOT: &str = "/music";

/// A playlist id no fixture reaches.
const ABSENT_PLAYLIST_ID: i64 = 1_000_000;

const PLAYED_AT: &str = "2026-10-18T00:00:00Z";

/// A smart playlist of the library's two most played tracks.
const TOP_TWO_BY_PLAYS: &str = r#"{"limit":{"count":2,"order":"play_count_desc"}}"#;

/// A smart playlist of the library's one Pop track, `Four`.
const POP_ONLY: &str =
    r#"{"rules":[{"field":"genre","op":"contains","value":{"kind":"text","value":"Pop"}}]}"#;

/// A smart playlist of three tracks its draw picks.
const RANDOM_THREE: &str = r#"{"limit":{"count":3,"order":"random"}}"#;

async fn empty_library() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, LIBRARY_ROOT, true).await?;
    Ok(db)
}

/// `title` by `artist` on `album`, tagged `genres` in that order. An empty artist or album is the
/// untagged partition: the unknown artist the scan credits, and no album at all.
fn tagged(title: &str, artist: &str, album: &str, genres: &[&str]) -> ExtractedMetadata {
    let mut meta = make_test_metadata(title);
    meta.artist = ArtistCredit::from_name(artist);
    meta.album = (!album.is_empty()).then(|| album.to_owned());
    meta.genres = GenreList::new(genres.iter().map(|&genre| genre.to_owned()).collect());
    meta
}

fn lasting(duration_ms: i64, meta: ExtractedMetadata) -> ExtractedMetadata {
    ExtractedMetadata { duration_ms, ..meta }
}

/// One track at a path its title names, so no two share a path or a hash.
async fn stage(db: &DbPool, meta: ExtractedMetadata) -> Result<i64, AppError> {
    let path = Path::new(LIBRARY_ROOT).join(format!("{}.mp3", meta.title));
    insert_tagged_track(db, &path.to_string_lossy(), &meta).await
}

/// Four tracks, each running twice as long as the one before, so a running time names exactly the
/// tracks it summed. `Two` carries `Metal` second, and `Four` has no artist and no album.
struct Library {
    db: DbPool,
    one: i64,
    two: i64,
    three: i64,
    four: i64,
}

async fn library() -> Result<Library, AppError> {
    let db = empty_library().await?;
    let one = stage(&db, lasting(100_000, tagged("One", "Alice", "First", &["Rock"]))).await?;
    let two =
        stage(&db, lasting(200_000, tagged("Two", "Alice", "First", &["Rock", "Metal"]))).await?;
    let three = stage(&db, lasting(400_000, tagged("Three", "Bob", "Second", &["Metal"]))).await?;
    let four = stage(&db, lasting(800_000, tagged("Four", "", "", &["Pop"]))).await?;
    Ok(Library { db, one, two, three, four })
}

async fn genre_id(db: &DbPool, name: &str) -> Result<i64, AppError> {
    let id = sqlx::query_scalar("SELECT id FROM genres WHERE name = ?")
        .bind(name)
        .fetch_one(db.read())
        .await?;
    Ok(id)
}

async fn album_id(db: &DbPool, name: &str) -> Result<i64, AppError> {
    let id = sqlx::query_scalar("SELECT id FROM albums WHERE name = ?")
        .bind(name)
        .fetch_one(db.read())
        .await?;
    Ok(id)
}

// === Scope facts ===

/// A genre's page lists every track tagged with it, a second tag included, so its spread is
/// counted over all of them.
#[tokio::test]
async fn a_genre_counts_every_track_tagged_with_it() -> Result<(), AppError> {
    let lib = library().await?;
    let metal = genre_id(&lib.db, "Metal").await?;

    let facts = queries::track::scope_facts(&lib.db, TrackListScope::Genre(metal)).await?;

    assert_eq!(facts, ScopeFacts { tracks: 2, duration_ms: 600_000, artists: 2, albums: 2 });
    Ok(())
}

#[tokio::test]
async fn a_playlist_counts_its_own_items_and_no_other_playlists() -> Result<(), AppError> {
    let lib = library().await?;
    let mix =
        queries::playlist::create_playlist_with_tracks(&lib.db, "Mix", None, &[lib.three, lib.one])
            .await?;
    queries::playlist::create_playlist_with_tracks(&lib.db, "Other", None, &[lib.two, lib.four])
        .await?;

    let scope = TrackListScope::Playlist(mix, Draw::default());
    let facts = queries::track::scope_facts(&lib.db, scope).await?;

    assert_eq!(facts, ScopeFacts { tracks: 2, duration_ms: 500_000, artists: 2, albums: 2 });
    Ok(())
}

/// A smart playlist's hero states its count and running time off these, so they are its two most
/// played tracks: not every track its rules match, nor the first two by title.
#[tokio::test]
async fn a_capped_smart_playlist_is_counted_under_its_cap() -> Result<(), AppError> {
    let lib = library().await?;
    queries::track::add_play_counts(
        &lib.db,
        &[(lib.one, 9), (lib.three, 5), (lib.two, 1)],
        PLAYED_AT,
    )
    .await?;
    let smart =
        queries::playlist::create_smart_playlist(&lib.db, "Top two", None, TOP_TWO_BY_PLAYS)
            .await?;

    let scope = TrackListScope::Playlist(smart.id, Draw::default());
    let facts = queries::track::scope_facts(&lib.db, scope).await?;

    assert_eq!(facts, ScopeFacts { tracks: 2, duration_ms: 500_000, artists: 2, albums: 2 });
    Ok(())
}

/// A playlist deleted under a page still naming it reads as an empty one, rather than failing the
/// hero's fetch.
#[tokio::test]
async fn an_empty_scope_states_nothing() -> Result<(), AppError> {
    let lib = library().await?;
    let empty = queries::playlist::create_playlist(&lib.db, "Empty", None).await?;
    let cases =
        [(empty.id, "an empty playlist"), (ABSENT_PLAYLIST_ID, "a playlist that no longer exists")];
    for (id, case) in cases {
        let scope = TrackListScope::Playlist(id, Draw::default());
        let facts = queries::track::scope_facts(&lib.db, scope).await?;

        assert_eq!(facts, ScopeFacts::default(), "{case}");
    }
    Ok(())
}

// === Genre lines ===

/// Album `First`, its lines inserted so the commonest is neither the first seen nor the first by
/// name, and three tied lines first seen in an order running neither way by name, since without
/// the tie-break `SQLite` returns a tie by name. `Bonus` carries one of its lines from another
/// album.
async fn tagged_album() -> Result<(DbPool, i64), AppError> {
    let db = empty_library().await?;
    stage(&db, tagged("One", "Alice", "First", &["Jazz"])).await?;
    stage(&db, tagged("Two", "Alice", "First", &["Rock", "Metal"])).await?;
    stage(&db, tagged("Three", "Alice", "First", &["Metal", "Rock"])).await?;
    stage(&db, tagged("Four", "Alice", "First", &[])).await?;
    stage(&db, tagged("Five", "Alice", "First", &["Rock", "Metal"])).await?;
    stage(&db, tagged("Six", "Alice", "First", &["Blues"])).await?;
    stage(&db, tagged("Bonus", "Alice", "Second", &["Jazz"])).await?;
    let first = album_id(&db, "First").await?;
    Ok((db, first))
}

/// A line is what a track's row shows, so the same two names in another order are another line,
/// and an untagged track adds none.
#[tokio::test]
async fn genre_lines_count_the_tracks_in_scope_per_rendered_line() -> Result<(), AppError> {
    let (db, first) = tagged_album().await?;

    let lines = queries::track::genre_lines(&db, TrackListScope::Album(first)).await?;

    let counted: HashSet<(String, i64)> = lines.into_iter().collect();
    let expected = HashSet::from([
        ("Rock, Metal".to_owned(), 2),
        ("Jazz".to_owned(), 1),
        ("Metal, Rock".to_owned(), 1),
        ("Blues".to_owned(), 1),
    ]);
    assert_eq!(counted, expected);
    Ok(())
}

/// Commonest first, the line `dominant_genre` settles a tie by, then by the lowest id carrying
/// each, so lines tied on count aren't left to the query plan.
#[tokio::test]
async fn genre_lines_come_commonest_first_and_tied_in_the_order_first_seen() -> Result<(), AppError>
{
    let (db, first) = tagged_album().await?;

    let lines = queries::track::genre_lines(&db, TrackListScope::Album(first)).await?;

    let order: Vec<&str> = lines.iter().map(|(line, _)| line.as_str()).collect();
    assert_eq!(order, ["Rock, Metal", "Jazz", "Metal, Rock", "Blues"]);
    Ok(())
}

// === Covers ===

/// The mosaic's candidate list: distinct covers, in the order the playlist first reaches them, so
/// a cover an early and a late track share sorts with the early one.
#[tokio::test]
async fn the_mosaic_candidates_are_distinct_and_in_playlist_order() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    // The first and last track share a cover; the middle one has its own.
    set_test_artwork(&db, track_ids[0], "/artwork/shared.jpg").await?;
    set_test_artwork(&db, track_ids[1], "/artwork/middle.jpg").await?;
    set_test_artwork(&db, track_ids[2], "/artwork/shared.jpg").await?;
    let pl = queries::playlist::create_playlist(&db, "Mosaic", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    let scope = TrackListScope::Playlist(pl.id, Draw::default());
    let candidates = queries::track::cover_paths(&db, scope, 4).await?;

    assert_eq!(candidates, ["/artwork/shared.jpg", "/artwork/middle.jpg"]);
    Ok(())
}

/// A track with no cover contributes nothing, and the blank string is the second spelling of
/// that — the column carries both. Without the `!= ''` half an empty path takes a mosaic tile
/// and paints a hole.
#[tokio::test]
async fn a_track_with_no_cover_offers_no_mosaic_candidate() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    set_test_artwork(&db, track_ids[0], "").await?;
    set_test_artwork(&db, track_ids[1], "/artwork/real.jpg").await?;
    let pl = queries::playlist::create_playlist(&db, "Sparse", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    let scope = TrackListScope::Playlist(pl.id, Draw::default());
    let candidates = queries::track::cover_paths(&db, scope, 4).await?;

    assert_eq!(candidates, ["/artwork/real.jpg"], "the blank and the null both contribute nothing");
    Ok(())
}

/// The cap the mosaic asks with, which is what stops a thousand-track playlist reading a
/// thousand rows to draw four tiles.
#[tokio::test]
async fn the_mosaic_candidate_list_stops_at_the_limit_it_was_given() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    for (index, id) in track_ids.iter().enumerate() {
        set_test_artwork(&db, *id, &format!("/artwork/{index}.jpg")).await?;
    }
    let pl = queries::playlist::create_playlist(&db, "Capped", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    let scope = TrackListScope::Playlist(pl.id, Draw::default());
    let candidates = queries::track::cover_paths(&db, scope, 2).await?;

    assert_eq!(candidates.len(), 2);
    Ok(())
}

/// Each of `lib`'s tracks under a cover of its own, named for its title.
async fn cover_every_track(lib: &Library) -> Result<(), AppError> {
    set_test_artwork(&lib.db, lib.one, "/artwork/One.jpg").await?;
    set_test_artwork(&lib.db, lib.two, "/artwork/Two.jpg").await?;
    set_test_artwork(&lib.db, lib.three, "/artwork/Three.jpg").await?;
    set_test_artwork(&lib.db, lib.four, "/artwork/Four.jpg").await
}

#[tokio::test]
async fn a_manual_playlist_offers_no_cover_from_a_track_it_does_not_hold() -> Result<(), AppError> {
    let lib = library().await?;
    cover_every_track(&lib).await?;
    let lone =
        queries::playlist::create_playlist_with_tracks(&lib.db, "Lone", None, &[lib.two]).await?;

    let scope = TrackListScope::Playlist(lone, Draw::default());
    let candidates = queries::track::cover_paths(&lib.db, scope, 4).await?;

    assert_eq!(candidates, ["/artwork/Two.jpg"]);
    Ok(())
}

/// A smart playlist has no `playlist_items` rows, so a pool read off them came back empty and the
/// picker had nothing to apply.
#[tokio::test]
async fn a_smart_playlist_offers_the_covers_of_the_tracks_its_rules_match() -> Result<(), AppError>
{
    let lib = library().await?;
    cover_every_track(&lib).await?;
    let pop = queries::playlist::create_smart_playlist(&lib.db, "Pop", None, POP_ONLY).await?;

    let scope = TrackListScope::Playlist(pop.id, Draw::default());
    let candidates = queries::track::cover_paths(&lib.db, scope, 4).await?;

    assert_eq!(candidates, ["/artwork/Four.jpg"]);
    Ok(())
}

/// Ranked against both the id and the title order, so a pool that kept the cap but lost its
/// ranking fails too.
#[tokio::test]
async fn a_capped_smart_playlist_offers_its_covers_in_the_order_its_cap_ranks_them()
-> Result<(), AppError> {
    let lib = library().await?;
    cover_every_track(&lib).await?;
    queries::track::add_play_counts(
        &lib.db,
        &[(lib.three, 9), (lib.one, 5), (lib.two, 1)],
        PLAYED_AT,
    )
    .await?;
    let top = queries::playlist::create_smart_playlist(&lib.db, "Top two", None, TOP_TWO_BY_PLAYS)
        .await?;

    let scope = TrackListScope::Playlist(top.id, Draw::default());
    let candidates = queries::track::cover_paths(&lib.db, scope, 4).await?;

    assert_eq!(candidates, ["/artwork/Three.jpg", "/artwork/One.jpg"]);
    Ok(())
}

/// The expected order is read off the page rather than written out: the pick is a hash of the
/// draw, and agreeing with what the page lists under it is the contract.
#[tokio::test]
async fn a_random_cap_offers_the_covers_its_draw_lists_in_that_order() -> Result<(), AppError> {
    let lib = library().await?;
    cover_every_track(&lib).await?;
    let random =
        queries::playlist::create_smart_playlist(&lib.db, "Random", None, RANDOM_THREE).await?;
    let scope = TrackListScope::Playlist(random.id, Draw(7));
    let own_order = TrackOrder { field: TrackSortField::Position, dir: SortDir::Asc };
    let page = queries::track::track_ids(&lib.db, scope, own_order, &Needle::default()).await?;
    assert_eq!(page.len(), 3, "the fixture's draw keeps its cap");
    let listed: Vec<String> = queries::track::get_track_list_rows_by_ids(&lib.db, &page)
        .await?
        .into_iter()
        .filter_map(|row| row.artwork_path)
        .collect();

    let candidates = queries::track::cover_paths(&lib.db, scope, 4).await?;

    assert_eq!(candidates, listed, "under Draw(7)");
    Ok(())
}

/// What the picker's empty state is for. The smart case is the one its hint names, and `One`
/// carries a cover so a pool that lost its narrowing offers it.
#[tokio::test]
async fn a_scope_with_no_cover_offers_none() -> Result<(), AppError> {
    let lib = library().await?;
    set_test_artwork(&lib.db, lib.one, "/artwork/One.jpg").await?;
    let empty = queries::playlist::create_playlist(&lib.db, "Empty", None).await?;
    let coverless =
        queries::playlist::create_smart_playlist(&lib.db, "Pop", None, POP_ONLY).await?;
    let cases = [
        (empty.id, "an empty playlist"),
        (ABSENT_PLAYLIST_ID, "a playlist that no longer exists"),
        (coverless.id, "a smart playlist whose tracks carry no cover"),
    ];
    for (id, case) in cases {
        let scope = TrackListScope::Playlist(id, Draw::default());
        let candidates = queries::track::cover_paths(&lib.db, scope, 4).await?;

        assert_eq!(candidates, Vec::<String>::new(), "{case}");
    }
    Ok(())
}
