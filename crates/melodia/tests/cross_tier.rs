//! Assertions that hold two tiers against each other, and so can live inside neither.
//!
//! Each one pins a value in one layer against a value in another, and each was written in the
//! lower of the two — where it compiled while the tree was one crate and would stop the moment
//! the boundary became a manifest line. A store test may not name the UI's cover tiers, a
//! settings test may not name the nav map, and a database test may not name the lists' sort.
//! Held from outside all of them instead, which is also the only place that can see both.
//!
//! `.claude/rules/testing.md` gives every corpus walk one home; these are the walks'
//! compile-time cousins and land in the same place for the same reason.

use melodia_core::entities::smart_criteria::Draw;
use melodia_core::entities::track::TrackListRow;
use melodia_core::entities::track_list::{SortDir, TrackListScope, TrackOrder, TrackSortField};
use melodia_core::error::AppError;
use melodia_views::ui::track_sort::TrackSortFields;

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

/// **The persisted nav index is clamped in one crate and guarded in another**, and both ends have
/// to take the bound from `MAX_NAV_INDEX` rather than restate it.
///
/// A source read because the read needs an `AppWindow`, and because
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
        .split_once("fn set_last_nav_index")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or(String::new(), |(body, _)| body.to_owned());
    assert!(!clamp.is_empty(), "`set_last_nav_index` moved, so this pin reads nothing");
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
/// comparator sorts last and `SQLite` sorts first, a case-mixed album name, and an accented case
/// pair that `to_lowercase()` ties and `NOCASE` does not. Its title order, its `sort_key` order and
/// its album order all disagree, so a tidy fixture's accidental agreement cannot carry the
/// assertion.
async fn sorted_library() -> Result<Sorted, AppError> {
    use melodia_core::entities::artist::ArtistCredit;
    use melodia_core::entities::genre::GenreList;
    use melodia_store::database::DbPool;
    use melodia_store::database::queries;
    use melodia_store::database::queries::fixtures::{insert_tagged_track, make_test_metadata};

    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let staged: [StagedTrack; 10] = [
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
        // Tied by `to_lowercase()`, so the title decides; `NOCASE` puts `Émile` first instead.
        ("Eleven", Some("émile"), Some(1), Some(1)),
        ("Twelve", Some("Émile"), Some(1), Some(1)),
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
fn as_the_page_shows(mut rows: Vec<TrackListRow>, field: &str) -> Vec<i64> {
    melodia_views::ui::track_sort::sort_track_list_rows(&mut rows, field, "asc");
    rows.into_iter().map(|r| r.id).collect()
}

/// The rows a detail page lists for `scope`, read through its own keyset, in id order: the order
/// SQL breaks a full tie in, which the comparator's stable sort keeps.
async fn page_rows_by_id(
    db: &melodia_store::database::DbPool,
    scope: TrackListScope,
) -> Result<Vec<TrackListRow>, AppError> {
    let mut ids = keyset(db, scope, "", SortDir::Asc).await?;
    ids.sort_unstable();
    melodia_store::database::queries::track::get_track_list_rows_by_ids(db, &ids).await
}

fn queued(pairs: &[(i64, i64)]) -> Vec<i64> {
    pairs.iter().map(|&(_, track_id)| track_id).collect()
}

/// **What `library-data.md` said nothing could check.** The card beside a detail page queues
/// through `entity_ids`, a second SQL spelling of the page's default order, and the two have
/// already shipped disagreeing: the same set played in one order from the page and another from
/// the card. Both are held to the comparator, Rust against SQL in crates that cannot name each
/// other, over the page's own membership.
#[tokio::test]
async fn a_card_queues_its_artist_in_the_order_the_page_shows_it() -> Result<(), AppError> {
    let s = sorted_library().await?;
    let rows = page_rows_by_id(&s.db, TrackListScope::Artist(s.artist)).await?;

    let queued_by_the_card =
        melodia_store::database::queries::track::track_ids_by_artists(&s.db, &[s.artist]).await?;

    assert_eq!(queued(&queued_by_the_card), as_the_page_shows(rows, "album"));
    Ok(())
}

#[tokio::test]
async fn a_card_queues_its_genre_in_the_order_the_page_shows_it() -> Result<(), AppError> {
    let s = sorted_library().await?;
    let rows = page_rows_by_id(&s.db, TrackListScope::Genre(s.genre)).await?;

    let queued_by_the_card =
        melodia_store::database::queries::track::track_ids_by_genres(&s.db, &[s.genre]).await?;

    assert_eq!(queued(&queued_by_the_card), as_the_page_shows(rows, "album"));
    Ok(())
}

#[tokio::test]
async fn a_card_queues_its_album_in_the_order_the_page_shows_it() -> Result<(), AppError> {
    let s = sorted_library().await?;
    let rows = page_rows_by_id(&s.db, TrackListScope::Album(s.album)).await?;

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

// === The keysets against the comparator ===
//
// `queries::track::keyset` sorts and filters in SQL for a list holding only ids, and has to land
// every row where `ui::track_sort` and `ui::row_match` put it in memory. Rust against SQL again,
// held from here for the reason the card tests above give.

/// Every token a header cell or a saved view can hand over, plus one nothing spells, which both
/// sides read as the natural order. `"position"` is a playlist's alone and is the natural order
/// everywhere else.
const SORT_TOKENS: [&str; 9] =
    ["title", "track_number", "artist", "album", "genre", "year", "length", "position", "unknown"];

const DIRECTIONS: [SortDir; 2] = [SortDir::Asc, SortDir::Desc];

/// One row of [`parity_library`]: title, artist, album, genre, year, track, disc, length in ms,
/// favourite.
type ParityTrack = (
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    i64,
    bool,
);

/// A library out of the partitions SQL and the comparator could answer differently: accented and
/// non-Latin case pairs, a final sigma, missing, empty and case-mixed albums, missing, zero and
/// negative years, missing and zero track and disc numbers, tied lengths, and two rows equal on
/// every key, which only the id can order.
///
/// The Songs tabs read a length clamped to `0..=i32::MAX`, so the last five sit on and either side
/// of both ends, titled against their raw order: wherever the clamp ties two of them, the title
/// puts them the other way round.
async fn parity_library() -> Result<melodia_store::database::DbPool, AppError> {
    use melodia_core::entities::artist::ArtistCredit;
    use melodia_core::entities::genre::GenreList;
    use melodia_store::database::DbPool;
    use melodia_store::database::queries;
    use melodia_store::database::queries::fixtures::{insert_tagged_track, make_test_metadata};

    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let staged: [ParityTrack; 16] = [
        ("Zebra", "beta", Some("Émile"), "Rock", Some(1999), Some(2), Some(1), 1000, true),
        ("apple", "Alpha", Some("émile"), "rock", None, None, None, 2000, true),
        ("Éclair", "ALPHA", None, "Jazz", Some(0), Some(1), Some(0), 3000, true),
        ("éclair", "Ωmega", Some(""), "jazz", Some(-5), Some(0), Some(2), 3000, true),
        ("ΟΔΟΣ", "ΟΔΟΣ", Some("apple"), "Σ", Some(2001), Some(3), Some(1), 1000, false),
        ("οδος", "οδος", Some("Banana"), "σ", Some(1999), Some(1), Some(1), 1000, false),
        ("Mango", "Mango", Some("Banana"), "Pop", Some(1999), Some(1), Some(1), 1000, true),
        ("mango", "mango", Some("banana"), "pop", Some(1999), Some(1), Some(1), 1000, true),
        ("mango", "mango", Some("banana"), "pop", Some(1999), Some(1), Some(1), 1000, true),
        ("Björk song", "Björk", Some("Banana"), "Pop", Some(1981), Some(4), Some(1), 500, false),
        ("ΣΟΦΙΑ", "x", Some("apple"), "Σ", None, Some(2), None, 500, false),
        ("Rowan", "x", None, "", None, None, None, 0, true),
        ("Sorrel", "x", None, "", None, None, None, -1, true),
        ("Zinnia", "x", None, "", None, None, None, i64::from(i32::MAX) - 1, true),
        ("Yarrow", "x", None, "", None, None, None, i64::from(i32::MAX), true),
        ("Willow", "x", None, "", None, None, None, i64::from(i32::MAX) + 1, true),
    ];
    for (i, (title, artist, album, genre, year, track, disc, length, favorite)) in
        staged.into_iter().enumerate()
    {
        let mut meta = make_test_metadata(title);
        meta.artist = ArtistCredit::from_name(artist);
        meta.album = album.map(str::to_owned);
        meta.genres = GenreList::from_name(genre);
        meta.year = year;
        meta.track_number = track;
        meta.disc_number = disc;
        meta.duration_ms = length;
        let path = std::path::Path::new("/music").join(format!("{i}.mp3"));
        let id = insert_tagged_track(&db, &path.to_string_lossy(), &meta).await?;
        if favorite {
            queries::track::set_favorite(&db, &[id], true).await?;
        }
    }
    Ok(db)
}

/// How the Songs tabs read a row: a missing year as `0` and the length clamped to an `i32` of
/// milliseconds, the way their converted display rows hold both.
struct SongsRow<'a>(&'a TrackListRow);

impl TrackSortFields for SongsRow<'_> {
    fn disc(&self) -> i32 {
        self.0.disc()
    }
    fn track(&self) -> i64 {
        self.0.track()
    }
    fn artist(&self) -> &str {
        self.0.artist()
    }
    fn album(&self) -> &str {
        self.0.album()
    }
    fn genre(&self) -> &str {
        self.0.genre()
    }
    fn year(&self) -> Option<i32> {
        Some(self.0.year.unwrap_or(0))
    }
    fn duration_ms(&self) -> i64 {
        self.0.duration_ms.clamp(0, i64::from(i32::MAX))
    }
    fn sort_key(&self) -> &str {
        self.0.sort_key()
    }
}

/// The comparator's order for a Songs tab, over `rows` handed over in id order: the order SQL
/// breaks a full tie in, which the comparator's stable sort keeps.
fn as_the_songs_tab_shows(rows: &[TrackListRow], token: &str, dir: SortDir) -> Vec<i64> {
    let songs: Vec<SongsRow> = rows.iter().map(SongsRow).collect();
    melodia_views::ui::track_sort::compute_track_order(&songs, token, dir.as_str())
        .into_iter()
        .map(|i| rows[i].id)
        .collect()
}

/// [`as_the_songs_tab_shows`] for a detail page, which sorts its rows as stored.
fn as_the_detail_shows(rows: &[TrackListRow], token: &str, dir: SortDir) -> Vec<i64> {
    let mut sorted = rows.to_vec();
    melodia_views::ui::track_sort::sort_track_list_rows(&mut sorted, token, dir.as_str());
    sorted.iter().map(|r| r.id).collect()
}

async fn keyset(
    db: &melodia_store::database::DbPool,
    scope: TrackListScope,
    token: &str,
    dir: SortDir,
) -> Result<Vec<i64>, AppError> {
    let order = TrackOrder { field: TrackSortField::from_token(token), dir };
    let empty = melodia_core::utils::needle::Needle::default();
    melodia_store::database::queries::track::track_ids(db, scope, order, &empty).await
}

async fn every_row_by_id(
    db: &melodia_store::database::DbPool,
) -> Result<Vec<TrackListRow>, AppError> {
    let mut rows = melodia_store::database::queries::track::get_all_tracks_for_list(db).await?;
    rows.sort_by_key(|r| r.id);
    Ok(rows)
}

#[tokio::test]
async fn the_library_keyset_sorts_as_the_songs_tab_does() -> Result<(), AppError> {
    let db = parity_library().await?;
    let rows = every_row_by_id(&db).await?;

    for token in SORT_TOKENS {
        for dir in DIRECTIONS {
            assert_eq!(
                keyset(&db, TrackListScope::Library, token, dir).await?,
                as_the_songs_tab_shows(&rows, token, dir),
                "Songs by {token} {}",
                dir.as_str()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn the_favorites_keyset_sorts_as_its_songs_tab_does() -> Result<(), AppError> {
    let db = parity_library().await?;
    let favorites: Vec<TrackListRow> =
        every_row_by_id(&db).await?.into_iter().filter(|r| r.is_favorite).collect();

    for token in SORT_TOKENS {
        for dir in DIRECTIONS {
            assert_eq!(
                keyset(&db, TrackListScope::Favorites, token, dir).await?,
                as_the_songs_tab_shows(&favorites, token, dir),
                "Favorites by {token} {}",
                dir.as_str()
            );
        }
    }
    Ok(())
}

/// The album, artist and genre pages, each over the rows the whole table holds for it. Every
/// track in [`parity_library`] carries one artist and one genre, so its first credit and first tag
/// name the entity its join tables do.
#[tokio::test]
async fn a_detail_keyset_sorts_as_its_page_does() -> Result<(), AppError> {
    let db = parity_library().await?;
    let id_of = |sql: &'static str| sqlx::query_scalar::<_, i64>(sql).fetch_one(db.read());
    let album = id_of("SELECT id FROM albums WHERE name = 'Banana'").await?;
    let artist = id_of("SELECT id FROM artists WHERE name = 'Mango'").await?;
    let genre = id_of("SELECT id FROM genres WHERE name = 'Pop'").await?;
    let rows = every_row_by_id(&db).await?;
    let held = |of: fn(&TrackListRow) -> Option<i64>, id: i64| -> Vec<TrackListRow> {
        rows.iter().filter(|r| of(r) == Some(id)).cloned().collect()
    };
    let pages = [
        ("album", TrackListScope::Album(album), held(|r| r.album_id, album)),
        ("artist", TrackListScope::Artist(artist), held(|r| r.artist_id, artist)),
        ("genre", TrackListScope::Genre(genre), held(|r| r.genre_id, genre)),
    ];

    for (page, scope, rows) in pages {
        for token in SORT_TOKENS {
            for dir in DIRECTIONS {
                assert_eq!(
                    keyset(&db, scope, token, dir).await?,
                    as_the_detail_shows(&rows, token, dir),
                    "{page} detail by {token} {}",
                    dir.as_str()
                );
            }
        }
    }
    Ok(())
}

/// A playlist sorted by anything but its own order sorts as a detail page does. Its own order is
/// pinned beside the query, needing no comparator.
#[tokio::test]
async fn a_playlist_keyset_sorts_as_its_page_does() -> Result<(), AppError> {
    let db = parity_library().await?;
    let rows = every_row_by_id(&db).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let playlist = melodia_store::database::queries::playlist::create_playlist_with_tracks(
        &db,
        "Everything",
        None,
        &ids,
    )
    .await?;

    for token in SORT_TOKENS.iter().filter(|&&token| token != "position") {
        for dir in DIRECTIONS {
            assert_eq!(
                keyset(&db, TrackListScope::Playlist(playlist, Draw::default()), token, dir)
                    .await?,
                as_the_detail_shows(&rows, token, dir),
                "playlist by {token} {}",
                dir.as_str()
            );
        }
    }
    Ok(())
}

/// **The SQL filter keeps the rows `track_matches` keeps**, accent and case folding, the year's
/// substring rule and a needle naming no field included, in the order the Songs tab shows them.
#[tokio::test]
async fn the_sql_filter_keeps_the_rows_the_filter_boxes_keep() -> Result<(), AppError> {
    use melodia_core::utils::needle::fold_needle;

    let db = parity_library().await?;
    let rows = every_row_by_id(&db).await?;
    let natural = TrackOrder { field: TrackSortField::Title, dir: SortDir::Asc };
    let needles = [
        "",
        "emile",
        "BJORK",
        "björk song",
        "οδος",
        "σοφια",
        "19",
        "199",
        "0",
        "5",
        "-5",
        "pop",
        "ma",
        "zzz",
    ];

    for raw in needles {
        let needle = fold_needle(raw);
        let kept: Vec<TrackListRow> = rows
            .iter()
            .filter(|r| melodia_views::ui::row_match::track_matches(r, &needle))
            .cloned()
            .collect();
        let filtered = melodia_store::database::queries::track::track_ids(
            &db,
            TrackListScope::Library,
            natural,
            &needle,
        )
        .await?;
        assert_eq!(
            filtered,
            as_the_songs_tab_shows(&kept, "title", SortDir::Asc),
            "needle {raw:?}"
        );
    }
    Ok(())
}
