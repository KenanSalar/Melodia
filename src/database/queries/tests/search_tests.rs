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

/// The artist arm carries its own copy of that `ORDER BY`, so the album
/// test above can't fail for it — drop the `DESC` on this one and every
/// other test still passes. "Zebra Quartet" is the only artist named for
/// the query; "Kite" and "Wren" are reachable through their tracks alone
/// and sort after it alphabetically.
#[tokio::test]
async fn an_artist_named_for_the_query_outranks_ones_found_through_tracks()
-> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/a.mp3", "Zebra Dance", "Kite", "Alpha Records", "Rock")
        .await?;
    insert_test_track(&db, "/music/b.mp3", "Quiet Hours", "Wren", "Zebra Sessions", "Rock")
        .await?;
    insert_test_track(&db, "/music/c.mp3", "Hush", "Zebra Quartet", "Quiet Records", "Rock")
        .await?;

    let results = queries::search::search_all(&db, "Zebra").await?;

    let artists: Vec<&str> = results.artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(artists, ["Zebra Quartet", "Kite", "Wren"]);
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

/// The bm25 weights are positional against the fts5 column list, so a ninth
/// column silently shifts every one of them onto the wrong field — the table
/// still builds, search still works, and only the ranking is wrong. Reading
/// the config back rather than the migration text is what makes this a pin on
/// the *applied* state: a weight list that never took would pass a source
/// scan. It is the one place anything reads a shadow table, and it stays
/// confined to this test.
#[tokio::test]
async fn bm25_weights_cover_every_indexed_column() -> Result<(), AppError> {
    const INDEXED: [&str; 8] = [
        "title",
        "artist",
        "album_artist",
        "album",
        "genre",
        "composer",
        "year",
        "file_name",
    ];

    let db = DbPool::test_pool().await;

    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('tracks_fts')")
            .fetch_all(db.read())
            .await?;
    assert_eq!(columns, INDEXED, "fts5 column list moved");

    let rank: String = sqlx::query_scalar("SELECT v FROM tracks_fts_config WHERE k = 'rank'")
        .fetch_one(db.read())
        .await?;
    // A config that isn't a `bm25(…)` expression at all parses to zero
    // weights, so the arity assertion below covers that too — and prints the
    // raw value either way.
    let args = rank
        .strip_prefix("bm25(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or_default();
    let weights: Vec<f64> = args.split(',').filter_map(|w| w.trim().parse().ok()).collect();

    assert_eq!(
        weights.len(),
        INDEXED.len(),
        "one bm25 weight per indexed column, got {rank}"
    );

    // Which way the ranking leans is the whole point of setting them at all:
    // the title is what people search for, the filename carries whatever the
    // tags don't — the extension, a track-number prefix, a spelling the
    // metadata never had.
    let title_weight = weights[0];
    let file_name_weight = weights[INDEXED.len() - 1];
    assert!(
        weights.iter().all(|&w| w <= title_weight),
        "title should carry the most weight, got {rank}"
    );
    assert!(
        weights.iter().all(|&w| w >= file_name_weight),
        "file_name should be the tiebreaker, got {rank}"
    );
    // Both bounds above hold for a uniform list, which is the fts5 default
    // and the one thing this migration exists to replace.
    assert!(
        file_name_weight < title_weight,
        "the weights must not be uniform, got {rank}"
    );
    Ok(())
}

/// The per-view filter boxes never reach this index — they narrow in-memory
/// caches through `ui::row_match::search_fields`, which mirrors the column
/// list by hand. So a ninth column is two edits, and only the first of them
/// fails anything on its own: the Search view answers a query the filter box
/// beside it comes up empty on, which is the split `20260802000001` was
/// written to close in the first place. Reading the applied schema rather
/// than a copied array is what makes it a pin on both sides at once.
///
/// It names a `ui::` module from a database test for the same reason the
/// weights above sit in the migration rather than in `search.rs`: what this
/// column list covers is load-bearing well outside the file that declares it.
#[tokio::test]
async fn the_filter_boxes_search_every_indexed_column_they_can_reach() -> Result<(), AppError> {
    // `composer` has no slot on `TrackListRow`, and `file_name` is left out
    // deliberately — the tiebreaker weight that keeps a filename echo below
    // the tags it repeats has no equivalent in an unranked substring filter.
    // `year` is an integer that joins the match through `row_match::year_matches`
    // instead of the text list.
    const NOT_TEXT_SEARCHED: [&str; 3] = ["composer", "file_name", "year"];

    let db = DbPool::test_pool().await;
    let indexed: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('tracks_fts')")
            .fetch_all(db.read())
            .await?;
    let expected: Vec<&str> = indexed
        .iter()
        .map(String::as_str)
        .filter(|c| !NOT_TEXT_SEARCHED.contains(c))
        .collect();

    // Every searchable field holds its own column name, so `search_fields`
    // hands back the list it claims to mirror and a failure names the column
    // that drifted. The literal is exhaustive on purpose: a new `TrackListRow`
    // field fails this file to compile, which is the prompt to decide whether
    // it belongs in the index and in the fold.
    let row = crate::entities::track::TrackListRow {
        id: 1,
        file_path: "/m/1.flac".to_owned(),
        file_name: "file_name".to_owned(),
        title: "title".to_owned(),
        artist: Some("artist".to_owned()),
        album_artist: Some("album_artist".to_owned()),
        album: Some("album".to_owned()),
        genre: Some("genre".to_owned()),
        track_number: None,
        disc_number: None,
        year: None,
        artwork_path: None,
        duration_ms: 0,
        is_favorite: false,
        rating: 0,
        album_id: None,
        artist_id: None,
        genre_id: None,
        date_added: "2026-01-01T00:00:00Z".to_owned(),
        sort_key: None,
    };
    assert_eq!(
        crate::ui::row_match::search_fields(&row).as_slice(),
        expected,
        "the fts5 column list and what the filter boxes search have drifted"
    );
    Ok(())
}

/// A filename normally repeats the title and artist beside it, so under the
/// default uniform weights a filename echo outranks the track the query
/// actually names. Here the term appears three times in one track's filename
/// and once as another's title — the loser under equal weights is the one
/// that has to come first. `LIMIT 50` sits under the same `ORDER BY rank`, so
/// this decides which rows come back, not only their order.
#[tokio::test]
async fn a_title_match_outranks_a_filename_only_match() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(
        &db,
        "/music/Kestrel Kestrel Kestrel.mp3",
        "Quiet Hours",
        "Wren",
        "Dusk Sessions",
        "Ambient",
    )
    .await?;
    insert_test_track(&db, "/music/b.mp3", "Kestrel", "Wren", "Dusk Sessions", "Ambient")
        .await?;

    let results = queries::search::search_all(&db, "Kestrel").await?;

    let titles: Vec<&str> = results.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Kestrel", "Quiet Hours"], "the titled match leads");
    Ok(())
}

/// `remove_diacritics 1`, the fts5 default, folds a character carrying one
/// combining mark but leaves two-mark characters alone — so an ASCII query
/// reaches Björk and stops dead at Vietnamese. `ế` is U+1EBF, the two-mark
/// case; under the default this finds nothing.
#[tokio::test]
async fn a_plain_ascii_query_matches_a_multi_mark_title() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/a.mp3", "Bế Tắc", "Ngoc", "Xuan", "Pop").await?;

    let results = queries::search::search_all(&db, "be").await?;

    let titles: Vec<&str> = results.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Bế Tắc"]);
    Ok(())
}

/// `build_fts_query` quotes each word and appends `*`, so a query of pure
/// punctuation reaches fts5 as a phrase the tokenizer reduces to nothing.
/// That returns no rows rather than raising, which is the behaviour the
/// search box depends on — this is a pin, not a fix.
#[tokio::test]
async fn punctuation_only_queries_return_no_rows_instead_of_erroring() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    for query in ["-", "---", "*", "(", "\"", "^ ~"] {
        let results = queries::search::search_all(&db, query).await?;
        assert!(
            results.tracks.is_empty(),
            "{query:?} should match no tracks, got {:?}",
            results.tracks.iter().map(|t| &t.title).collect::<Vec<_>>()
        );
    }
    Ok(())
}

/// The other half of that: a quote beside a word the fixture actually has.
/// `build_fts_query` doubles the quote to keep it inside the phrase, and the
/// unit test above pins the string it builds — but nothing ran that string
/// past fts5, which is where the escaping is the difference between a match
/// and a hard `unterminated string` at step time. Asserting the *hit* rather
/// than the absence of an error is what keeps this from passing vacuously.
#[tokio::test]
async fn a_word_carrying_a_quote_still_matches_instead_of_failing_to_parse()
-> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    let results = queries::search::search_all(&db, "Alpha\"").await?;

    let titles: Vec<&str> = results.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Alpha Song"]);
    Ok(())
}
