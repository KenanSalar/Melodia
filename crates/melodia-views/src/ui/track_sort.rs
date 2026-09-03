//! Shared in-memory sort for already-fetched track-list rows.
//!
//! The single source of truth for every track-list sort in the app. Callers differ only in
//! the element type — a bare `TrackListRow`, a `BrowseFile` wrapper, a `usize` index — and
//! the deterministic tie-breaker, so [`sort_track_rows_by`] factors the
//! `match field → sort_by_cached_key → reverse-if-desc` shape behind a generic accessor.
//!
//! These arms are the **only** answer to a header click, the SQL `track_list_order_by`
//! they were modelled on having gone once every caller started retaining its rows.
//! `ui::tests::track_sort_tests` pins which tokens can reach them.
//!
//! `sort_by_cached_key` is deliberate: a plain `sort_by` would re-`to_lowercase()` both
//! operands on every one of its `O(n log n)` comparisons. `field` is fixed for the whole
//! sort, so branching once and picking the key type per branch beats a uniform key enum —
//! and **per branch is what lets the natural-order arm drop its tie-breaker**, keying on a
//! bare `String` where the other six keep a pair.
//!
//! **What the comparator reads is a trait, not a row type**, because the two views with
//! the largest lists keep pre-converted display rows plus a sidecar rather than DB rows
//! (`ui::track_list_cache`), and forking it would give the app two spellings of its sort
//! semantics.

use crate::entities::track::TrackListRow as RsTrackListRow;

/// The fields a track-list sort compares, so the comparator can be handed either a DB row
/// or a cached display row. The flattening rules live here rather than in the arms, so
/// both implementors are held to the same one.
pub trait TrackSortFields {
    /// Effective disc number; `NULL` and `0` both mean disc 1.
    fn disc(&self) -> i32;
    /// Effective track number, widened so the sentinel can be negated for a
    /// descending sort. `NULL` and `0` sort last ascending.
    fn track(&self) -> i64;
    fn artist(&self) -> &str;
    fn album(&self) -> &str;
    fn genre(&self) -> &str;
    /// `None` sorts first ascending (NULLs-first), matching `SQLite`.
    fn year(&self) -> Option<i32>;
    fn duration_ms(&self) -> i64;
    /// Natural-order key, raw — the comparator owns the case fold.
    fn sort_key(&self) -> &str;
}

impl TrackSortFields for RsTrackListRow {
    fn disc(&self) -> i32 {
        match self.disc_number {
            Some(d) if d != 0 => d,
            _ => 1,
        }
    }

    fn track(&self) -> i64 {
        match self.track_number {
            Some(n) if n != 0 => i64::from(n),
            _ => i64::from(i32::MAX),
        }
    }

    fn artist(&self) -> &str {
        self.artist.as_deref().unwrap_or("")
    }

    fn album(&self) -> &str {
        self.album.as_deref().unwrap_or("")
    }

    fn genre(&self) -> &str {
        self.genre.as_deref().unwrap_or("")
    }

    fn year(&self) -> Option<i32> {
        self.year
    }

    fn duration_ms(&self) -> i64 {
        self.duration_ms
    }

    fn sort_key(&self) -> &str {
        self.sort_key.as_deref().unwrap_or("")
    }
}

/// Sort `items` in place by `field` / `dir`.
///
/// * `row` projects each element to something implementing [`TrackSortFields`].
/// * `secondary` yields the lowercased deterministic tie-breaker — the track title for
///   detail views, the file name for the Files view.
///
/// Unknown and `"title"` fields fall back to the natural-order `sort_key`, then the
/// secondary. `"desc"` reverses the whole order for every field except `track_number`,
/// whose disc and secondary components stay ascending: only the track component flips,
/// by negating its sentinel rather than reversing the tuple.
pub fn sort_track_rows_by<T, F, R, S>(items: &mut [T], field: &str, dir: &str, row: R, secondary: S)
where
    F: TrackSortFields + ?Sized,
    R: Fn(&T) -> &F,
    S: Fn(&T) -> String,
{
    sort_rows(items, field, dir, row, secondary, false);
}

/// [`sort_track_rows_by`] plus the one thing only [`compute_track_order`] can say about
/// itself: `secondary_is_natural_key` marks a caller whose tie-breaker *is* the value the
/// natural-order arm already compares on, so that arm keys on it once instead of twice.
/// `(k, k)` and `k` are the same comparison — arithmetic, not an assumption about the
/// input — and the second copy cost a `String` per row on the default sort, which every
/// cold fetch takes. Only that arm can drop it.
///
/// A private flag rather than a public parameter because exactly one caller may pass
/// `true` and it lives in this file.
fn sort_rows<T, F, R, S>(
    items: &mut [T],
    field: &str,
    dir: &str,
    row: R,
    secondary: S,
    secondary_is_natural_key: bool,
) where
    F: TrackSortFields + ?Sized,
    R: Fn(&T) -> &F,
    S: Fn(&T) -> String,
{
    let desc = dir == "desc";

    // Disc ASC, track ASC/DESC, secondary ASC. The direction flips only the track
    // component, so it is negated for `desc` and the whole tuple sorted ascending —
    // never `.reverse()`, which would flip disc and secondary too.
    if field == "track_number" {
        items.sort_by_cached_key(|t| {
            let r = row(t);
            let track = r.track();
            (r.disc(), if desc { -track } else { track }, secondary(t))
        });
        return;
    }

    match field {
        "artist" => {
            items.sort_by_cached_key(|t| (row(t).artist().to_lowercase(), secondary(t)));
        }
        "album" => {
            items.sort_by_cached_key(|t| (row(t).album().to_lowercase(), secondary(t)));
        }
        "genre" => {
            items.sort_by_cached_key(|t| (row(t).genre().to_lowercase(), secondary(t)));
        }
        "year" => {
            items.sort_by_cached_key(|t| (row(t).year(), secondary(t)));
        }
        "length" => {
            items.sort_by_cached_key(|t| (row(t).duration_ms(), secondary(t)));
        }
        // "title" and any unrecognised field → natural-order `sort_key`.
        _ if secondary_is_natural_key => {
            items.sort_by_cached_key(|t| row(t).sort_key().to_ascii_lowercase());
        }
        _ => items.sort_by_cached_key(|t| (row(t).sort_key().to_ascii_lowercase(), secondary(t))),
    }
    if desc {
        items.reverse();
    }
}

/// Sort a detail view's `TrackListRow` slice in place, with the track title as the
/// tie-breaker — the shared shape for the Album / Artist / Genre detail lists. Playlist
/// Detail has its own position-aware variant and doesn't use this.
pub fn sort_track_list_rows(rows: &mut [RsTrackListRow], field: &str, dir: &str) {
    sort_track_rows_by(rows, field, dir, |r| r, |r| r.title.to_lowercase());
}

/// The display-order permutation of `rows`, without reordering `rows` itself — for the two
/// cached list views, which keep rows and keys in a fixed fetch order and re-sort by
/// swapping a separate `Vec<usize>`. Generic over the element, so a caller holding rows
/// and keys in two parallel `Vec`s can zip a slice of borrowing views rather than the sort
/// growing a second spelling. Its natural-order `sort_key` tie-breaker is also what lets
/// it take [`sort_rows`]' `secondary_is_natural_key` arm.
pub fn compute_track_order<F: TrackSortFields>(rows: &[F], field: &str, dir: &str) -> Vec<usize> {
    let mut indexed: Vec<(usize, &F)> = rows.iter().enumerate().collect();
    sort_rows(&mut indexed, field, dir, |t| t.1, |t| t.1.sort_key().to_ascii_lowercase(), true);
    indexed.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
#[path = "tests/track_sort_tests.rs"]
mod tests;
