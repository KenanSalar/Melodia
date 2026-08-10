//! Tests for the retained track-list cache.
//!
//! Two things here are worth more than the rest. `RowSearchKey` is the one
//! matcher that doesn't call `row_match::track_matches` — the two largest
//! lists fold their fields once per fetch and pack them into a single string
//! — so it is the one place the two answers to "does this row match" could
//! drift apart. And the cache sorts *converted* rows through a two-field
//! sidecar rather than the DB rows the rest of the app sorts, so the two
//! inputs to the shared comparator have to agree field for field.

use super::*;

/// A row with a distinct value in every searchable field, so a needle can
/// only be answered by the field it names.
fn row(id: i64) -> RsTrackListRow {
    RsTrackListRow {
        id,
        file_path: "/m/1.flac".to_owned(),
        file_name: "1.flac".to_owned(),
        title: "Ghost Town".to_owned(),
        artist: Some("The Specials".to_owned()),
        album_artist: Some("Various Artists".to_owned()),
        album: Some("More Specials".to_owned()),
        genre: Some("Ska".to_owned()),
        track_number: None,
        disc_number: None,
        year: Some(1981),
        artwork_path: None,
        duration_ms: 0,
        is_favorite: false,
        rating: 0,
        album_id: None,
        artist_id: None,
        genre_id: None,
        date_added: "2026-01-01T00:00:00Z".to_owned(),
        sort_key: None,
    }
}

// --- RowSearchKey --------------------------------------------------------

#[test]
fn the_packed_key_matches_every_searchable_field() {
    let key = RowSearchKey::from_row(&row(1));
    for needle in ["ghost", "specials", "various", "more", "ska", "1981", "198"] {
        assert!(key.matches(&row_match::fold_needle(needle)), "missed {needle:?}");
    }
    assert!(!key.matches(&row_match::fold_needle("zzz")));
}

#[test]
fn an_empty_needle_matches_the_packed_key() {
    // The unfiltered list relies on this rather than branching itself.
    assert!(RowSearchKey::from_row(&row(1)).matches(&row_match::fold_needle("")));
}

#[test]
fn the_packed_key_folds_accents() {
    let mut r = row(1);
    r.artist = Some("Björk".to_owned());
    r.title = "Bế Tắc".to_owned();
    let key = RowSearchKey::from_row(&r);
    assert!(key.matches(&row_match::fold_needle("bjork")));
    assert!(key.matches(&row_match::fold_needle("be")));
}

#[test]
fn a_needle_cannot_straddle_the_packed_separator() {
    // Fields are joined by `\0`; a needle spanning two of them would make
    // the cached lists match rows no other list does.
    let key = RowSearchKey::from_row(&row(1));
    assert!(!key.matches(&row_match::fold_needle("townthe")));
}

#[test]
fn a_nul_in_a_field_cannot_forge_a_separator() {
    // `push_folded` maps NUL to a space, so a tag carrying one can't split
    // its own field in two and let a needle match across the seam.
    let mut r = row(1);
    r.title = "Ghost\0Town".to_owned();
    let key = RowSearchKey::from_row(&r);
    assert!(key.matches(&row_match::fold_needle("ghost town")));
}

#[test]
fn the_packed_key_and_track_matches_agree_field_for_field() {
    // Structural drift is already hard — both read `row_match::search_fields`
    // — but the year is matched by a rule each calls separately, and a
    // disagreement would show as the cached lists narrowing differently from
    // every detail page for the same query.
    //
    // The second row is the case that isn't structural at all: a field
    // carrying a `\0` (ID3v2.4's multi-value separator) is folded to a space
    // on the packed side, and `Needle::contains`' ASCII byte walk used to skip
    // that mapping — so the two agreed on every clean row and parted on the
    // one shape the packing exists to handle.
    let mut nul = row(2);
    nul.artist = Some("Queen\0David Bowie".to_owned());

    for r in [row(1), nul] {
        let key = RowSearchKey::from_row(&r);
        for needle in [
            "ghost", "specials", "various", "more", "ska", "1981", "198", "queen david", "zzz", "",
        ] {
            let folded = row_match::fold_needle(needle);
            assert_eq!(
                key.matches(&folded),
                row_match::track_matches(&r, &folded),
                "packed key and track_matches disagree on {needle:?} for row {}",
                r.id
            );
        }
    }
}

// --- the cache -----------------------------------------------------------

/// Every sortable field carries a distinct value per row, and every arm's
/// edge case is represented: a missing disc beside an explicit one, a
/// `None`/`0` track number beside real ones, a missing `sort_key`.
fn sortable_rows() -> Vec<RsTrackListRow> {
    let mut rows = Vec::new();
    for (id, title, artist, album, genre, year, disc, track, len) in [
        (1, "Delta", "Zebra", "Nadir", "Rock", 1999, Some(2), Some(3), 300_000),
        (2, "alpha", "acorn", "Zenith", "Jazz", 1970, None, Some(1), 100_000),
        (3, "Charlie", "Mango", "apex", "Blues", 1985, Some(1), None, 200_000),
        (4, "bravo", "mango", "Apex", "ambient", 2020, Some(1), Some(0), 400_000),
    ] {
        let mut r = row(id);
        r.title = title.to_owned();
        r.artist = Some(artist.to_owned());
        r.album = Some(album.to_owned());
        r.genre = Some(genre.to_owned());
        r.year = Some(year);
        r.disc_number = disc;
        r.track_number = track;
        r.duration_ms = len;
        // Deliberately unset on one row: the default arm and every arm's
        // tie-breaker read it, so `None` must reach the sentinel from both
        // sides of the comparator.
        r.sort_key = (id != 3).then(|| format!("{title} {artist}").to_lowercase());
        rows.push(r);
    }
    rows
}

/// The seven tokens a header cell can emit, plus the unrecognised one that
/// must fall through to the natural-order arm.
const FIELDS: [&str; 8] = [
    "title",
    "artist",
    "album",
    "genre",
    "year",
    "length",
    "track_number",
    "bogus",
];

#[test]
fn the_cache_sorts_exactly_as_the_db_rows_do() {
    // The cache hands the shared comparator converted rows plus a two-field
    // sidecar, where every other view hands it `TrackListRow`s. If the two
    // inputs disagree, the largest list in the app sorts differently from the
    // detail pages — so this walks both through the same comparator and
    // demands the same permutation.
    //
    // Mutations this catches: dropping `TrackSortKey::disc`'s flattening,
    // storing a pre-folded `sort_key` while the comparator still folds, and
    // reading `year`/`track_number` off the UI row without reconstructing the
    // sentinel a `NULL` collapsed onto.
    let rows = sortable_rows();
    for field in FIELDS {
        for dir in ["asc", "desc"] {
            let expected: Vec<i64> = track_sort::compute_track_order(&rows, field, dir)
                .into_iter()
                .map(|i| rows[i].id)
                .collect();

            let cache = TrackListCache::new();
            cache.store(rows.clone(), field, dir);
            let actual = cache.snapshot().ids_filtered(&row_match::fold_needle(""));

            assert_eq!(actual, expected, "cache diverged on {field:?}/{dir:?}");
        }
    }
}

#[test]
fn a_stored_set_leaves_its_four_vectors_aligned() {
    // `order` indexes `rows`, `search` and `sort` alike, and the whole reason
    // they share one lock is that a reader must never see them disagree.
    let rows = sortable_rows();
    let cache = TrackListCache::new();
    cache.store(rows.clone(), "title", "asc");
    let snap = cache.snapshot();

    assert_eq!(snap.total(), 4);
    assert!(!snap.is_empty());

    // Every row is reachable exactly once, and the ids the sort keys carry
    // line up with the rows the display walk yields.
    let ids = snap.ids_filtered(&row_match::fold_needle(""));
    let mut seen = ids.clone();
    seen.sort_unstable();
    assert_eq!(seen, [1, 2, 3, 4]);

    let titles: Vec<String> = snap
        .visible(&row_match::fold_needle(""))
        .iter()
        .map(|r| r.title.to_string())
        .collect();
    let by_id: Vec<String> = ids
        .iter()
        .map(|id| {
            rows.iter()
                .find(|r| r.id == *id)
                .map_or_else(String::new, |r| r.title.clone())
        })
        .collect();
    assert_eq!(titles, by_id, "visible() and ids_filtered() disagree on order");
}

#[test]
fn a_filter_narrows_both_walks_the_same_way() {
    let cache = TrackListCache::new();
    cache.store(sortable_rows(), "title", "asc");
    let snap = cache.snapshot();

    let needle = row_match::fold_needle("mango");
    assert_eq!(snap.ids_filtered(&needle), [3, 4]);
    assert_eq!(snap.visible(&needle).len(), 2);
    // The unfiltered count is the view's `total-count` and must not follow
    // the filter — the header states the library, not the query.
    assert_eq!(snap.total(), 4);
}

#[test]
fn a_removal_keeps_the_order_pointing_at_the_rows_it_names() {
    // Favorites removes a row when it is unfavourited, which is the one
    // operation that changes the set's length. `order` holds indices, so
    // every slot past the removed one shifts — miss that and the list either
    // panics or renders a neighbour under the wrong id.
    let cache = TrackListCache::new();
    cache.store(sortable_rows(), "title", "asc");

    cache.remove(3);
    let snap = cache.snapshot();
    assert_eq!(snap.total(), 3);

    let ids = snap.ids_filtered(&row_match::fold_needle(""));
    assert!(!ids.contains(&3), "removed id still in display order");
    let mut seen = ids.clone();
    seen.sort_unstable();
    assert_eq!(seen, [1, 2, 4]);

    // The surviving rows still line up with the ids beside them.
    let visible = snap.visible(&row_match::fold_needle(""));
    assert_eq!(visible.len(), ids.len());
    for (row, id) in visible.iter().zip(&ids) {
        assert_eq!(i64::from(row.id), *id);
    }

    // Removing an id that isn't there is a no-op, not a panic.
    cache.remove(999);
    assert_eq!(cache.snapshot().total(), 3);
}

#[test]
fn a_single_row_patch_leaves_a_live_snapshot_alone() {
    // The patches are copy-on-write so a rebuild already holding a snapshot
    // keeps a consistent view. That is the property the `Arc` is for; without
    // it a star click mid-refilter would mutate the set being walked.
    let cache = TrackListCache::new();
    cache.store(sortable_rows(), "title", "asc");

    let before = cache.snapshot();
    cache.set_rating(1, 5);
    cache.set_favorite(1, true);
    let after = cache.snapshot();

    let rating_of = |snap: &CacheData| {
        snap.visible(&row_match::fold_needle(""))
            .into_iter()
            .find(|r| r.id == 1)
            .map(|r| (r.rating, r.is_favorite))
    };

    assert_eq!(rating_of(&before), Some((0, false)));
    assert_eq!(rating_of(&after), Some((5, true)));
}

#[test]
fn a_cleared_cache_reads_as_empty_rather_than_stale() {
    // A section leave hands the rows back; anything still answering off the
    // old set would repopulate a view the leave just emptied.
    let cache = TrackListCache::new();
    cache.store(sortable_rows(), "title", "asc");
    cache.clear();

    let snap = cache.snapshot();
    assert!(snap.is_empty());
    assert_eq!(snap.total(), 0);
    assert!(snap.ids_filtered(&row_match::fold_needle("")).is_empty());
    assert!(snap.visible(&row_match::fold_needle("")).is_empty());
}

#[test]
fn the_cache_conflates_a_zero_year_with_a_missing_one() {
    // Recorded rather than incidental. A converted row stores `year` as a
    // plain `i32` with `NULL` already folded onto `0`, so the cache cannot
    // tell the two apart and orders them together, where a DB row sorts
    // `None` strictly first. Both render an identical (blank) cell and the
    // tie is broken deterministically by `sort_key`, so this is unobservable
    // — but it is a real difference from `TrackListRow`'s ordering, and the
    // next person to compare the two should find it stated.
    let mut null_year = row(1);
    null_year.year = None;
    null_year.sort_key = Some("b".to_owned());
    let mut zero_year = row(2);
    zero_year.year = Some(0);
    zero_year.sort_key = Some("a".to_owned());
    let rows = vec![null_year, zero_year];

    // DB rows: `None` first regardless of the tie-breaker.
    assert_eq!(
        track_sort::compute_track_order(&rows, "year", "asc"),
        [0, 1]
    );

    // Cache: tied on year, so `sort_key` decides and row 2 leads.
    let cache = TrackListCache::new();
    cache.store(rows, "year", "asc");
    assert_eq!(
        cache.snapshot().ids_filtered(&row_match::fold_needle("")),
        [2, 1]
    );
}
