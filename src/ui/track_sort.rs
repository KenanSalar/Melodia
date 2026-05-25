//! Shared in-memory sort for already-fetched track-list rows.
//!
//! The single source of truth for every track-list sort in the app — the
//! Tracks view, the Album / Artist / Genre / Playlist detail track lists,
//! the Files view, and Search results. Callers differ only in the element
//! type (a bare `TrackListRow`, a `BrowseFile` wrapper, or a `usize` index)
//! and the deterministic tie-breaker, so [`sort_track_rows_by`] factors the
//! `match field → sort_by_cached_key → reverse-if-desc` shape behind a
//! generic accessor. [`sort_track_list_rows`] and [`compute_track_order`]
//! are thin specialisations over it.
//!
//! Sort semantics mirror the SQL `track_list_order_by`
//! (`src/database/queries/track.rs`): the default/`"title"` sort uses the
//! natural-order `sort_key`, `track_number` uses a disc/track sentinel
//! composite, and `Option` fields put `None` first ascending.
//!
//! `sort_by_cached_key` is deliberate: a plain `sort_by` would
//! re-`to_lowercase()` both operands on every one of its `O(n log n)`
//! comparisons. `field` is fixed for the whole sort, so branching on it
//! once and picking the key type per branch is cheaper than a uniform key
//! enum.

use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::ui::util::opt_lc;

/// Sort `items` in place by `field` / `dir`.
///
/// * `row` projects each element to its backing [`RsTrackListRow`].
/// * `secondary` yields the lowercased deterministic tie-breaker key (the
///   track title for detail views, the file name for the Files view).
///
/// Unknown / `"title"` fields fall back to the natural-order `sort_key`,
/// then the secondary key. `"desc"` reverses the whole order for every
/// field except `track_number`, whose disc + secondary components stay
/// ascending — only the track component flips (the track sentinel is
/// negated instead of reversing the tuple).
pub fn sort_track_rows_by<T, R, S>(items: &mut [T], field: &str, dir: &str, row: R, secondary: S)
where
    R: Fn(&T) -> &RsTrackListRow,
    S: Fn(&T) -> String,
{
    let desc = dir == "desc";

    // `track_number`: disc ASC, track ASC/DESC (NULL/0 → sentinel), then
    // the secondary key ASC. The direction flips only the track component,
    // so it is negated for `desc` and the whole tuple sorted ascending —
    // never `.reverse()`, which would also flip disc + secondary.
    if field == "track_number" {
        items.sort_by_cached_key(|t| {
            let r = row(t);
            let disc = match r.disc_number {
                Some(d) if d != 0 => d,
                _ => 1,
            };
            let track = match r.track_number {
                Some(n) if n != 0 => i64::from(n),
                _ => i64::from(i32::MAX),
            };
            (disc, if desc { -track } else { track }, secondary(t))
        });
        return;
    }

    match field {
        "artist" => {
            items.sort_by_cached_key(|t| (opt_lc(row(t).artist.as_deref()), secondary(t)));
        }
        "album" => {
            items.sort_by_cached_key(|t| (opt_lc(row(t).album.as_deref()), secondary(t)));
        }
        "genre" => {
            items.sort_by_cached_key(|t| (opt_lc(row(t).genre.as_deref()), secondary(t)));
        }
        // `Option<i32>` key — `None` sorts first ascending (NULLs-first),
        // last after the `desc` reversal, matching SQLite's `year` ordering.
        "year" => {
            items.sort_by_cached_key(|t| (row(t).year, secondary(t)));
        }
        "length" => {
            items.sort_by_cached_key(|t| (row(t).duration_ms, secondary(t)));
        }
        // "title" and any unrecognised field → natural-order `sort_key`.
        _ => items.sort_by_cached_key(|t| {
            (
                row(t).sort_key.as_deref().unwrap_or("").to_ascii_lowercase(),
                secondary(t),
            )
        }),
    }
    if desc {
        items.reverse();
    }
}

/// Sort a detail view's `TrackListRow` slice in place by `field` /
/// `dir`, with the track title as the deterministic tie-breaker — the
/// shared shape for the Album / Artist / Genre detail track lists.
/// (Playlist Detail has its own position-aware variant and doesn't use
/// this.) Thin specialisation of [`sort_track_rows_by`] over a plain
/// `[RsTrackListRow]` slice.
pub fn sort_track_list_rows(rows: &mut [RsTrackListRow], field: &str, dir: &str) {
    sort_track_rows_by(rows, field, dir, |r| r, |r| r.title.to_lowercase());
}

/// Compute the display-order permutation of `rows` for `field` / `dir`,
/// without reordering `rows` itself. Used by the Tracks view, which keeps
/// `full` / `search_keys` in a fixed fetch order and re-sorts by swapping
/// a separate `Vec<usize>` permutation. Thin specialisation of
/// [`sort_track_rows_by`] — sorts `(index, &row)` pairs so the same
/// generic accessor shape applies, with the natural-order `sort_key` as
/// the tie-breaker.
pub fn compute_track_order(rows: &[RsTrackListRow], field: &str, dir: &str) -> Vec<usize> {
    let mut indexed: Vec<(usize, &RsTrackListRow)> = rows.iter().enumerate().collect();
    sort_track_rows_by(
        &mut indexed,
        field,
        dir,
        |t| t.1,
        |t| t.1.sort_key.as_deref().unwrap_or("").to_ascii_lowercase(),
    );
    indexed.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
#[path = "tests/track_sort_tests.rs"]
mod tests;
