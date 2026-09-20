//! Assertions that hold two tiers against each other, and so can live inside neither.
//!
//! Each one pins a value in one layer against a value in another, and each was written in the
//! lower of the two — where it compiled while the tree was one crate and would stop the moment
//! the boundary became a manifest line. A store test may not name the UI's cover tiers, a
//! settings test may not name the nav map, and a database test may not name the filter boxes.
//! Held from outside all of them instead, which is also the only place that can see both.
//!
//! `.claude/rules/testing.md` gives every corpus walk one home; these are the walks'
//! compile-time cousins and land in the same place for the same reason.

use melodia_core::error::AppError;

/// The invariant `STORE_MAX_DIM` was picked from: the store must hold at least what the largest
/// tier decodes, or every tier upscales from a source the store already threw away.
///
/// A runtime test rather than a `const _`, `row_cover_size` and `cover_size` both being
/// functions.
#[test]
fn every_cover_tier_decodes_within_the_store_cap() {
    use melodia_artwork::media::image::artwork::STORE_MAX_DIM;
    use melodia_artwork::media::image::cover_thumbs::row_cover_size;
    use melodia_views::ui::grid_prewarm::{GRID_COVER_FALLBACK, cover_size};
    use melodia_views::ui::util::COVER_SIZE;

    for (tier, size) in [
        ("GRID_COVER_FALLBACK", GRID_COVER_FALLBACK),
        ("COVER_SIZE", COVER_SIZE),
        ("row_cover_size(1.0)", row_cover_size(1.0)),
        ("row_cover_size(2.0)", row_cover_size(2.0)),
    ] {
        assert!(
            size <= STORE_MAX_DIM,
            "{tier} is {size}, past the {STORE_MAX_DIM} px the store keeps — raise \
             `STORE_MAX_DIM` and renormalize, or the tier upscales from a capped source"
        );
    }

    // The grid tier is derived rather than named, so the question is what it can *answer*: a
    // panel narrow enough to pack one huge card, on a display scaled far past anything the two
    // retired constants covered.
    for logical_w in [320, 480, 640, 960, 1280, 1920, 2560, 3840, 7680] {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let size = cover_size(logical_w, scale);
            assert!(
                size <= STORE_MAX_DIM,
                "the grid tier answers {size} px at {logical_w} logical / {scale}×, past the \
                 {STORE_MAX_DIM} px the store keeps"
            );
        }
    }
}

/// **The persisted nav index has to survive a round trip at the top of its range**, and until
/// Phase 4 of the radio work it did not: `set_last_nav_index` clamped writes to `0..=9` and
/// `install_views` guarded reads with the same literal, so a Radio index was rewritten as
/// Settings on the way out *and* dropped on the way in. Neither half is visible from the other,
/// which is why both now read `MAX_NAV_INDEX` and why this pins the bound against the section
/// that actually sits at the top of it.
#[test]
fn the_nav_bound_reaches_the_highest_section_that_routes() {
    assert_eq!(
        melodia_app::services::view_state::MAX_NAV_INDEX,
        melodia_views::ui::radio::NAV_RADIO,
        "Radio is the highest index `nav.slint` routes, so the bound is its index — a section \
         added above it moves both"
    );
}

/// The per-view filter boxes never reach the fts5 index — they narrow in-memory caches through
/// `ui::row_match::search_fields`, which mirrors the column list by hand. So a ninth column is
/// two edits, and only the first of them fails anything on its own: the Search view answers a
/// query the filter box beside it comes up empty on, which is the split `20260802000001` was
/// written to close in the first place. Reading the applied schema rather than a copied array is
/// what makes it a pin on both sides at once.
#[tokio::test]
async fn the_filter_boxes_search_every_indexed_column_they_can_reach() -> Result<(), AppError> {
    use melodia_store::database::DbPool;

    // `credits` has no slot on `TrackListRow` — the role credits live in `track_credits` and reach
    // the index through a denormalized column no list renders. `file_name` is left out
    // deliberately: the tiebreaker weight that keeps a filename echo below the tags it repeats has
    // no equivalent in an unranked substring filter. `year` is an integer that joins the match
    // through `row_match::Needle::matches_number` instead of the text list.
    const NOT_TEXT_SEARCHED: [&str; 3] = ["credits", "file_name", "year"];

    let db = DbPool::test_pool().await?;
    let indexed: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('tracks_fts')")
            .fetch_all(db.read())
            .await?;
    let expected: Vec<&str> =
        indexed.iter().map(String::as_str).filter(|c| !NOT_TEXT_SEARCHED.contains(c)).collect();

    // Every searchable field holds its own column name, so `search_fields` hands back the list it
    // claims to mirror and a failure names the column that drifted. The literal is exhaustive on
    // purpose: a new `TrackListRow` field fails this file to compile, which is the prompt to
    // decide whether it belongs in the index and in the fold.
    let row = melodia_core::entities::track::TrackListRow {
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
        melodia_views::ui::row_match::search_fields(&row).as_slice(),
        expected,
        "the fts5 column list and what the filter boxes search have drifted"
    );
    Ok(())
}

/// **The persisted nav index is clamped in one crate and guarded in another**, and both ends have
/// to take the bound from `MAX_NAV_INDEX` rather than restate it.
///
/// A source read because the write needs an `AppState` and the read an `AppWindow`, and because
/// what failed before was not the arithmetic but the *literal*: two sites agreeing on `9` for
/// reasons neither could see. Here rather than beside either half because the write is
/// `melodia-app`'s and the read is the binary's, so no crate can `include_str!` both.
#[test]
fn both_ends_of_the_nav_bound_take_it_from_one_const() {
    const WRITE: &str = include_str!(concat!(
        env!("MELODIA_REPO_ROOT"),
        "crates/melodia-app/src/library/settings/view.rs"
    ));
    const READ: &str = include_str!(concat!(
        env!("MELODIA_REPO_ROOT"),
        "crates/melodia/src/boot/ui_setup/views.rs"
    ));

    let clamp = melodia_testkit::strip_line_comments(WRITE)
        .split_once("fn write_last_nav_index")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or(String::new(), |(body, _)| body.to_owned());
    assert!(!clamp.is_empty(), "`write_last_nav_index` moved, so this pin reads nothing");
    assert!(
        clamp.contains("view_state::MAX_NAV_INDEX"),
        "the write clamp must bound against `MAX_NAV_INDEX`, never a literal"
    );

    let read = melodia_testkit::strip_line_comments(READ);
    assert!(
        read.contains("(0..=services::view_state::MAX_NAV_INDEX).contains("),
        "`install_views` must guard the persisted index against the same const the write clamps to"
    );
}

/// **A shipped migration seeds role credits by name, and the enum owns those names.**
/// `track_credits.role` is written as a literal in SQL nobody may edit again, and read back
/// through `CreditRole::from_db_str` — so renaming a variant's stored form strands the seeded
/// rows: they stop reaching the credits list with no error anywhere and no column to blame.
///
/// An equality rather than a containment, so a migration that starts seeding a second role has to
/// say so here, and so a rename that empties the set fails rather than passing vacuously.
#[test]
fn every_role_a_migration_seeds_is_one_this_build_reads_back() {
    use melodia_core::entities::credits::{CreditRole, ROLES};

    let dir = std::path::Path::new(melodia_testkit::REPO_ROOT).join("migrations");
    let (sql, unreadable): (Vec<String>, Vec<std::path::PathBuf>) =
        migration_sources(&dir).unwrap_or_default();
    assert!(unreadable.is_empty(), "unreadable migrations: {unreadable:?}");
    assert!(sql.len() >= 5, "the migration set is smaller than any release has shipped");

    let corpus = sql.join("\n");
    let mut seeded: Vec<&'static str> = ROLES
        .into_iter()
        .map(CreditRole::as_db_str)
        .filter(|role| corpus.contains(&format!("'{role}'")))
        .collect();
    seeded.sort_unstable();

    assert_eq!(
        seeded,
        ["composer"],
        "a migration names a role this build no longer stores under that string, or seeds a new one"
    );
}

/// Every `.sql` under `dir`, and the paths that would not read.
fn migration_sources(
    dir: &std::path::Path,
) -> std::io::Result<(Vec<String>, Vec<std::path::PathBuf>)> {
    let mut sources = Vec::new();
    let mut unreadable = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "sql") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => sources.push(text),
            Err(_) => unreadable.push(path),
        }
    }
    Ok((sources, unreadable))
}

/// Where the page's own default sort is spelled, against the arm of `ui::track_sort` each
/// `queries::track::entity_ids` clause was written to reproduce.
const DETAIL_DEFAULTS: [(&str, &str, &str); 3] = [
    ("artists", include_str!("../../melodia-views/src/ui/artists/detail.rs"), "album"),
    ("genres", include_str!("../../melodia-views/src/ui/genres/detail.rs"), "album"),
    ("albums", include_str!("../../melodia-views/src/ui/albums/detail.rs"), "track_number"),
];

/// One row of [`sorted_library`]'s fixture: title, album, disc, track.
type StagedTrack = (&'static str, Option<&'static str>, Option<i32>, Option<i32>);

/// The library the two sides are asked about, plus the ids to ask with.
struct Sorted {
    db: melodia_store::database::DbPool,
    album: i64,
    artist: i64,
    genre: i64,
}

/// A library built out of the partitions the two sides answer differently: a missing album where
/// the comparator reads `""` and `SQLite` sorts `NULL` first, an untagged track number where the
/// comparator sorts last and `SQLite` sorts first, and a case-mixed album name. Its title order,
/// its `sort_key` order and its album order all disagree, so a tidy fixture's accidental
/// agreement cannot carry the assertion.
async fn sorted_library() -> Result<Sorted, AppError> {
    use melodia_core::entities::artist::ArtistCredit;
    use melodia_core::entities::genre::GenreList;
    use melodia_store::database::DbPool;
    use melodia_store::database::queries;
    use melodia_store::database::queries::fixtures::{insert_tagged_track, make_test_metadata};

    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let staged: [StagedTrack; 8] = [
        // The album arm's own partitions: an untagged number and a zero disc beside real ones.
        ("One", Some("Mid"), Some(1), None),
        ("Two", Some("Mid"), Some(1), Some(1)),
        ("Three", Some("Mid"), Some(2), Some(1)),
        ("Eight", Some("Mid"), Some(0), Some(2)),
        // The album term's: a missing name, an empty one, and a case pair that `COLLATE NOCASE`
        // and `BINARY` order differently.
        ("Four", None, Some(1), Some(0)),
        ("Five", Some(""), None, Some(1)),
        ("Six", Some("apple"), Some(1), Some(1)),
        ("Seven", Some("Banana"), Some(1), Some(1)),
    ];
    for (title, album, disc, track) in staged {
        let mut meta = make_test_metadata(title);
        meta.artist = ArtistCredit::from_name("Sorted Artist");
        meta.genres = GenreList::from_name("Sorted Genre");
        meta.album = album.map(str::to_owned);
        meta.disc_number = disc;
        meta.track_number = track;
        let path = std::path::Path::new("/music").join(format!("{title}.mp3"));
        insert_tagged_track(&db, &path.to_string_lossy(), &meta).await?;
    }

    let album = sqlx::query_scalar::<_, i64>("SELECT id FROM albums WHERE name = 'Mid'")
        .fetch_one(db.read())
        .await?;
    let artist =
        sqlx::query_scalar::<_, i64>("SELECT id FROM artists WHERE name = 'Sorted Artist'")
            .fetch_one(db.read())
            .await?;
    let genre = sqlx::query_scalar::<_, i64>("SELECT id FROM genres WHERE name = 'Sorted Genre'")
        .fetch_one(db.read())
        .await?;
    Ok(Sorted { db, album, artist, genre })
}

/// The ids `sort_track_list_rows` puts `rows` in, which is the order the page paints.
fn as_the_page_shows(
    mut rows: Vec<melodia_core::entities::track::TrackListRow>,
    field: &str,
) -> Vec<i64> {
    melodia_views::ui::track_sort::sort_track_list_rows(&mut rows, field, "asc");
    rows.into_iter().map(|r| r.id).collect()
}

fn queued(pairs: &[(i64, i64)]) -> Vec<i64> {
    pairs.iter().map(|&(_, track_id)| track_id).collect()
}

/// **What `library-data.md` said nothing could check.** A detail page re-sorts what it fetched, so
/// its query's own `ORDER BY` decides nothing; the card beside it has no re-sort and its clause is
/// the whole answer. The two are Rust against SQL in crates that cannot name each other, and they
/// have already shipped disagreeing: the same set played in one order from the page and another
/// from the card.
#[tokio::test]
async fn a_card_queues_its_artist_in_the_order_the_page_shows_it() -> Result<(), AppError> {
    let s = sorted_library().await?;
    let rows =
        melodia_store::database::queries::track::get_tracks_by_artist_for_list(&s.db, s.artist)
            .await?;

    let queued_by_the_card =
        melodia_store::database::queries::track::track_ids_by_artists(&s.db, &[s.artist]).await?;

    assert_eq!(queued(&queued_by_the_card), as_the_page_shows(rows, "album"));
    Ok(())
}

#[tokio::test]
async fn a_card_queues_its_genre_in_the_order_the_page_shows_it() -> Result<(), AppError> {
    let s = sorted_library().await?;
    let rows =
        melodia_store::database::queries::track::get_tracks_by_genre_for_list(&s.db, s.genre)
            .await?;

    let queued_by_the_card =
        melodia_store::database::queries::track::track_ids_by_genres(&s.db, &[s.genre]).await?;

    assert_eq!(queued(&queued_by_the_card), as_the_page_shows(rows, "album"));
    Ok(())
}

#[tokio::test]
async fn a_card_queues_its_album_in_the_order_the_page_shows_it() -> Result<(), AppError> {
    let s = sorted_library().await?;
    let rows =
        melodia_store::database::queries::track::get_tracks_by_album_for_list(&s.db, s.album)
            .await?;

    let queued_by_the_card =
        melodia_store::database::queries::track::track_ids_by_albums(&s.db, &[s.album]).await?;

    assert_eq!(queued(&queued_by_the_card), as_the_page_shows(rows, "track_number"));
    Ok(())
}

/// The other half of the agreement, and the one the assertions above cannot see: each page's
/// default is a literal in its own file, so moving one silently leaves the card reproducing an arm
/// nobody's page takes any more.
#[test]
fn every_detail_page_still_defaults_to_the_arm_its_card_reproduces() {
    for (page, source, field) in DETAIL_DEFAULTS {
        // The call's own arguments rather than the file: a `"album"` anywhere else in it would
        // keep this passing after the default moved, which is the one thing it is here for.
        let args = source
            .split_once("resolve_view_sort(")
            .and_then(|(_, rest)| rest.split_once(')'))
            .map_or("", |(args, _)| args);
        assert!(
            args.contains(&format!("\"{field}\"")),
            "{page} detail no longer defaults to `{field}`, and `queries::track::entity_ids` \
             reproduces that arm, so it has to move with it"
        );
    }
}
