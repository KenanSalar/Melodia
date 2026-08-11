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
//! The arms below are now the **only** answer to a header click: the SQL
//! `track_list_order_by` they were modelled on had an arm per column too, and
//! went once every caller that could ask for one started retaining its rows
//! and re-permuting them here (`src/database/queries/track.rs` keeps the one
//! fetch order that survived, as a const). The default/`"title"` sort uses the
//! natural-order `sort_key`, `track_number` uses a disc/track sentinel
//! composite, and `Option` fields put `None` first ascending. Which tokens can
//! reach them is pinned by
//! `ui::tests::track_sort_tests::every_header_column_asks_for_a_field_the_comparator_knows`.
//!
//! `sort_by_cached_key` is deliberate: a plain `sort_by` would
//! re-`to_lowercase()` both operands on every one of its `O(n log n)`
//! comparisons. `field` is fixed for the whole sort, so branching on it
//! once and picking the key type per branch is cheaper than a uniform key
//! enum. **Per branch is what makes the natural-order arm able to drop its
//! tie-breaker**: each arm is its own `sort_by_cached_key` call with its own
//! key type, so the one arm whose tie-breaker duplicates its primary can key
//! on a bare `String` while the other six keep a pair. See [`sort_rows`].
//!
//! **What the comparator reads is a trait, not a row type**, because the two
//! views with the largest lists no longer retain DB rows to hand it — they
//! keep pre-converted display rows plus a two-field sidecar
//! (`ui::track_list_cache`), and forking the comparator to match would give
//! the app two spellings of its sort semantics. [`TrackSortFields`] is the
//! eight fields the arms below actually read; everything else on a
//! `TrackListRow` is invisible here.

use crate::entities::track::TrackListRow as RsTrackListRow;

/// The fields a track-list sort compares, so the comparator can be handed
/// either a DB row or a cached display row.
///
/// The two flattening rules live here rather than in the arms so both
/// implementors are held to the same one: `disc` and `track` fold their
/// `NULL`/`0` cases onto the sentinel the SQL ordering uses, and the three
/// text fields fold a missing value onto `""` — which is what the old
/// `opt_lc(Option<&str>)` did anyway, so no ordering moves.
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
/// * `row` projects each element to something implementing
///   [`TrackSortFields`] — a `TrackListRow` for the detail views, Browse and
///   Search, a `ui::track_list_cache::SortRow` for the two cached lists.
/// * `secondary` yields the lowercased deterministic tie-breaker key (the
///   track title for detail views, the file name for the Files view).
///
/// Unknown / `"title"` fields fall back to the natural-order `sort_key`,
/// then the secondary key. `"desc"` reverses the whole order for every
/// field except `track_number`, whose disc + secondary components stay
/// ascending — only the track component flips (the track sentinel is
/// negated instead of reversing the tuple).
pub fn sort_track_rows_by<T, F, R, S>(items: &mut [T], field: &str, dir: &str, row: R, secondary: S)
where
    F: TrackSortFields + ?Sized,
    R: Fn(&T) -> &F,
    S: Fn(&T) -> String,
{
    sort_rows(items, field, dir, row, secondary, false);
}

/// [`sort_track_rows_by`] plus the one thing only [`compute_track_order`] can
/// say about itself.
///
/// `secondary_is_natural_key` marks a caller whose tie-breaker *is* the value
/// the natural-order arm already compares on, so that arm keys on it once
/// instead of twice. `(k, k)` and `k` are the same comparison — this is
/// arithmetic, not an assumption about the input — and the second copy cost a
/// `String` per row on the default sort, which is the one every cold fetch and
/// every re-sort back to title order takes. Only that arm can drop it: the
/// other six compare a different primary and genuinely need the tie-breaker.
///
/// A private flag rather than a public parameter because there is exactly one
/// caller that may pass `true`, and it lives in this file — a fifth argument on
/// the shared entry point would put the question in front of four callers who
/// have no way to answer it wrong.
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

    // `track_number`: disc ASC, track ASC/DESC (NULL/0 → sentinel), then
    // the secondary key ASC. The direction flips only the track component,
    // so it is negated for `desc` and the whole tuple sorted ascending —
    // never `.reverse()`, which would also flip disc + secondary.
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
/// without reordering `rows` itself. Used by the two cached list views
/// (`ui::track_list_cache`), which keep their rows and keys in a fixed
/// fetch order and re-sort by swapping a separate `Vec<usize>`. Thin
/// specialisation of [`sort_track_rows_by`] — sorts `(index, &row)` pairs
/// so the same generic accessor shape applies, with the natural-order
/// `sort_key` as the tie-breaker.
///
/// Generic over the element so a caller holding rows and sort keys in two
/// parallel `Vec`s can zip a slice of borrowing views and pass that,
/// rather than the sort growing a second spelling for the pair.
///
/// Its tie-breaker being `sort_key` is what lets it take [`sort_rows`]'
/// `secondary_is_natural_key` arm: on the default sort the two halves of the
/// key are the same string, so only one is built.
pub fn compute_track_order<F: TrackSortFields>(rows: &[F], field: &str, dir: &str) -> Vec<usize> {
    let mut indexed: Vec<(usize, &F)> = rows.iter().enumerate().collect();
    sort_rows(
        &mut indexed,
        field,
        dir,
        |t| t.1,
        |t| t.1.sort_key().to_ascii_lowercase(),
        true,
    );
    indexed.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
#[path = "tests/track_sort_tests.rs"]
mod tests;
