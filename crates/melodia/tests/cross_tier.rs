//! Assertions that hold two tiers against each other, and so can live inside neither.
//!
//! Each one pins a value in one layer against a value in another, and each was written in the
//! lower of the two — where it compiled while the tree was one crate and would stop the moment
//! the boundary became a manifest line. A store test may not name the UI's cover tiers, a
//! settings test may not name the nav map, and a database test may not name the filter boxes.
//! Held from outside all of them instead, which is also the only place that can see both.
//!
//! `docs/plans/WORKSPACE_SPLIT.md` finding 8 gives every corpus walk one home; these are the
//! walks' compile-time cousins and land in the same place for the same reason.

use melodia::error::AppError;

/// The invariant `STORE_MAX_DIM` was picked from: the store must hold at least what the largest
/// tier decodes, or every tier upscales from a source the store already threw away.
///
/// A runtime test rather than a `const _`, `row_cover_size` and `cover_size` both being
/// functions.
#[test]
fn every_cover_tier_decodes_within_the_store_cap() {
    use melodia::media::image::artwork::STORE_MAX_DIM;
    use melodia::media::image::cover_thumbs::row_cover_size;
    use melodia::ui::grid_prewarm::{GRID_COVER_FALLBACK, cover_size};
    use melodia::ui::util::COVER_SIZE;

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
        melodia::services::view_state::MAX_NAV_INDEX,
        melodia::ui::radio::NAV_RADIO,
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
    use melodia::database::DbPool;

    // `composer` has no slot on `TrackListRow`, and `file_name` is left out deliberately — the
    // tiebreaker weight that keeps a filename echo below the tags it repeats has no equivalent in
    // an unranked substring filter. `year` is an integer that joins the match through
    // `row_match::Needle::matches_number` instead of the text list.
    const NOT_TEXT_SEARCHED: [&str; 3] = ["composer", "file_name", "year"];

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
    let row = melodia::entities::track::TrackListRow {
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
        melodia::ui::row_match::search_fields(&row).as_slice(),
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
        .split_once("pub fn set_last_nav_index")
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
