//! Tests for the unified in-memory track sorter — `sort_track_rows_by`,
//! `sort_track_list_rows`, and the `compute_track_order` permutation
//! wrapper, which all share one core.
//!
//! These arms are the app's only sort semantics — the SQL `track_list_order_by`
//! they were modelled on is gone, having become unreachable once every caller
//! that could ask for a sort started retaining its rows instead. The default/
//! `"title"` sort uses the natural-order `sort_key`, `track_number` uses a
//! disc/track sentinel composite (disc stays ascending on `desc`), and
//! `Option` fields put `None` first ascending. Seeded values are distinct per
//! field so expected orders are exact.

use super::*;

#[expect(clippy::too_many_arguments, reason = "test row builder mirrors TrackListRow")]
fn mk(
    id: i64,
    sort_key: &str,
    artist: Option<&str>,
    album: Option<&str>,
    genre: Option<&str>,
    year: Option<i32>,
    duration_ms: i64,
    disc_number: Option<i32>,
    track_number: Option<i32>,
) -> RsTrackListRow {
    RsTrackListRow {
        id,
        file_path: format!("/m/{id}.flac"),
        file_name: format!("{id}.flac"),
        title: format!("title{id}"),
        artist: artist.map(str::to_owned),
        album_artist: None,
        album: album.map(str::to_owned),
        genre: genre.map(str::to_owned),
        track_number,
        disc_number,
        year,
        artwork_path: None,
        duration_ms,
        is_favorite: false,
        rating: 0,
        album_id: None,
        artist_id: None,
        genre_id: None,
        date_added: "2026-01-01T00:00:00Z".to_owned(),
        sort_key: Some(sort_key.to_owned()),
    }
}

/// Project a permutation back to track ids.
fn perm_ids(rows: &[RsTrackListRow], order: &[usize]) -> Vec<i64> {
    order.iter().map(|&i| rows[i].id).collect()
}

/// Project a sorted slice to track ids.
fn ids(rows: &[RsTrackListRow]) -> Vec<i64> {
    rows.iter().map(|r| r.id).collect()
}

/// Four rows with distinct values on every sortable column, plus
/// deliberate `None`s (artist/album/genre/year) and a disc/track spread.
fn fixture() -> Vec<RsTrackListRow> {
    vec![
        mk(1, "b", Some("Zeta"), Some("M"), Some("rock"), Some(2000), 300, Some(1), Some(2)),
        mk(2, "a", None, Some("a"), Some("Jazz"), None, 100, Some(1), Some(1)),
        mk(3, "c", Some("alpha"), Some("z"), Some("blues"), Some(1990), 200, Some(2), None),
        mk(4, "d", Some("Mike"), None, None, Some(2010), 250, None, Some(5)),
    ]
}

// --- compute_track_order (permutation) ----------------------------------

#[test]
fn title_sorts_by_sort_key() {
    let rows = fixture();
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "title", "asc")), [2, 1, 3, 4]);
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "title", "desc")), [4, 3, 1, 2]);
    // Unrecognised field falls through to the same `sort_key` sort.
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "bogus", "asc")), [2, 1, 3, 4]);
}

#[test]
fn artist_sort_folds_case_and_puts_nulls_first_asc() {
    let rows = fixture();
    // None/"" < "alpha" < "mike" < "zeta" (case-folded).
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "artist", "asc")), [2, 3, 4, 1]);
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "artist", "desc")), [1, 4, 3, 2]);
}

#[test]
fn album_sort_handles_nulls() {
    let rows = fixture();
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "album", "asc")), [4, 2, 1, 3]);
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "album", "desc")), [3, 1, 2, 4]);
}

#[test]
fn genre_sort_handles_nulls() {
    let rows = fixture();
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "genre", "asc")), [4, 3, 2, 1]);
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "genre", "desc")), [1, 2, 3, 4]);
}

#[test]
fn year_sort_puts_nulls_first_asc() {
    let rows = fixture();
    // None < 1990 < 2000 < 2010.
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "year", "asc")), [2, 3, 1, 4]);
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "year", "desc")), [4, 1, 3, 2]);
}

#[test]
fn length_sorts_by_duration() {
    let rows = fixture();
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "length", "asc")), [2, 3, 4, 1]);
    assert_eq!(perm_ids(&rows, &compute_track_order(&rows, "length", "desc")), [1, 4, 3, 2]);
}

#[test]
fn track_number_sorts_disc_then_track_with_null_sentinel() {
    let rows = fixture();
    // disc 1: track 1 (id2), 2 (id1), 5 (id4); disc 2: NULL (id3) last.
    assert_eq!(
        perm_ids(&rows, &compute_track_order(&rows, "track_number", "asc")),
        [2, 1, 4, 3]
    );
    // desc flips only the track component — disc + NULL placement stay:
    // disc 1: track 5 (id4), 2 (id1), 1 (id2); disc 2: NULL (id3).
    assert_eq!(
        perm_ids(&rows, &compute_track_order(&rows, "track_number", "desc")),
        [4, 1, 2, 3]
    );
}

#[test]
fn order_is_a_permutation_of_all_indices() {
    let rows = fixture();
    for field in ["title", "artist", "album", "genre", "year", "length", "track_number"] {
        for dir in ["asc", "desc"] {
            let mut order = compute_track_order(&rows, field, dir);
            order.sort_unstable();
            assert_eq!(order, [0, 1, 2, 3], "field={field} dir={dir}");
        }
    }
}

// --- sort_track_list_rows / sort_track_rows_by (in place) ---------------

#[test]
fn sort_track_list_rows_matches_the_permutation() {
    // The in-place wrapper and the permutation wrapper share one core, so
    // sorting a slice must yield the same id order as the permutation.
    for field in ["title", "artist", "album", "genre", "year", "length", "track_number"] {
        for dir in ["asc", "desc"] {
            let base = fixture();
            let expect = perm_ids(&base, &compute_track_order(&base, field, dir));
            let mut rows = fixture();
            sort_track_list_rows(&mut rows, field, dir);
            assert_eq!(ids(&rows), expect, "field={field} dir={dir}");
        }
    }
}

#[test]
fn track_number_desc_keeps_disc_ascending() {
    // Regression guard: `desc` must not reverse disc order — only the
    // track component flips.
    let mut rows = vec![
        mk(1, "a", None, None, None, None, 0, Some(2), Some(1)),
        mk(2, "b", None, None, None, None, 0, Some(1), Some(1)),
        mk(3, "c", None, None, None, None, 0, Some(1), Some(2)),
    ];
    sort_track_list_rows(&mut rows, "track_number", "desc");
    // disc 1 before disc 2; within disc 1, track 2 before track 1.
    assert_eq!(ids(&rows), [3, 2, 1]);
}

#[test]
fn custom_secondary_key_breaks_ties() {
    // Browse-style: file name, not title/sort_key, is the tie-breaker.
    let mut rows = vec![
        mk(1, "same", Some("A"), None, None, None, 0, None, None),
        mk(2, "same", Some("A"), None, None, None, 0, None, None),
    ];
    sort_track_rows_by(&mut rows, "artist", "asc", |r| r, |r| r.file_name.to_lowercase());
    // file_name "1.flac" < "2.flac".
    assert_eq!(ids(&rows), [1, 2]);
}

/// Every field a `TrackList` header cell can ask for has to be one the
/// comparator has an arm for.
///
/// The token is a bare string on both sides — a `field:` on a `HeaderCell`
/// mount, a `match` arm in [`sort_track_rows_by`] — so a rename on either side
/// compiles and the column quietly sorts by the natural-order default while
/// painting its arrow as though it had worked. This is the `SortPillRow` pin
/// (`ui::my_library::tests`, `ui::favorites::tests`) for the other half of the
/// tree's sortable surfaces, and it earns its place now that `track_list_order_by`
/// is gone — nothing else answers these tokens any more.
#[test]
fn every_header_column_asks_for_a_field_the_comparator_knows() {
    const HEADER: &str =
        include_str!("../../../melodia-ui/ui/components/track-list/track-list-header.slint");
    // The arms above, restated. `track_number` is the one handled ahead of the
    // `match`; the rest are its named arms. A field dropped there and left here
    // fails the round-trip below.
    const ARMS: [&str; 7] =
        ["track_number", "title", "artist", "album", "genre", "year", "length"];

    let asked: Vec<&str> = HEADER
        .lines()
        .filter_map(|line| line.trim().strip_prefix("field: \""))
        .filter_map(|rest| rest.split_once('"'))
        .map(|(field, _)| field)
        .collect();

    assert_eq!(
        asked.len(),
        ARMS.len(),
        "track-list-header.slint must name one sort field per column: got {asked:?}"
    );
    for field in &asked {
        assert!(
            ARMS.contains(field),
            "a header cell asks for `{field}`, which falls through to the sort_key arm"
        );
    }
}
